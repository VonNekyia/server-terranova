//! templates/ -> servers/<name>: Paper, gemeinsame Configs, Plugin-Jars und
//! server.properties.
//!
//! Jedes Jar liegt genau einmal im Repository, naemlich unter templates/.
//! Ein Plugin-Update ist damit eine Datei und kein viermaliges Kopieren.
//!
//! Neu gegenueber sync-servers.ps1: was einmal hierher kopiert wurde, steht
//! in einer Liste (.terranova-sync.json). Verschwindet ein Jar aus der
//! Vorlage, wird die Kopie geloescht. Das alte Copy-Item hat nie etwas
//! entfernt - nach einem Plugin-Update lagen HuskSync-4.0.0.jar und
//! HuskSync-4.0.1.jar nebeneinander im Serververzeichnis, und Paper haette
//! beide geladen.

use std::collections::BTreeSet;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::config::{Config, NodeSpec};
use crate::paths::Paths;
use crate::{paperyml, props, secrets};

/// Liste der Dateien, die eine frueherer Lauf hierher kopiert hat.
const MANIFEST: &str = ".terranova-sync.json";

#[derive(Debug, Default, Serialize, Deserialize)]
struct Manifest {
    /// Pfade relativ zum Serververzeichnis, mit / getrennt
    files: Vec<String>,
}

#[derive(Debug, Default)]
pub struct Report {
    pub copied: Vec<String>,
    pub pruned: Vec<String>,
    pub props: bool,
    pub paper_global: bool,
}

impl Report {
    pub fn nothing_to_do(&self) -> bool {
        self.copied.is_empty() && self.pruned.is_empty() && !self.props && !self.paper_global
    }
}

pub struct Syncer<'a> {
    paths: &'a Paths,
    forwarding_secret: String,
    rcon_password: String,
}

