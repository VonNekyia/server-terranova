//! Argumente lesen und den passenden Befehl ausfuehren.

use std::io::{BufRead, Write};
use std::path::PathBuf;
use std::process::ExitCode;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::OnceLock;
use std::time::Duration;

use lexopt::prelude::*;
use serde_json::json;

use crate::client::{self, Client};
use crate::config::Config;
use crate::paths::Paths;
use crate::sync::Syncer;
use crate::{doctor, mines, sys, VERSION};

pub const HELP: &str = "\
terranova - startet, beaufsichtigt und stoppt das Terranova-Netzwerk

  terranova <befehl> [optionen]

Netzwerk
  start [--detach]      Datenbanken, Proxy und Server hochfahren
  start <name...>       nur diese Server - fuer schwaechere Rechner
  stop [name...]        alles oder einzelne Knoten sauber herunterfahren
  restart <name>        stoppen, bestuecken, wieder starten
  status                was laeuft
  console <name>        Konsole mitlesen und Befehle eintippen
  logs <name> [-n N]    die letzten Zeilen
  cmd <name> <befehl>   einen Befehl schicken

Dungeons
  mine open [anzahl]    oeffnen (--slot N, --template <name>)
  mine templates        welche Vorlagen es gibt
  mine close <n>        schliessen
  mine list             was offen ist
  mine reap             abgelaufene abraeumen (--dry-run zeigt nur)

Oberflaeche
  dashboard             im Browser oeffnen

Wartung
  sync [name...]        Server aus templates/ bestuecken
  doctor                pruefen, ob alles startklar ist
  init                  eine terranova.yml anlegen
  version

Optionen
  --root <pfad>         Netzwerkverzeichnis
  --detach              Netzwerk starten, aber nicht zusehen
  --dry-run             nichts aendern, nur zeigen
  --slot <n>            bestimmter Dungeon-Platz
  --template <name>     Vorlage fuer mine open
  -n <anzahl>           wie viele Zeilen
  -h, --help
";

#[derive(Debug, Default)]
struct Args {
    root: Option<PathBuf>,
    command: Option<String>,
    values: Vec<String>,
    dry_run: bool,
    detach: bool,
    stop_running: bool,
    slot: Option<u8>,
    template: Option<String>,
    lines: Option<usize>,
    help: bool,
}

fn parse() -> Result<Args, lexopt::Error> {
    let mut a = Args::default();
    let mut p = lexopt::Parser::from_env();
    while let Some(arg) = p.next()? {
        match arg {
            Short('h') | Long("help") => a.help = true,
            Long("root") => a.root = Some(p.value()?.into()),
            Long("dry-run") => a.dry_run = true,
            Long("detach") => a.detach = true,
            Long("stop-running") => a.stop_running = true,
            Long("slot") => a.slot = Some(p.value()?.parse()?),
            Long("template") => a.template = Some(p.value()?.string()?),
            Short('n') => a.lines = Some(p.value()?.parse()?),
            Value(v) if a.command.is_none() => a.command = Some(v.string()?),
            Value(v) => a.values.push(v.string()?),
            _ => return Err(arg.unexpected()),
        }
    }
    Ok(a)
}

