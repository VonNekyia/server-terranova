//! Wie ein Knoten zu einem laufenden Prozess wird.
//!
//! Nativ ist das ein Java-Prozess, unter Docker der angehaengte
//! `docker attach`. Beide liefern dasselbe: einen Kindprozess mit
//! angeschlossenen Pipes. Alles darueber - Konsole, Aufsicht, Stoppen -
//! kennt den Unterschied nicht mehr.

use std::io;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::Duration;

use crate::config::{Config, NodeKind, NodeSpec};
use crate::paths::Paths;
use crate::sys;

/// Die Aikar-Flags, unveraendert aus dem alten Startskript uebernommen.
pub const AIKAR: &[&str] = &[
    "-XX:+AlwaysPreTouch",
    "-XX:+DisableExplicitGC",
    "-XX:+ParallelRefProcEnabled",
    "-XX:+PerfDisableSharedMem",
    "-XX:+UnlockExperimentalVMOptions",
    "-XX:+UseG1GC",
    "-XX:G1HeapRegionSize=8M",
    "-XX:G1HeapWastePercent=5",
    "-XX:G1MaxNewSizePercent=40",
    "-XX:G1MixedGCCountTarget=4",
    "-XX:G1MixedGCLiveThresholdPercent=90",
    "-XX:G1NewSizePercent=30",
    "-XX:G1RSetUpdatingPauseTimePercent=5",
    "-XX:G1ReservePercent=20",
    "-XX:InitiatingHeapOccupancyPercent=15",
    "-XX:MaxGCPauseMillis=200",
    "-XX:MaxTenuringThreshold=1",
    "-XX:SurvivorRatio=32",
    "-Dusing.aikars.flags=https://mcflags.emc.gs",
    "-Daikars.new.flags=true",
];

/// Damit Paper und Velocity ihre Konsole an einer Pipe richtig bedienen.
///
/// jline aus: TerminalConsoleAppender liest dann mit einem einfachen Reader
/// von System.in - sonst wartet es auf ein Terminal, das es hier nicht gibt,
/// und nimmt keine Befehle an.
///
/// stdout.encoding: sobald stdout eine Pipe ist, faellt die JVM auf die
/// Codepage der Konsole zurueck (auf einem deutschen Windows Cp1252). Ohne
/// diese beiden Zeilen kommen Umlaute als Fragezeichen an.
pub const CONSOLE_FLAGS: &[&str] = &[
    "-Dterminal.jline=false",
    "-Dterminal.ansi=false",
    "-Dstdout.encoding=UTF-8",
    "-Dstderr.encoding=UTF-8",
];

/// Zeitgrenzen fuer Netzverbindungen, die selbst keine setzen.
///
/// PlaceholderAPI prueft beim Laden seine Erweiterungen gegen eine Liste im
/// Netz - ueber eine HttpsURLConnection ohne Zeitgrenze. Nahm die Gegenstelle
/// die Verbindung an und antwortete dann nicht mehr, hing der Server-Thread
/// von main ueber zwanzig Minuten im TLS-Handshake: kein Start, keine
/// Konsole, kein RCON. Diese beiden Eigenschaften gelten fuer jede solche
/// Verbindung ohne eigene Grenze. Aus einem Plugin, das ewig wartet, wird
/// eines, das nach einer Minute Stille mit einem Fehler weitermacht.
///
/// Die Lesegrenze zaehlt Stille, nicht Dauer: ein Download, bei dem Daten
/// fliessen, darf so lange laufen, wie er braucht.
pub const NETWORK_FLAGS: &[&str] = &[
    "-Dsun.net.client.defaultConnectTimeout=15000",
    "-Dsun.net.client.defaultReadTimeout=60000",
];

/// Ein bereits laufender Prozess, den wir nicht selbst gestartet haben.
#[derive(Debug, Clone)]
pub struct Discovered {
    pub pid: u32,
    pub created: Option<u64>,
    pub image: String,
}