impl<'a> Syncer<'a> {
    /// Laedt die Geheimnisse oder legt sie an.
    pub fn new(paths: &'a Paths, cfg: &'a Config) -> io::Result<Syncer<'a>> {
        let fwd = paths.resolve(&cfg.proxy.dir).join("forwarding.secret");
        let (forwarding_secret, _) = secrets::load_or_create(&fwd)?;
        let (rcon_password, _) = secrets::load_or_create(&paths.rcon_secret())?;
        Ok(Syncer {
            paths,
            forwarding_secret,
            rcon_password,
        })
    }

    /// Bestueckt einen Server. Der Proxy hat keine Vorlage und wird
    /// uebersprungen.
    pub fn sync(&self, node: &NodeSpec, dry_run: bool) -> io::Result<Report> {
        let mut r = Report::default();
        let Some(template) = node.template.as_deref() else {
            return Ok(r);
        };
        let dir = &node.dir;
        let common = self.paths.common();
        let own = self.paths.template(template);

        if !dry_run {
            fs::create_dir_all(dir)?;
        }

        // Alles, was dieser Lauf verwaltet - danach abgeglichen mit dem
        // letzten Lauf.
        let mut managed: BTreeSet<String> = BTreeSet::new();

        // --- Paper und die gemeinsamen Configs ------------------------------
        for src in jars_matching(&common, "paper-") {
            let rel = file_name(&src);
            copy_into(&src, dir, &rel, dry_run, &mut r)?;
            managed.insert(rel);
        }
        for name in ["eula.txt", "bukkit.yml", "spigot.yml"] {
            let src = common.join(name);
            if src.is_file() {
                copy_into(&src, dir, name, dry_run, &mut r)?;
            }
        }
        for src in files_with_ext(&common.join("config"), "yml") {
            // paper-global.yml wird weiter unten erzeugt, nicht kopiert: dort
            // kommt noch das Forwarding-Secret hinein. Kopieren und danach
            // hineinschreiben hiesse, die Datei bei jedem Lauf zweimal zu
            // schreiben - und sie waere nie fertig.
            if file_name(&src) == "paper-global.yml" {
                continue;
            }
            let rel = format!("config/{}", file_name(&src));
            copy_into(&src, dir, &rel, dry_run, &mut r)?;
        }

        // --- Plugin-Jars ----------------------------------------------------
        for src in jars_matching(&common.join("plugins"), "") {
            let rel = format!("plugins/{}", file_name(&src));
            copy_into(&src, dir, &rel, dry_run, &mut r)?;
            managed.insert(rel);
        }
        let expansions = common
            .join("plugins")
            .join("PlaceholderAPI")
            .join("expansions");
        for src in jars_matching(&expansions, "") {
            let rel = format!("plugins/PlaceholderAPI/expansions/{}", file_name(&src));
            copy_into(&src, dir, &rel, dry_run, &mut r)?;
            managed.insert(rel);
        }
        // Was nur dieser Server braucht.
        for src in jars_matching(&own.join("plugins"), "") {
            let rel = format!("plugins/{}", file_name(&src));
            copy_into(&src, dir, &rel, dry_run, &mut r)?;
            managed.insert(rel);
        }

        // Die HuskSync-Config nur anlegen, wenn noch keine da ist: eine
        // bestehende koennte von Hand angepasst sein.
        let hs_src = common.join("plugins").join("HuskSync").join("config.yml");
        let hs_dst = dir.join("plugins").join("HuskSync").join("config.yml");
        if hs_src.is_file() && !hs_dst.exists() {
            copy_into(&hs_src, dir, "plugins/HuskSync/config.yml", dry_run, &mut r)?;
        }

        // --- server.properties ----------------------------------------------
        let tpl_props = {
            let own_props = own.join("server.properties");
            if own_props.is_file() {
                own_props
            } else {
                common.join("server.properties")
            }
        };
        if tpl_props.is_file() {
            let want = props::wants(
                node.port,
                node.rcon_port.unwrap_or(node.port),
                &self.rcon_password,
                node.motd.as_deref(),
            );
            let rendered = props::render(&fs::read_to_string(&tpl_props)?, &want);
            r.props = write_if_changed(&dir.join("server.properties"), &rendered, dry_run)?;
        }

        // --- paper-global.yml samt Velocity-Weiterleitung -----------------------
        // Quelle ist immer die Vorlage: die Fassung im Serververzeichnis ist
        // abgeleitet und in .gitignore. Wer etwas dauerhaft aendern will,
        // aendert templates/common/config/paper-global.yml.
        let pg_src = common.join("config").join("paper-global.yml");
        if pg_src.is_file() {
            let injected = paperyml::inject(&fs::read_to_string(&pg_src)?, &self.forwarding_secret);
            let pg = dir.join("config").join("paper-global.yml");
            r.paper_global = write_if_changed(&pg, &injected, dry_run)?;
        }

        // --- Aufraeumen: was frueher von hier kam und jetzt nicht mehr ---------
        let manifest_path = dir.join(MANIFEST);
        let previous: Manifest = fs::read_to_string(&manifest_path)
            .ok()
            .and_then(|t| serde_json::from_str(&t).ok())
            .unwrap_or_default();
        for old in &previous.files {
            if managed.contains(old) {
                continue;
            }
            let victim = dir.join(old.replace('/', std::path::MAIN_SEPARATOR_STR));
            if victim.is_file() {
                if !dry_run {
                    fs::remove_file(&victim)?;
                }
                r.pruned.push(old.clone());
            }
        }
        if !dry_run {
            let m = Manifest {
                files: managed.into_iter().collect(),
            };
            fs::write(
                &manifest_path,
                serde_json::to_vec_pretty(&m).expect("Manifest"),
            )?;
        }

        Ok(r)
    }
}

fn file_name(p: &Path) -> String {
    p.file_name()
        .unwrap_or_default()
        .to_string_lossy()
        .into_owned()
}

