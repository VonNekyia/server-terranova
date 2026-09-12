//! Besitzt die Prozesse und haelt sie am Laufen.
//!
//! Ein Knoten ist alles, was einen Prozess hat: MariaDB, Redis, der Proxy,
//! die festen Server und jeder offene Dungeon. Fuer alle gilt dieselbe
//! Maschinerie - Konsole mitlesen, Ende bemerken, neu starten - und nur das
//! Starten und Stoppen unterscheidet sich.
//!
//! Zwei Entscheidungen praegen alles andere:
//!
//! Die Kinder ueberleben den Supervisor. Kein Job-Objekt, das sie mitreisst:
//! ein Absturz des Supervisors wuerde sonst main, die Dungeons und MariaDB
//! gleichzeitig hart beenden - genau der Fall, der Chunks kostet. Stattdessen
//! werden sie beim naechsten Start wieder uebernommen.
//!
//! Gestoppt wird ueber die Konsole des Servers ("stop"), nicht ueber das
//! Betriebssystem. Erst wenn das nicht geht, ueber RCON, und erst danach
//! hart - und das wird als Fehler protokolliert.

use std::collections::BTreeMap;
use std::fs;
use std::io::{self, Write};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{sync_channel, SyncSender};
use std::sync::{Arc, Condvar, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};

use crate::backend::{self, Backend, Native};
use crate::config::{Config, NodeKind, NodeSpec, Runtime};
use crate::console::{Kind, LineRing};
use crate::paths::Paths;
use crate::{config, deps, mines, rcon, secrets, sync, sys, velocity};

