//! Wo alles liegt.
//!
//! Jeder Pfad wird aus der Netzwerkwurzel abgeleitet und mit `join` gebaut -
//! nie als Zeichenkette mit Schraegstrichen. So steckt nirgends eine Annahme
//! darueber, auf welchem System das hier laeuft.

use std::env;
use std::path::{Path, PathBuf};

/// An dieser Datei wird die Wurzel des Netzwerks erkannt.
pub const CONFIG_FILE: &str = "terranova.yml";

#[derive(Clone, Debug)]
pub struct Paths {
    pub root: PathBuf,
}

impl Paths {
    pub fn new(root: impl Into<PathBuf>) -> Paths {
        Paths { root: root.into() }
    }

    /// Findet die Wurzel des Netzwerks.
    ///
    /// Reihenfolge: ausdruecklich uebergeben (`--root`), dann das Verzeichnis
    /// ueber `bin\` - dort liegt terranova.exe im Repository -, dann vom
    /// Arbeitsverzeichnis aufwaerts bis zu einer terranova.yml.
    pub fn discover(explicit: Option<&Path>) -> Result<Paths, String> {
        if let Some(root) = explicit {
            let root = std::path::absolute(root).unwrap_or_else(|_| root.to_path_buf());
            return if root.join(CONFIG_FILE).is_file() {
                Ok(Paths::new(root))
            } else {
                Err(format!("{} enthaelt keine {CONFIG_FILE}", root.display()))
            };
        }
        if let Ok(exe) = env::current_exe() {
            if let Some(root) = exe.parent().and_then(Path::parent) {
                if root.join(CONFIG_FILE).is_file() {
                    return Ok(Paths::new(root));
                }
            }
        }
        let mut dir = env::current_dir().map_err(|e| e.to_string())?;
        loop {
            if dir.join(CONFIG_FILE).is_file() {
                return Ok(Paths::new(dir));
            }
            if !dir.pop() {
                return Err(format!(
                    "keine {CONFIG_FILE} gefunden - im Netzwerkverzeichnis starten oder --root angeben"
                ));
            }
        }
    }

    /// Relativ angegebene Pfade aus der Config gelten ab der Wurzel.
    pub fn resolve(&self, p: &Path) -> PathBuf {
        if p.is_absolute() {
            p.to_path_buf()
        } else {
            self.root.join(p)
        }
    }

    pub fn config(&self) -> PathBuf {
        self.root.join(CONFIG_FILE)
    }

    pub fn templates(&self) -> PathBuf {
        self.root.join("templates")
    }

    pub fn template(&self, name: &str) -> PathBuf {
        self.templates().join(name)
    }

    pub fn common(&self) -> PathBuf {
        self.template("common")
    }

    pub fn servers(&self) -> PathBuf {
        self.root.join("servers")
    }

    pub fn server(&self, name: &str) -> PathBuf {
        self.servers().join(name)
    }

    /// Die dynamischen Server - Dungeons.
    ///
    /// Sie liegen getrennt von den festen, weil sie das Gegenteil von ihnen
    /// sind: sie entstehen aus einer Vorlage, leben einen Tag und
    /// verschwinden wieder. Nichts davon gehoert ins Repository, und wer
    /// unter servers/ nachsieht, soll dort nur finden, woran gearbeitet wird.
    pub fn dynamic(&self) -> PathBuf {
        self.root.join("servers_dynamic")
    }

    pub fn mine(&self, name: &str) -> PathBuf {
        self.dynamic().join(name)
    }

    /// Abgelaufene Dungeons werden erst hierher verschoben und dann geloescht.
    /// Das Verschieben scheitert, solange ein Prozess Dateien offen haelt - so
    /// kann kein halb geloeschter Dungeon entstehen.
    pub fn trash(&self) -> PathBuf {
        self.dynamic().join(".trash")
    }

    pub fn runtime(&self) -> PathBuf {
        self.root.join("runtime")
    }

    pub fn mariadb_home(&self) -> PathBuf {
        self.runtime().join("mariadb")
    }

    pub fn redis_home(&self) -> PathBuf {
        self.runtime().join("redis")
    }

    pub fn rcon_secret(&self) -> PathBuf {
        self.runtime().join("rcon.secret")
    }

    /// Alles, was der Supervisor zur Laufzeit ablegt.
    pub fn state_dir(&self) -> PathBuf {
        self.runtime().join("terranova")
    }

    pub fn api_token(&self) -> PathBuf {
        self.state_dir().join("api.token")
    }

    pub fn supervisor_info(&self) -> PathBuf {
        self.state_dir().join("supervisor.json")
    }

    pub fn supervisor_lock(&self) -> PathBuf {
        self.state_dir().join("supervisor.lock")
    }

    pub fn supervisor_log(&self) -> PathBuf {
        self.state_dir().join("supervisor.log")
    }

    pub fn node_state(&self) -> PathBuf {
        self.state_dir().join("state.json")
    }
}
