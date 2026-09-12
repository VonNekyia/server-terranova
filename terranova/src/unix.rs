//! Alles, was nur unter Unix geht, an einer Stelle.
//!
//! Gegenstueck zu win.rs: dieselben Funktionen mit derselben Bedeutung. Der
//! Rest des Programms kennt nur `sys::` und merkt nicht, worauf er laeuft.
//!
//! Die Auskuenfte ueber fremde Prozesse kommen aus /proc statt aus einer API.
//! Das ist der uebliche Weg unter Linux und braucht keine Rechte, solange nur
//! gelesen wird - genau das tut die Uebernahme nach einem Absturz.

use std::ffi::OsStr;
use std::fs;
use std::io::{self, Read as _};
use std::mem;
use std::os::unix::process::CommandExt as _;
use std::path::Path;
use std::process::{Command, Stdio};
use std::ptr;
use std::sync::OnceLock;
use std::thread;
use std::time::{Duration, Instant};

/// Gegenstueck zu CREATE_NO_WINDOW. Hier gibt es kein Fenster, das aufgehen
/// koennte - der Aufruf bleibt trotzdem stehen, damit die Aufrufstellen auf
/// beiden Systemen gleich aussehen.
pub fn hide_window(cmd: &mut Command) -> &mut Command {
    cmd
}

// --- Prozesse -----------------------------------------------------------------

/// Laeuft der Prozess noch?
pub fn alive(pid: u32) -> bool {
    // Signal 0 wird nur zugestellt, nicht gesendet: 0 heisst da, EPERM heisst
    // da, aber fremd, ESRCH heisst weg.
    if unsafe { libc::kill(pid as libc::pid_t, 0) } == 0 {
        return true;
    }
    io::Error::last_os_error().raw_os_error() == Some(libc::EPERM)
}

/// Startzeit in Uhrticks seit dem Hochfahren. Zusammen mit der PID eindeutig -
/// eine wiederverwendete PID hat eine andere Startzeit. Daran erkennt die
/// Uebernahme, ob der Prozess wirklich noch der alte ist.
///
/// Der Wert bedeutet etwas anderes als unter Windows und wird auch nur so
/// benutzt: verglichen, nie gedeutet.
pub fn created(pid: u32) -> Option<u64> {
    let stat = fs::read_to_string(format!("/proc/{pid}/stat")).ok()?;
    // Der Programmname steht in Klammern und darf selbst Leerzeichen und
    // Klammern enthalten. Deshalb ab der letzten schliessenden Klammer zaehlen
    // und nicht am Anfang.
    let rest = &stat[stat.rfind(')')? + 1..];
    // Danach ist das erste Feld der Zustand, starttime ist Feld 22 - also der
    // zwanzigste Eintrag hier.
    rest.split_whitespace().nth(19)?.parse().ok()
}

/// Vollstaendiger Pfad der Programmdatei.
pub fn image(pid: u32) -> Option<String> {
    if let Ok(p) = fs::read_link(format!("/proc/{pid}/exe")) {
        return Some(p.to_string_lossy().into_owned());
    }
    // Ohne Rechte am fremden Prozess bleibt nur comm. Der Name ist dort auf
    // 15 Zeichen gekuerzt, reicht aber, um java von mariadbd zu unterscheiden.
    let comm = fs::read_to_string(format!("/proc/{pid}/comm")).ok()?;
    let comm = comm.trim();
    (!comm.is_empty()).then(|| comm.to_string())
}

/// Nur der Dateiname der Programmdatei, klein geschrieben: "java".
pub fn image_name(pid: u32) -> Option<String> {
    let path = image(pid)?;
    let name = path.rsplit('/').next().unwrap_or(&path);
    Some(name.to_ascii_lowercase())
}

/// Wartet hoechstens `ms` Millisekunden auf das Ende. true = beendet (oder
/// nicht mehr auffindbar).
///
/// Gepollt statt gewartet: waitpid gibt es nur fuer eigene Kinder, und die
/// Uebernahme nach einem Absturz hat es mit fremden zu tun.
pub fn wait_exit(pid: u32, ms: u32) -> bool {
    let deadline = Instant::now() + Duration::from_millis(u64::from(ms));
    loop {
        if !alive(pid) {
            return true;
        }
        if Instant::now() >= deadline {
            return false;
        }
        thread::sleep(Duration::from_millis(50));
    }
}

