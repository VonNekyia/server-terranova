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

#[cfg(windows)]
use std::os::windows::process::CommandExt as _;

use crate::config::{Config, NodeKind, NodeSpec};
use crate::paths::Paths;
use crate::win;

/// Die Flags aus network.ps1, unveraendert uebernommen.
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

/// Ein bereits laufender Prozess, den wir nicht selbst gestartet haben.
#[derive(Debug, Clone)]
pub struct Discovered {
    pub pid: u32,
    pub created: Option<u64>,
    pub image: String,
}

pub trait Backend: Send + Sync {
    fn name(&self) -> &'static str;

    /// Startet den Knoten mit angeschlossenen Pipes.
    fn spawn(&self, node: &NodeSpec) -> io::Result<Child>;

    /// Wer horcht schon auf dem Port des Knotens?
    fn discover(&self, node: &NodeSpec) -> Option<Discovered> {
        let pid = win::port_owner(node.port)?;
        Some(Discovered {
            pid,
            created: win::created(pid),
            image: win::image_name(pid).unwrap_or_default(),
        })
    }

    /// Sanfter Ausweg, wenn ueber stdin nichts mehr geht. Nativ gibt es
    /// keinen - unter Windows kennt eine JVM kein Signal.
    fn soft_stop(&self, _node: &NodeSpec, _timeout: Duration) -> bool {
        false
    }

    /// Letztes Mittel.
    fn kill(&self, _node: &NodeSpec, pid: u32) -> bool {
        win::terminate(pid)
    }
}

/// Welche Programmdateien zu einem Knoten passen - fuer die Uebernahme.
pub fn plausible_image(kind: NodeKind, image: &str) -> bool {
    let image = image.to_ascii_lowercase();
    match kind {
        NodeKind::MariaDb => image.contains("mysqld"),
        NodeKind::Redis => image.contains("redis"),
        _ => image.contains("java"),
    }
}

pub struct Native {
    java: String,
    mariadb_bin: PathBuf,
    mariadb_data: PathBuf,
    redis_home: PathBuf,
    mariadb_port: u16,
    redis_port: u16,
    aikar: bool,
    extra: Vec<String>,
}

impl Native {
    pub fn new(paths: &Paths, cfg: &Config) -> Native {
        Native {
            java: std::env::var("TERRANOVA_JAVA").unwrap_or_else(|_| cfg.java.path.clone()),
            mariadb_bin: crate::deps::mariadb_bin(paths, &cfg.deps.mariadb.version),
            mariadb_data: paths.mariadb_home().join("data"),
            redis_home: paths.redis_home(),
            mariadb_port: cfg.deps.mariadb.port,
            redis_port: cfg.deps.redis.port,
            aikar: cfg.java.flags == crate::config::JavaFlags::Aikar,
            extra: cfg.java.extra.clone(),
        }
    }

    fn java_command(&self, node: &NodeSpec, jar: &Path) -> Command {
        let mut c = Command::new(&self.java);
        c.arg(format!("-Xms{}M", node.memory.0));
        c.arg(format!("-Xmx{}M", node.memory.0));
        if self.aikar && !node.is_proxy() {
            c.args(AIKAR);
        }
        c.args(CONSOLE_FLAGS);
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
    fn name(&self) -> &'static str {
        "native"
    }

    fn spawn(&self, node: &NodeSpec) -> io::Result<Child> {
        let mut cmd = match node.kind {
            NodeKind::MariaDb => {
                let mut c = Command::new(self.mariadb_bin.join("mysqld.exe"));
                c.arg("--no-defaults")
                    .arg("--console")
                    .arg(format!("--port={}", self.mariadb_port))
                    // Bisher horchte MariaDB auf allen Schnittstellen, mit
                    // passwortlosem root@localhost. Hier nicht mehr.
                    .arg("--bind-address=127.0.0.1")
                    .arg(format!("--datadir={}", self.mariadb_data.display()))
                    .arg("--max_allowed_packet=64M");
                c
            }
            NodeKind::Redis => {
                let mut c = Command::new(self.redis_home.join("redis-server.exe"));
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
                let pattern = if node.is_proxy() { "velocity" } else { "paper-" };
                let jar = find_jar(&node.dir, pattern).ok_or_else(|| {
                    io::Error::new(
                        io::ErrorKind::NotFound,
                        format!("kein {pattern}*.jar in {}", node.dir.display()),
                    )
                })?;
                self.java_command(node, &jar)
            }
        };

        cmd.current_dir(&node.dir)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        #[cfg(windows)]
        cmd.creation_flags(win::CREATE_NO_WINDOW);
        cmd.spawn()
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
        assert!(is_ready_line(NodeKind::Server, "[00:21:24] [Server thread/INFO]: Done (56.579s)! For help, type \"help\""));
        // deutsches Dezimalkomma
        assert!(is_ready_line(NodeKind::Proxy, "[main/INFO]: Done (0,93s)!"));
        assert!(!is_ready_line(NodeKind::Server, "Preparing spawn area"));
        assert!(is_ready_line(NodeKind::MariaDb, "mysqld.exe: ready for connections."));
        assert!(is_ready_line(NodeKind::Redis, "* Ready to accept connections tcp"));
    }

    #[test]
    fn uebernahme_prueft_die_programmdatei() {
        assert!(plausible_image(NodeKind::Server, "C:\\jdk\\bin\\java.exe"));
        assert!(!plausible_image(NodeKind::Server, "nginx.exe"));
        assert!(plausible_image(NodeKind::MariaDb, "mysqld.exe"));
        assert!(!plausible_image(NodeKind::MariaDb, "java.exe"));
        assert!(plausible_image(NodeKind::Redis, "redis-server.exe"));
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
