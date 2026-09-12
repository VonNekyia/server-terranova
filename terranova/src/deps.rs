//! MariaDB und Redis: besorgen, einrichten, bereitstellen, beenden.
//!
//! Beide laufen bewusst hier und nicht in einem Plugin: Paper liest
//! server.properties und die Plugins ihre Configs, bevor ein Plugin
//! ueberhaupt laden koennte. Die Datenbanken muessen also schon stehen,
//! bevor der erste Server hochfaehrt.

use std::fs;
use std::io::{self, Read, Write};
use std::net::{SocketAddr, TcpStream};
use std::path::Path;
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use sha2::{Digest, Sha256};

use crate::config::Config;
use crate::paths::Paths;
use crate::sys;

// --- Wo die Programme liegen ---------------------------------------------
//
// Unter Windows bringt Terranova MariaDB und Redis selbst mit: es gibt keine
// Paketverwaltung, auf die man sich verlassen koennte, und ein entpacktes
// Verzeichnis ist dort der uebliche Weg.
//
// Unter Unix genau andersherum. Jede Distribution liefert beide als Paket,
// und ein zweites, danebengelegtes MariaDB waere vor allem eine Quelle fuer
// Verwechslungen - zumal die Windows-Archive ohnehin nicht passen. Gesucht
// wird deshalb, was schon da ist; fehlt es, sagt Terranova das und laedt
// nichts nach. Pakete einzuspielen ist Sache des Systems, nicht dieses
// Programms.

/// Wo MariaDB nach dem Entpacken liegt.
#[cfg(windows)]
pub fn mariadb_base(paths: &Paths, version: &str) -> PathBuf {
    paths
        .mariadb_home()
        .join(format!("mariadb-{version}-winx64"))
}

#[cfg(windows)]
pub fn mariadb_bin(paths: &Paths, version: &str) -> PathBuf {
    mariadb_base(paths, version).join("bin")
}

/// Das erste dieser Programme, das sich finden laesst.
///
/// Neben dem PATH werden die ueblichen Orte fuer Serverprogramme abgesucht:
/// unter Debian liegt mariadbd in /usr/sbin, und das steht in einer
/// gewoehnlichen Benutzersitzung nicht im PATH.
#[cfg(unix)]
fn which(names: &[&str]) -> Option<PathBuf> {
    let path = std::env::var_os("PATH").unwrap_or_default();
    let extra = ["/usr/sbin", "/usr/local/sbin", "/sbin"];
    for name in names {
        let dirs = std::env::split_paths(&path).chain(extra.iter().map(PathBuf::from));
        for dir in dirs {
            let p = dir.join(name);
            if p.is_file() {
                return Some(p);
            }
        }
    }
    None
}

/// Das Serverprogramm von MariaDB.
#[cfg(windows)]
pub fn mariadb_server(paths: &Paths, version: &str) -> PathBuf {
    mariadb_bin(paths, version).join("mysqld.exe")
}

/// MariaDB heisst seit 10.5 mariadbd; mysqld ist nur noch ein Zweitname und
/// fehlt in manchen Paketen.
#[cfg(unix)]
pub fn mariadb_server(_paths: &Paths, _version: &str) -> PathBuf {
    which(&["mariadbd", "mysqld"]).unwrap_or_else(|| PathBuf::from("mariadbd"))
}

/// Der Kommandozeilenclient - fuer das Anlegen der Datenbanken und fuers
/// Herunterfahren.
#[cfg(windows)]
pub fn mariadb_client(paths: &Paths, version: &str) -> PathBuf {
    mariadb_bin(paths, version).join("mysql.exe")
}

#[cfg(unix)]
pub fn mariadb_client(_paths: &Paths, _version: &str) -> PathBuf {
    which(&["mariadb", "mysql"]).unwrap_or_else(|| PathBuf::from("mariadb"))
}