pub fn run() -> ExitCode {
    let args = match parse() {
        Ok(a) => a,
        Err(e) => {
            eprintln!("terranova: {e}\n\n{HELP}");
            return ExitCode::from(2);
        }
    };
    if args.help || args.command.is_none() {
        print!("{HELP}");
        return ExitCode::SUCCESS;
    }
    let command = args.command.clone().unwrap_or_default();
    if command == "version" {
        println!("terranova {VERSION}");
        return ExitCode::SUCCESS;
    }

    let paths = match Paths::discover(args.root.as_deref()) {
        Ok(p) => p,
        Err(e) => {
            if command == "init" {
                return init(args.root.as_deref());
            }
            eprintln!("terranova: {e}");
            return ExitCode::FAILURE;
        }
    };
    if command == "init" {
        println!("terranova: {} gibt es schon", paths.config().display());
        return ExitCode::SUCCESS;
    }

    let cfg = match Config::load(&paths) {
        Ok(c) => c,
        Err(e) => {
            eprintln!("terranova: {e}");
            return ExitCode::FAILURE;
        }
    };

    match command.as_str() {
        "supervise" => supervise(paths, cfg, args.values),
        "start" => start(&paths, &cfg, args.detach, &args.values),
        "stop" => stop(&paths, &cfg, &args.values),
        "restart" => restart(&paths, &cfg, &args.values),
        "status" => status(&paths, &cfg),
        "console" => console(&paths, &cfg, &args.values),
        "logs" => logs(&paths, &cfg, &args.values, args.lines.unwrap_or(200)),
        "cmd" => cmd(&paths, &cfg, &args.values),
        "mine" => mine(&paths, &cfg, &args),
        "dashboard" => dashboard(&paths, &cfg),
        "sync" => sync_cmd(&paths, &cfg, &args.values, args.dry_run),
        "doctor" => {
            let checks = doctor::run(&paths, &cfg);
            print!("{}", doctor::render(&checks));
            match doctor::worst(&checks) {
                doctor::Level::Fail => ExitCode::FAILURE,
                _ => ExitCode::SUCCESS,
            }
        }
        other => {
            eprintln!("terranova: unbekannter Befehl '{other}'\n\n{HELP}");
            ExitCode::from(2)
        }
    }
}

// --- Der Supervisor selbst ------------------------------------------------

/// Wird nicht von Hand aufgerufen: `terranova start` startet das hier
/// losgeloest im Hintergrund.
fn supervise(paths: Paths, cfg: Config, only: Vec<String>) -> ExitCode {
    use std::fs::OpenOptions;
    #[cfg(windows)]
    use std::os::windows::fs::OpenOptionsExt as _;

    if let Err(e) = std::fs::create_dir_all(paths.state_dir()) {
        eprintln!("terranova: {e}");
        return ExitCode::FAILURE;
    }
    // Sperrdatei ohne Freigabe: solange dieser Prozess lebt, kann kein
    // zweiter Supervisor sie oeffnen. Stuerzt er ab, gibt Windows sie frei.
    let mut opts = OpenOptions::new();
    opts.write(true).create(true).truncate(true);
    #[cfg(windows)]
    opts.share_mode(0);
    let _lock = match opts.open(paths.supervisor_lock()) {
        Ok(f) => f,
        Err(_) => {
            eprintln!("terranova: es laeuft schon ein Supervisor");
            return ExitCode::from(3);
        }
    };

    let sup = match crate::supervisor::Supervisor::new(paths, cfg) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("terranova: {e}");
            return ExitCode::FAILURE;
        }
    };
    let (port, _token) = match crate::api::serve(sup.clone()) {
        Ok(p) => p,
        Err(e) => {
            sup.log(format!(
                "Schnittstelle auf Port {} : {e}",
                sup.cfg.dashboard.port
            ));
            return ExitCode::FAILURE;
        }
    };
    let _ = client::write_info(&sup.paths, port);
    sup.log(format!(
        "Terranova {VERSION} - Schnittstelle auf 127.0.0.1:{port}"
    ));

    sup.start_site();

    // Was schon laeuft, uebernehmen - etwa nach einem Absturz des
    // Supervisors, waehrend die Server weiterliefen.
    sup.adopt_running();

    if !only.is_empty() {
        sup.log(format!("Nur: {}", only.join(", ")));
    }
    if let Err(e) = sup.start_network(&only) {
        sup.log(format!("Start fehlgeschlagen: {e}"));
    }

    let w = sup.clone();
    std::thread::spawn(move || w.watchdog());
    let s = sup.clone();
    std::thread::spawn(move || crate::sched::run(s));

    // Der Supervisor endet erst, wenn ihn jemand darum bittet.
    loop {
        std::thread::sleep(Duration::from_secs(3600));
    }
}

