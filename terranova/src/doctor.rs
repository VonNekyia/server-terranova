//! Prueft, ob das Netzwerk startklar ist - bevor es jemand startet.

use std::fmt::Write as _;
use std::fs;
use std::net::ToSocketAddrs as _;
// Nur noch fuer die geplante Windows-Aufgabe; Java fragt deps ab.
#[cfg(windows)]
use std::process::{Command, Stdio};

use crate::config::{Config, Runtime};
use crate::paths::Paths;
use crate::{paperyml, sys};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Level {
    Ok,
    Warn,
    Fail,
}

impl Level {
    fn mark(self) -> &'static str {
        match self {
            Level::Ok => "  ok  ",
            Level::Warn => " warn ",
            Level::Fail => "FEHLER",
        }
    }
}

#[derive(Debug)]
pub struct Check {
    pub level: Level,
    pub what: String,
    pub detail: String,
}

pub fn run(paths: &Paths, cfg: &Config) -> Vec<Check> {
    let mut c = Vec::new();
    let mut add = |level, what: &str, detail: String| {
        c.push(Check {
            level,
            what: what.to_string(),
            detail,
        })
    };

    add(
        Level::Ok,
        "Konfiguration",
        format!("{} - Laufzeit {}", paths.config().display(), cfg.runtime()),
    );

    // --- Java ---------------------------------------------------------------
    if cfg.runtime() == Runtime::Native {
        use crate::deps::JavaSource;
        let want = cfg.java.version;
        let (java, source) = crate::deps::resolve_java(paths, &cfg.java);
        let line = crate::deps::java_version_line(&java).unwrap_or_default();
        let shown = java.display();
        match source {
            JavaSource::Env => add(Level::Ok, "Java", format!("TERRANOVA_JAVA {shown}: {line}")),
            JavaSource::Configured => add(Level::Ok, "Java", format!("{shown}: {line}")),
            JavaSource::Bundled => add(Level::Ok, "Java", format!("nachgeladen, {shown}: {line}")),
            JavaSource::Missing if cfg.java.download.is_some() => add(
                Level::Warn,
                "Java",
                format!("kein Java {want} gefunden - der naechste Start laedt Temurin {want} nach (ca. 60 MB)"),
            ),
            JavaSource::Missing => add(
                Level::Fail,
                "Java",
                format!("kein Java {want} gefunden - installieren oder TERRANOVA_JAVA setzen"),
            ),
        }
    }

    // --- Vorlagen ------------------------------------------------------------
    let common = paths.common();
    if !common.is_dir() {
        add(
            Level::Fail,
            "Vorlagen",
            format!("{} fehlt", common.display()),
        );
    } else {
        let paper = fs::read_dir(&common)
            .into_iter()
            .flatten()
            .flatten()
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .find(|n| n.starts_with("paper-") && n.ends_with(".jar"));
        match paper {
            Some(j) => add(Level::Ok, "Paper", j),
            None => add(
                Level::Fail,
                "Paper",
                format!("kein paper-*.jar in {}", common.display()),
            ),
        }
        for f in ["server.properties", "eula.txt"] {
            if !common.join(f).is_file() {
                add(
                    Level::Warn,
                    "Vorlagen",
                    format!("templates/common/{f} fehlt"),
                );
            }
        }
        if !common.join("config").join("paper-global.yml").is_file() {
            add(
                Level::Warn,
                "Vorlagen",
                "templates/common/config/paper-global.yml fehlt - ohne sie keine Weiterleitung"
                    .into(),
            );
        }
    }
    // Ein fester Server hat keine Vorlage: er traegt sein Paper und seine
    // Plugins selbst, versioniert unter servers/<name>/. Geprueft wird
    // deshalb, ob das auch wirklich dort liegt - und ob die beiden Quellen da
    // sind, aus denen beim Start die Dateien mit den Geheimnissen entstehen.
    let mut feste_ok = Vec::new();
    for name in cfg.servers.keys() {
        let dir = paths.server(name);
        let mut ok = true;
        let has_paper = fs::read_dir(&dir)
            .into_iter()
            .flatten()
            .flatten()
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .any(|n| n.starts_with("paper-") && n.ends_with(".jar"));
        if !has_paper {
            add(
                Level::Fail,
                "Fester Server",
                format!("kein paper-*.jar in servers/{name}/ - ohne es startet er nicht"),
            );
            ok = false;
        }
        for (rel, wozu) in [
            ("server.properties.dist", "Port, RCON und MOTD"),
            ("config/paper-global.yml.dist", "die Weiterleitung"),
        ] {
            if !dir
                .join(rel.replace('/', std::path::MAIN_SEPARATOR_STR))
                .is_file()
            {
                add(
                    Level::Warn,
                    "Fester Server",
                    format!("servers/{name}/{rel} fehlt - dann schreibt Terranova {wozu} nicht"),
                );
                ok = false;
            }
        }
        if ok {
            feste_ok.push(name.clone());
        }
    }
    if !feste_ok.is_empty() {
        add(
            Level::Ok,
            "Feste Server",
            format!("{} - Paper und Quellen vollstaendig", feste_ok.join(", ")),
        );
    }
    if !paths.template(&cfg.mines.template).is_dir() {
        add(
            Level::Fail,
            "Vorlagen",
            format!(
                "templates/{}/ fehlt - ohne sie kein Dungeon",
                cfg.mines.template
            ),
        );
    }

    // --- Proxy ---------------------------------------------------------------
    let proxy_dir = paths.resolve(&cfg.proxy.dir);
    let vjar = fs::read_dir(&proxy_dir)
        .into_iter()
        .flatten()
        .flatten()
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .find(|n| n.starts_with("velocity") && n.ends_with(".jar"));
    match vjar {
        Some(j) => add(Level::Ok, "Velocity", j),
        None => add(
            Level::Fail,
            "Velocity",
            format!("kein velocity-*.jar in {}", proxy_dir.display()),
        ),
    }

    let vtoml = proxy_dir.join("velocity.toml");
    match fs::read_to_string(&vtoml) {
        Err(e) => add(
            Level::Fail,
            "velocity.toml",
            format!("{}: {e}", vtoml.display()),
        ),
        Ok(text) => {
            let v = parse_velocity(&text);
            match v.forwarding.as_deref() {
                Some("modern") => add(Level::Ok, "Weiterleitung", "modern forwarding".into()),
                Some(other) => add(
                    Level::Warn,
                    "Weiterleitung",
                    format!("player-info-forwarding-mode = \"{other}\" - bei online-mode=false auf den Servern sollte hier modern stehen"),
                ),
                None => add(Level::Warn, "Weiterleitung", "player-info-forwarding-mode fehlt".into()),
            }
            // Die festen Server muessen dastehen. Die Dungeons nicht: die
            // traegt Terranova beim Oeffnen ein und laesst den Proxy neu
            // laden - stehen sie trotzdem von Hand drin, weicht der eigene
            // Block ihnen beim naechsten Schreiben.
            let missing: Vec<String> = cfg
                .servers
                .keys()
                .filter(|n| !v.servers.contains(n))
                .cloned()
                .collect();
            if missing.is_empty() {
                add(
                    Level::Ok,
                    "Servereintraege",
                    format!("{} in velocity.toml", v.servers.len()),
                );
            } else {
                add(
                    Level::Fail,
                    "Servereintraege",
                    format!("in velocity.toml fehlen: {}", missing.join(", ")),
                );
            }

            // Geschlossene zaehlen nicht mit: sie stehen absichtlich nicht
            // im Proxy, weil sie nicht wieder hochfahren.
            let offen = crate::mines::existing(&paths.dynamic(), cfg.mines.slots)
                .into_iter()
                .filter(|s| crate::mines::closed_at(&paths.mine(&crate::mines::name(*s))).is_none())
                .count();
            let eingetragen = v
                .servers
                .iter()
                .filter(|n| crate::mines::parse_name(n).is_some())
                .count();
            if eingetragen == offen {
                add(
                    Level::Ok,
                    "Dungeon-Eintraege",
                    format!("{offen} offen, {eingetragen} in velocity.toml"),
                );
            } else {
                add(
                    Level::Warn,
                    "Dungeon-Eintraege",
                    format!(
                        "{offen} offen, aber {eingetragen} in velocity.toml - der naechste Start oder das naechste Oeffnen zieht das nach"
                    ),
                );
            }
        }
    }

    // --- Geheimnisse -----------------------------------------------------------
    let fwd = proxy_dir.join("forwarding.secret");
    if fwd.is_file() {
        add(Level::Ok, "Forwarding-Secret", "vorhanden".into());
    } else {
        add(
            Level::Warn,
            "Forwarding-Secret",
            "fehlt - wird beim naechsten Start erzeugt".into(),
        );
    }
    for name in cfg.servers.keys() {
        let pg = paths.server(name).join("config").join("paper-global.yml");
        if pg.is_file() && !paperyml::has_secret(&fs::read_to_string(&pg).unwrap_or_default()) {
            add(
                Level::Warn,
                "Weiterleitung",
                format!(
                    "{name}: kein Secret in config/paper-global.yml - 'terranova sync' setzt es"
                ),
            );
        }
    }

    // --- Web ---------------------------------------------------------------
    let site = &cfg.web.site;
    let root = paths.root.join(&site.dir);
    if site.port == 0 {
        add(
            Level::Ok,
            "Website",
            "abgeschaltet (web.site.port: 0)".into(),
        );
    } else if !root.join("index.html").is_file() {
        add(
            Level::Warn,
            "Website",
            format!(
                "{} hat keine index.html - es wird nichts ausgeliefert",
                root.display()
            ),
        );
    } else {
        add(
            Level::Ok,
            "Website",
            format!("{} auf {}:{}", root.display(), site.bind, site.port),
        );
        if site.bind != "127.0.0.1" && site.bind != "localhost" {
            add(
                Level::Warn,
                "Website",
                format!(
                    "gebunden an {} - die Seite ist aus dem Netz erreichbar. Davor gehoert                      etwas, das TLS spricht und Last abfaengt; dieser Server kann beides nicht.",
                    site.bind
                ),
            );
        }
    }

    // Die Karte gehoert Pl3xMap. Wir koennen nur pruefen, ob das, was in
    // terranova.yml steht, zu dem passt, was das Plugin tatsaechlich tut -
    // sonst zeigt das Dashboard eine Karte als tot an, die laeuft.
    let map = &cfg.web.map;
    if map.port == 0 {
        add(Level::Ok, "Karte", "abgeschaltet (web.map.port: 0)".into());
    } else if !cfg.servers.contains_key(&map.server) {
        add(
            Level::Fail,
            "Karte",
            format!(
                "web.map.server: {} - diesen Server gibt es nicht",
                map.server
            ),
        );
    } else {
        match crate::web::pl3xmap_webserver(&paths.server(&map.server)) {
            None => add(
                Level::Warn,
                "Karte",
                format!(
                    "{}: plugins/Pl3xMap/config.yml nicht lesbar - laeuft das Plugin dort?",
                    map.server
                ),
            ),
            Some((false, _)) => add(
                Level::Warn,
                "Karte",
                format!("{}: Pl3xMaps interner Webserver ist aus", map.server),
            ),
            Some((true, p)) if p != map.port => add(
                Level::Fail,
                "Karte",
                format!(
                    "web.map.port: {}, aber Pl3xMap horcht auf {p} ({}/plugins/Pl3xMap/config.yml)",
                    map.port, map.server
                ),
            ),
            Some((true, p)) => add(
                Level::Ok,
                "Karte",
                format!("Pl3xMap in {} auf Port {p}", map.server),
            ),
        }

        // Und ob der oeffentliche Name ueberhaupt existiert.
        //
        // Genau daran hing es einmal: lokal antwortete Pl3xMap munter mit 200
        // und doctor meldete lauter Haken, waehrend der Name aus web.map.url
        // gar nicht im DNS stand. Wer die Karte im Browser aufrief, sah nur
        // eine leere Seite.
        //
        // Geprueft wird bewusst nur der Name, nicht die Seite selbst: fuer
        // HTTPS braeuchte es TLS und damit eine Abhaengigkeit, die sich fuer
        // diese eine Pruefung nicht lohnt.
        if let Some(host) = host_of(&map.url) {
            let loest_auf = (host.as_str(), 443u16)
                .to_socket_addrs()
                .is_ok_and(|mut a| a.next().is_some());
            if loest_auf {
                add(Level::Ok, "Karte", format!("{host} loest auf"));
            } else {
                add(
                    Level::Warn,
                    "Karte",
                    format!(
                        "{host} loest nicht auf - web.map.url zeigt ins Leere. \
                         Fehlt der DNS-Eintrag fuer die Unterdomain?"
                    ),
                );
            }
        }
    }

    // --- Ports -----------------------------------------------------------------
    let mut busy = Vec::new();
    for (port, what) in ports_of(cfg) {
        if let Some(pid) = sys::port_owner(port) {
            let who = sys::image_name(pid).unwrap_or_else(|| format!("PID {pid}"));
            busy.push(format!("{port} ({what}) belegt von {who}"));
        }
    }
    if busy.is_empty() {
        add(Level::Ok, "Ports", "alle frei".into());
    } else {
        add(
            Level::Warn,
            "Ports",
            format!("{} - laeuft das Netzwerk schon?", busy.join("; ")),
        );
    }

    // --- Docker ---------------------------------------------------------------
    if cfg.runtime() == Runtime::Docker {
        match crate::docker::Docker::new(paths, cfg).preflight() {
            Ok(()) => add(Level::Ok, "Docker", "Host-Netzwerk funktioniert".into()),
            Err(e) => add(Level::Fail, "Docker", e),
        }
        if let Some(mb) = crate::docker::mem_total_mb() {
            let base: u32 =
                cfg.proxy.memory.0 + cfg.servers.values().map(|s| s.memory.0).sum::<u32>();
            // Grob: JVM braucht ueber dem Heap noch etwas, und die
            // Datenbanken wollen auch leben.
            let fits = (mb as i64 - i64::from(base) - 1536) / i64::from(cfg.mines.memory.0);
            let detail = format!(
                "{mb} MB in der Docker-Maschine, Grundlast {base} MB - Platz fuer etwa {} Dungeon(s)",
                fits.max(0)
            );
            if fits < 1 {
                add(Level::Fail, "Speicher", format!(
                    "{detail}. In %USERPROFILE%\\.wslconfig eintragen: [wsl2] memory=28GB, dann wsl --shutdown"
                ));
            } else if fits < i64::from(cfg.mines.slots) {
                add(Level::Warn, "Speicher", format!(
                    "{detail}, aber {} Plaetze vorgesehen. Mehr geht ueber %USERPROFILE%\\.wslconfig: [wsl2] memory=28GB",
                    cfg.mines.slots
                ));
            } else {
                add(Level::Ok, "Speicher", detail);
            }
        }
    }

    // --- Reste aus der Skript-Zeit ----------------------------------------------
    if paths.root.join("scripts").is_dir() {
        add(
            Level::Warn,
            "Altlast",
            "scripts/ gibt es noch - start.bat darf nicht parallel laufen, zwei Aufsichten streiten sich".into(),
        );
    }
    if scheduled_task_exists("Terranova Neustart") {
        add(
            Level::Warn,
            "Altlast",
            "geplante Aufgabe 'Terranova Neustart' ist noch da - sie wuerde zusaetzlich zum eingebauten Zeitplan neu starten".into(),
        );
    }

    c
}