/// Das Programm, das ein leeres Datenverzeichnis einrichtet.
#[cfg(windows)]
fn mariadb_installer(paths: &Paths, version: &str) -> Option<PathBuf> {
    let p = mariadb_bin(paths, version).join("mysql_install_db.exe");
    p.is_file().then_some(p)
}

#[cfg(unix)]
fn mariadb_installer(_paths: &Paths, _version: &str) -> Option<PathBuf> {
    which(&["mariadb-install-db", "mysql_install_db"])
}

/// Das Serverprogramm von Redis.
#[cfg(windows)]
pub fn redis_server(paths: &Paths) -> PathBuf {
    paths.redis_home().join("redis-server.exe")
}

#[cfg(unix)]
pub fn redis_server(_paths: &Paths) -> PathBuf {
    which(&["redis-server", "valkey-server"]).unwrap_or_else(|| PathBuf::from("redis-server"))
}

/// curl und tar aus System32 statt vom PATH.
///
/// Auf dieser Maschine liegt Gits GNU-tar im PATH, und das kann keine
/// Zip-Dateien auspacken. Windows bringt bsdtar mit, das kann es.
#[cfg(windows)]
fn system32(tool: &str) -> PathBuf {
    let root = std::env::var("SystemRoot").unwrap_or_else(|_| "C:\\Windows".into());
    Path::new(&root).join("System32").join(tool)
}

/// curl und tar: unter Windows die aus System32, sonst die vom PATH.
#[cfg(windows)]
fn curl() -> PathBuf {
    system32("curl.exe")
}
#[cfg(unix)]
fn curl() -> PathBuf {
    PathBuf::from("curl")
}
#[cfg(windows)]
fn tar() -> PathBuf {
    system32("tar.exe")
}
#[cfg(unix)]
fn tar() -> PathBuf {
    PathBuf::from("tar")
}

fn quiet(cmd: &mut Command) -> &mut Command {
    cmd.stdin(Stdio::null());
    sys::hide_window(cmd)
}

fn sha256_of(path: &Path) -> io::Result<String> {
    let mut f = fs::File::open(path)?;
    let mut h = Sha256::new();
    let mut buf = vec![0u8; 1 << 16];
    loop {
        let n = f.read(&mut buf)?;
        if n == 0 {
            break;
        }
        h.update(&buf[..n]);
    }
    Ok(h.finalize().iter().map(|b| format!("{b:02x}")).collect())
}

/// Laedt eine Datei und prueft ihre Pruefsumme. Stimmt sie nicht, bleibt
/// nichts liegen - lieber gar keins als ein fremdes.
fn download(url: &str, dest: &Path, expected: Option<&str>) -> io::Result<()> {
    if let Some(dir) = dest.parent() {
        fs::create_dir_all(dir)?;
    }
    let tmp = dest.with_extension("teil");
    let status = quiet(&mut Command::new(curl()))
        .args(["-L", "--fail", "--silent", "--show-error", "-o"])
        .arg(&tmp)
        .arg(url)
        .status()
        .map_err(|e| {
            io::Error::other(format!(
                "curl laesst sich nicht starten ({e}) - unter Linux etwa: sudo apt install curl"
            ))
        })?;
    if !status.success() {
        let _ = fs::remove_file(&tmp);
        return Err(io::Error::other(format!("Download fehlgeschlagen: {url}")));
    }
    if let Some(want) = expected {
        let got = sha256_of(&tmp)?;
        if !got.eq_ignore_ascii_case(want) {
            let _ = fs::remove_file(&tmp);
            return Err(io::Error::other(format!(
                "{url}\n  erwartete Pruefsumme {want}\n  bekommen           {got}"
            )));
        }
    }
    fs::rename(&tmp, dest)?;
    Ok(())
}