// --- Netzwerk -----------------------------------------------------------------

/// Port und Token fuer den Konsolen-Ereignisbehandler, der keine Umgebung
/// mitbekommen kann.
static VIEWER: OnceLock<(u16, String, crate::config::WindowClose)> = OnceLock::new();
static CTRL_COUNT: AtomicU32 = AtomicU32::new(0);

fn on_ctrl(ev: sys::CtrlEvent) -> bool {
    let Some((port, token, on_close)) = VIEWER.get() else {
        return false;
    };
    let stop = |timeout| {
        let _ = crate::http::request(
            *port,
            "POST",
            "/api/network/stop",
            token,
            Some(b"{}"),
            timeout,
        );
    };
    match ev {
        sys::CtrlEvent::Interrupt | sys::CtrlEvent::Break => {
            if CTRL_COUNT.fetch_add(1, Ordering::SeqCst) == 0 {
                println!("\n[terranova] Netzwerk wird heruntergefahren - das dauert, main speichert seine Welt.");
                println!("[terranova] Noch einmal Strg+C schliesst nur dieses Fenster; das Netzwerk laeuft dann weiter.");
                stop(Duration::from_secs(20));
            } else {
                println!("\n[terranova] Fenster wird geschlossen, das Netzwerk laeuft weiter.");
                std::process::exit(0);
            }
            true
        }
        // Hier bleiben etwa fuenf Sekunden. Weil der Supervisor eigenstaendig
        // laeuft, genuegt es, ihm Bescheid zu sagen - er bringt das
        // Herunterfahren allein zu Ende, auch wenn dieses Fenster schon weg
        // ist. Mit on_window_close: detach laeuft das Netzwerk einfach weiter.
        sys::CtrlEvent::Close | sys::CtrlEvent::Shutdown => {
            if *on_close == crate::config::WindowClose::Stop {
                stop(Duration::from_secs(2));
            }
            true
        }
    }
}

fn start(paths: &Paths, cfg: &Config, detach: bool, only: &[String]) -> ExitCode {
    let c = match Client::new(paths, cfg) {
        Ok(c) => c,
        Err(e) => {
            eprintln!("terranova: {e}");
            return ExitCode::FAILURE;
        }
    };

    if client::supervisor_running(paths).is_some() && c.alive() {
        println!("[terranova] Supervisor laeuft bereits.");
    } else {
        let exe = match client::shadow_copy(paths) {
            Ok(e) => e,
            Err(e) => {
                eprintln!("terranova: {e}");
                return ExitCode::FAILURE;
            }
        };
        match client::spawn_detached(paths, &exe, only) {
            Ok(pid) => println!("[terranova] Supervisor gestartet (PID {pid})"),
            Err(e) => {
                eprintln!("terranova: Supervisor laesst sich nicht starten: {e}");
                return ExitCode::FAILURE;
            }
        }
        // Das Token entsteht erst im Supervisor.
        std::thread::sleep(Duration::from_millis(500));
        let c = Client::new(paths, cfg).unwrap_or(c);
        if !c.wait_alive(Duration::from_secs(20)) {
            eprintln!(
                "terranova: Supervisor antwortet nicht - siehe {}",
                paths.supervisor_log().display()
            );
            return ExitCode::FAILURE;
        }
    }

    if detach {
        println!("[terranova] laeuft im Hintergrund. Zusehen: terranova start");
        return ExitCode::SUCCESS;
    }
    view(paths, cfg)
}

