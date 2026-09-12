//! Alles, was nur ueber die Windows-API geht, an einer Stelle.
//!
//! Die Zugriffsrechte und Flags stehen als Zahlen hier, statt ueber die
//! windows-sys-Namen importiert zu werden: die Werte sind seit Jahrzehnten
//! fest, die Modulpfade in windows-sys dagegen wandern zwischen Versionen.

use std::ffi::{c_void, OsStr};
use std::mem;
use std::os::windows::ffi::OsStrExt as _;
use std::os::windows::process::CommandExt as _;
use std::path::Path;
use std::process::Command;
use std::ptr;
use std::sync::OnceLock;

use windows_sys::Win32::Foundation::{CloseHandle, FILETIME, HANDLE, SYSTEMTIME};
use windows_sys::Win32::NetworkManagement::IpHelper::{
    GetExtendedTcpTable, TCP_TABLE_OWNER_PID_LISTENER,
};
use windows_sys::Win32::Security::Cryptography::{
    BCryptGenRandom, BCRYPT_USE_SYSTEM_PREFERRED_RNG,
};
use windows_sys::Win32::System::Console::SetConsoleCtrlHandler;
use windows_sys::Win32::System::SystemInformation::GetLocalTime;
use windows_sys::Win32::System::Threading::{
    CreateProcessW, GetExitCodeProcess, GetProcessTimes, OpenProcess, QueryFullProcessImageNameW,
    TerminateProcess, WaitForSingleObject, PROCESS_INFORMATION, STARTUPINFOW,
};

// --- Flags fuer std::process::Command::creation_flags ----------------------

/// Kein Konsolenfenster. Jeder Kindprozess des Supervisors bekommt das - ein
/// losgeloester Supervisor, der ein Konsolenprogramm ohne startet (docker,
/// mysql, curl), wuerde sonst ein sichtbares Fenster aufreissen. Nebenbei
/// bekommt so jeder Server eine eigene, unsichtbare Konsole, und ein Strg+C
/// im Startfenster erreicht Java nie direkt.
pub const CREATE_NO_WINDOW: u32 = 0x0800_0000;
pub const DETACHED_PROCESS: u32 = 0x0000_0008;
pub const CREATE_NEW_PROCESS_GROUP: u32 = 0x0000_0200;

/// Startet ein Hilfsprogramm ohne sichtbares Fenster.
///
/// Steht hier, damit die Aufrufstellen kein `creation_flags` und kein
/// `#[cfg(windows)]` mehr brauchen - unter Unix ist es eine leere Geste.
pub fn hide_window(cmd: &mut Command) -> &mut Command {
    cmd.creation_flags(CREATE_NO_WINDOW)
}

const PROCESS_TERMINATE: u32 = 0x0001;
const PROCESS_QUERY_LIMITED_INFORMATION: u32 = 0x1000;
const SYNCHRONIZE: u32 = 0x0010_0000;
const STILL_ACTIVE: u32 = 259;
const WAIT_OBJECT_0: u32 = 0;
const AF_INET: u32 = 2;
const AF_INET6: u32 = 23;

struct Handle(HANDLE);

impl Drop for Handle {
    fn drop(&mut self) {
        unsafe {
            CloseHandle(self.0);
        }
    }
}

fn open(pid: u32, access: u32) -> Option<Handle> {
    let h = unsafe { OpenProcess(access, 0, pid) };
    (!h.is_null()).then_some(Handle(h))
}

// --- Prozesse -----------------------------------------------------------------

/// Laeuft der Prozess noch?
pub fn alive(pid: u32) -> bool {
    let Some(h) = open(pid, PROCESS_QUERY_LIMITED_INFORMATION) else {
        return false;
    };
    let mut code = 0u32;
    (unsafe { GetExitCodeProcess(h.0, &mut code) } != 0) && code == STILL_ACTIVE
}