/// Stellt sicher, dass MariaDB bereitsteht, und richtet das Datenverzeichnis
/// ein.
///
/// Das Datenverzeichnis gehoert in beiden Faellen uns: Terranova faehrt eine
/// eigene Instanz auf einem eigenen Port hoch und fasst eine etwaige
/// Systeminstanz auf 3306 nicht an.
pub fn ensure_mariadb(paths: &Paths, cfg: &Config, log: &mut dyn FnMut(&str)) -> io::Result<()> {
    let version = &cfg.deps.mariadb.version;

    #[cfg(windows)]
    {
        let bin = mariadb_bin(paths, version);
        if !bin.join("mysqld.exe").is_file() {
            let zip = paths
                .mariadb_home()
                .join(format!("mariadb-{version}-winx64.zip"));
            log(&format!(
                "MariaDB {version} wird einmalig heruntergeladen (ca. 87 MB)..."
            ));
            download(
                &format!(
                    "https://archive.mariadb.org/mariadb-{version}/winx64-packages/mariadb-{version}-winx64.zip"
                ),
                &zip,
                cfg.deps.mariadb.sha256.as_deref(),
            )?;
            log("wird entpackt...");
            let status = quiet(&mut Command::new(system32("tar.exe")))
                .arg("-xf")
                .arg(&zip)
                .arg("-C")
                .arg(paths.mariadb_home())
                .status()?;
            if !status.success() {
                return Err(io::Error::other("Entpacken fehlgeschlagen"));
            }
            let _ = fs::remove_file(&zip);
        }
        if !bin.join("mysqld.exe").is_file() {
            return Err(io::Error::other(format!(
                "MariaDB nicht gefunden unter {}",
                bin.display()
            )));
        }
    }

    // Unter Unix wird nichts nachgeladen: die Windows-Archive passen nicht,
    // und ein Paket einzuspielen ist Sache des Systems. Fehlt MariaDB, sagt
    // Terranova nur, wie es hereinkommt.
    #[cfg(unix)]
    if which(&["mariadbd", "mysqld"]).is_none() {
        return Err(io::Error::other(
            "MariaDB ist nicht installiert.\n  \
             Debian/Ubuntu:  sudo apt install mariadb-server\n  \
             Fedora:         sudo dnf install mariadb-server\n  \
             Arch:           sudo pacman -S mariadb",
        ));
    }

    let data = paths.mariadb_home().join("data");
    if !data.join("mysql").is_dir() {
        log("Datenverzeichnis wird eingerichtet...");
        let Some(installer) = mariadb_installer(paths, version) else {
            return Err(io::Error::other(
                "mariadb-install-db nicht gefunden - MariaDB unvollstaendig installiert",
            ));
        };
        let mut cmd = Command::new(installer);
        cmd.arg(format!("--datadir={}", data.display()));
        // Debian richtet root auf unix_socket ein. Terranova spricht MariaDB
        // aber ueber TCP an, und dann greift das nicht. Mit "normal" kommt
        // root ohne Passwort ueber TCP herein - die Instanz horcht ohnehin
        // nur auf 127.0.0.1.
        #[cfg(unix)]
        cmd.arg("--auth-root-authentication-method=normal");
        let status = quiet(&mut cmd).status()?;
        if !status.success() {
            return Err(io::Error::other(
                "Einrichten des Datenverzeichnisses fehlgeschlagen",
            ));
        }
    }
    Ok(())
}

/// Besorgt die Redis-Exes vom festgenagelten Commit.
#[cfg(windows)]
pub fn ensure_redis(paths: &Paths, cfg: &Config, log: &mut dyn FnMut(&str)) -> io::Result<()> {
    let home = paths.redis_home();
    let commit = &cfg.deps.redis.windows_commit;
    let mut said = false;
    for file in ["redis-server.exe", "redis-cli.exe"] {
        let dest = home.join(file);
        if dest.is_file() {
            continue;
        }
        if !said {
            log("Redis wird einmalig heruntergeladen (ca. 4 MB)...");
            said = true;
        }
        download(
            &format!("https://raw.githubusercontent.com/zkteco-home/redis-windows/{commit}/{file}"),
            &dest,
            cfg.deps.redis.sha256.get(file).map(String::as_str),
        )?;
    }
    Ok(())
}

