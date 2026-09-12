//! terranova.yml - was laeuft, mit wie viel Speicher, auf welchem Port.

use std::collections::BTreeMap;
use std::fmt;
use std::fs;
use std::path::PathBuf;
use std::time::Duration;

use serde::de::{self, Deserializer, Visitor};
use serde::Deserialize;

use crate::paths::Paths;

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Config {
    #[serde(default)]
    pub runtime: RuntimeChoice,
    #[serde(default)]
    pub java: Java,
    pub deps: Deps,
    pub proxy: Proxy,
    pub servers: BTreeMap<String, Server>,
    #[serde(default = "default_rcon_offset")]
    pub rcon_offset: u16,
    pub mines: Mines,
    #[serde(default)]
    pub schedule: Schedule,
    #[serde(default)]
    pub dashboard: Dashboard,
    #[serde(default)]
    pub web: Web,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum RuntimeChoice {
    #[default]
    Auto,
    Native,
    Docker,
}

/// Die tatsaechlich benutzte Laufzeit, nachdem `auto` aufgeloest ist.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Runtime {
    Native,
    Docker,
}

impl fmt::Display for Runtime {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Runtime::Native => "native",
            Runtime::Docker => "docker",
        })
    }
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Java {
    #[serde(default = "default_java_path")]
    pub path: String,
    #[serde(default = "default_java_image")]
    pub image: String,
    #[serde(default)]
    pub flags: JavaFlags,
    #[serde(default)]
    pub extra: Vec<String>,
    /// Welche Java-Hauptversion die Server mindestens brauchen. Paper
    /// schreibt sie in sein Jar (version.json -> java_version).
    #[serde(default = "default_java_version")]
    pub version: u32,
    /// Was nachgeladen wird, wenn kein passendes Java da ist.
    #[serde(default)]
    pub download: Option<JavaDownload>,
}

/// Ein festgenageltes Temurin-JRE: welcher Build, und je Plattform die
/// Pruefsumme des Archivs.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct JavaDownload {
    /// So, wie Adoptium ihn nennt, etwa "jdk-25.0.4.1+1"
    pub release: String,
    /// "windows-x64", "linux-x64", "linux-aarch64" -> sha256
    pub sha256: BTreeMap<String, String>,
}