/// Erzeugungszeit (100 ns seit 1601). Zusammen mit der PID eindeutig - eine
/// wiederverwendete PID hat eine andere Erzeugungszeit. Daran erkennt die
/// Uebernahme nach einem Absturz, ob der Prozess wirklich noch der alte ist.
pub fn created(pid: u32) -> Option<u64> {
    let h = open(pid, PROCESS_QUERY_LIMITED_INFORMATION)?;
    let mut c: FILETIME = unsafe { mem::zeroed() };
    let mut e: FILETIME = unsafe { mem::zeroed() };
    let mut k: FILETIME = unsafe { mem::zeroed() };
    let mut u: FILETIME = unsafe { mem::zeroed() };
    if unsafe { GetProcessTimes(h.0, &mut c, &mut e, &mut k, &mut u) } == 0 {
        return None;
    }
    Some((u64::from(c.dwHighDateTime) << 32) | u64::from(c.dwLowDateTime))
}

/// Vollstaendiger Pfad der Programmdatei.
pub fn image(pid: u32) -> Option<String> {
    let h = open(pid, PROCESS_QUERY_LIMITED_INFORMATION)?;
    let mut buf = vec![0u16; 1024];
    let mut len = buf.len() as u32;
    if unsafe { QueryFullProcessImageNameW(h.0, 0, buf.as_mut_ptr(), &mut len) } == 0 {
        return None;
    }
    Some(String::from_utf16_lossy(&buf[..len as usize]))
}

/// Nur der Dateiname der Programmdatei, klein geschrieben: "java.exe".
pub fn image_name(pid: u32) -> Option<String> {
    let path = image(pid)?;
    let name = path.rsplit(['\\', '/']).next().unwrap_or(&path);
    Some(name.to_ascii_lowercase())
}

/// Wartet hoechstens `ms` Millisekunden auf das Ende. true = beendet (oder
/// nicht mehr auffindbar).
pub fn wait_exit(pid: u32, ms: u32) -> bool {
    match open(pid, SYNCHRONIZE) {
        Some(h) => unsafe { WaitForSingleObject(h.0, ms) == WAIT_OBJECT_0 },
        None => true,
    }
}

/// Beendet einen Prozess hart. Nur als letztes Mittel - ein Server verliert
/// dabei, was er nicht gespeichert hat.
pub fn terminate(pid: u32) -> bool {
    match open(pid, PROCESS_TERMINATE) {
        Some(h) => unsafe { TerminateProcess(h.0, 1) != 0 },
        None => false,
    }
}

// Ein Gegenstueck zu sys::soft_stop gibt es hier bewusst nicht. Eine JVM
// unter Windows kennt kein Signal, das sie sauber beenden wuerde:
// CloseMainWindow laeuft ins Leere, weil sie kein Fenster mit
// Nachrichtenschleife hat, und taskkill ohne /F verweigert den Dienst. Der
// einzige sanfte Weg ist die Konsole des Servers oder RCON - deshalb bleibt
// es bei der Vorgabe aus dem Backend-Trait.

/// Startet ein Programm losgeloest: kein Fenster, eigene Prozessgruppe, und
/// vor allem **ohne geerbte Handles**.
///
/// Das letzte ist der Grund, warum hier nicht `std::process::Command` steht.
/// Rust ruft CreateProcessW immer mit `bInheritHandles = TRUE` auf. Der neue
/// Supervisor bekaeme damit auch die Ausgabepipe dessen, der ihn gestartet
/// hat, und hielte sie offen, solange er lebt - `terranova start --detach`
/// in einer Shell oder in der CI kaeme nie zurueck, obwohl der Aufruf laengst
/// fertig ist.
///
/// Ohne geerbte Handles hat das Kind keine Standardausgabe. Das ist hier
/// richtig so: der Supervisor schreibt in sein Logbuch, nicht ins Fenster.
pub fn spawn_detached(exe: &Path, args: &[&OsStr], cwd: &Path) -> std::io::Result<u32> {
    let mut line = Vec::new();
    quote(exe.as_os_str(), &mut line);
    for a in args {
        line.push(b' ' as u16);
        quote(a, &mut line);
    }
    line.push(0);

    let app = wide(exe.as_os_str());
    let dir = wide(cwd.as_os_str());

    let mut si: STARTUPINFOW = unsafe { mem::zeroed() };
    si.cb = mem::size_of::<STARTUPINFOW>() as u32;
    let mut pi: PROCESS_INFORMATION = unsafe { mem::zeroed() };

    let ok = unsafe {
        CreateProcessW(
            app.as_ptr(),
            line.as_mut_ptr(),
            ptr::null(),
            ptr::null(),
            0, // bInheritHandles - darum geht es
            DETACHED_PROCESS | CREATE_NEW_PROCESS_GROUP | CREATE_NO_WINDOW,
            ptr::null(),
            dir.as_ptr(),
            &si,
            &mut pi,
        )
    };
    if ok == 0 {
        return Err(std::io::Error::last_os_error());
    }
    // Wir wollen nicht auf ihn warten, nur wissen, wer er ist.
    unsafe {
        CloseHandle(pi.hThread);
        CloseHandle(pi.hProcess);
    }
    Ok(pi.dwProcessId)
}