pub trait Backend: Send + Sync {
    /// Startet den Knoten mit angeschlossenen Pipes.
    fn spawn(&self, node: &NodeSpec) -> io::Result<Child>;

    /// Wer horcht schon auf dem Port des Knotens?
    fn discover(&self, node: &NodeSpec) -> Option<Discovered> {
        let pid = sys::port_owner(node.port)?;
        Some(Discovered {
            pid,
            created: sys::created(pid),
            image: sys::image_name(pid).unwrap_or_default(),
        })
    }

    /// Sanfter Ausweg, wenn ueber stdin nichts mehr geht. Nativ gibt es
    /// keinen - unter Windows kennt eine JVM kein Signal.
    fn soft_stop(&self, _node: &NodeSpec, _timeout: Duration) -> bool {
        false
    }

    /// Letztes Mittel.
    fn kill(&self, _node: &NodeSpec, pid: u32) -> bool {
        sys::terminate(pid)
    }
}

/// Welche Programmdateien zu einem Knoten passen - fuer die Uebernahme.
pub fn plausible_image(kind: NodeKind, image: &str) -> bool {
    let image = image.to_ascii_lowercase();
    match kind {
        // Unter Linux heisst das Serverprogramm mariadbd - mysqld ist dort
        // nur noch ein Zweitname und fehlt in manchen Paketen. Wer nur auf
        // mysqld prueft, erkennt seine eigene Datenbank nach einem Absturz
        // nicht wieder.
        NodeKind::MariaDb => image.contains("mysqld") || image.contains("mariadbd"),
        NodeKind::Redis => image.contains("redis") || image.contains("valkey"),
        _ => image.contains("java"),
    }
}

pub struct Native {
    paths: Paths,
    java: crate::config::Java,
    mariadb_server: PathBuf,
    mariadb_data: PathBuf,
    #[cfg(unix)]
    mariadb_socket: PathBuf,
    redis_server: PathBuf,
    mariadb_port: u16,
    redis_port: u16,
    aikar: bool,
    extra: Vec<String>,
}

impl Native {
    pub fn new(paths: &Paths, cfg: &Config) -> Native {
        Native {
            paths: paths.clone(),
            java: cfg.java.clone(),
            mariadb_server: crate::deps::mariadb_server(paths, &cfg.deps.mariadb.version),
            mariadb_data: paths.mariadb_home().join("data"),
            #[cfg(unix)]
            mariadb_socket: crate::deps::mariadb_socket(paths, cfg.deps.mariadb.port),
            redis_server: crate::deps::redis_server(paths),
            mariadb_port: cfg.deps.mariadb.port,
            redis_port: cfg.deps.redis.port,
            aikar: cfg.java.flags == crate::config::JavaFlags::Aikar,
            extra: cfg.java.extra.clone(),
        }
    }

    fn java_command(&self, node: &NodeSpec, jar: &Path) -> Command {
        // Erst hier aufgeloest: beim ersten Start wird ein fehlendes Java
        // nachgeladen, nachdem dieser Knoten schon angelegt ist.
        let (java, _) = crate::deps::resolve_java(&self.paths, &self.java);
        let mut c = Command::new(java);
        c.arg(format!("-Xms{}M", node.memory.0));
        c.arg(format!("-Xmx{}M", node.memory.0));
        if self.aikar && !node.is_proxy() {
            c.args(AIKAR);
        }
        c.args(CONSOLE_FLAGS);
        c.args(NETWORK_FLAGS);
        c.args(&self.extra);
        c.arg("-jar");
        // Nur der Dateiname: das Arbeitsverzeichnis ist ohnehin das des
        // Servers, und so bleibt die Kommandozeile lesbar.
        c.arg(jar.file_name().unwrap_or_default());
        if !node.is_proxy() {
            c.arg("--nogui");
        }
        c
    }
}

