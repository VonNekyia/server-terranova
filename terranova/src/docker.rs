//! Dieselben Knoten, nur in Containern.
//!
//! Der Trick, der alles andere einfach haelt: die Container laufen im
//! Host-Netzwerk. Damit bedeutet 127.0.0.1:13306 im Container dasselbe wie
//! davor, und keine einzige Plugin-Config muss anders aussehen als nativ.
//! Ohne das muessten LuckPerms, HuskSync, Nations und der Rest je Laufzeit
//! umgeschrieben werden.
//!
//! Der Kindprozess ist hier `docker attach`, nicht `docker run`. Ein
//! angehaengtes `docker run -i` setzt StdinOnce: sobald dieser Client einmal
//! weg ist, nimmt der Container nie wieder eine Eingabe an - nach einem
//! Neustart des Supervisors waere die Konsole fuer immer tot.

use std::io;
use std::process::{Child, Command, Stdio};
use std::time::Duration;

use sha2::{Digest, Sha256};

use crate::backend::{Backend, Discovered, AIKAR, CONSOLE_FLAGS};
use crate::config::{Config, JavaFlags, NodeKind, NodeSpec};
use crate::paths::Paths;
use crate::sys;

pub struct Docker {
    project: String,
    cfg: Config,
}

fn docker() -> Command {
    let mut c = Command::new("docker");
    c.stdin(Stdio::null());
    sys::hide_window(&mut c);
    c
}

/// Ruft docker auf und gibt die Ausgabe zurueck.
fn run(args: &[&str]) -> io::Result<String> {
    let out = docker()
        .args(args)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()?;
    if !out.status.success() {
        return Err(io::Error::other(
            String::from_utf8_lossy(&out.stderr).trim().to_string(),
        ));
    }
    Ok(String::from_utf8_lossy(&out.stdout).trim().to_string())
}

impl Docker {
    pub fn new(paths: &Paths, cfg: &Config) -> Docker {
        // Zwei Checkouts auf derselben Maschine duerfen sich nicht in die
        // Quere kommen.
        let mut h = Sha256::new();
        h.update(paths.root.to_string_lossy().to_lowercase().as_bytes());
        let project = h
            .finalize()
            .iter()
            .take(4)
            .map(|b| format!("{b:02x}"))
            .collect();
        Docker {
            project,
            cfg: cfg.clone(),
        }
    }

    pub fn container(&self, node: &str) -> String {
        format!("tn-{}-{}", self.project, node)
    }

    /// Prueft, was der Docker-Rueckhalt koennen muss - mit klarer Ansage,
    /// wenn etwas fehlt.
    pub fn preflight(&self) -> Result<(), String> {
        run(&["info", "--format", "{{.ServerVersion}}"])
            .map_err(|e| format!("Docker antwortet nicht ({e}). Laeuft Docker Desktop?"))?;

        // Host-Netzwerk ist unter Docker Desktop standardmaessig aus. Ohne es
        // landen die Container in einem eigenen Netz, und 127.0.0.1 zeigt
        // dort auf den Container selbst - jede Datenbankverbindung eines
        // Plugins liefe ins Leere.
        let probe = format!("tn-{}-probe", self.project);
        let _ = run(&["rm", "-f", &probe]);
        let port = "25599";
        let started = run(&[
            "run",
            "-d",
            "--rm",
            "--name",
            &probe,
            "--network",
            "host",
            &self.cfg.deps.redis.image,
            "redis-server",
            "--port",
            port,
            "--bind",
            "127.0.0.1",
            "--save",
            "",
        ]);
        let result = match started {
            Err(e) => Err(format!("Probelauf laesst sich nicht starten: {e}")),
            Ok(_) => {
                let reachable = crate::deps::wait_until(
                    || crate::deps::redis_ready(25599),
                    Duration::from_secs(20),
                );
                if reachable {
                    Ok(())
                } else {
                    Err(
                        "Host-Netzwerk ist aus. In Docker Desktop: Settings -> Resources -> \
                         Network -> Enable host networking, dann Apply & restart. \
                         Sonst erreichen die Server weder MariaDB noch Redis. \
                         Alternativ in terranova.yml runtime: native setzen."
                            .to_string(),
                    )
                }
            }
        };
        let _ = run(&["rm", "-f", &probe]);
        result
    }