/// Zeigt den Ereignisstrom des Supervisors, bis Strg+C kommt.
fn view(paths: &Paths, cfg: &Config) -> ExitCode {
    let Ok(c) = Client::new(paths, cfg) else {
        return ExitCode::FAILURE;
    };
    let _ = VIEWER.set((c.port, c.token().to_string(), cfg.dashboard.on_window_close));
    sys::on_console_ctrl(on_ctrl);

    let beim_schliessen = match cfg.dashboard.on_window_close {
        crate::config::WindowClose::Stop => "Fenster schliessen faehrt es ebenfalls herunter",
        crate::config::WindowClose::Detach => "Fenster schliessen laesst es weiterlaufen",
    };
    // Zwei Aufrufe statt einer mehrzeiligen Zeichenkette: cargo fmt zieht
    // deren Fortsetzung sonst die Einrueckung des Quelltexts mit.
    println!("[terranova] Strg+C faehrt das Netzwerk herunter, {beim_schliessen}.");
    println!("[terranova] Dashboard: http://127.0.0.1:{}", c.port);
    let r = c.follow("/api/events", None, |ev| {
        if ev.name != "gap" {
            println!("{}", ev.data);
        }
        true
    });
    match r {
        // Der Strom endet, wenn der Supervisor geht - das ist der Normalfall
        // nach einem Stopp.
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("terranova: Verbindung verloren: {e}");
            ExitCode::FAILURE
        }
    }
}

fn need_supervisor(paths: &Paths, cfg: &Config) -> Result<Client, ExitCode> {
    let c = Client::new(paths, cfg).map_err(|e| {
        eprintln!("terranova: {e}");
        ExitCode::FAILURE
    })?;
    if !c.alive() {
        eprintln!("terranova: der Supervisor laeuft nicht - erst 'terranova start'");
        return Err(ExitCode::FAILURE);
    }
    Ok(c)
}

fn report(r: std::io::Result<(u16, String)>, ok: &str) -> ExitCode {
    match r {
        Ok((200, _)) => {
            if !ok.is_empty() {
                println!("{ok}");
            }
            ExitCode::SUCCESS
        }
        Ok((status, body)) => {
            eprintln!("terranova: Fehler {status}: {}", error_of(&body));
            ExitCode::FAILURE
        }
        Err(e) => {
            eprintln!("terranova: {e}");
            ExitCode::FAILURE
        }
    }
}

fn error_of(body: &str) -> String {
    serde_json::from_str::<serde_json::Value>(body)
        .ok()
        .and_then(|v| v.get("error").and_then(|e| e.as_str()).map(String::from))
        .unwrap_or_else(|| body.trim().to_string())
}

fn stop(paths: &Paths, cfg: &Config, names: &[String]) -> ExitCode {
    let c = match need_supervisor(paths, cfg) {
        Ok(c) => c,
        Err(e) => return e,
    };
    if names.is_empty() {
        println!("[terranova] Netzwerk wird heruntergefahren...");
        let r = c.post("/api/network/stop", json!({}));
        // Warten, bis er wirklich weg ist - sonst kommt die Eingabe zurueck,
        // waehrend main noch speichert.
        if r.is_ok() {
            let deadline = std::time::Instant::now() + Duration::from_secs(300);
            while c.alive() && std::time::Instant::now() < deadline {
                std::thread::sleep(Duration::from_millis(500));
            }
            println!("[terranova] unten.");
            return ExitCode::SUCCESS;
        }
        return report(r, "");
    }
    for name in names {
        let code = report(
            c.post(&format!("/api/nodes/{name}/stop"), json!({})),
            &format!("[terranova] {name} gestoppt"),
        );
        if code != ExitCode::SUCCESS {
            return code;
        }
    }
    ExitCode::SUCCESS
}

fn restart(paths: &Paths, cfg: &Config, names: &[String]) -> ExitCode {
    let c = match need_supervisor(paths, cfg) {
        Ok(c) => c,
        Err(e) => return e,
    };
    if names.is_empty() {
        eprintln!("terranova: welchen Knoten?");
        return ExitCode::from(2);
    }
    for name in names {
        let code = report(
            c.post(&format!("/api/nodes/{name}/restart"), json!({})),
            &format!("[terranova] {name} wird neu gestartet"),
        );
        if code != ExitCode::SUCCESS {
            return code;
        }
    }
    ExitCode::SUCCESS
}