impl Default for Java {
    fn default() -> Java {
        Java {
            path: default_java_path(),
            image: default_java_image(),
            flags: JavaFlags::default(),
            extra: Vec::new(),
            version: default_java_version(),
            download: None,
        }
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum JavaFlags {
    #[default]
    Aikar,
    None,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Deps {
    pub mariadb: MariaDb,
    pub redis: Redis,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MariaDb {
    pub version: String,
    pub port: u16,
    /// Pruefsumme des Windows-Archivs. Unter Unix wird nichts geladen, also
    /// auch nichts geprueft - das Feld bleibt trotzdem erlaubt, damit
    /// dieselbe terranova.yml auf beiden Systemen liest.
    #[serde(default)]
    #[cfg_attr(unix, allow(dead_code))]
    pub sha256: Option<String>,
    pub image: String,
    pub user: String,
    pub password: String,
    pub databases: Vec<String>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Redis {
    pub port: u16,
    /// Nur fuer Windows: dort gibt es Redis offiziell nicht und Terranova
    /// laedt einen festgenagelten Fremdbau. Unter Unix kommt Redis aus der
    /// Paketverwaltung, beide Felder bleiben dann ungelesen.
    #[cfg_attr(unix, allow(dead_code))]
    pub windows_commit: String,
    /// Dateiname -> erwarteter sha256 der heruntergeladenen Windows-Exes
    #[serde(default)]
    #[cfg_attr(unix, allow(dead_code))]
    pub sha256: BTreeMap<String, String>,
    pub image: String,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Proxy {
    #[serde(default = "default_proxy_dir")]
    pub dir: PathBuf,
    pub port: u16,
    pub memory: Mem,
    #[serde(default = "default_proxy_stop")]
    pub stop_timeout: Dur,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Server {
    pub port: u16,
    pub memory: Mem,
    #[serde(default)]
    pub stop_timeout: Option<Dur>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Mines {
    /// Name der Vorlage unter templates/
    #[serde(default = "default_mine_template")]
    pub template: String,
    pub slots: u8,
    pub base_port: u16,
    pub memory: Mem,
    pub lifetime: Dur,
    #[serde(default = "default_mine_motd")]
    pub motd: String,
    /// Offene Dungeons nach einem Netzwerk-Neustart wieder hochfahren.
    ///
    /// An, solange nichts anderes dasteht: ein Dungeon existiert nur, weil
    /// jemand ihn geoeffnet hat - dann soll er auch laufen. Von selbst
    /// angelegt wird trotzdem keiner. Geschlossene bleiben unten.
    #[serde(default = "default_true")]
    pub resume: bool,
    #[serde(default)]
    pub stop_timeout: Option<Dur>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Schedule {
    #[serde(default)]
    pub daily_restart: Option<DailyRestart>,
    #[serde(default = "default_reap_every")]
    pub reap_every: Dur,
}

impl Default for Schedule {
    fn default() -> Schedule {
        Schedule {
            daily_restart: None,
            reap_every: default_reap_every(),
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DailyRestart {
    /// "HH:MM", Ortszeit
    pub at: String,
    /// Leer heisst: alle festen Server
    #[serde(default)]
    pub servers: Vec<String>,
    #[serde(default = "default_restart_gap")]
    pub gap: Dur,
    #[serde(default = "default_restart_warn")]
    pub warn: Dur,
    #[serde(default)]
    pub message: String,
}

impl DailyRestart {
    pub fn time(&self) -> Result<(u8, u8), String> {
        parse_hhmm(&self.at)
    }
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Dashboard {
    #[serde(default = "default_dashboard_port")]
    pub port: u16,
    #[serde(default)]
    pub on_window_close: WindowClose,
}

impl Default for Dashboard {
    fn default() -> Dashboard {
        Dashboard {
            port: default_dashboard_port(),
            on_window_close: WindowClose::default(),
        }
    }
}

/// Was im Browser liegt: die oeffentliche Seite und die Weltkarte.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Web {
    #[serde(default)]
    pub site: Site,
    #[serde(default)]
    pub map: Map,
}

/// Die oeffentliche Seite. Terranova liefert sie selbst aus - ein eigener
/// Webserver daneben waere ein zweites Ding, das jemand starten, aktuell
/// halten und ueberwachen muesste.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Site {
    /// Verzeichnis mit dem fertigen Build, relativ zum Netzwerkverzeichnis.
    #[serde(default = "default_site_dir")]
    pub dir: String,
    /// 0 schaltet die Auslieferung ab.
    #[serde(default = "default_site_port")]
    pub port: u16,
    /// 127.0.0.1 = nur dieser Rechner. Fuer den oeffentlichen Betrieb
    /// 0.0.0.0 - das ist eine Entscheidung, die bewusst hier steht und
    /// nicht als Vorgabe.
    #[serde(default = "default_bind")]
    pub bind: String,
}

impl Default for Site {
    fn default() -> Site {
        Site {
            dir: default_site_dir(),
            port: default_site_port(),
            bind: default_bind(),
        }
    }
}

/// Die Weltkarte. Sie laeuft nicht als eigener Prozess, sondern im
/// Pl3xMap-Plugin von main - hier steht nur, wo nachzusehen ist.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Map {
    /// internal-webserver.port aus plugins/Pl3xMap/config.yml. 0 = keine
    /// Karte, dann wird auch nicht nachgesehen.
    #[serde(default = "default_map_port")]
    pub port: u16,
    /// In welchem Server das Plugin steckt.
    #[serde(default = "default_map_server")]
    pub server: String,
    /// Die oeffentliche Adresse, falls es eine gibt - nur zum Anzeigen.
    #[serde(default)]
    pub url: String,
}

impl Default for Map {
    fn default() -> Map {
        Map {
            port: default_map_port(),
            server: default_map_server(),
            url: String::new(),
        }
    }
}

fn default_site_dir() -> String {
    "website".into()
}
fn default_site_port() -> u16 {
    8081
}
fn default_bind() -> String {
    "127.0.0.1".into()
}
fn default_map_port() -> u16 {
    8080
}
fn default_map_server() -> String {
    "main".into()
}

/// Was passiert, wenn das Startfenster geschlossen wird.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum WindowClose {
    #[default]
    Stop,
    Detach,
}

fn default_rcon_offset() -> u16 {
    100
}
fn default_java_path() -> String {
    "java".into()
}
fn default_java_image() -> String {
    "eclipse-temurin:25-jre".into()
}
fn default_java_version() -> u32 {
    25
}
fn default_proxy_dir() -> PathBuf {
    PathBuf::from("proxy")
}
fn default_proxy_stop() -> Dur {
    Dur(Duration::from_secs(15))
}
fn default_mine_template() -> String {
    "mining".into()
}
fn default_mine_motd() -> String {
    "Terranova Mine {n}".into()
}
fn default_true() -> bool {
    true
}
fn default_reap_every() -> Dur {
    Dur(Duration::from_secs(15 * 60))
}
fn default_restart_gap() -> Dur {
    Dur(Duration::from_secs(120))
}
fn default_restart_warn() -> Dur {
    Dur(Duration::from_secs(30))
}
fn default_dashboard_port() -> u16 {
    25590
}

/// Standard fuer Server und Dungeons, wenn nichts angegeben ist.
pub const SERVER_STOP_TIMEOUT: Duration = Duration::from_secs(120);

// --- Speicher und Dauer: "4G", "512M", "24h", "180s" ------------------------

/// Speicher in Megabyte.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Mem(pub u32);

/// Eine Dauer; in der Datei als "30s", "15m", "24h", "2d" oder blanke Sekunden.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Dur(pub Duration);

fn split_unit(s: &str) -> (&str, &str) {
    let i = s.find(|c: char| !c.is_ascii_digit()).unwrap_or(s.len());
    (&s[..i], s[i..].trim())
}

pub fn parse_mem(s: &str) -> Result<Mem, String> {
    let s = s.trim();
    let (num, unit) = split_unit(s);
    let n: u64 = num
        .parse()
        .map_err(|_| format!("'{s}' ist keine Speicherangabe (z.B. 512M, 4G)"))?;
    let mb = match unit.to_ascii_uppercase().as_str() {
        "" | "M" | "MB" => n,
        "G" | "GB" => n * 1024,
        _ => return Err(format!("'{s}': unbekannte Einheit, erlaubt sind M und G")),
    };
    if mb < 64 {
        return Err(format!("'{s}' ist zu wenig Speicher fuer eine JVM"));
    }
    u32::try_from(mb)
        .map(Mem)
        .map_err(|_| format!("'{s}' ist zu viel"))
}

pub fn parse_dur(s: &str) -> Result<Dur, String> {
    let s = s.trim();
    let (num, unit) = split_unit(s);
    let n: u64 = num
        .parse()
        .map_err(|_| format!("'{s}' ist keine Dauer (z.B. 30s, 15m, 24h)"))?;
    let secs = match unit {
        "" | "s" => n,
        "m" => n * 60,
        "h" => n * 3600,
        "d" => n * 86_400,
        _ => {
            return Err(format!(
                "'{s}': unbekannte Einheit, erlaubt sind s, m, h, d"
            ))
        }
    };
    Ok(Dur(Duration::from_secs(secs)))
}

pub fn parse_hhmm(s: &str) -> Result<(u8, u8), String> {
    let err = || format!("'{s}' ist keine Uhrzeit im Format HH:MM");
    let (h, m) = s.trim().split_once(':').ok_or_else(err)?;
    let h: u8 = h.parse().map_err(|_| err())?;
    let m: u8 = m.parse().map_err(|_| err())?;
    if h > 23 || m > 59 {
        return Err(err());
    }
    Ok((h, m))
}

/// Zahl oder Zeichenkette - `memory: 2048` soll genauso gehen wie `memory: 2G`.
struct NumOrStr<T> {
    parse: fn(&str) -> Result<T, String>,
    what: &'static str,
}

impl<'de, T> Visitor<'de> for NumOrStr<T> {
    type Value = T;

    fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.what)
    }

    fn visit_str<E: de::Error>(self, v: &str) -> Result<T, E> {
        (self.parse)(v).map_err(E::custom)
    }

    fn visit_u64<E: de::Error>(self, v: u64) -> Result<T, E> {
        (self.parse)(&v.to_string()).map_err(E::custom)
    }

    fn visit_i64<E: de::Error>(self, v: i64) -> Result<T, E> {
        (self.parse)(&v.to_string()).map_err(E::custom)
    }
}

impl<'de> Deserialize<'de> for Mem {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Mem, D::Error> {
        d.deserialize_any(NumOrStr {
            parse: parse_mem,
            what: "Speicher wie 512M oder 4G",
        })
    }
}

impl<'de> Deserialize<'de> for Dur {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Dur, D::Error> {
        d.deserialize_any(NumOrStr {
            parse: parse_dur,
            what: "eine Dauer wie 30s, 15m oder 24h",
        })
    }
}

// --- Knoten: was der Supervisor startet --------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum NodeKind {
    /// Wird zuerst gestartet und zuerst gestoppt
    MariaDb,
    Redis,
    Proxy,
    Server,
    Mine(u8),
}

impl NodeKind {
    /// Haelt der Knoten eine Welt, die beim Stoppen gespeichert werden muss?
    pub fn is_minecraft(self) -> bool {
        matches!(self, NodeKind::Proxy | NodeKind::Server | NodeKind::Mine(_))
    }
}

/// Alles, was es braucht, um einen Proxy oder Server zu starten.
#[derive(Debug, Clone)]
pub struct NodeSpec {
    pub name: String,
    pub kind: NodeKind,
    pub dir: PathBuf,
    /// Vorlage unter templates/, aus der sync-servers bestueckt; der Proxy hat keine.
    pub template: Option<String>,
    pub port: u16,
    pub rcon_port: Option<u16>,
    pub memory: Mem,
    pub stop_timeout: Duration,
    pub motd: Option<String>,
}

impl NodeSpec {
    pub fn is_proxy(&self) -> bool {
        self.kind == NodeKind::Proxy
    }
}

pub const PROXY: &str = "proxy";

impl Config {
    pub fn load(paths: &Paths) -> Result<Config, String> {
        let file = paths.config();
        let text = fs::read_to_string(&file).map_err(|e| format!("{}: {e}", file.display()))?;
        Config::parse(&text).map_err(|e| format!("{}: {e}", file.display()))
    }

    pub fn parse(text: &str) -> Result<Config, String> {
        let cfg: Config = serde_saphyr::from_str(text).map_err(|e| e.to_string())?;
        cfg.validate()?;
        Ok(cfg)
    }

    /// `auto` ist unter Windows immer native. Docker wird nie gewaehlt, nur
    /// weil es installiert ist - wer native Prozesse erwartet, soll nicht
    /// ploetzlich Container bekommen.
    pub fn runtime(&self) -> Runtime {
        match self.runtime {
            RuntimeChoice::Native => Runtime::Native,
            RuntimeChoice::Docker => Runtime::Docker,
            // auto heisst nativ, auf beiden Systemen. Docker nur, wenn es
            // ausdruecklich dasteht: wer native Prozesse erwartet, soll nicht
            // ploetzlich Container bekommen, bloss weil Docker installiert
            // ist. Solange es unter Linux keine native Laufzeit gab, stand
            // hier Docker - jetzt gibt es sie.
            RuntimeChoice::Auto => Runtime::Native,
        }
    }

    pub fn rcon_port(&self, port: u16) -> u16 {
        port + self.rcon_offset
    }

    pub fn proxy_node(&self, paths: &Paths) -> NodeSpec {
        NodeSpec {
            name: PROXY.into(),
            kind: NodeKind::Proxy,
            dir: paths.resolve(&self.proxy.dir),
            template: None,
            port: self.proxy.port,
            rcon_port: None,
            memory: self.proxy.memory,
            stop_timeout: self.proxy.stop_timeout.0,
            motd: None,
        }
    }

    /// Die festen Server, nach Port sortiert - so steht main vor build vor farm.
    pub fn server_nodes(&self, paths: &Paths) -> Vec<NodeSpec> {
        let mut v: Vec<NodeSpec> = self
            .servers
            .iter()
            .map(|(name, s)| NodeSpec {
                name: name.clone(),
                kind: NodeKind::Server,
                dir: paths.server(name),
                // Ein fester Server wird nicht bestueckt: was in seinem
                // Verzeichnis liegt, ist was laeuft, und es ist versioniert.
                // Nur seine server.properties und paper-global.yml entstehen
                // beim Start neu - da gehoeren Geheimnisse hinein.
                template: None,
                port: s.port,
                rcon_port: Some(self.rcon_port(s.port)),
                memory: s.memory,
                stop_timeout: s.stop_timeout.map_or(SERVER_STOP_TIMEOUT, |d| d.0),
                motd: None,
            })
            .collect();
        v.sort_by_key(|n| n.port);
        v
    }

    pub fn mine_node(&self, paths: &Paths, slot: u8) -> NodeSpec {
        let name = crate::mines::name(slot);
        let port = self.mines.base_port + u16::from(slot);
        let dir = paths.mine(&name);
        // Bestueckt wird aus der Vorlage, aus der er auch entstanden ist -
        // die steht in seiner Markierung, sobald beim Oeffnen eine andere
        // gewaehlt wurde. Ohne Markierung gilt die Vorgabe.
        let template =
            crate::mines::template_of(&dir).unwrap_or_else(|| self.mines.template.clone());
        NodeSpec {
            dir,
            name,
            kind: NodeKind::Mine(slot),
            template: Some(template),
            port,
            rcon_port: Some(self.rcon_port(port)),
            memory: self.mines.memory,
            stop_timeout: self.mines.stop_timeout.map_or(SERVER_STOP_TIMEOUT, |d| d.0),
            motd: Some(self.mines.motd.replace("{n}", &slot.to_string())),
        }
    }

    /// Proxy, feste Server oder Dungeon - nach Namen.
    pub fn node(&self, paths: &Paths, name: &str) -> Option<NodeSpec> {
        if name == PROXY {
            return Some(self.proxy_node(paths));
        }
        if let Some(slot) = crate::mines::parse_name(name) {
            return (slot <= self.mines.slots).then(|| self.mine_node(paths, slot));
        }
        self.server_nodes(paths)
            .into_iter()
            .find(|n| n.name == name)
    }

    pub fn validate(&self) -> Result<(), String> {
        let mut problems = Vec::new();

        for name in self.servers.keys() {
            let ok = !name.is_empty()
                && name
                    .chars()
                    .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-');
            if !ok {
                problems.push(format!("Servername '{name}': nur a-z, 0-9 und -"));
            }
            if name == PROXY || crate::mines::parse_name(name).is_some() {
                problems.push(format!("Servername '{name}' ist reserviert"));
            }
        }

        if self.mines.slots == 0 || self.mines.slots > 99 {
            problems.push("mines.slots muss zwischen 1 und 99 liegen".into());
        }
        if self.rcon_offset == 0 {
            problems.push("rcon_offset darf nicht 0 sein".into());
        }

        // Kein Port darf doppelt vergeben sein - auch nicht ueber den
        // RCON-Versatz hinweg.
        let mut ports: Vec<(u32, String)> = vec![
            (self.proxy.port.into(), "proxy".into()),
            (self.deps.mariadb.port.into(), "mariadb".into()),
            (self.deps.redis.port.into(), "redis".into()),
            (self.dashboard.port.into(), "dashboard".into()),
        ];
        for (name, s) in &self.servers {
            ports.push((s.port.into(), name.clone()));
            ports.push((
                u32::from(s.port) + u32::from(self.rcon_offset),
                format!("{name} (RCON)"),
            ));
        }
        for slot in 1..=u32::from(self.mines.slots) {
            let p = u32::from(self.mines.base_port) + slot;
            ports.push((p, format!("mining-{slot}")));
            ports.push((
                p + u32::from(self.rcon_offset),
                format!("mining-{slot} (RCON)"),
            ));
        }
        let mut sorted = ports.clone();
        sorted.sort();
        for w in sorted.windows(2) {
            if w[0].0 == w[1].0 {
                problems.push(format!(
                    "Port {} doppelt: {} und {}",
                    w[0].0, w[0].1, w[1].1
                ));
            }
        }
        for (p, what) in &ports {
            if *p > 65_535 {
                problems.push(format!("{what}: Port {p} liegt ausserhalb von 1-65535"));
            }
        }

        if let Some(r) = &self.schedule.daily_restart {
            if let Err(e) = r.time() {
                problems.push(format!("schedule.daily_restart.at: {e}"));
            }
            for s in &r.servers {
                if !self.servers.contains_key(s) {
                    problems.push(format!(
                        "schedule.daily_restart.servers: '{s}' gibt es nicht"
                    ));
                }
            }
        }

        for db in &self.deps.mariadb.databases {
            let ok = !db.is_empty() && db.chars().all(|c| c.is_ascii_alphanumeric() || c == '_');
            if !ok {
                problems.push(format!("Datenbankname '{db}': nur a-z, 0-9 und _"));
            }
        }

        if problems.is_empty() {
            Ok(())
        } else {
            Err(problems.join("\n"))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use crate::testutil::EXAMPLE_CONFIG as EXAMPLE;

    #[test]
    fn beispiel_aus_dem_repository_ist_gueltig() {
        let c = Config::parse(EXAMPLE).expect("terranova.yml");
        assert_eq!(c.servers.len(), 3);
        assert_eq!(c.servers["main"].memory, Mem(4096));
        assert_eq!(
            c.servers["main"].stop_timeout,
            Some(Dur(Duration::from_secs(180)))
        );
        assert_eq!(c.proxy.memory, Mem(512));
        assert_eq!(c.mines.lifetime, Dur(Duration::from_secs(24 * 3600)));
        assert_eq!(c.schedule.reap_every, Dur(Duration::from_secs(15 * 60)));
        assert_eq!(
            c.schedule.daily_restart.as_ref().unwrap().time(),
            Ok((4, 0))
        );
        assert_eq!(c.deps.mariadb.databases.len(), 8);
    }

    #[test]
    fn auto_ist_unter_windows_native() {
        let c = Config::parse(EXAMPLE).unwrap();
        assert_eq!(c.runtime, RuntimeChoice::Auto);
        // Auf beiden Systemen nativ - Docker kommt nur, wenn es in der
        // Config steht.
        assert_eq!(c.runtime(), Runtime::Native);
    }

    #[test]
    fn knoten_nach_port_sortiert() {
        let c = Config::parse(EXAMPLE).unwrap();
        let p = Paths::new("C:\\net");
        let names: Vec<_> = c.server_nodes(&p).into_iter().map(|n| n.name).collect();
        assert_eq!(names, ["main", "build", "farm"]);
        let m = c.mine_node(&p, 3);
        assert_eq!(
            (m.name.as_str(), m.port, m.rcon_port),
            ("mining-3", 25573, Some(25673))
        );
        assert_eq!(m.motd.as_deref(), Some("Terranova Mine 3"));
        assert_eq!(m.dir, p.root.join("servers_dynamic").join("mining-3"));
        assert!(c.node(&p, "mining-9").is_none(), "nur 8 Plaetze");
        assert!(c.node(&p, "proxy").unwrap().is_proxy());
    }

    #[test]
    fn einheiten() {
        assert_eq!(parse_mem("512M"), Ok(Mem(512)));
        assert_eq!(parse_mem("4G"), Ok(Mem(4096)));
        assert_eq!(parse_mem("2048"), Ok(Mem(2048)));
        assert!(parse_mem("4T").is_err());
        assert!(parse_mem("16M").is_err());
        assert_eq!(parse_dur("90"), Ok(Dur(Duration::from_secs(90))));
        assert_eq!(parse_dur("2m"), Ok(Dur(Duration::from_secs(120))));
        assert_eq!(parse_dur("24h"), Ok(Dur(Duration::from_secs(86_400))));
        assert!(parse_dur("5y").is_err());
        assert_eq!(parse_hhmm("04:00"), Ok((4, 0)));
        assert!(parse_hhmm("24:00").is_err());
        assert!(parse_hhmm("4").is_err());
    }

    #[test]
    fn doppelte_ports_werden_abgelehnt() {
        // main auf 25571 kollidiert mit mining-1
        let bad = EXAMPLE.replace("port: 25566", "port: 25571");
        let err = Config::parse(&bad).unwrap_err();
        assert!(err.contains("25571"), "{err}");
    }

    #[test]
    fn rcon_versatz_kollidiert_auch() {
        // build auf 25666 kollidiert mit mains RCON-Port 25566+100
        let bad = EXAMPLE.replace("port: 25567", "port: 25666");
        assert!(Config::parse(&bad).unwrap_err().contains("25666"));
    }

    #[test]
    fn unbekannte_felder_und_reservierte_namen() {
        assert!(Config::parse(&EXAMPLE.replace("rcon_offset:", "rcon_ofset:")).is_err());
        let bad = EXAMPLE.replace("farm:  {", "mining-2:  {");
        assert!(Config::parse(&bad).unwrap_err().contains("reserviert"));
    }
}
