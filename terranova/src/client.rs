//! Die CLI als Gegenstelle des Supervisors.
//!
//! Jeder Befehl, der etwas mit laufenden Prozessen zu tun hat, geht hier
//! durch - genau wie das Dashboard. Es gibt keinen zweiten Weg, an die
//! Server heranzukommen.

use std::fs;
use std::io;
use std::path::Path;
use std::time::{Duration, Instant};

use serde::Deserialize;
use serde_json::json;

use crate::config::Config;
use crate::http;
use crate::paths::Paths;

const TIMEOUT: Duration = Duration::from_secs(30);

#[derive(Debug, Clone, Deserialize)]
pub struct SupervisorInfo {
    pub pid: u32,
    pub created: Option<u64>,
    pub port: u16,
}

pub struct Client {
    pub port: u16,
    token: String,
}

impl Client {
    pub fn new(paths: &Paths, cfg: &Config) -> io::Result<Client> {
        let token = fs::read_to_string(paths.api_token())
            .map(|t| t.trim().to_string())
            .unwrap_or_default();
        // Laeuft schon einer, gilt sein Port - die Config koennte seit
        // seinem Start geaendert worden sein.
        let port = read_info(paths).map_or(cfg.dashboard.port, |i| i.port);
        Ok(Client { port, token })
    }

    pub fn token(&self) -> &str {
        &self.token
    }

    pub fn get(&self, path: &str) -> io::Result<(u16, String)> {
        http::request(self.port, "GET", path, &self.token, None, TIMEOUT)
    }

    pub fn post(&self, path: &str, body: serde_json::Value) -> io::Result<(u16, String)> {
        http::request(
            self.port,
            "POST",
            path,
            &self.token,
            Some(body.to_string().as_bytes()),
            TIMEOUT,
        )
    }

    /// Antwortet der Supervisor?
    pub fn alive(&self) -> bool {
        http::request(
            self.port,
            "GET",
            "/api/health",
            &self.token,
            None,
            Duration::from_secs(2),
        )
        .is_ok_and(|(s, _)| s == 200)
    }

    pub fn wait_alive(&self, timeout: Duration) -> bool {
        let deadline = Instant::now() + timeout;
        while Instant::now() < deadline {
            if self.alive() {
                return true;
            }
            std::thread::sleep(Duration::from_millis(300));
        }
        false
    }

    /// Verfolgt einen Ereignisstrom, bis `f` false sagt oder die Gegenstelle
    /// geht.
    pub fn follow(
        &self,
        path: &str,
        since: Option<u64>,
        f: impl FnMut(http::Event) -> bool,
    ) -> io::Result<()> {
        http::sse(self.port, path, &self.token, since, f)
    }

    pub fn command(&self, node: &str, line: &str) -> io::Result<(u16, String)> {
        self.post(
            &format!("/api/nodes/{node}/command"),
            json!({ "line": line }),
        )
    }
}

/// Was der Supervisor ueber sich selbst hinterlegt.
pub fn read_info(paths: &Paths) -> Option<SupervisorInfo> {
    let text = fs::read_to_string(paths.supervisor_info()).ok()?;
    serde_json::from_str(&text).ok()
}

pub fn write_info(paths: &Paths, port: u16) -> io::Result<()> {
    let pid = std::process::id();
    let info = json!({ "pid": pid, "created": crate::win::created(pid), "port": port });
    fs::write(paths.supervisor_info(), info.to_string())
}

/// Laeuft wirklich noch ein Supervisor? PID allein genuegt nicht - sie wird
/// wiederverwendet.
pub fn supervisor_running(paths: &Paths) -> Option<SupervisorInfo> {
    let info = read_info(paths)?;
    let alive = crate::win::alive(info.pid)
        && info
            .created
            .is_none_or(|c| crate::win::created(info.pid) == Some(c));
    alive.then_some(info)
}

/// Kopiert die Programmdatei beiseite, bevor der Supervisor daraus startet.
///
/// Sonst sperrt der laufende Supervisor bin/terranova.exe, und ein
/// git pull koennte sie nicht mehr ersetzen.
pub fn shadow_copy(paths: &Paths) -> io::Result<std::path::PathBuf> {
    let exe = std::env::current_exe()?;
    let dir = paths.state_dir();
    fs::create_dir_all(&dir)?;
    let shadow = dir.join("terranova-laufend.exe");
    // Eine alte Kopie kann noch gesperrt sein; dann tut es auch sie.
    match fs::copy(&exe, &shadow) {
        Ok(_) => Ok(shadow),
        Err(_) if shadow.is_file() => Ok(shadow),
        Err(e) => Err(e),
    }
}

/// Startet den Supervisor losgeloest vom Fenster.
///
/// Er darf nicht an dieser Konsole haengen: beim Schliessen eines Fensters
/// bleiben einem Prozess etwa fuenf Sekunden, und ein Supervisor, der in
/// dieser Zeit stirbt, nimmt die Pipes aller Server mit.
pub fn spawn_detached(paths: &Paths, exe: &Path) -> io::Result<u32> {
    #[cfg(windows)]
    use std::os::windows::process::CommandExt as _;
    use std::process::{Command, Stdio};

    let mut cmd = Command::new(exe);
    cmd.arg("supervise")
        .arg("--root")
        .arg(&paths.root)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .current_dir(&paths.root);
    #[cfg(windows)]
    cmd.creation_flags(
        crate::win::DETACHED_PROCESS
            | crate::win::CREATE_NEW_PROCESS_GROUP
            | crate::win::CREATE_NO_WINDOW,
    );
    Ok(cmd.spawn()?.id())
}