fn status(paths: &Paths, cfg: &Config) -> ExitCode {
    let Ok(c) = Client::new(paths, cfg) else {
        return ExitCode::FAILURE;
    };
    if !c.alive() {
        println!("Der Supervisor laeuft nicht.");
        // Trotzdem nachsehen, ob Server von frueher noch da sind.
        let mut found = false;
        for node in std::iter::once(cfg.proxy_node(paths))
            .chain(cfg.server_nodes(paths))
            .chain(
                mines::existing(&paths.dynamic(), cfg.mines.slots)
                    .into_iter()
                    .map(|s| cfg.mine_node(paths, s)),
            )
        {
            if let Some(pid) = sys::port_owner(node.port) {
                println!(
                    "  {:<10} laeuft noch (PID {pid}, Port {}) - 'terranova start' uebernimmt ihn",
                    node.name, node.port
                );
                found = true;
            }
        }
        if !found {
            println!("Es laeuft nichts.");
        }
        return ExitCode::SUCCESS;
    }

    match c.get("/api/status") {
        Ok((200, body)) => {
            let v: serde_json::Value = serde_json::from_str(&body).unwrap_or_default();
            println!("Laufzeit: {}", v["runtime"].as_str().unwrap_or("?"));
            for n in v["nodes"].as_array().into_iter().flatten() {
                let up = n["uptime_s"].as_u64().map_or("-".into(), fmt_secs);
                println!(
                    "  {:<10} {:<10} Port {:<6} PID {:<8} {:>7}{}",
                    n["name"].as_str().unwrap_or("?"),
                    n["status"].as_str().unwrap_or("?"),
                    n["port"].as_u64().unwrap_or(0),
                    n["pid"].as_u64().map_or("-".to_string(), |p| p.to_string()),
                    up,
                    if n["control"] == "adopted" {
                        "  uebernommen"
                    } else {
                        ""
                    },
                );
            }
            print_web(&v["web"]);
            ExitCode::SUCCESS
        }
        other => report(other, ""),
    }
}

/// Die beiden Dinge, die im Browser landen. Die Seite liefert Terranova
/// selbst aus, die Karte steckt im Server-Prozess - deshalb steht bei ihr
/// kein PID, sondern nur, ob sie antwortet.
fn print_web(web: &serde_json::Value) {
    let site = &web["site"];
    let map = &web["map"];
    if site.is_null() && map["status"] == "off" {
        return;
    }
    println!("Web:");
    if let Some(port) = site["port"].as_u64() {
        println!(
            "  {:<10} {:<10} Port {:<6} {}{}",
            "website",
            "ready",
            port,
            site["url"].as_str().unwrap_or(""),
            if site["public"] == true {
                "   (aus dem Netz erreichbar)"
            } else {
                "   (nur dieser Rechner)"
            },
        );
    }
    if map["status"] != "off" {
        let worlds = map["worlds"].as_u64().unwrap_or(0);
        println!(
            "  {:<10} {:<10} Port {:<6} {}   {}",
            "karte",
            map["status"].as_str().unwrap_or("?"),
            map["port"].as_u64().unwrap_or(0),
            if map["url"].as_str().unwrap_or("").is_empty() {
                map["local"].as_str().unwrap_or("")
            } else {
                map["url"].as_str().unwrap_or("")
            },
            {
                let host = map["server"].as_str().unwrap_or("?");
                match map["status"].as_str().unwrap_or("") {
                    "waiting" => format!(
                        "wartet auf {host} ({})",
                        map["server_status"].as_str().unwrap_or("stopped")
                    ),
                    "down" => format!("{host} laeuft, aber der Kartenport antwortet nicht"),
                    _ if worlds == 0 => "noch keine Kacheln gerendert".to_string(),
                    _ => format!("{worlds} Welten"),
                }
            },
        );
    }
}