/// Unter Unix gibt es Redis als Paket - und anders als unter Windows sogar
/// offiziell. Heruntergeladen wird deshalb nichts.
#[cfg(unix)]
pub fn ensure_redis(_paths: &Paths, _cfg: &Config, _log: &mut dyn FnMut(&str)) -> io::Result<()> {
    if which(&["redis-server", "valkey-server"]).is_none() {
        return Err(io::Error::other(
            "Redis ist nicht installiert.\n  \
             Debian/Ubuntu:  sudo apt install redis-server\n  \
             Fedora:         sudo dnf install redis\n  \
             Arch:           sudo pacman -S redis",
        ));
    }
    Ok(())
}

// --- Java ---------------------------------------------------------------------
//
// Anders als MariaDB und Redis laedt Terranova Java auch unter Linux selbst
// nach, wenn keines passt. Paper verlangt eine bestimmte Hauptversion, und
// die Paketquellen verbreiteter Distributionen haengen dem oft ein, zwei
// Jahre hinterher - ein "apt install openjdk" liefert dann ein Java, mit dem
// der Server gar nicht erst startet.
//
// Ein Java, das schon da ist und passt, hat Vorrang: niemand soll 60 MB
// herunterladen, der ein brauchbares JDK installiert hat.

const JAVA_EXE: &str = if cfg!(windows) { "java.exe" } else { "java" };

/// Wo ein nachgeladenes Java liegt.
fn java_home(paths: &Paths) -> PathBuf {
    paths.runtime().join("java")
}

/// Das nachgeladene Java, falls es eines gibt.
///
/// Gesucht wird eine Ebene tief: das Archiv packt sich in ein Verzeichnis
/// mit dem Namen des Builds aus, und der soll hier nicht festgeschrieben
/// werden.
fn bundled_java(paths: &Paths) -> Option<PathBuf> {
    let mut found: Vec<PathBuf> = fs::read_dir(java_home(paths))
        .ok()?
        .flatten()
        .map(|e| e.path().join("bin").join(JAVA_EXE))
        .filter(|p| p.is_file())
        .collect();
    found.sort();
    found.pop()
}

/// Die Hauptversion aus der ersten Zeile von `java -version`.
///
/// Seit Java 9 steht dort etwa "25.0.4.1", davor "1.8.0_402" - dann zaehlt
/// die zweite Stelle.
pub fn parse_java_major(line: &str) -> Option<u32> {
    let quoted = line.split('"').nth(1)?;
    let mut parts = quoted.split(['.', '_', '-', '+']);
    let first: u32 = parts.next()?.parse().ok()?;
    if first == 1 {
        parts.next()?.parse().ok()
    } else {
        Some(first)
    }
}

/// Die erste Zeile von `java -version`, falls sich das Programm starten laesst.
pub fn java_version_line(java: &Path) -> Option<String> {
    let mut cmd = Command::new(java);
    cmd.arg("-version");
    let out = quiet(&mut cmd).output().ok()?;
    // java -version schreibt nach stderr
    let text = String::from_utf8_lossy(&out.stderr);
    text.lines().next().map(|l| l.trim().to_string())
}

/// Woher das Java eines Servers kommt.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum JavaSource {
    /// TERRANOVA_JAVA - ausdruecklich gesetzt, wird nicht geprueft
    Env,
    /// java.path aus terranova.yml, und die Version passt
    Configured,
    /// Von Terranova nachgeladen, unter runtime/java
    Bundled,
    /// Nichts Passendes da - ensure_java laedt es
    Missing,
}