/// Jars in einem Verzeichnis, deren Name mit `prefix` beginnt. Sortiert,
/// damit die Reihenfolge nicht vom Dateisystem abhaengt.
fn jars_matching(dir: &Path, prefix: &str) -> Vec<PathBuf> {
    let mut v: Vec<PathBuf> = fs::read_dir(dir)
        .into_iter()
        .flatten()
        .flatten()
        .map(|e| e.path())
        .filter(|p| {
            p.is_file()
                && p.extension().is_some_and(|e| e.eq_ignore_ascii_case("jar"))
                && file_name(p).starts_with(prefix)
        })
        .collect();
    v.sort();
    v
}

fn files_with_ext(dir: &Path, ext: &str) -> Vec<PathBuf> {
    let mut v: Vec<PathBuf> = fs::read_dir(dir)
        .into_iter()
        .flatten()
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.is_file() && p.extension().is_some_and(|e| e == ext))
        .collect();
    v.sort();
    v
}

/// Kopiert, wenn das Ziel fehlt oder sich unterscheidet. Ein 64-MB-Paper-Jar
/// jedes Mal neu zu schreiben waere Verschwendung.
fn copy_into(src: &Path, dir: &Path, rel: &str, dry_run: bool, r: &mut Report) -> io::Result<()> {
    let dst = dir.join(rel.replace('/', std::path::MAIN_SEPARATOR_STR));
    if same_file(src, &dst)? {
        return Ok(());
    }
    if !dry_run {
        if let Some(parent) = dst.parent() {
            fs::create_dir_all(parent)?;
        }
        fs::copy(src, &dst)?;
    }
    r.copied.push(rel.to_string());
    Ok(())
}

fn same_file(src: &Path, dst: &Path) -> io::Result<bool> {
    let (Ok(a), Ok(b)) = (fs::metadata(src), fs::metadata(dst)) else {
        return Ok(false);
    };
    if a.len() != b.len() {
        return Ok(false);
    }
    // Gleiche Groesse und das Ziel ist nicht aelter - dann ist es dieselbe
    // Datei. Fuer Jars reicht das; wer eine Vorlage aendert, aendert ihre Zeit.
    match (a.modified(), b.modified()) {
        (Ok(ta), Ok(tb)) => Ok(tb >= ta),
        _ => Ok(false),
    }
}