fn fmt_secs(s: u64) -> String {
    if s < 90 {
        format!("{s}s")
    } else if s < 5400 {
        format!("{}m", s / 60)
    } else {
        format!("{}h{:02}m", s / 3600, (s % 3600) / 60)
    }
}

/// Konsole mitlesen und Befehle eintippen.
fn console(paths: &Paths, cfg: &Config, names: &[String]) -> ExitCode {
    let Some(name) = names.first() else {
        eprintln!("terranova: welcher Server?");
        return ExitCode::from(2);
    };
    let c = match need_supervisor(paths, cfg) {
        Ok(c) => c,
        Err(e) => return e,
    };

    // Eingaben in einem eigenen Thread: der Ereignisstrom soll weiterlaufen.
    let name2 = name.clone();
    let c2 = Client::new(paths, cfg).ok();
    std::thread::spawn(move || {
        let stdin = std::io::stdin();
        for line in stdin.lock().lines().map_while(Result::ok) {
            let line = line.trim().to_string();
            if line.is_empty() {
                continue;
            }
            if let Some(c) = &c2 {
                match c.command(&name2, &line) {
                    Ok((200, body)) => {
                        let reply = serde_json::from_str::<serde_json::Value>(&body)
                            .ok()
                            .and_then(|v| v["reply"].as_str().map(String::from))
                            .unwrap_or_default();
                        if !reply.trim().is_empty() {
                            println!("{}", reply.trim_end());
                        }
                    }
                    Ok((_, body)) => eprintln!("[terranova] {}", error_of(&body)),
                    Err(e) => eprintln!("[terranova] {e}"),
                }
            }
        }
    });

    println!("[terranova] Konsole von {name}. Eingaben gehen an den Server, Strg+C beendet nur das Zusehen.");
    let r = c.follow(&format!("/api/nodes/{name}/console"), None, |ev| {
        match ev.name.as_str() {
            "gap" => println!("[terranova] {}", ev.data),
            _ => println!("{}", ev.data),
        }
        let _ = std::io::stdout().flush();
        true
    });
    match r {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("terranova: {e}");
            ExitCode::FAILURE
        }
    }
}

fn logs(paths: &Paths, cfg: &Config, names: &[String], n: usize) -> ExitCode {
    let Some(name) = names.first() else {
        eprintln!("terranova: welcher Server?");
        return ExitCode::from(2);
    };
    let c = match need_supervisor(paths, cfg) {
        Ok(c) => c,
        Err(e) => return e,
    };
    match c.get(&format!("/api/nodes/{name}/logs?n={n}")) {
        Ok((200, body)) => {
            let v: serde_json::Value = serde_json::from_str(&body).unwrap_or_default();
            for l in v["lines"].as_array().into_iter().flatten() {
                println!("{}", l["text"].as_str().unwrap_or(""));
            }
            ExitCode::SUCCESS
        }
        other => report(other, ""),
    }
}

fn cmd(paths: &Paths, cfg: &Config, values: &[String]) -> ExitCode {
    let Some((name, rest)) = values.split_first() else {
        eprintln!("terranova: welcher Server, welcher Befehl?");
        return ExitCode::from(2);
    };
    if rest.is_empty() {
        eprintln!("terranova: welcher Befehl?");
        return ExitCode::from(2);
    }
    let c = match need_supervisor(paths, cfg) {
        Ok(c) => c,
        Err(e) => return e,
    };
    match c.command(name, &rest.join(" ")) {
        Ok((200, body)) => {
            let reply = serde_json::from_str::<serde_json::Value>(&body)
                .ok()
                .and_then(|v| v["reply"].as_str().map(String::from))
                .unwrap_or_default();
            if !reply.trim().is_empty() {
                println!("{}", reply.trim_end());
            }
            ExitCode::SUCCESS
        }
        other => report(other, ""),
    }
}