pub fn render(checks: &[Check]) -> String {
    let mut s = String::new();
    for c in checks {
        let _ = writeln!(s, "[{}] {:<18} {}", c.level.mark(), c.what, c.detail);
    }
    let fails = checks.iter().filter(|c| c.level == Level::Fail).count();
    let warns = checks.iter().filter(|c| c.level == Level::Warn).count();
    let _ = writeln!(
        s,
        "\n{fails} Fehler, {warns} Hinweise, {} geprueft",
        checks.len()
    );
    s
}

pub fn worst(checks: &[Check]) -> Level {
    if checks.iter().any(|c| c.level == Level::Fail) {
        Level::Fail
    } else if checks.iter().any(|c| c.level == Level::Warn) {
        Level::Warn
    } else {
        Level::Ok
    }
}

fn ports_of(cfg: &Config) -> Vec<(u16, String)> {
    let mut v = vec![
        (cfg.proxy.port, "proxy".to_string()),
        (cfg.deps.mariadb.port, "mariadb".into()),
        (cfg.deps.redis.port, "redis".into()),
        (cfg.dashboard.port, "dashboard".into()),
    ];
    if cfg.web.site.port != 0 {
        v.push((cfg.web.site.port, "website".into()));
    }
    for (name, s) in &cfg.servers {
        v.push((s.port, name.clone()));
    }
    v
}