    /// Die Argumente, mit denen ein Knoten im Container laeuft.
    fn run_args(&self, node: &NodeSpec) -> Vec<String> {
        let name = self.container(&node.spec_name());
        let mut a: Vec<String> = vec![
            "run".into(),
            "-d".into(),
            // -i ohne Anhaengen: stdin bleibt offen und wieder anhaengbar.
            "-i".into(),
            "--name".into(),
            name,
            "--network".into(),
            "host".into(),
            // Neu gestartet wird von unserer Aufsicht, nicht von Docker -
            // sonst streiten sich zwei darum.
            "--restart".into(),
            "no".into(),
            "--stop-timeout".into(),
            node.stop_timeout.as_secs().to_string(),
            "--label".into(),
            format!("terranova.project={}", self.project),
            "--label".into(),
            format!("terranova.node={}", node.name),
        ];

        match node.kind {
            NodeKind::MariaDb => {
                a.extend([
                    "-e".into(),
                    "MARIADB_ALLOW_EMPTY_ROOT_PASSWORD=1".into(),
                    // Benanntes Volume statt Einhaengen aus dem Repository:
                    // InnoDB schreibt staendig fsync, und ueber die
                    // Windows-Freigabe ist das quaelend langsam.
                    "-v".into(),
                    format!("tn-{}-mariadb:/var/lib/mysql", self.project),
                    self.cfg.deps.mariadb.image.clone(),
                    format!("--port={}", self.cfg.deps.mariadb.port),
                    "--bind-address=127.0.0.1".into(),
                    "--max_allowed_packet=64M".into(),
                ]);
            }
            NodeKind::Redis => {
                a.extend([
                    self.cfg.deps.redis.image.clone(),
                    "redis-server".into(),
                    "--port".into(),
                    self.cfg.deps.redis.port.to_string(),
                    "--bind".into(),
                    "127.0.0.1".into(),
                    "--save".into(),
                    String::new(),
                    "--appendonly".into(),
                    "no".into(),
                ]);
            }
            _ => {
                let jar = crate::backend::find_jar(
                    &node.dir,
                    if node.is_proxy() {
                        "velocity"
                    } else {
                        "paper-"
                    },
                )
                .map(|p| {
                    p.file_name()
                        .unwrap_or_default()
                        .to_string_lossy()
                        .into_owned()
                })
                .unwrap_or_default();
                a.extend([
                    "-v".into(),
                    format!("{}:/srv", node.dir.display()),
                    "-w".into(),
                    "/srv".into(),
                    self.cfg.java.image.clone(),
                    "java".into(),
                    format!("-Xms{}M", node.memory.0),
                    format!("-Xmx{}M", node.memory.0),
                ]);
                if self.cfg.java.flags == JavaFlags::Aikar && !node.is_proxy() {
                    a.extend(AIKAR.iter().map(|s| s.to_string()));
                }
                a.extend(CONSOLE_FLAGS.iter().map(|s| s.to_string()));
                a.extend(self.cfg.java.extra.clone());
                a.extend(["-jar".into(), jar]);
                if !node.is_proxy() {
                    a.push("--nogui".into());
                }
            }
        }
        a
    }

    fn state_of(&self, container: &str) -> Option<String> {
        run(&["inspect", "-f", "{{.State.Status}}", container]).ok()
    }
}

/// Kleiner Helfer, damit der Containername auch fuer Abhaengigkeiten passt.
trait SpecName {
    fn spec_name(&self) -> String;
}
impl SpecName for NodeSpec {
    fn spec_name(&self) -> String {
        self.name.clone()
    }
}

impl Backend for Docker {
    fn spawn(&self, node: &NodeSpec) -> io::Result<Child> {
        let name = self.container(&node.name);
        match self.state_of(&name).as_deref() {
            Some("running") => {}
            Some(_) => {
                // Es gibt ihn schon, er steht nur.
                run(&["start", &name]).map_err(io::Error::other)?;
            }
            None => {
                let args = self.run_args(node);
                let refs: Vec<&str> = args.iter().map(String::as_str).collect();
                run(&refs).map_err(io::Error::other)?;
            }
        }

        // Anhaengen ist der eigentliche Kindprozess: seine Pipes sind die
        // Konsole des Servers. --sig-proxy=false, damit ein Strg+C hier nie
        // im Container landet.
        let mut cmd = docker();
        cmd.args(["attach", "--sig-proxy=false", &name])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        cmd.spawn()
    }

    fn discover(&self, node: &NodeSpec) -> Option<Discovered> {
        let name = self.container(&node.name);
        if self.state_of(&name).as_deref() != Some("running") {
            return None;
        }
        let pid = run(&["inspect", "-f", "{{.State.Pid}}", &name])
            .ok()
            .and_then(|s| s.parse().ok())
            .unwrap_or(0);
        Some(Discovered {
            pid,
            created: None,
            // Die Pruefung auf die Programmdatei gilt fuer native Prozesse;
            // hier buergt der Name des Containers.
            image: if node.kind == NodeKind::MariaDb {
                "mysqld".into()
            } else if node.kind == NodeKind::Redis {
                "redis".into()
            } else {
                "java".into()
            },
        })
    }

    /// Der sanfte Ausweg, den es nativ nicht gibt: SIGTERM laesst die JVM
    /// ihren Shutdown-Hook laufen, und Paper speichert.
    fn soft_stop(&self, node: &NodeSpec, timeout: Duration) -> bool {
        let name = self.container(&node.name);
        run(&["stop", "-t", &timeout.as_secs().to_string(), &name]).is_ok()
    }