/// Welches Java ein Server bekommt.
///
/// Wird bei jedem Start eines Servers neu gefragt, nicht einmal beim Anlegen
/// des Supervisors: beim ersten Start wird ein fehlendes Java erst
/// nachgeladen, wenn der Supervisor schon steht.
pub fn resolve_java(paths: &Paths, java: &crate::config::Java) -> (PathBuf, JavaSource) {
    if let Ok(p) = std::env::var("TERRANOVA_JAVA") {
        return (PathBuf::from(p), JavaSource::Env);
    }
    let fits = |candidate: &Path| {
        java_version_line(candidate)
            .and_then(|l| parse_java_major(&l))
            .is_some_and(|major| major >= java.version)
    };
    let configured = PathBuf::from(&java.path);
    if fits(&configured) {
        return (configured, JavaSource::Configured);
    }
    if let Some(b) = bundled_java(paths) {
        if fits(&b) {
            return (b, JavaSource::Bundled);
        }
    }
    (configured, JavaSource::Missing)
}

/// Unter welchem Schluessel die Pruefsumme fuer diese Maschine steht.
fn java_platform() -> Option<(&'static str, &'static str)> {
    let os = if cfg!(windows) {
        "windows"
    } else if cfg!(target_os = "linux") {
        "linux"
    } else {
        return None;
    };
    let arch = match std::env::consts::ARCH {
        "x86_64" => "x64",
        "aarch64" => "aarch64",
        _ => return None,
    };
    Some((os, arch))
}

/// Die Download-Adresse bei Adoptium fuer einen Build.
///
/// Aus "jdk-25.0.4.1+1" wird "25.0.4.1_1" im Dateinamen und
/// "jdk-25.0.4.1%2B1" im Pfad.
pub fn java_url(release: &str, os: &str, arch: &str) -> Option<String> {
    let version = release.strip_prefix("jdk-")?;
    let major = version.split('.').next()?;
    let file_version = version.replace('+', "_");
    let ext = if os == "windows" { "zip" } else { "tar.gz" };
    Some(format!(
        "https://github.com/adoptium/temurin{major}-binaries/releases/download/{}/OpenJDK{major}U-jre_{arch}_{os}_hotspot_{file_version}.{ext}",
        release.replace('+', "%2B")
    ))
}

/// Sorgt dafuer, dass ein passendes Java da ist - und laedt es sonst nach.
pub fn ensure_java(paths: &Paths, cfg: &Config, log: &mut dyn FnMut(&str)) -> io::Result<()> {
    if resolve_java(paths, &cfg.java).1 != JavaSource::Missing {
        return Ok(());
    }
    let want = cfg.java.version;
    let Some(dl) = &cfg.java.download else {
        return Err(io::Error::other(format!(
            "kein Java {want} gefunden, und java.download fehlt in terranova.yml - \
             Java {want} installieren oder TERRANOVA_JAVA setzen"
        )));
    };
    let Some((os, arch)) = java_platform() else {
        return Err(io::Error::other(format!(
            "kein Java {want} gefunden, und fuer {} gibt es keinen Download - \
             Java {want} installieren oder TERRANOVA_JAVA setzen",
            std::env::consts::ARCH
        )));
    };
    let key = format!("{os}-{arch}");
    let Some(sum) = dl.sha256.get(&key) else {
        return Err(io::Error::other(format!(
            "java.download.sha256 kennt {key} nicht - \
             Java {want} installieren oder TERRANOVA_JAVA setzen"
        )));
    };
    let url = java_url(&dl.release, os, arch).ok_or_else(|| {
        io::Error::other(format!(
            "java.download.release '{}' ist kein Adoptium-Build",
            dl.release
        ))
    })?;

    let home = java_home(paths);
    let archive = home.join(if os == "windows" {
        "jre.zip"
    } else {
        "jre.tar.gz"
    });
    log(&format!(
        "Java {want} ({}) wird einmalig heruntergeladen (ca. 60 MB)...",
        dl.release
    ));
    download(&url, &archive, Some(sum.as_str()))?;
    log("wird entpackt...");
    let status = quiet(&mut Command::new(tar()))
        .arg("-xf")
        .arg(&archive)
        .arg("-C")
        .arg(&home)
        .status()?;
    let _ = fs::remove_file(&archive);
    if !status.success() {
        return Err(io::Error::other("Entpacken von Java fehlgeschlagen"));
    }
    match resolve_java(paths, &cfg.java) {
        (p, JavaSource::Bundled) => {
            log(&format!("Java bereit: {}", p.display()));
            Ok(())
        }
        _ => Err(io::Error::other(format!(
            "Java entpackt, aber unter {} liegt kein lauffaehiges Java {want}",
            home.display()
        ))),
    }
}