/// Der Rechnername aus einer Adresse wie `https://karte.example.org/karte`.
///
/// Kein URL-Parser dafuer: gebraucht wird genau dieser eine Teil, und die
/// Adresse steht in der eigenen Konfiguration - sie kommt nicht von aussen.
fn host_of(url: &str) -> Option<String> {
    let rest = url.split_once("://").map_or(url, |(_, r)| r);
    // Pfad, Abfrage und Anker abschneiden, etwaige Anmeldedaten davor weg
    let host = rest.split(['/', '?', '#']).next()?.rsplit('@').next()?;
    // Port abtrennen - eine IPv6-Adresse steht dabei in Klammern
    let host = if let Some(v6) = host.strip_prefix('[') {
        v6.split(']').next()?
    } else {
        host.split(':').next()?
    };
    (!host.is_empty()).then(|| host.to_string())
}

/// Ob noch eine geplante Windows-Aufgabe fuer den Neustart eingetragen ist.
/// Den Neustart macht der Supervisor inzwischen selbst; eine uebrig
/// gebliebene Aufgabe wuerde ein zweites Mal stoppen.
#[cfg(windows)]
fn scheduled_task_exists(name: &str) -> bool {
    let mut cmd = Command::new("schtasks");
    cmd.args(["/Query", "/TN", name])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    sys::hide_window(&mut cmd)
        .status()
        .is_ok_and(|s| s.success())
}