/// Beendet einen Prozess hart. Nur als letztes Mittel - ein Server verliert
/// dabei, was er nicht gespeichert hat.
pub fn terminate(pid: u32) -> bool {
    unsafe { libc::kill(pid as libc::pid_t, libc::SIGKILL) == 0 }
}

/// Bittet den Prozess zu gehen.
///
/// Anders als unter Windows ist das hier ein echter Weg: die JVM behandelt
/// SIGTERM ueber ihre Abschalthaken, Paper speichert die Welt und beendet
/// sich selbst. Wo unter Windows nur RCON oder die Konsole bleibt, genuegt
/// hier ein Signal - auch dann noch, wenn die Konsole des Servers schon
/// verstopft ist.
pub fn soft_stop(pid: u32) -> bool {
    unsafe { libc::kill(pid as libc::pid_t, libc::SIGTERM) == 0 }
}

/// Startet ein Programm losgeloest: eigene Prozessgruppe, keine geerbten
/// Kanaele.
///
/// Beides aus demselben Grund wie unter Windows. Ohne eigene Gruppe nimmt ein
/// Strg+C im Startfenster den Supervisor mit; mit geerbter Ausgabepipe kaeme
/// `terranova start --detach` nie zurueck, solange er lebt. Dass das Kind
/// danach keine Standardausgabe hat, ist richtig so - der Supervisor schreibt
/// in sein Logbuch, nicht ins Fenster.
pub fn spawn_detached(exe: &Path, args: &[&OsStr], cwd: &Path) -> io::Result<u32> {
    let child = Command::new(exe)
        .args(args)
        .current_dir(cwd)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .process_group(0)
        .spawn()?;
    Ok(child.id())
}

// --- Netz -----------------------------------------------------------------------

/// Welcher Prozess horcht auf diesem TCP-Port? IPv4 und IPv6: Velocity horcht
/// auf [::]:25565.
pub fn port_owner(port: u16) -> Option<u32> {
    let inodes = listening_inodes(port);
    if inodes.is_empty() {
        return None;
    }
    owner_of(&inodes)
}

/// Die Inodes der Sockets, die auf diesem Port horchen.
///
/// /proc/net/tcp fuehrt je Zeile einen Socket: Feld 2 ist "Adresse:Port" in
/// Hex, Feld 4 der Zustand (0A = LISTEN), Feld 10 die Inode.
fn listening_inodes(port: u16) -> Vec<u64> {
    let mut out = Vec::new();
    for table in ["/proc/net/tcp", "/proc/net/tcp6"] {
        let Ok(text) = fs::read_to_string(table) else {
            continue;
        };
        for line in text.lines().skip(1) {
            let f: Vec<&str> = line.split_whitespace().collect();
            if f.len() < 10 || f[3] != "0A" {
                continue;
            }
            let Some((_, hex)) = f[1].rsplit_once(':') else {
                continue;
            };
            if u16::from_str_radix(hex, 16).ok() != Some(port) {
                continue;
            }
            if let Ok(ino) = f[9].parse::<u64>() {
                out.push(ino);
            }
        }
    }
    out
}

/// Wer haelt einen dieser Sockets offen? Die Dateideskriptoren unter
/// /proc/<pid>/fd zeigen als Symlink auf "socket:[inode]".
///
/// Fremde Prozesse duerfen wir nicht durchsuchen; deren fd-Verzeichnis
/// bleibt uns verschlossen und wird uebersprungen. Fuer den Zweck genuegt
/// das: gesucht sind die eigenen Server.
fn owner_of(inodes: &[u64]) -> Option<u32> {
    let wanted: Vec<String> = inodes.iter().map(|i| format!("socket:[{i}]")).collect();
    for entry in fs::read_dir("/proc").ok()?.flatten() {
        let Some(pid) = entry
            .file_name()
            .to_str()
            .and_then(|s| s.parse::<u32>().ok())
        else {
            continue;
        };
        let Ok(fds) = fs::read_dir(entry.path().join("fd")) else {
            continue;
        };
        for fd in fds.flatten() {
            let Ok(target) = fs::read_link(fd.path()) else {
                continue;
            };
            if wanted.iter().any(|w| *w == target.to_string_lossy()) {
                return Some(pid);
            }
        }
    }
    None
}

// --- Zeit und Zufall ------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LocalTime {
    pub year: u16,
    pub month: u16,
    pub day: u16,
    pub hour: u16,
    pub minute: u16,
    pub second: u16,
}