// --- Bereitschaft -------------------------------------------------------------

fn local(port: u16) -> SocketAddr {
    SocketAddr::from(([127, 0, 0, 1], port))
}

/// MariaDB begruesst jede Verbindung mit einem Handshake-Paket; im fuenften
/// Byte steht die Protokollfassung 10. Das ist billiger, als in einer
/// Schleife mysql.exe zu starten.
pub fn mariadb_ready(port: u16) -> bool {
    let Ok(mut s) = TcpStream::connect_timeout(&local(port), Duration::from_millis(500)) else {
        return false;
    };
    let _ = s.set_read_timeout(Some(Duration::from_millis(1500)));
    let mut buf = [0u8; 5];
    s.read_exact(&mut buf).is_ok() && buf[4] == 10
}

pub fn redis_ready(port: u16) -> bool {
    redis_command(port, &["PING"]).is_ok_and(|r| r.starts_with("+PONG"))
}

/// Ein Redis-Befehl im RESP-Format.
fn redis_command(port: u16, args: &[&str]) -> io::Result<String> {
    let mut s = TcpStream::connect_timeout(&local(port), Duration::from_millis(500))?;
    s.set_read_timeout(Some(Duration::from_secs(5)))?;
    s.set_write_timeout(Some(Duration::from_secs(5)))?;
    let mut out = format!("*{}\r\n", args.len());
    for a in args {
        out.push_str(&format!("${}\r\n{a}\r\n", a.len()));
    }
    s.write_all(out.as_bytes())?;
    s.flush()?;
    let mut buf = [0u8; 256];
    let n = s.read(&mut buf).unwrap_or(0);
    Ok(String::from_utf8_lossy(&buf[..n]).into_owned())
}

pub fn wait_until(mut ready: impl FnMut() -> bool, timeout: Duration) -> bool {
    let deadline = Instant::now() + timeout;
    while Instant::now() < deadline {
        if ready() {
            return true;
        }
        std::thread::sleep(Duration::from_millis(250));
    }
    false
}

// --- Datenbanken anlegen ---------------------------------------------------------

fn sql_quote(s: &str) -> String {
    s.replace('\\', "\\\\").replace('\'', "\\'")
}