/// Unter Unix gibt es diese Aufgabe nicht - dort war der Neustart nie etwas
/// anderes als der eigene Zeitplan.
#[cfg(unix)]
fn scheduled_task_exists(_name: &str) -> bool {
    false
}

/// Was aus velocity.toml interessiert.
#[derive(Debug, Default, PartialEq)]
pub struct Velocity {
    pub servers: Vec<String>,
    pub forwarding: Option<String>,
}

/// Ein kleiner Blick in die TOML-Datei statt eines TOML-Parsers: gebraucht
/// werden zwei Dinge, und die Datei gehoert Velocity.
pub fn parse_velocity(text: &str) -> Velocity {
    let mut v = Velocity::default();
    let mut in_servers = false;
    for line in text.lines() {
        let line = line.trim();
        if line.starts_with('#') || line.is_empty() {
            continue;
        }
        if line.starts_with('[') {
            in_servers = line == "[servers]";
            continue;
        }
        let Some((key, value)) = line.split_once('=') else {
            continue;
        };
        let key = key.trim();
        let value = value.trim().trim_matches(['"', '\'']);
        if in_servers {
            // try = ["main"] ist eine Liste, kein Server
            if key != "try" && !value.starts_with('[') {
                v.servers.push(key.to_string());
            }
        } else if key == "player-info-forwarding-mode" {
            v.forwarding = Some(value.to_string());
        }
    }
    v
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rechnername_aus_der_adresse() {
        let h = |s| host_of(s).unwrap_or_default();
        assert_eq!(h("https://karte.example.org"), "karte.example.org");
        assert_eq!(
            h("https://karte.example.org/karte?z=3#hier"),
            "karte.example.org"
        );
        assert_eq!(h("http://localhost:8080"), "localhost");
        // ohne Schema
        assert_eq!(h("karte.example.org"), "karte.example.org");
        // IPv6 steht in Klammern, der Doppelpunkt darin ist kein Port
        assert_eq!(h("http://[::1]:8080/"), "::1");
        // Leer heisst: keine oeffentliche Adresse eingetragen, nichts zu pruefen
        assert_eq!(host_of(""), None);
        assert_eq!(host_of("https://"), None);
    }

    const TOML: &str = r#"
config-version = "2.9"
bind = "0.0.0.0:25565"
player-info-forwarding-mode = "modern"
forwarding-secret-file = "forwarding.secret"

[servers]
	# Die drei festen Server.
	main = "127.0.0.1:25566"
	build = "127.0.0.1:25567"
	mining-1 = "127.0.0.1:25571"
	try = ["main"]

[advanced]
	compression-threshold = 256
"#;

    #[test]
    fn liest_server_und_weiterleitung() {
        let v = parse_velocity(TOML);
        assert_eq!(v.forwarding.as_deref(), Some("modern"));
        assert_eq!(v.servers, ["main", "build", "mining-1"]);
    }

    #[test]
    fn die_echte_velocity_toml_passt_zur_config() {
        // Gegen die Datei, die wirklich ausgeliefert wird.
        let text = include_str!("../../proxy/velocity.toml");
        let v = parse_velocity(text);
        let cfg = Config::parse(crate::testutil::EXAMPLE_CONFIG).unwrap();
        assert_eq!(v.forwarding.as_deref(), Some("modern"));
        for name in cfg.servers.keys() {
            assert!(v.servers.contains(name), "{name} fehlt in velocity.toml");
        }
    }
}