impl Backend for Native {
    fn spawn(&self, node: &NodeSpec) -> io::Result<Child> {
        let mut cmd = match node.kind {
            NodeKind::MariaDb => {
                let mut c = Command::new(&self.mariadb_server);
                c.arg("--no-defaults")
                    .arg("--console")
                    .arg(format!("--port={}", self.mariadb_port))
                    // Bisher horchte MariaDB auf allen Schnittstellen, mit
                    // passwortlosem root@localhost. Hier nicht mehr.
                    .arg("--bind-address=127.0.0.1")
                    .arg(format!("--datadir={}", self.mariadb_data.display()))
                    .arg("--max_allowed_packet=64M");
                // Sonst will mariadbd nach /run/mysqld schreiben und bricht als
                // gewoehnlicher Benutzer sofort ab - siehe deps::mariadb_socket.
                // Unter Windows gibt es keinen Unix-Socket.
                #[cfg(unix)]
                c.arg(format!("--socket={}", self.mariadb_socket.display()))
                    .arg(format!(
                        "--pid-file={}",
                        self.mariadb_data.with_file_name("mysqld.pid").display()
                    ));
                c
            }
            NodeKind::Redis => {
                let mut c = Command::new(&self.redis_server);
                c.arg("--port")
                    .arg(self.redis_port.to_string())
                    .arg("--bind")
                    .arg("127.0.0.1")
                    // Ohne Persistenz: massgeblich ist MariaDB, Redis ist nur
                    // Zwischenlage. So kann auch keine kaputte RDB-Datei den
                    // Start blockieren.
                    .arg("--save")
                    .arg("")
                    .arg("--appendonly")
                    .arg("no");
                c
            }
            _ => {
                let pattern = if node.is_proxy() {
                    "velocity"
                } else {
                    "paper-"
                };
                let jar = find_jar(&node.dir, pattern).ok_or_else(|| {
                    io::Error::new(
                        io::ErrorKind::NotFound,
                        format!("kein {pattern}*.jar in {}", node.dir.display()),
                    )
                })?;
                self.java_command(node, &jar)
            }
        };

        // Das Arbeitsverzeichnis muss es geben, sonst scheitert der Start
        // mit einem nackten "No such file or directory" - auch wenn das
        // Programm selbst da ist. Unter Windows legt der Download von Redis
        // runtime/redis nebenbei an; unter Linux kommt Redis aus dem Paket,
        // und niemand legte das Verzeichnis je an.
        std::fs::create_dir_all(&node.dir).map_err(|e| {
            io::Error::new(
                e.kind(),
                format!("Verzeichnis {} anlegen: {e}", node.dir.display()),
            )
        })?;
        cmd.current_dir(&node.dir)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        sys::hide_window(&mut cmd);
        // Ein nacktes "No such file or directory" sagt nicht, ob das Programm
        // fehlt oder das Verzeichnis - also beides dazuschreiben.
        cmd.spawn().map_err(|e| {
            io::Error::new(
                e.kind(),
                format!(
                    "{e} (Programm {}, Verzeichnis {})",
                    Path::new(cmd.get_program()).display(),
                    node.dir.display()
                ),
            )
        })
    }

    /// Unter Unix gibt es einen echten sanften Ausweg: SIGTERM. Die JVM
    /// behandelt es ueber ihre Abschalthaken, Paper speichert die Welt und
    /// beendet sich selbst - auch dann noch, wenn seine Konsole nicht mehr
    /// annimmt.
    ///
    /// Unter Windows bleibt es bei der Vorgabe aus dem Trait: dort kennt eine
    /// JVM kein Signal, das sie sauber beendet.
    #[cfg(unix)]
    fn soft_stop(&self, node: &NodeSpec, timeout: Duration) -> bool {
        let Some(found) = self.discover(node) else {
            return true;
        };
        if !sys::soft_stop(found.pid) {
            return false;
        }
        let ms = timeout.as_millis().min(u128::from(u32::MAX)) as u32;
        sys::wait_exit(found.pid, ms)
    }
}