// --- Dungeons ---------------------------------------------------------------------

fn mine(paths: &Paths, cfg: &Config, args: &Args) -> ExitCode {
    let Some(action) = args.values.first().map(String::as_str) else {
        eprintln!("terranova: mine open | close | list | reap | templates");
        return ExitCode::from(2);
    };
    // Welche Vorlagen es gibt, steht im Dateisystem - danach zu fragen, soll
    // nicht voraussetzen, dass das Netzwerk schon laeuft.
    if action == "templates" {
        let static_names: Vec<String> = cfg.servers.keys().cloned().collect();
        let list = crate::mines::templates(&paths.templates(), &static_names);
        if list.is_empty() {
            println!("Keine Vorlage unter templates/ - ausser der gemeinsamen.");
        }
        for name in list {
            let mark = if name == cfg.mines.template {
                "  (Vorgabe)"
            } else {
                ""
            };
            println!("  {name}{mark}");
        }
        return ExitCode::SUCCESS;
    }

    let c = match need_supervisor(paths, cfg) {
        Ok(c) => c,
        Err(e) => return e,
    };

    match action {
        "open" => {
            let count: u8 = args.values.get(1).and_then(|v| v.parse().ok()).unwrap_or(1);
            let mut body = json!({ "count": count });
            if let Some(s) = args.slot {
                body["slot"] = json!(s);
            }
            if let Some(t) = &args.template {
                body["template"] = json!(t);
            }
            match c.post("/api/mines/open", body) {
                Ok((200, body)) => {
                    let v: serde_json::Value = serde_json::from_str(&body).unwrap_or_default();
                    for m in v["opened"].as_array().into_iter().flatten() {
                        println!(
                            "[terranova] {} offen auf Port {}",
                            m["name"].as_str().unwrap_or("?"),
                            m["port"].as_u64().unwrap_or(0)
                        );
                    }
                    ExitCode::SUCCESS
                }
                other => report(other, ""),
            }
        }
        "close" => {
            let Some(slot) = args
                .values
                .get(1)
                .and_then(|v| v.parse::<u8>().ok())
                .or(args.slot)
            else {
                eprintln!("terranova: welcher Dungeon?");
                return ExitCode::from(2);
            };
            report(
                c.post(&format!("/api/mines/{slot}/close"), json!({})),
                &format!(
                    "[terranova] mining-{slot} geschlossen - die Welt bleibt bis zum Abraeumen"
                ),
            )
        }
        "list" => match c.get("/api/mines") {
            Ok((200, body)) => {
                let v: serde_json::Value = serde_json::from_str(&body).unwrap_or_default();
                let list = v["mines"].as_array().cloned().unwrap_or_default();
                if list.is_empty() {
                    println!("Kein Dungeon offen. Plaetze: {}", v["slots"]);
                }
                for m in list {
                    println!(
                        "  {:<10} {:<9} Port {:<6} noch {}",
                        m["name"].as_str().unwrap_or("?"),
                        m["status"].as_str().unwrap_or("?"),
                        m["port"].as_u64().unwrap_or(0),
                        m["expires_in_s"].as_u64().map_or("?".into(), fmt_secs),
                    );
                }
                ExitCode::SUCCESS
            }
            other => report(other, ""),
        },
        "reap" => match c.post(
            "/api/mines/reap",
            json!({ "dry_run": args.dry_run, "stop_running": args.stop_running }),
        ) {
            Ok((200, body)) => {
                let v: serde_json::Value = serde_json::from_str(&body).unwrap_or_default();
                for line in v["reaped"].as_array().into_iter().flatten() {
                    println!("  {}", line.as_str().unwrap_or(""));
                }
                ExitCode::SUCCESS
            }
            other => report(other, ""),
        },
        other => {
            eprintln!("terranova: mine {other}? Es gibt open, close, list, reap.");
            ExitCode::from(2)
        }
    }
}