fn wide(s: &OsStr) -> Vec<u16> {
    s.encode_wide().chain(std::iter::once(0)).collect()
}

/// Ein Argument so einpacken, wie die C-Laufzeit es wieder auseinandernimmt:
/// Backslashes zaehlen nur vor einem Anfuehrungszeichen, dort verdoppelt.
fn quote(arg: &OsStr, out: &mut Vec<u16>) {
    const Q: u16 = b'"' as u16;
    const BS: u16 = b'\\' as u16;
    out.push(Q);
    let mut slashes = 0;
    for c in arg.encode_wide() {
        if c == BS {
            slashes += 1;
        } else {
            if c == Q {
                for _ in 0..=slashes {
                    out.push(BS);
                }
            }
            slashes = 0;
        }
        out.push(c);
    }
    for _ in 0..slashes {
        out.push(BS);
    }
    out.push(Q);
}

// --- Netz -----------------------------------------------------------------------

/// Welcher Prozess horcht auf diesem TCP-Port? IPv4 und IPv6: Velocity
/// horcht auf [::]:25565.
pub fn port_owner(port: u16) -> Option<u32> {
    listeners(AF_INET, 24, 8, 20)
        .into_iter()
        .chain(listeners(AF_INET6, 56, 20, 52))
        .find(|&(p, _)| p == port)
        .map(|(_, pid)| pid)
}

/// Liest die Tabelle der horchenden Sockets.
///
/// Die Zeilen werden ueber feste Offsets gelesen statt ueber die
/// windows-sys-Structs. IPv4-Zeile (MIB_TCPROW_OWNER_PID): 6 x u32, Port an
/// Offset 8, PID an 20. IPv6-Zeile (MIB_TCP6ROW_OWNER_PID): 56 Bytes, Port an
/// 20, PID an 52. Der Port steht in Netzwerk-Bytefolge in den unteren 16 Bit.
fn listeners(af: u32, row: usize, port_off: usize, pid_off: usize) -> Vec<(u16, u32)> {
    let mut size: u32 = 0;
    unsafe {
        GetExtendedTcpTable(
            ptr::null_mut(),
            &mut size,
            0,
            af,
            TCP_TABLE_OWNER_PID_LISTENER,
            0,
        );
    }
    // Die Tabelle kann zwischen den Aufrufen wachsen.
    for _ in 0..4 {
        // u32 statt u8, damit der Puffer fuer die DWORDs ausgerichtet ist
        let words = (size as usize + 4096) / 4 + 1;
        let mut buf = vec![0u32; words];
        let mut len = (words * 4) as u32;
        let r = unsafe {
            GetExtendedTcpTable(
                buf.as_mut_ptr() as *mut c_void,
                &mut len,
                0,
                af,
                TCP_TABLE_OWNER_PID_LISTENER,
                0,
            )
        };
        if r == 0 {
            let bytes: &[u8] =
                unsafe { std::slice::from_raw_parts(buf.as_ptr() as *const u8, words * 4) };
            let n = u32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]) as usize;
            let mut out = Vec::with_capacity(n);
            for i in 0..n {
                let base = 4 + i * row;
                if base + row > bytes.len() {
                    break;
                }
                let at = |o: usize| {
                    u32::from_le_bytes([
                        bytes[base + o],
                        bytes[base + o + 1],
                        bytes[base + o + 2],
                        bytes[base + o + 3],
                    ])
                };
                let p = at(port_off);
                let port = (((p & 0xFF) << 8) | ((p >> 8) & 0xFF)) as u16;
                out.push((port, at(pid_off)));
            }
            return out;
        }
        size = len;
    }
    Vec::new()
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
    let mut st: SYSTEMTIME = unsafe { mem::zeroed() };
    unsafe { GetLocalTime(&mut st) };
    LocalTime {
        year: st.wYear,
        month: st.wMonth,
        day: st.wDay,
        hour: st.wHour,
        minute: st.wMinute,
        second: st.wSecond,
    }
}

