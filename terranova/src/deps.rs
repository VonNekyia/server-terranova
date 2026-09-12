//! MariaDB und Redis: besorgen, einrichten, bereitstellen, beenden.
//!
//! Beide laufen bewusst hier und nicht in einem Plugin: Paper liest
//! server.properties und die Plugins ihre Configs, bevor ein Plugin
//! ueberhaupt laden koennte. Die Datenbanken muessen also schon stehen,
//! bevor der erste Server hochfaehrt.

// fs, Path und sha2 braucht nur der Windows-Pfad: das Herunterladen und
// Auspacken von MariaDB und Redis.
#[cfg(windows)]
use std::fs;
use std::io::{self, Read, Write};
use std::net::{SocketAddr, TcpStream};
#[cfg(windows)]
use std::path::Path;
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

#[cfg(windows)]
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

fn quiet(cmd: &mut Command) -> &mut Command {
    cmd.stdin(Stdio::null());
    sys::hide_window(cmd)
}

#[cfg(windows)]
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
/// nichts liegen - lieber kein MariaDB als ein fremdes.
#[cfg(windows)]
fn download(url: &str, dest: &Path, expected: Option<&str>) -> io::Result<()> {
    if let Some(dir) = dest.parent() {
        fs::create_dir_all(dir)?;
    }
    let tmp = dest.with_extension("teil");
    let status = quiet(&mut Command::new(system32("curl.exe")))
        .args(["-L", "--fail", "--silent", "--show-error", "-o"])
        .arg(&tmp)
        .arg(url)
        .status()?;
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