/// Oeffnet das Dashboard im Browser.
///
/// Die Seite selbst legt die Sitzung an - ein Lesezeichen darauf tut es
/// also genauso.
fn dashboard(paths: &Paths, cfg: &Config) -> ExitCode {
    let c = match need_supervisor(paths, cfg) {
        Ok(c) => c,
        Err(e) => return e,
    };
    let url = format!("http://127.0.0.1:{}/", c.port);
    println!("[terranova] {url}");
    // explorer.exe nimmt eine Adresse und oeffnet den eingestellten Browser.
    let _ = std::process::Command::new("explorer.exe").arg(&url).spawn();
    ExitCode::SUCCESS
}

// --- Wartung ------------------------------------------------------------------------

fn sync_cmd(paths: &Paths, cfg: &Config, which: &[String], dry_run: bool) -> ExitCode {
    let syncer = match Syncer::new(paths, cfg) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("terranova: {e}");
            return ExitCode::FAILURE;
        }
    };
    let mut nodes = Vec::new();
    if which.is_empty() {
        nodes.extend(cfg.server_nodes(paths));
        for slot in mines::existing(&paths.dynamic(), cfg.mines.slots) {
            nodes.push(cfg.mine_node(paths, slot));
        }
    } else {
        for name in which {
            match cfg.node(paths, name) {
                Some(n) => nodes.push(n),
                None => {
                    eprintln!("terranova: '{name}' gibt es nicht");
                    return ExitCode::FAILURE;
                }
            }
        }
    }

    let mut failed = false;
    for node in nodes {
        if node.template.is_none() {
            continue;
        }
        if !dry_run {
            if let Some(pid) = sys::port_owner(node.port) {
                let who = sys::image_name(pid).unwrap_or_else(|| format!("PID {pid}"));
                eprintln!(
                    "[sync] {}: laeuft ({who} auf Port {}) - uebersprungen, Windows sperrt die Jars",
                    node.name, node.port
                );
                continue;
            }
        }
        match syncer.sync(&node, dry_run) {
            Ok(r) => {
                let what = if r.nothing_to_do() {
                    "unveraendert".to_string()
                } else {
                    let mut parts = Vec::new();
                    if !r.copied.is_empty() {
                        parts.push(format!("{} kopiert", r.copied.len()));
                    }
                    if !r.pruned.is_empty() {
                        parts.push(format!(
                            "{} entfernt ({})",
                            r.pruned.len(),
                            r.pruned.join(", ")
                        ));
                    }
                    if r.props {
                        parts.push("server.properties".into());
                    }
                    if r.paper_global {
                        parts.push("paper-global.yml".into());
                    }
                    parts.join(", ")
                };
                let prefix = if dry_run {
                    "[sync, Probelauf]"
                } else {
                    "[sync]"
                };
                println!("{prefix} {:<10} {what}", node.name);
            }
            Err(e) => {
                eprintln!("[sync] {}: {e}", node.name);
                failed = true;
            }
        }
    }
    if failed {
        ExitCode::FAILURE
    } else {
        ExitCode::SUCCESS
    }
}

fn init(root: Option<&std::path::Path>) -> ExitCode {
    let dir = match root {
        Some(r) => r.to_path_buf(),
        None => match std::env::current_dir() {
            Ok(d) => d,
            Err(e) => {
                eprintln!("terranova: {e}");
                return ExitCode::FAILURE;
            }
        },
    };
    let file = dir.join(crate::paths::CONFIG_FILE);
    if file.exists() {
        println!("terranova: {} gibt es schon", file.display());
        return ExitCode::SUCCESS;
    }
    match std::fs::write(&file, crate::DEFAULT_CONFIG) {
        Ok(()) => {
            println!("terranova: {} angelegt", file.display());
            ExitCode::SUCCESS
        }
        Err(e) => {
            eprintln!("terranova: {}: {e}", file.display());
            ExitCode::FAILURE
        }
    }
}