fn write_if_changed(path: &Path, content: &str, dry_run: bool) -> io::Result<bool> {
    if fs::read_to_string(path).is_ok_and(|old| old == content) {
        return Ok(false);
    }
    if !dry_run {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        fs::write(path, content)?;
    }
    Ok(true)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Config;
    use crate::testutil::tempdir;

    /// Baut ein kleines Netzwerkverzeichnis mit Vorlagen auf.
    fn fixture(tag: &str) -> (PathBuf, Config, Paths) {
        let root = tempdir(tag);
        let common = root.join("templates").join("common");
        fs::create_dir_all(common.join("config")).unwrap();
        fs::create_dir_all(
            common
                .join("plugins")
                .join("PlaceholderAPI")
                .join("expansions"),
        )
        .unwrap();
        fs::create_dir_all(common.join("plugins").join("HuskSync")).unwrap();
        fs::write(common.join("paper-26.2-123.jar"), b"paper").unwrap();
        fs::write(common.join("eula.txt"), "eula=true\n").unwrap();
        fs::write(common.join("bukkit.yml"), "a: 1\n").unwrap();
        fs::write(common.join("spigot.yml"), "b: 2\n").unwrap();
        fs::write(
            common.join("config").join("paper-global.yml"),
            "proxies:\n  velocity:\n    enabled: false\n    online-mode: true\n    secret: ''\n",
        )
        .unwrap();
        fs::write(
            common.join("server.properties"),
            "motd=A Minecraft Server\nserver-port=25565\n",
        )
        .unwrap();
        fs::write(common.join("plugins").join("LuckPerms-5.5.81.jar"), b"lp").unwrap();
        fs::write(
            common.join("plugins").join("HuskSync").join("config.yml"),
            "husksync: 1\n",
        )
        .unwrap();
        fs::write(
            common
                .join("plugins")
                .join("PlaceholderAPI")
                .join("expansions")
                .join("Expansion-player.jar"),
            b"exp",
        )
        .unwrap();

        let own = root.join("templates").join("main");
        fs::create_dir_all(own.join("plugins")).unwrap();
        fs::write(own.join("plugins").join("Nations-1.0.0.jar"), b"nations").unwrap();

        let mine = root.join("templates").join("mining");
        fs::create_dir_all(mine.join("plugins")).unwrap();
        fs::write(
            mine.join("plugins").join("BountyfulMining-1.0.0.jar"),
            b"bm",
        )
        .unwrap();

        let cfg = Config::parse(crate::testutil::EXAMPLE_CONFIG).unwrap();
        let paths = Paths::new(&root);
        (root, cfg, paths)
    }

    #[test]
    fn bestueckt_einen_server_vollstaendig() {
        let (root, cfg, paths) = fixture("sync-full");
        let s = Syncer::new(&paths, &cfg).unwrap();
        let main = cfg.node(&paths, "main").unwrap();
        let r = s.sync(&main, false).unwrap();

        let dir = root.join("servers").join("main");
        for f in [
            "paper-26.2-123.jar",
            "eula.txt",
            "bukkit.yml",
            "spigot.yml",
            "config/paper-global.yml",
            "plugins/LuckPerms-5.5.81.jar",
            "plugins/Nations-1.0.0.jar",
            "plugins/PlaceholderAPI/expansions/Expansion-player.jar",
            "plugins/HuskSync/config.yml",
            "server.properties",
        ] {
            assert!(
                dir.join(f.replace('/', std::path::MAIN_SEPARATOR_STR))
                    .is_file(),
                "{f} fehlt"
            );
        }
        assert!(r.props && r.paper_global);

        // Port und RCON stehen drin, das Secret ebenfalls
        let p = fs::read_to_string(dir.join("server.properties")).unwrap();
        assert!(p.contains("server-port=25566") && p.contains("rcon.port=25666"));
        assert!(p.contains("enable-rcon=true") && p.contains("server-ip=127.0.0.1"));
        let pg = fs::read_to_string(dir.join("config").join("paper-global.yml")).unwrap();
        assert!(pg.contains("enabled: true") && !pg.contains("secret: ''"));
    }

    #[test]
    fn zweiter_lauf_kopiert_nichts_mehr() {
        let (_root, cfg, paths) = fixture("sync-idempotent");
        let s = Syncer::new(&paths, &cfg).unwrap();
        let main = cfg.node(&paths, "main").unwrap();
        s.sync(&main, false).unwrap();
        let again = s.sync(&main, false).unwrap();
        assert!(again.nothing_to_do(), "{again:?}");
    }

    #[test]
    fn altes_jar_verschwindet_nach_einem_update() {
        let (root, cfg, paths) = fixture("sync-prune");
        let s = Syncer::new(&paths, &cfg).unwrap();
        let main = cfg.node(&paths, "main").unwrap();
        s.sync(&main, false).unwrap();

        // Plugin-Update in der Vorlage
        let plugins = root.join("templates").join("common").join("plugins");
        fs::remove_file(plugins.join("LuckPerms-5.5.81.jar")).unwrap();
        fs::write(plugins.join("LuckPerms-5.6.0.jar"), b"lp neu").unwrap();

        let r = s.sync(&main, false).unwrap();
        let dir = root.join("servers").join("main");
        assert_eq!(r.pruned, ["plugins/LuckPerms-5.5.81.jar"]);
        assert!(!dir.join("plugins").join("LuckPerms-5.5.81.jar").exists());
        assert!(dir.join("plugins").join("LuckPerms-5.6.0.jar").is_file());
    }

    #[test]
    fn fremde_dateien_bleiben_unangetastet() {
        let (root, cfg, paths) = fixture("sync-foreign");
        let s = Syncer::new(&paths, &cfg).unwrap();
        let main = cfg.node(&paths, "main").unwrap();
        s.sync(&main, false).unwrap();

        // Von Hand hingelegt und eine Plugin-Config - nichts davon gehoert uns.
        let dir = root.join("servers").join("main");
        fs::write(dir.join("plugins").join("VonHand.jar"), b"x").unwrap();
        fs::create_dir_all(dir.join("plugins").join("Nations")).unwrap();
        fs::write(
            dir.join("plugins").join("Nations").join("config.yml"),
            "x: 1\n",
        )
        .unwrap();

        s.sync(&main, false).unwrap();
        assert!(dir.join("plugins").join("VonHand.jar").is_file());
        assert!(dir
            .join("plugins")
            .join("Nations")
            .join("config.yml")
            .is_file());
    }

    #[test]
    fn husksync_config_wird_nicht_ueberschrieben() {
        let (root, cfg, paths) = fixture("sync-husksync");
        let s = Syncer::new(&paths, &cfg).unwrap();
        let main = cfg.node(&paths, "main").unwrap();
        s.sync(&main, false).unwrap();
        let hs = root
            .join("servers")
            .join("main")
            .join("plugins")
            .join("HuskSync")
            .join("config.yml");
        fs::write(&hs, "von hand angepasst\n").unwrap();
        s.sync(&main, false).unwrap();
        assert_eq!(fs::read_to_string(&hs).unwrap(), "von hand angepasst\n");
    }

    #[test]
    fn dungeon_bekommt_seine_motd_und_sein_plugin() {
        let (root, cfg, paths) = fixture("sync-mine");
        let s = Syncer::new(&paths, &cfg).unwrap();
        let mine = cfg.node(&paths, "mining-3").unwrap();
        s.sync(&mine, false).unwrap();
        let dir = root.join("servers").join("mining-3");
        let p = fs::read_to_string(dir.join("server.properties")).unwrap();
        assert!(p.contains("motd=Terranova Mine 3"), "{p}");
        assert!(p.contains("server-port=25573") && p.contains("rcon.port=25673"));
        assert!(dir
            .join("plugins")
            .join("BountyfulMining-1.0.0.jar")
            .is_file());
        // Nations gehoert main, nicht dem Dungeon
        assert!(!dir.join("plugins").join("Nations-1.0.0.jar").exists());
    }

    #[test]
    fn probelauf_schreibt_nichts() {
        let (root, cfg, paths) = fixture("sync-dry");
        let s = Syncer::new(&paths, &cfg).unwrap();
        let main = cfg.node(&paths, "main").unwrap();
        let r = s.sync(&main, true).unwrap();
        assert!(!r.copied.is_empty() && r.props);
        assert!(!root.join("servers").join("main").exists());
    }

    #[test]
    fn der_proxy_hat_keine_vorlage() {
        let (_root, cfg, paths) = fixture("sync-proxy");
        let s = Syncer::new(&paths, &cfg).unwrap();
        let proxy = cfg.node(&paths, "proxy").unwrap();
        assert!(s.sync(&proxy, false).unwrap().nothing_to_do());
    }

    #[test]
    fn geheimnisse_werden_einmal_angelegt() {
        let (root, cfg, paths) = fixture("sync-secrets");
        let a = Syncer::new(&paths, &cfg).unwrap();
        let b = Syncer::new(&paths, &cfg).unwrap();
        assert_eq!(a.forwarding_secret, b.forwarding_secret);
        assert_eq!(a.rcon_password, b.rcon_password);
        assert!(root.join("proxy").join("forwarding.secret").is_file());
        assert!(root.join("runtime").join("rcon.secret").is_file());
    }
}