/// Das erste Jar im Verzeichnis, dessen Name mit `prefix` beginnt.
pub fn find_jar(dir: &Path, prefix: &str) -> Option<PathBuf> {
    let mut found: Vec<PathBuf> = std::fs::read_dir(dir)
        .into_iter()
        .flatten()
        .flatten()
        .map(|e| e.path())
        .filter(|p| {
            p.is_file()
                && p.extension().is_some_and(|e| e.eq_ignore_ascii_case("jar"))
                && p.file_name()
                    .is_some_and(|n| n.to_string_lossy().starts_with(prefix))
        })
        .collect();
    found.sort();
    found.into_iter().next()
}

/// Der Konsolenbefehl, mit dem sich dieser Knoten selbst beendet.
pub fn stop_command(kind: NodeKind) -> Option<&'static str> {
    match kind {
        // Velocity kennt shutdown (end ist nur ein anderer Name dafuer).
        NodeKind::Proxy => Some("shutdown"),
        NodeKind::Server | NodeKind::Mine(_) => Some("stop"),
        // Datenbanken bekommen ihr Ende ueber ihr eigenes Protokoll.
        NodeKind::MariaDb | NodeKind::Redis => None,
    }
}

/// Woran man sieht, dass ein Server oben ist.
pub fn is_ready_line(kind: NodeKind, line: &str) -> bool {
    match kind {
        NodeKind::Proxy | NodeKind::Server | NodeKind::Mine(_) => {
            // "Done (41.647s)! For help, type "help"" - Velocity schreibt auf
            // einem deutschen System "Done (0,93s)!".
            line.contains("Done (") && line.contains("s)!")
        }
        NodeKind::MariaDb => line.contains("ready for connections"),
        NodeKind::Redis => line.contains("Ready to accept connections"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stoppbefehle() {
        assert_eq!(stop_command(NodeKind::Proxy), Some("shutdown"));
        assert_eq!(stop_command(NodeKind::Server), Some("stop"));
        assert_eq!(stop_command(NodeKind::Mine(3)), Some("stop"));
        assert_eq!(stop_command(NodeKind::MariaDb), None);
    }

    #[test]
    fn fertigmeldungen() {
        assert!(is_ready_line(
            NodeKind::Server,
            "[00:21:24] [Server thread/INFO]: Done (56.579s)! For help, type \"help\""
        ));
        // deutsches Dezimalkomma
        assert!(is_ready_line(NodeKind::Proxy, "[main/INFO]: Done (0,93s)!"));
        assert!(!is_ready_line(NodeKind::Server, "Preparing spawn area"));
        assert!(is_ready_line(
            NodeKind::MariaDb,
            "mysqld.exe: ready for connections."
        ));
        assert!(is_ready_line(
            NodeKind::Redis,
            "* Ready to accept connections tcp"
        ));
    }

    #[test]
    fn uebernahme_prueft_die_programmdatei() {
        assert!(plausible_image(NodeKind::Server, "C:\\jdk\\bin\\java.exe"));
        assert!(!plausible_image(NodeKind::Server, "nginx.exe"));
        assert!(plausible_image(NodeKind::MariaDb, "mysqld.exe"));
        assert!(!plausible_image(NodeKind::MariaDb, "java.exe"));
        assert!(plausible_image(NodeKind::Redis, "redis-server.exe"));
        // wie es unter Linux heisst
        assert!(plausible_image(
            NodeKind::Server,
            "/usr/lib/jvm/jdk-25/bin/java"
        ));
        assert!(plausible_image(NodeKind::MariaDb, "/usr/sbin/mariadbd"));
        assert!(plausible_image(NodeKind::Redis, "/usr/bin/redis-server"));
        assert!(plausible_image(NodeKind::Redis, "/usr/bin/valkey-server"));
    }

    #[test]
    fn jar_wird_nach_praefix_gefunden() {
        let dir = crate::testutil::tempdir("findjar");
        std::fs::write(dir.join("paper-26.2-123.jar"), b"x").unwrap();
        std::fs::write(dir.join("plugin.jar"), b"x").unwrap();
        assert_eq!(
            find_jar(&dir, "paper-").unwrap().file_name().unwrap(),
            "paper-26.2-123.jar"
        );
        assert!(find_jar(&dir, "velocity").is_none());
    }
}