/// Legt Datenbanken und den Benutzer an. Wie in start.bat, nur enger:
/// Rechte je Datenbank statt ALL PRIVILEGES ON *.*, und keinen Benutzer
/// mehr fuer '%' - alles laeuft auf dieser Maschine.
pub fn provision(paths: &Paths, cfg: &Config, runtime: crate::config::Runtime) -> io::Result<()> {
    let db = &cfg.deps.mariadb;
    let mut sql = String::new();
    for name in &db.databases {
        sql.push_str(&format!(
            "CREATE DATABASE IF NOT EXISTS `{name}` CHARACTER SET utf8mb4 COLLATE utf8mb4_unicode_ci;"
        ));
    }
    // MySQL sieht localhost und 127.0.0.1 als verschiedene Hosts.
    for host in ["localhost", "127.0.0.1"] {
        sql.push_str(&format!(
            "CREATE USER IF NOT EXISTS '{}'@'{host}' IDENTIFIED BY '{}';",
            sql_quote(&db.user),
            sql_quote(&db.password)
        ));
        for name in &db.databases {
            sql.push_str(&format!(
                "GRANT ALL PRIVILEGES ON `{name}`.* TO '{}'@'{host}';",
                sql_quote(&db.user)
            ));
        }
    }
    sql.push_str("FLUSH PRIVILEGES;");

    if runtime == crate::config::Runtime::Docker {
        let d = crate::docker::Docker::new(paths, cfg);
        return crate::docker::provision(&d, &sql);
    }

    let out = quiet(&mut Command::new(mariadb_client(paths, &db.version)))
        .args(["-h", "127.0.0.1", "-P"])
        .arg(db.port.to_string())
        .args(["-u", "root", "--protocol=tcp", "-e"])
        .arg(&sql)
        .output()?;
    if !out.status.success() {
        return Err(io::Error::other(format!(
            "Datenbanken anlegen fehlgeschlagen: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        )));
    }
    Ok(())
}

// --- Beenden ------------------------------------------------------------------------

/// Faehrt MariaDB ueber ihr eigenes Protokoll herunter - nie ueber die
/// Programmdatei. Auf dieser Maschine laeuft daneben ein eigenstaendiger
/// MariaDB-Dienst auf 3306, den ein pauschales Beenden mit erwischt haette.
pub fn stop_mariadb(paths: &Paths, cfg: &Config) -> bool {
    quiet(&mut Command::new(mariadb_client(
        paths,
        &cfg.deps.mariadb.version,
    )))
    .args(["-h", "127.0.0.1", "-P"])
    .arg(cfg.deps.mariadb.port.to_string())
    .args(["-u", "root", "--protocol=tcp", "-e", "SHUTDOWN"])
    .stdout(Stdio::null())
    .stderr(Stdio::null())
    .status()
    .is_ok_and(|s| s.success())
}

pub fn stop_redis(port: u16) -> bool {
    // Die Verbindung bricht ab, wenn Redis geht - das ist der Erfolgsfall.
    redis_command(port, &["SHUTDOWN", "NOSAVE"]).is_ok()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testutil::EXAMPLE_CONFIG;

    /// Unter Windows bringt Terranova MariaDB selbst mit - dann muss der
    /// Pfad ins ausgepackte Verzeichnis zeigen.
    #[cfg(windows)]
    #[test]
    fn pfade() {
        let p = Paths::new("C:\\net");
        assert!(mariadb_bin(&p, "11.4.5").ends_with("mariadb-11.4.5-winx64\\bin"));
        assert!(system32("curl.exe").ends_with("System32\\curl.exe"));
    }

    /// Unter Unix kommen die Programme aus dem System. Welche genau, haengt
    /// von der Distribution ab - geprueft wird deshalb nur, dass ein
    /// plausibler Name herauskommt und nichts im Netzwerkverzeichnis gesucht
    /// wird. Dort wird unter Unix nichts ausgepackt.
    #[cfg(unix)]
    #[test]
    fn programme_kommen_aus_dem_system() {
        let p = Paths::new("/opt/terranova");
        let server = mariadb_server(&p, "11.4.5");
        let name = server.file_name().unwrap().to_string_lossy().into_owned();
        assert!(name == "mariadbd" || name == "mysqld", "unerwartet: {name}");
        assert!(!server.starts_with("/opt/terranova"));

        let client = mariadb_client(&p, "11.4.5");
        let name = client.file_name().unwrap().to_string_lossy().into_owned();
        assert!(name == "mariadb" || name == "mysql", "unerwartet: {name}");
    }

    #[test]
    fn sql_wird_maskiert() {
        assert_eq!(sql_quote("o'brien"), "o\\'brien");
        assert_eq!(sql_quote("a\\b"), "a\\\\b");
    }

    #[test]
    fn niemand_horcht_auf_einem_freien_port() {
        // Port 1 ist sicher frei; die Pruefungen duerfen dort nicht haengen.
        assert!(!mariadb_ready(1));
        assert!(!redis_ready(1));
    }

    /// Nur Windows laedt etwas herunter, also gibt es auch nur dort etwas zu
    /// pruefen.
    #[cfg(windows)]
    #[test]
    fn pruefsumme_stimmt_mit_bekanntem_wert() {
        let dir = crate::testutil::tempdir("sha");
        let f = dir.join("leer.bin");
        fs::write(&f, b"").unwrap();
        assert_eq!(
            sha256_of(&f).unwrap(),
            "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
        );
    }

    #[test]
    fn wartet_und_gibt_auf() {
        let t0 = Instant::now();
        assert!(!wait_until(|| false, Duration::from_millis(400)));
        assert!(t0.elapsed() >= Duration::from_millis(350));
        assert!(wait_until(|| true, Duration::from_secs(5)));
    }

    #[test]
    fn die_acht_datenbanken_stehen_in_der_config() {
        let cfg = Config::parse(EXAMPLE_CONFIG).unwrap();
        assert!(cfg.deps.mariadb.databases.contains(&"husksync".to_string()));
        assert!(cfg.deps.mariadb.sha256.is_some(), "Pruefsumme festgenagelt");
        assert_eq!(cfg.deps.redis.sha256.len(), 2);
    }
}

#[cfg(test)]
mod java_tests {
    use super::*;

    #[test]
    fn java_hauptversion_aus_der_ersten_zeile() {
        let m = parse_java_major;
        assert_eq!(m(r#"openjdk version "26.0.2.1" 2026-08-18"#), Some(26));
        assert_eq!(m(r#"openjdk version "25" 2025-09-16"#), Some(25));
        assert_eq!(m(r#"openjdk version "25-ea" 2025-09-16"#), Some(25));
        assert_eq!(m(r#"java version "1.8.0_402""#), Some(8));
        assert_eq!(m("Error: could not find java"), None);
    }

    #[test]
    fn download_adressen_wie_bei_adoptium() {
        // Genau die Adressen, die api.adoptium.net fuer diesen Build nennt.
        let r = "jdk-25.0.4.1+1";
        assert_eq!(
            java_url(r, "windows", "x64").as_deref(),
            Some("https://github.com/adoptium/temurin25-binaries/releases/download/jdk-25.0.4.1%2B1/OpenJDK25U-jre_x64_windows_hotspot_25.0.4.1_1.zip")
        );
        assert_eq!(
            java_url(r, "linux", "x64").as_deref(),
            Some("https://github.com/adoptium/temurin25-binaries/releases/download/jdk-25.0.4.1%2B1/OpenJDK25U-jre_x64_linux_hotspot_25.0.4.1_1.tar.gz")
        );
        assert_eq!(
            java_url(r, "linux", "aarch64").as_deref(),
            Some("https://github.com/adoptium/temurin25-binaries/releases/download/jdk-25.0.4.1%2B1/OpenJDK25U-jre_aarch64_linux_hotspot_25.0.4.1_1.tar.gz")
        );
        assert_eq!(java_url("25.0.4", "linux", "x64"), None);
    }

    #[test]
    fn die_echte_config_nagelt_alle_plattformen_fest() {
        let cfg = Config::parse(crate::testutil::EXAMPLE_CONFIG).unwrap();
        assert_eq!(cfg.java.version, 25);
        let dl = cfg.java.download.expect("java.download in terranova.yml");
        for key in ["windows-x64", "linux-x64", "linux-aarch64"] {
            let sum = dl.sha256.get(key).unwrap_or_else(|| panic!("{key} fehlt"));
            assert_eq!(sum.len(), 64, "{key}");
            assert!(sum.chars().all(|c| c.is_ascii_hexdigit()), "{key}");
        }
        assert!(java_url(&dl.release, "linux", "x64").is_some());
    }

    #[test]
    fn nachgeladenes_java_wird_gefunden() {
        let root = crate::testutil::tempdir("java-bundled");
        let paths = Paths::new(&root);
        assert!(bundled_java(&paths).is_none());
        let bin = root
            .join("runtime")
            .join("java")
            .join("jdk-25.0.4.1+1-jre")
            .join("bin");
        fs::create_dir_all(&bin).unwrap();
        fs::write(bin.join(JAVA_EXE), b"").unwrap();
        assert_eq!(bundled_java(&paths), Some(bin.join(JAVA_EXE)));
    }
}