pub fn random(buf: &mut [u8]) {
    let status = unsafe {
        BCryptGenRandom(
            ptr::null_mut(),
            buf.as_mut_ptr(),
            buf.len() as u32,
            BCRYPT_USE_SYSTEM_PREFERRED_RNG,
        )
    };
    assert!(status >= 0, "BCryptGenRandom fehlgeschlagen: {status:#x}");
}

// --- Konsole ----------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CtrlEvent {
    /// Strg+C
    Interrupt,
    /// Strg+Pause
    Break,
    /// Fenster geschlossen - danach bleiben etwa 5 Sekunden
    Close,
    /// Abmelden oder Herunterfahren - etwa 20 Sekunden
    Shutdown,
}

static CTRL_HANDLER: OnceLock<fn(CtrlEvent) -> bool> = OnceLock::new();

unsafe extern "system" fn ctrl_trampoline(ctrl: u32) -> i32 {
    let ev = match ctrl {
        0 => CtrlEvent::Interrupt,
        1 => CtrlEvent::Break,
        2 => CtrlEvent::Close,
        5 | 6 => CtrlEvent::Shutdown,
        _ => return 0,
    };
    match CTRL_HANDLER.get() {
        Some(f) => i32::from(f(ev)),
        None => 0,
    }
}

/// Meldet Strg+C, Fenster-Schliessen und Herunterfahren an `f`. Gibt `f`
/// true zurueck, gilt das Ereignis als erledigt; bei Close und Shutdown wird
/// der Prozess danach trotzdem beendet - die Arbeit muss also in `f` passieren.
///
/// Bewusst die rohe API statt eines Signal-Crates: die gaengigen melden true
/// sofort zurueck, und bei CTRL_CLOSE heisst das, der Prozess stirbt, bevor
/// irgendwer reagieren konnte.
pub fn on_console_ctrl(f: fn(CtrlEvent) -> bool) {
    if CTRL_HANDLER.set(f).is_ok() {
        unsafe {
            SetConsoleCtrlHandler(Some(ctrl_trampoline), 1);
        }
    }
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
        assert!(image_name(me).unwrap().ends_with(".exe"));
        assert!(!alive(u32::MAX - 3));
    }

    #[test]
    fn horchender_port_gehoert_uns() {
        let l = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = l.local_addr().unwrap().port();
        assert_eq!(port_owner(port), Some(std::process::id()));
        drop(l);
    }

    fn q(s: &str) -> String {
        let mut out = Vec::new();
        quote(OsStr::new(s), &mut out);
        String::from_utf16(&out).unwrap()
    }

    /// So packt die C-Laufzeit es wieder aus. Vor allem der Pfad mit
    /// Leerzeichen muss halten: `--root C:\Program Files\...` kam frueher
    /// als drei Argumente an.
    #[test]
    fn argumente_einpacken() {
        assert_eq!(q("start"), r#""start""#);
        assert_eq!(q(r"C:\Program Files\tn"), r#""C:\Program Files\tn""#);
        // Ein Backslash am Ende wuerde sonst das schliessende
        // Anfuehrungszeichen schlucken.
        assert_eq!(q(r"C:\netz\"), r#""C:\netz\\""#);
        assert_eq!(q(r#"a"b"#), r#""a\"b""#);
        assert_eq!(q(r#"a\"b"#), r#""a\\\"b""#);
        assert_eq!(q(""), r#""""#);
    }

    #[test]
    fn uhrzeit_ist_plausibel() {
        let t = local_time();
        assert!(t.year >= 2024 && t.hour < 24 && t.minute < 60);
    }
}