pub fn local_time() -> LocalTime {
    let now = unsafe { libc::time(ptr::null_mut()) };
    let mut tm: libc::tm = unsafe { mem::zeroed() };
    unsafe { libc::localtime_r(&now, &mut tm) };
    LocalTime {
        year: (tm.tm_year + 1900) as u16,
        month: (tm.tm_mon + 1) as u16,
        day: tm.tm_mday as u16,
        hour: tm.tm_hour as u16,
        minute: tm.tm_min as u16,
        second: tm.tm_sec as u16,
    }
}

pub fn random(buf: &mut [u8]) {
    let mut f = fs::File::open("/dev/urandom").expect("/dev/urandom laesst sich nicht oeffnen");
    f.read_exact(buf).expect("/dev/urandom liefert nichts");
}

// --- Signale ----------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CtrlEvent {
    /// Strg+C
    Interrupt,
    /// Strg+Backslash
    Break,
    /// Das Terminal ist weg
    Close,
    /// Der Dienst soll enden - systemd schickt das beim Stoppen
    Shutdown,
}

static CTRL_HANDLER: OnceLock<fn(CtrlEvent) -> bool> = OnceLock::new();

/// Meldet Strg+C und die Beendigungssignale an `f`.
///
/// Bewusst nicht ueber einen Signalhandler: in einem solchen darf fast nichts
/// passieren, schon gar nicht Sperren nehmen oder Prozesse stoppen - und
/// genau das tut `f`. Stattdessen werden die Signale blockiert und ein
/// eigener Thread wartet mit sigwait darauf. Der laeuft dann als ganz
/// normaler Code und darf alles.
pub fn on_console_ctrl(f: fn(CtrlEvent) -> bool) {
    if CTRL_HANDLER.set(f).is_err() {
        return;
    }

    let mut set: libc::sigset_t = unsafe { mem::zeroed() };
    unsafe {
        libc::sigemptyset(&mut set);
        libc::sigaddset(&mut set, libc::SIGINT);
        libc::sigaddset(&mut set, libc::SIGTERM);
        libc::sigaddset(&mut set, libc::SIGHUP);
        libc::sigaddset(&mut set, libc::SIGQUIT);
        // In allen Threads blockieren, sonst raeumt die Standardbehandlung den
        // Prozess ab, bevor der Wartethread ueberhaupt an der Reihe war.
        libc::pthread_sigmask(libc::SIG_BLOCK, &set, ptr::null_mut());
    }

    thread::spawn(move || loop {
        let mut sig: libc::c_int = 0;
        if unsafe { libc::sigwait(&set, &mut sig) } != 0 {
            return;
        }
        let ev = match sig {
            libc::SIGINT => CtrlEvent::Interrupt,
            libc::SIGQUIT => CtrlEvent::Break,
            libc::SIGHUP => CtrlEvent::Close,
            libc::SIGTERM => CtrlEvent::Shutdown,
            _ => continue,
        };
        if let Some(h) = CTRL_HANDLER.get() {
            h(ev);
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::TcpListener;

    #[test]
    fn eigener_prozess() {
        let me = std::process::id();
        assert!(alive(me));
        assert!(created(me).is_some());
        assert!(image_name(me).is_some());
        assert!(!alive(u32::MAX - 3));
    }

    /// Zwei Aufrufe muessen denselben Wert liefern - sonst taugt er nicht,
    /// um einen Prozess wiederzuerkennen.
    #[test]
    fn startzeit_ist_stabil() {
        let me = std::process::id();
        assert_eq!(created(me), created(me));
    }

    #[test]
    fn horchender_port_gehoert_uns() {
        let l = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = l.local_addr().unwrap().port();
        assert_eq!(port_owner(port), Some(std::process::id()));
        drop(l);
    }

    #[test]
    fn freier_port_gehoert_niemandem() {
        let l = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = l.local_addr().unwrap().port();
        drop(l);
        assert_eq!(port_owner(port), None);
    }

    #[test]
    fn uhrzeit_ist_plausibel() {
        let t = local_time();
        assert!(t.year >= 2024 && t.hour < 24 && t.minute < 60);
        assert!(t.month >= 1 && t.month <= 12 && t.day >= 1 && t.day <= 31);
    }

    #[test]
    fn zufall_ist_nicht_leer() {
        let mut a = [0u8; 32];
        let mut b = [0u8; 32];
        random(&mut a);
        random(&mut b);
        assert_ne!(a, [0u8; 32]);
        assert_ne!(a, b);
    }
}