    fn kill(&self, node: &NodeSpec, _pid: u32) -> bool {
        run(&["kill", &self.container(&node.name)]).is_ok()
    }
}

/// Legt die Datenbanken im Container an.
pub fn provision(d: &Docker, sql: &str) -> io::Result<()> {
    let name = d.container("mariadb");
    let out = docker()
        .args(["exec", &name, "mariadb", "-u", "root", "-e", sql])
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()?;
    if !out.status.success() {
        return Err(io::Error::other(
            String::from_utf8_lossy(&out.stderr).trim().to_string(),
        ));
    }
    Ok(())
}

/// Holt die Abbilder, damit der erste Start nicht minutenlang haengt.
pub fn pull_images(cfg: &Config, log: &mut dyn FnMut(&str)) {
    for image in [
        &cfg.java.image,
        &cfg.deps.mariadb.image,
        &cfg.deps.redis.image,
    ] {
        if run(&["image", "inspect", image]).is_ok() {
            continue;
        }
        log(&format!("Abbild {image} wird geholt..."));
        if let Err(e) = run(&["pull", image]) {
            log(&format!("{image}: {e}"));
        }
    }
}

/// Wie viel Arbeitsspeicher die Docker-Maschine hat, in MB.
///
/// Unter Windows laeuft alles in einer WSL2-Maschine, die ohne .wslconfig
/// nur die Haelfte des Rechners bekommt - viel weniger, als nativ zur
/// Verfuegung stuende.
pub fn mem_total_mb() -> Option<u64> {
    run(&["info", "--format", "{{.MemTotal}}"])
        .ok()?
        .parse::<u64>()
        .ok()
        .map(|b| b / 1024 / 1024)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testutil::EXAMPLE_CONFIG;

    fn fixture() -> Docker {
        let cfg = Config::parse(EXAMPLE_CONFIG).unwrap();
        Docker::new(&Paths::new("C:\\net\\terranova"), &cfg)
    }

    #[test]
    fn containernamen_sind_je_checkout_verschieden() {
        let cfg = Config::parse(EXAMPLE_CONFIG).unwrap();
        let a = Docker::new(&Paths::new("C:\\eins"), &cfg);
        let b = Docker::new(&Paths::new("C:\\zwei"), &cfg);
        assert_ne!(a.container("main"), b.container("main"));
        assert!(a.container("main").starts_with("tn-"));
        // Gross- und Kleinschreibung sind unter Windows dasselbe Verzeichnis
        let c = Docker::new(&Paths::new("C:\\EINS"), &cfg);
        assert_eq!(a.container("main"), c.container("main"));
    }

    #[test]
    fn server_bekommt_host_netz_und_sein_verzeichnis() {
        let d = fixture();
        let cfg = Config::parse(EXAMPLE_CONFIG).unwrap();
        let node = cfg.node(&Paths::new("C:\\net\\terranova"), "main").unwrap();
        let a = d.run_args(&node);
        assert!(a.contains(&"--network".to_string()) && a.contains(&"host".to_string()));
        assert!(a.iter().any(|x| x.ends_with(":/srv")));
        assert!(a.contains(&"-Xmx4096M".to_string()));
        assert!(a.contains(&"--nogui".to_string()));
        assert!(a.iter().any(|x| x.starts_with("terranova.node=main")));
        // Die Aufsicht startet neu, nicht Docker
        let i = a.iter().position(|x| x == "--restart").unwrap();
        assert_eq!(a[i + 1], "no");
    }

    #[test]
    fn der_proxy_bekommt_kein_nogui() {
        let d = fixture();
        let cfg = Config::parse(EXAMPLE_CONFIG).unwrap();
        let node = cfg
            .node(&Paths::new("C:\\net\\terranova"), "proxy")
            .unwrap();
        let a = d.run_args(&node);
        assert!(!a.contains(&"--nogui".to_string()));
        // Aikar-Flags sind fuer Paper, nicht fuer Velocity
        assert!(!a.contains(&"-XX:+UseG1GC".to_string()));
    }

    #[test]
    fn mariadb_bekommt_ein_benanntes_volume() {
        let d = fixture();
        let cfg = Config::parse(EXAMPLE_CONFIG).unwrap();
        let paths = Paths::new("C:\\net\\terranova");
        let node = NodeSpec {
            name: "mariadb".into(),
            kind: NodeKind::MariaDb,
            dir: paths.mariadb_home(),
            template: None,
            port: cfg.deps.mariadb.port,
            rcon_port: None,
            memory: crate::config::Mem(0),
            stop_timeout: Duration::from_secs(60),
            motd: None,
        };
        let a = d.run_args(&node);
        assert!(a.iter().any(|x| x.ends_with(":/var/lib/mysql")));
        assert!(a.contains(&"--bind-address=127.0.0.1".to_string()));
        assert!(a.contains(&"--port=13306".to_string()));
    }
}