/// So viele Zeilen Konsole haelt jeder Knoten vor.
const CONSOLE_LINES: usize = 2000;
/// Startet ein Knoten oefter als das hintereinander binnen einer Minute,
/// wird nicht weiter versucht.
const MAX_FAST_EXITS: u32 = 3;
const RESTART_DELAY: Duration = Duration::from_secs(10);
/// Kuerzer als das gelaufen heisst: es war kein richtiger Start.
const SHORT_RUN: Duration = Duration::from_secs(60);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Status {
    Stopped,
    Starting,
    Ready,
    Stopping,
    /// Startet immer wieder ab - es wird nicht weiter versucht
    Crashloop,
    /// Auf dem Port sitzt etwas Fremdes
    Conflict,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Control {
    /// Von uns gestartet, wir haben seine Konsole
    Owned,
    /// Lief schon - wir reden ueber RCON mit ihm
    Adopted,
}

pub struct Node {
    pub spec: NodeSpec,
    pub console: Arc<LineRing>,
    st: Mutex<NodeState>,
    changed: Condvar,
}

struct NodeState {
    status: Status,
    control: Control,
    pid: Option<u32>,
    created: Option<u64>,
    started: Option<Instant>,
    /// Soll laufen - danach richtet sich die Aufsicht
    desired: bool,
    stdin: Option<SyncSender<String>>,
    restarts: u32,
    fast_exits: u32,
    last_exit: Option<i32>,
    next_try: Option<Instant>,
}

impl Node {
    fn new(spec: NodeSpec) -> Arc<Node> {
        Arc::new(Node {
            spec,
            console: Arc::new(LineRing::new(CONSOLE_LINES)),
            st: Mutex::new(NodeState {
                status: Status::Stopped,
                control: Control::Owned,
                pid: None,
                created: None,
                started: None,
                desired: false,
                stdin: None,
                restarts: 0,
                fast_exits: 0,
                last_exit: None,
                next_try: None,
            }),
            changed: Condvar::new(),
        })
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, NodeState> {
        self.st.lock().unwrap_or_else(|e| e.into_inner())
    }

    pub fn status(&self) -> Status {
        self.lock().status
    }

    pub fn pid(&self) -> Option<u32> {
        self.lock().pid
    }

    pub fn control(&self) -> Control {
        self.lock().control
    }

    pub fn uptime(&self) -> Option<Duration> {
        self.lock().started.map(|t| t.elapsed())
    }

    pub fn restarts(&self) -> u32 {
        self.lock().restarts
    }

    fn set_status(&self, s: Status) {
        self.lock().status = s;
        self.changed.notify_all();
    }

    fn mark_ready(&self) {
        let mut g = self.lock();
        if g.status == Status::Starting {
            g.status = Status::Ready;
            self.changed.notify_all();
        }
    }

    /// Wartet, bis der Knoten gestoppt ist. true = geschafft.
    fn wait_stopped(&self, timeout: Duration) -> bool {
        let g = self.lock();
        let (g, _) = self
            .changed
            .wait_timeout_while(g, timeout, |s| s.status != Status::Stopped)
            .unwrap_or_else(|e| e.into_inner());
        g.status == Status::Stopped
    }

    /// Schickt eine Zeile an die Konsole des Servers.
    fn write_stdin(&self, line: &str) -> bool {
        let tx = self.lock().stdin.clone();
        matches!(tx, Some(tx) if tx.try_send(line.to_string()).is_ok())
    }
}

#[derive(Debug, Serialize, Deserialize, Default)]
struct SavedState {
    nodes: Vec<SavedNode>,
    /// Welche Dungeons offen waren - zum Wiederaufnehmen nach einem Neustart
    open_mines: Vec<u8>,
}

#[derive(Debug, Serialize, Deserialize)]
struct SavedNode {
    name: String,
    pid: u32,
    created: Option<u64>,
    port: u16,
}

pub struct Supervisor {
    pub paths: Paths,
    pub cfg: Config,
    pub events: Arc<LineRing>,
    backend: Box<dyn Backend>,
    nodes: Mutex<BTreeMap<String, Arc<Node>>>,
    rcon_password: String,
    shutting_down: AtomicBool,
    log_file: Mutex<Option<fs::File>>,
    /// Wo die oeffentliche Seite ausgeliefert wird, falls sie es wird.
    site: Mutex<Option<std::net::SocketAddr>>,
}

impl Supervisor {
    pub fn new(paths: Paths, cfg: Config) -> io::Result<Arc<Supervisor>> {
        let backend: Box<dyn Backend> = match cfg.runtime() {
            Runtime::Native => Box::new(Native::new(&paths, &cfg)),
            Runtime::Docker => {
                let d = crate::docker::Docker::new(&paths, &cfg);
                // Lieber hier klar scheitern als spaeter mit Servern, die
                // ihre Datenbank nicht finden.
                d.preflight().map_err(io::Error::other)?;
                Box::new(d)
            }
        };
        // Einmalig: Dungeons aus der Zeit, als sie noch unter servers/ lagen.
        for name in mines::migrate(&paths.servers(), &paths.dynamic()) {
            eprintln!("[terranova] {name} nach servers_dynamic/ verschoben");
        }

        let (rcon_password, _) = secrets::load_or_create(&paths.rcon_secret())?;
        fs::create_dir_all(paths.state_dir())?;
        let log_file = fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(paths.supervisor_log())
            .ok();

        let sup = Arc::new(Supervisor {
            events: Arc::new(LineRing::new(CONSOLE_LINES)),
            backend,
            nodes: Mutex::new(BTreeMap::new()),
            rcon_password,
            shutting_down: AtomicBool::new(false),
            log_file: Mutex::new(log_file),
            site: Mutex::new(None),
            paths,
            cfg,
        });
        sup.build_nodes();
        Ok(sup)
    }

    /// Alle Knoten in Startreihenfolge: erst die Datenbanken, dann der Proxy,
    /// dann die Server, zuletzt offene Dungeons.
    fn build_nodes(&self) {
        let mut n = self.nodes.lock().unwrap();
        for spec in self.dep_specs() {
            n.insert(spec.name.clone(), Node::new(spec));
        }
        let proxy = self.cfg.proxy_node(&self.paths);
        n.insert(proxy.name.clone(), Node::new(proxy));
        for spec in self.cfg.server_nodes(&self.paths) {
            n.insert(spec.name.clone(), Node::new(spec));
        }
        for slot in mines::existing(&self.paths.dynamic(), self.cfg.mines.slots) {
            let spec = self.cfg.mine_node(&self.paths, slot);
            n.insert(spec.name.clone(), Node::new(spec));
        }
    }

    fn dep_specs(&self) -> Vec<NodeSpec> {
        use crate::config::Mem;
        vec![
            NodeSpec {
                name: "mariadb".into(),
                kind: NodeKind::MariaDb,
                dir: self.paths.mariadb_home(),
                template: None,
                port: self.cfg.deps.mariadb.port,
                rcon_port: None,
                memory: Mem(0),
                stop_timeout: Duration::from_secs(60),
                motd: None,
            },
            NodeSpec {
                name: "redis".into(),
                kind: NodeKind::Redis,
                dir: self.paths.redis_home(),
                template: None,
                port: self.cfg.deps.redis.port,
                rcon_port: None,
                memory: Mem(0),
                stop_timeout: Duration::from_secs(30),
                motd: None,
            },
        ]
    }

    pub fn node(&self, name: &str) -> Option<Arc<Node>> {
        self.nodes.lock().unwrap().get(name).cloned()
    }

    /// Alle Knoten in Startreihenfolge.
    pub fn all(&self) -> Vec<Arc<Node>> {
        let n = self.nodes.lock().unwrap();
        let mut v: Vec<Arc<Node>> = n.values().cloned().collect();
        v.sort_by_key(|x| (order_of(x.spec.kind), x.spec.port));
        v
    }

    pub fn log(&self, msg: impl AsRef<str>) {
        let msg = msg.as_ref();
        self.events.push(Kind::Sys, msg);
        if let Some(f) = self.log_file.lock().unwrap().as_mut() {
            let t = sys::local_time();
            let _ = writeln!(
                f,
                "[{:02}.{:02}. {:02}:{:02}:{:02}] {msg}",
                t.day, t.month, t.hour, t.minute, t.second
            );
            let _ = f.flush();
        }
    }

    // --- Starten ------------------------------------------------------------

    /// Uebernimmt einen Knoten, der schon laeuft.
    ///
    /// Erkannt wird er am Port. PID, Erzeugungszeit und Programmdatei muessen
    /// zusammenpassen; sitzt dort etwas Fremdes, wird es nicht angefasst.
    fn adopt(self: &Arc<Self>, node: &Arc<Node>) -> bool {
        let Some(d) = self.backend.discover(&node.spec) else {
            return false;
        };
        if !backend::plausible_image(node.spec.kind, &d.image) {
            node.set_status(Status::Conflict);
            self.log(format!(
                "{}: Port {} gehoert {} - nicht angefasst",
                node.spec.name, node.spec.port, d.image
            ));
            return false;
        }
        {
            let mut g = node.lock();
            g.status = Status::Ready;
            g.control = Control::Adopted;
            g.pid = Some(d.pid);
            g.created = d.created;
            g.started = Some(Instant::now());
            g.desired = true;
            g.stdin = None;
        }
        node.changed.notify_all();
        self.log(format!(
            "{}: laeuft schon (PID {}) - uebernommen",
            node.spec.name, d.pid
        ));
        node.console.push(
            Kind::Sys,
            "von Terranova uebernommen - Befehle gehen ueber RCON, die Konsole kommt aus logs/latest.log",
        );

        // Ende bemerken
        let sup = self.clone();
        let name = node.spec.name.clone();
        let pid = d.pid;
        thread::Builder::new()
            .name(format!("tn-wait-{name}"))
            .spawn(move || {
                sys::wait_exit(pid, u32::MAX);
                sup.on_exit(&name, None);
            })
            .ok();

        if node.spec.kind.is_minecraft() {
            tail_log(node.clone());
        }
        true
    }

    /// Startet einen Knoten, wenn er nicht ohnehin schon laeuft.
    pub fn start_node(self: &Arc<Self>, node: &Arc<Node>) -> io::Result<()> {
        {
            let mut g = node.lock();
            g.desired = true;
            g.next_try = None;
            if matches!(g.status, Status::Ready | Status::Starting) {
                return Ok(());
            }
            // Ein Startversuch von Hand hebt die Sperre auf.
            if g.status == Status::Crashloop {
                g.fast_exits = 0;
            }
            g.status = Status::Starting;
        }

        if self.adopt(node) {
            return Ok(());
        }

        let mut child = match self.backend.spawn(&node.spec) {
            Ok(c) => c,
            Err(e) => {
                node.set_status(Status::Stopped);
                return Err(e);
            }
        };
        let pid = child.id();
        let stdout = child.stdout.take();
        let stderr = child.stderr.take();
        let stdin = child.stdin.take();

        // Schreibthread fuer die Konsole. Begrenzte Warteschlange: wer
        // schneller schreibt, als der Server liest, bekommt ein Nein statt
        // einen haengenden Supervisor.
        let (tx, rx) = sync_channel::<String>(64);
        if let Some(mut si) = stdin {
            thread::Builder::new()
                .name(format!("tn-stdin-{}", node.spec.name))
                .spawn(move || {
                    while let Ok(line) = rx.recv() {
                        if si.write_all(line.as_bytes()).is_err()
                            || si.write_all(b"\n").is_err()
                            || si.flush().is_err()
                        {
                            break;
                        }
                    }
                })
                .ok();
        }

        {
            let mut g = node.lock();
            g.pid = Some(pid);
            g.created = sys::created(pid);
            g.started = Some(Instant::now());
            g.control = Control::Owned;
            g.stdin = Some(tx);
            g.restarts += 1;
        }
        node.changed.notify_all();

        // Lesethreads. Sie duerfen nie blockieren: laeuft die Pipe voll,
        // haengt irgendwann der Server selbst.
        for (stream, kind) in [
            (stdout.map(Reader::Out), Kind::Out),
            (stderr.map(Reader::Err), Kind::Err),
        ] {
            let Some(stream) = stream else { continue };
            let node2 = node.clone();
            thread::Builder::new()
                .name(format!("tn-read-{}", node.spec.name))
                .spawn(move || {
                    let node_kind = node2.spec.kind;
                    stream.pump(|line| {
                        if backend::is_ready_line(node_kind, &line) {
                            node2.mark_ready();
                        }
                        node2.console.push(kind, line);
                    });
                })
                .ok();
        }

        let sup = self.clone();
        let name = node.spec.name.clone();
        thread::Builder::new()
            .name(format!("tn-wait-{name}"))
            .spawn(move || {
                let code = child.wait().ok().and_then(|s| s.code());
                sup.on_exit(&name, code);
            })
            .ok();

        self.log(format!("{}: gestartet (PID {pid})", node.spec.name));
        self.save_state();
        Ok(())
    }

    fn on_exit(self: &Arc<Self>, name: &str, code: Option<i32>) {
        let Some(node) = self.node(name) else { return };
        let short;
        {
            let mut g = node.lock();
            // Ein zweiter Waiter (uebernommen und danach selbst gestartet)
            // darf den Zustand nicht durcheinanderbringen.
            if g.status == Status::Stopped {
                return;
            }
            short = g.started.is_some_and(|t| t.elapsed() < SHORT_RUN);
            g.status = Status::Stopped;
            g.pid = None;
            g.created = None;
            g.stdin = None;
            g.started = None;
            g.last_exit = code;
            if g.desired && short {
                g.fast_exits += 1;
            } else if !short {
                g.fast_exits = 0;
            }
            if g.desired {
                if g.fast_exits >= MAX_FAST_EXITS {
                    g.status = Status::Crashloop;
                } else {
                    g.next_try = Some(Instant::now() + RESTART_DELAY);
                }
            }
        }
        node.changed.notify_all();

        let what = code.map_or_else(|| "beendet".to_string(), |c| format!("beendet (Code {c})"));
        node.console.push(Kind::Sys, &what);
        if node.status() == Status::Crashloop {
            self.log(format!(
                "{name}: {what} - startet immer wieder ab, kein weiterer Versuch. \
                 Nach der Ursache sehen, dann 'terranova restart {name}'"
            ));
        } else {
            self.log(format!("{name}: {what}"));
        }
        self.save_state();
    }

    // --- Stoppen ---------------------------------------------------------------

    /// Faehrt einen Knoten herunter: erst ueber seine eigene Konsole, dann
    /// ueber RCON, erst zuletzt hart.
    pub fn stop_node(self: &Arc<Self>, node: &Arc<Node>) -> bool {
        {
            let mut g = node.lock();
            g.desired = false;
            g.next_try = None;
            if g.status == Status::Stopped {
                return true;
            }
            g.status = Status::Stopping;
        }
        let name = &node.spec.name;
        let timeout = node.spec.stop_timeout;

        match node.spec.kind {
            NodeKind::MariaDb => {
                // Nativ ueber ihr eigenes Protokoll; im Container reicht
                // SIGTERM, das mariadbd sauber behandelt.
                if self.cfg.runtime() == Runtime::Native {
                    deps::stop_mariadb(&self.paths, &self.cfg);
                } else {
                    self.backend.soft_stop(&node.spec, timeout);
                }
            }
            NodeKind::Redis => {
                deps::stop_redis(node.spec.port);
            }
            kind => {
                let cmd = backend::stop_command(kind).unwrap_or("stop");
                if !node.write_stdin(cmd) {
                    // Uebernommen oder die Pipe ist hin - dann ueber RCON.
                    self.rcon(node, cmd);
                }
            }
        }

        if node.wait_stopped(timeout) {
            self.log(format!("{name}: gestoppt"));
            return true;
        }

        // Zweiter Versuch ueber RCON, falls stdin nicht angekommen ist.
        if node.spec.kind.is_minecraft() {
            self.log(format!("{name}: reagiert nicht, Versuch ueber RCON"));
            self.rcon(
                node,
                backend::stop_command(node.spec.kind).unwrap_or("stop"),
            );
            if node.wait_stopped(Duration::from_secs(30)) {
                self.log(format!("{name}: gestoppt"));
                return true;
            }
        }

        // Unter Docker gibt es noch einen sanften Weg: docker stop schickt
        // SIGTERM, und die JVM laeuft ihren Shutdown-Hook.
        if self.backend.soft_stop(&node.spec, Duration::from_secs(60)) {
            self.log(format!("{name}: ueber den Container gestoppt"));
            if node.wait_stopped(Duration::from_secs(90)) {
                return true;
            }
        }

        if let Some(pid) = node.pid() {
            // Ab hier geht Datenverlust nicht mehr aus - deshalb als Fehler.
            self.log(format!(
                "FEHLER {name}: reagiert nicht, wird hart beendet - ungespeicherte Chunks gehen verloren"
            ));
            self.backend.kill(&node.spec, pid);
            node.wait_stopped(Duration::from_secs(15));
        }
        false
    }

    fn rcon(&self, node: &Arc<Node>, command: &str) -> Option<String> {
        let port = node.spec.rcon_port?;
        match rcon::command(port, &self.rcon_password, command, Duration::from_secs(10)) {
            Ok(reply) => Some(reply),
            Err(e) => {
                self.log(format!("{}: RCON {port}: {e}", node.spec.name));
                None
            }
        }
    }

    /// Einen Befehl an die Konsole eines Servers schicken.
    pub fn send_command(&self, node: &Arc<Node>, line: &str) -> Result<String, String> {
        if node.status() != Status::Ready && node.status() != Status::Starting {
            return Err(format!("{} laeuft nicht", node.spec.name));
        }
        if node.write_stdin(line) {
            return Ok(String::new());
        }
        match node.spec.rcon_port {
            Some(_) => self
                .rcon(node, line)
                .ok_or_else(|| "weder Konsole noch RCON erreichbar".to_string()),
            None => Err("dieser Knoten nimmt keine Befehle".into()),
        }
    }

    // --- Netzwerk ----------------------------------------------------------------

    /// Faehrt das Netzwerk hoch. `only` leer heisst: alles.
    ///
    /// Mit Namen darin bleibt es beim Noetigsten - Datenbanken, Proxy und
    /// die genannten Server. Dungeons kommen dann nicht von selbst zurueck:
    /// wer bewusst wenig startet, will nicht acht Welten im Speicher.
    pub fn start_network(self: &Arc<Self>, only: &[String]) -> io::Result<()> {
        self.shutting_down.store(false, Ordering::SeqCst);

        // Datenbanken zuerst: die Plugins lesen ihre Configs, bevor sie
        // ueberhaupt laden koennten.
        self.start_deps()?;

        // Was es an Dungeons gibt, gehoert in velocity.toml, bevor der Proxy
        // die Datei liest.
        self.sync_proxy_servers();

        // Bestuecken, dann starten. Ein laufender Server haelt seine Jars
        // offen, deshalb passiert das vorher.
        let syncer = sync::Syncer::new(&self.paths, &self.cfg)
            .map_err(|e| io::Error::other(format!("Bestuecken: {e}")))?;

        let mut to_start: Vec<Arc<Node>> = self
            .all()
            .into_iter()
            .filter(|n| match n.spec.kind {
                // Der Proxy immer: ohne ihn kommt niemand auf 25565 an.
                NodeKind::Proxy => true,
                NodeKind::Server => only.is_empty() || only.contains(&n.spec.name),
                _ => false,
            })
            .collect();

        // Offene Dungeons wieder hochfahren, solange sie nicht abgelaufen sind.
        if self.cfg.mines.resume && only.is_empty() {
            for slot in mines::existing(&self.paths.dynamic(), self.cfg.mines.slots) {
                let dir = self.paths.mine(&mines::name(slot));
                // Geschlossen heisst geschlossen - der wartet nur noch aufs
                // Abraeumen.
                if mines::closed_at(&dir).is_some() {
                    continue;
                }
                let age_ok = matches!(
                    mines::reap_decision(
                        mines::expires_from(&dir),
                        mines::now_unix(),
                        self.cfg.mines.lifetime.0,
                        false,
                        false
                    ),
                    mines::Reap::Keep { .. }
                );
                if age_ok {
                    if let Some(n) = self.node(&mines::name(slot)) {
                        to_start.push(n);
                    }
                }
            }
        }

        for node in &to_start {
            // Auch ohne Vorlage: server.properties und paper-global.yml
            // entstehen hier, und in beiden steckt ein Geheimnis.
            if let Err(e) = syncer.sync(&node.spec, false) {
                self.log(format!(
                    "{}: Einrichten fehlgeschlagen: {e}",
                    node.spec.name
                ));
            }
            if let Err(e) = self.start_node(node) {
                self.log(format!("{}: Start fehlgeschlagen: {e}", node.spec.name));
            }
            // Dem Vorigen einen Moment geben, bevor der Naechste die Platte
            // belegt.
            thread::sleep(Duration::from_secs(2));
        }
        Ok(())
    }

    /// Liefert die oeffentliche Seite aus.
    ///
    /// Haengt bewusst nicht am Netzwerk: gerade wenn die Server unten sind,
    /// soll die Seite noch stehen - dort steht dann, warum.
    pub fn start_site(&self) {
        let site = &self.cfg.web.site;
        let root = self.paths.root.join(&site.dir);
        match crate::web::serve_site(root.clone(), &site.bind, site.port) {
            Ok(Some(addr)) => {
                *self.site.lock().unwrap() = Some(addr);
                self.log(format!(
                    "Webseite auf http://{addr}/ aus {}",
                    root.display()
                ));
            }
            Ok(None) if site.port != 0 => {
                self.log(format!("Webseite: {} hat keine index.html", root.display()));
            }
            Ok(None) => {}
            Err(e) => self.log(format!("Webseite auf Port {}: {e}", site.port)),
        }
    }

    pub fn site_addr(&self) -> Option<std::net::SocketAddr> {
        *self.site.lock().unwrap()
    }

    /// Was die Karte macht.
    ///
    /// Pl3xMap laeuft im Server-Prozess, nicht als eigener Dienst. Deshalb
    /// ist "die Karte ist unten, weil main unten ist" etwas anderes als
    /// "main laeuft, aber auf dem Kartenport antwortet niemand" - das zweite
    /// ist ein Fehler, das erste nicht.
    pub fn map_state(&self) -> &'static str {
        let m = &self.cfg.web.map;
        if m.port == 0 {
            return "off";
        }
        if self.node(&m.server).map(|n| n.status()) != Some(Status::Ready) {
            return "waiting";
        }
        if crate::web::reachable(m.port) {
            "ready"
        } else {
            "down"
        }
    }

    pub fn start_deps(self: &Arc<Self>) -> io::Result<()> {
        let mut log = |m: &str| self.log(m);
        if self.cfg.runtime() == Runtime::Native {
            // Zuerst: ohne Java startet hinterher kein einziger Server, und
            // dann haette MariaDB umsonst gewartet.
            deps::ensure_java(&self.paths, &self.cfg, &mut log)?;
            deps::ensure_mariadb(&self.paths, &self.cfg, &mut log)?;
            deps::ensure_redis(&self.paths, &self.cfg, &mut log)?;
        } else {
            crate::docker::pull_images(&self.cfg, &mut log);
        }

        if let Some(n) = self.node("mariadb") {
            self.start_node(&n)?;
            let port = self.cfg.deps.mariadb.port;
            if !deps::wait_until(|| deps::mariadb_ready(port), Duration::from_secs(60)) {
                return Err(io::Error::other("MariaDB antwortet nicht"));
            }
            n.mark_ready();
            deps::provision(&self.paths, &self.cfg, self.cfg.runtime())?;
            self.log("Datenbanken bereit");
        }
        if let Some(n) = self.node("redis") {
            self.start_node(&n)?;
            let port = self.cfg.deps.redis.port;
            if !deps::wait_until(|| deps::redis_ready(port), Duration::from_secs(30)) {
                return Err(io::Error::other("Redis antwortet nicht"));
            }
            n.mark_ready();
            self.log("Redis bereit");
        }
        Ok(())
    }

    /// Faehrt alles herunter.
    ///
    /// Erst der Proxy: danach kann niemand mehr auf einen Server kommen, der
    /// gerade speichert. Dann alle Minecraft-Server gleichzeitig - jeder hat
    /// seine eigene Welt, nacheinander waere nur langsamer. Zuletzt die
    /// Datenbanken.
    pub fn stop_network(self: &Arc<Self>) {
        self.shutting_down.store(true, Ordering::SeqCst);
        self.log("Netzwerk wird heruntergefahren...");

        if let Some(proxy) = self.node(crate::config::PROXY) {
            self.stop_node(&proxy);
        }

        let servers: Vec<Arc<Node>> = self
            .all()
            .into_iter()
            .filter(|n| matches!(n.spec.kind, NodeKind::Server | NodeKind::Mine(_)))
            .collect();
        let mut handles = Vec::new();
        for node in servers {
            let sup = self.clone();
            handles.push(thread::spawn(move || sup.stop_node(&node)));
        }
        for h in handles {
            let _ = h.join();
        }

        for name in ["redis", "mariadb"] {
            if let Some(n) = self.node(name) {
                self.stop_node(&n);
            }
        }
        self.save_state();
        self.log("Netzwerk ist unten");
    }

    pub fn shutting_down(&self) -> bool {
        self.shutting_down.load(Ordering::SeqCst)
    }

    /// Startet neu, was abgestuerzt ist. Laeuft in einem eigenen Thread.
    pub fn watchdog(self: &Arc<Self>) {
        while !self.shutting_down() {
            thread::sleep(Duration::from_secs(2));
            if self.shutting_down() {
                break;
            }
            for node in self.all() {
                let due = {
                    let g = node.lock();
                    g.desired
                        && g.status == Status::Stopped
                        && g.next_try.is_some_and(|t| Instant::now() >= t)
                };
                if due {
                    self.log(format!("{}: Neustart", node.spec.name));
                    if let Err(e) = self.start_node(&node) {
                        self.log(format!("{}: Neustart fehlgeschlagen: {e}", node.spec.name));
                        node.lock().next_try = Some(Instant::now() + RESTART_DELAY);
                    }
                }
            }
        }
    }

    /// Uebernimmt alles, was schon laeuft - nach einem Absturz des
    /// Supervisors.
    pub fn adopt_running(self: &Arc<Self>) {
        for node in self.all() {
            if node.status() == Status::Stopped {
                self.adopt(&node);
            }
        }
    }

    // --- Dungeons -------------------------------------------------------------------

    /// Legt einen Dungeon an, falls noetig, und startet ihn.
    /// Oeffnet einen Dungeon. `template` waehlt die Vorlage; ohne Angabe die
    /// aus der Konfiguration.
    ///
    /// Die Vorlage zaehlt nur beim Anlegen - ein vorhandener Dungeon behaelt
    /// seine Welt und wird weiter aus der Vorlage bestueckt, aus der er
    /// entstanden ist.
    pub fn open_mine(self: &Arc<Self>, slot: u8, template: Option<&str>) -> io::Result<Arc<Node>> {
        let dir = self.paths.mine(&mines::name(slot));
        if !dir.exists() {
            let chosen = template.unwrap_or(&self.cfg.mines.template);
            let from = self.paths.template(chosen);
            if !from.is_dir() {
                return Err(io::Error::new(
                    io::ErrorKind::NotFound,
                    format!("keine Vorlage templates/{chosen}"),
                ));
            }
            copy_tree(&from, &dir)?;
            mines::write_marker(&dir, slot, mines::now_unix(), Some(chosen))?;
            self.log(format!(
                "{}: aus der Vorlage {chosen} angelegt",
                mines::name(slot)
            ));
        }
        // War er geschlossen, ist er es jetzt nicht mehr: seine Welt kommt
        // zurueck, und die Uhr laeuft wieder auf das urspruengliche Ende zu.
        if mines::closed_at(&dir).is_some() {
            mines::mark_open(&dir)?;
            self.log(format!("{}: wieder geoeffnet", mines::name(slot)));
        }

        // Erst jetzt: der Bauplan liest die Vorlage aus der Markierung.
        let spec = self.cfg.mine_node(&self.paths, slot);
        let node = {
            let mut n = self.nodes.lock().unwrap();
            n.entry(spec.name.clone())
                .or_insert_with(|| Node::new(spec.clone()))
                .clone()
        };
        let syncer = sync::Syncer::new(&self.paths, &self.cfg)
            .map_err(|e| io::Error::other(e.to_string()))?;
        syncer.sync(&node.spec, false)?;
        // Vor dem Start: sonst stuende der Server schon bereit, waehrend der
        // Proxy ihn noch nicht kennt.
        self.sync_proxy_servers();
        self.start_node(&node)?;
        self.save_state();
        Ok(node)
    }

    /// Stoppen, bestuecken, wieder starten - in dieser Reihenfolge.
    ///
    /// Das Bestuecken gehoert dazwischen: ein laufender Server haelt seine
    /// Jars offen, Windows laesst sie dann nicht ersetzen. So wird ein
    /// Plugin-Update beim naechsten Neustart wirksam, ohne dass jemand daran
    /// denken muss.
    pub fn restart_node(self: &Arc<Self>, node: &Arc<Node>) {
        self.stop_node(node);
        match sync::Syncer::new(&self.paths, &self.cfg).and_then(|s| s.sync(&node.spec, false)) {
            Ok(r) if !r.pruned.is_empty() => self.log(format!(
                "{}: {} veraltete Datei(en) entfernt",
                node.spec.name,
                r.pruned.len()
            )),
            Ok(_) => {}
            Err(e) => self.log(format!("{}: Einrichten: {e}", node.spec.name)),
        }
        if let Err(e) = self.start_node(node) {
            self.log(format!("{}: Start fehlgeschlagen: {e}", node.spec.name));
        }
    }

    /// Traegt die offenen Dungeons in velocity.toml ein und laesst den Proxy
    /// neu laden.
    ///
    /// Vorher standen alle acht Plaetze fest in der Datei, ob offen oder
    /// nicht. Jetzt steht dort, was es wirklich gibt - wer die Zahl der
    /// Plaetze in terranova.yml aendert, muss nichts mehr nachziehen.
    ///
    /// Aendert sich am Inhalt nichts, wird weder geschrieben noch neu
    /// geladen: ein Neuladen bei jedem Anlass waere ein Grund, es sein zu
    /// lassen.
    pub fn sync_proxy_servers(&self) {
        let file = self
            .paths
            .resolve(&self.cfg.proxy.dir)
            .join("velocity.toml");
        let text = match fs::read_to_string(&file) {
            Ok(t) => t,
            Err(e) => {
                self.log(format!("velocity.toml: {e}"));
                return;
            }
        };
        let open: Vec<(String, u16)> = mines::existing(&self.paths.dynamic(), self.cfg.mines.slots)
            .into_iter()
            // Ein geschlossener Dungeon kommt nicht wieder hoch - dann soll der
            // Proxy auch niemanden mehr dorthin schicken.
            .filter(|slot| mines::closed_at(&self.paths.mine(&mines::name(*slot))).is_none())
            .map(|slot| {
                (
                    mines::name(slot),
                    self.cfg.mines.base_port + u16::from(slot),
                )
            })
            .collect();
        let Some(new) = velocity::with_mines(&text, &open) else {
            return;
        };
        if let Err(e) = fs::write(&file, new) {
            self.log(format!("velocity.toml schreiben: {e}"));
            return;
        }

        // Der Proxy liest die Datei nur beim Start und auf Zuruf. Laeuft er
        // nicht, genuegt die geschriebene Datei - er liest sie ohnehin gleich.
        let Some(proxy) = self.node(config::PROXY) else {
            return;
        };
        if proxy.status() != Status::Ready {
            return;
        }
        match self.send_command(&proxy, "velocity reload") {
            Ok(_) => self.log(format!(
                "Proxy neu geladen - {} Dungeon(s) eingetragen",
                open.len()
            )),
            Err(e) => self.log(format!("Proxy neu laden: {e}")),
        }
    }

    /// Schliesst einen Dungeon: stoppen und zum Abraeumen vormerken.
    ///
    /// Die Welt bleibt stehen - wer ihn binnen seiner Lebenszeit wieder
    /// oeffnet, bekommt sie zurueck. Passiert das nicht, raeumt der Reaper ihn
    /// ab, gerechnet ab jetzt und nicht ab dem Oeffnen: ein Dungeon, den
    /// jemand nach dreiundzwanzig Stunden schliesst, soll nicht eine Stunde
    /// spaeter verschwinden.
    pub fn close_mine(self: &Arc<Self>, slot: u8) -> bool {
        let Some(node) = self.node(&mines::name(slot)) else {
            return false;
        };
        let ok = self.stop_node(&node);
        if let Err(e) = mines::mark_closed(&node.spec.dir, mines::now_unix()) {
            self.log(format!("{}: Markierung schreiben: {e}", node.spec.name));
        }
        self.sync_proxy_servers();
        ok
    }

    /// Nimmt einen Knoten aus der Liste - fuer einen abgeraeumten Dungeon.
    pub fn remove_node(&self, name: &str) {
        self.nodes.lock().unwrap().remove(name);
    }

    pub fn mine_nodes(&self) -> Vec<Arc<Node>> {
        self.all()
            .into_iter()
            .filter(|n| matches!(n.spec.kind, NodeKind::Mine(_)))
            .collect()
    }

    // --- Zustand ----------------------------------------------------------------------

    fn save_state(&self) {
        let mut s = SavedState::default();
        for node in self.all() {
            let g = node.lock();
            if let Some(pid) = g.pid {
                s.nodes.push(SavedNode {
                    name: node.spec.name.clone(),
                    pid,
                    created: g.created,
                    port: node.spec.port,
                });
            }
            if let NodeKind::Mine(slot) = node.spec.kind {
                s.open_mines.push(slot);
            }
        }
        if let Ok(text) = serde_json::to_vec_pretty(&s) {
            let _ = fs::write(self.paths.node_state(), text);
        }
    }
}

fn order_of(kind: NodeKind) -> u8 {
    match kind {
        NodeKind::MariaDb => 0,
        NodeKind::Redis => 1,
        NodeKind::Proxy => 2,
        NodeKind::Server => 3,
        NodeKind::Mine(_) => 4,
    }
}

/// stdout und stderr sind verschiedene Typen - hier zusammengefasst, damit
/// derselbe Lesethread beide bedienen kann.
enum Reader {
    Out(std::process::ChildStdout),
    Err(std::process::ChildStderr),
}

impl Reader {
    fn pump(self, f: impl FnMut(String)) {
        match self {
            Reader::Out(r) => crate::console::pump(r, f),
            Reader::Err(r) => crate::console::pump(r, f),
        }
    }
}

/// Liest die Logdatei eines uebernommenen Servers mit - seine Pipe gehoert
/// uns ja nicht.
fn tail_log(node: Arc<Node>) {
    let path = node.spec.dir.join("logs").join("latest.log");
    thread::Builder::new()
        .name(format!("tn-tail-{}", node.spec.name))
        .spawn(move || {
            let mut pos: u64 = fs::metadata(&path).map(|m| m.len()).unwrap_or(0);
            let mut rest = Vec::new();
            while node.status() != Status::Stopped {
                thread::sleep(Duration::from_millis(500));
                let Ok(meta) = fs::metadata(&path) else {
                    continue;
                };
                if meta.len() < pos {
                    pos = 0; // Datei wurde gedreht
                }
                if meta.len() == pos {
                    continue;
                }
                let Ok(mut f) = fs::File::open(&path) else {
                    continue;
                };
                use std::io::{Read, Seek, SeekFrom};
                if f.seek(SeekFrom::Start(pos)).is_err() {
                    continue;
                }
                let mut buf = Vec::new();
                if f.read_to_end(&mut buf).is_err() {
                    continue;
                }
                pos += buf.len() as u64;
                rest.extend_from_slice(&buf);
                while let Some(i) = rest.iter().position(|&b| b == b'\n') {
                    let line: Vec<u8> = rest.drain(..=i).collect();
                    let text = String::from_utf8_lossy(&line);
                    node.console
                        .push(Kind::Out, crate::console::strip_ansi(text.trim_end()));
                }
            }
        })
        .ok();
}

/// Kopiert einen Verzeichnisbaum - fuer einen frischen Dungeon.
pub fn copy_tree(from: &std::path::Path, to: &std::path::Path) -> io::Result<()> {
    fs::create_dir_all(to)?;
    for entry in fs::read_dir(from)? {
        let entry = entry?;
        let src = entry.path();
        let dst = to.join(entry.file_name());
        if entry.file_type()?.is_dir() {
            copy_tree(&src, &dst)?;
        } else {
            fs::copy(&src, &dst)?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn startreihenfolge() {
        let mut kinds = [
            NodeKind::Mine(1),
            NodeKind::Server,
            NodeKind::MariaDb,
            NodeKind::Proxy,
            NodeKind::Redis,
        ];
        kinds.sort_by_key(|k| order_of(*k));
        assert_eq!(
            kinds,
            [
                NodeKind::MariaDb,
                NodeKind::Redis,
                NodeKind::Proxy,
                NodeKind::Server,
                NodeKind::Mine(1)
            ]
        );
    }

    #[test]
    fn baum_kopieren() {
        let dir = crate::testutil::tempdir("copytree");
        let from = dir.join("vorlage");
        fs::create_dir_all(from.join("plugins").join("X")).unwrap();
        fs::write(from.join("server.properties"), "a=1").unwrap();
        fs::write(from.join("plugins").join("X").join("config.yml"), "b: 2").unwrap();
        let to = dir.join("mining-1");
        copy_tree(&from, &to).unwrap();
        assert_eq!(
            fs::read_to_string(to.join("server.properties")).unwrap(),
            "a=1"
        );
        assert!(to.join("plugins").join("X").join("config.yml").is_file());
    }
}
