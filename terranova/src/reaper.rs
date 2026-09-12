//! Raeumt abgelaufene Dungeons ab.
//!
//! Ein Dungeon ist eine Zeit lang offen - danach ist er weg, und der naechste
//! auf diesem Platz bekommt eine frische Welt.
//!
//! Geloescht wird in zwei Schritten: erst wird das Verzeichnis nach
//! servers/.trash verschoben, dann geloescht. Das Verschieben scheitert,
//! solange noch ein Prozess Dateien offen haelt - so kann kein halb
//! geloeschter Dungeon entstehen.

use std::fs;
use std::sync::Arc;

use crate::mines::{self, Reap};
use crate::supervisor::{Status, Supervisor};

pub fn reap(sup: &Arc<Supervisor>, dry_run: bool, stop_running: bool) -> Vec<String> {
    let mut done = Vec::new();
    let now = mines::now_unix();
    let lifetime = sup.cfg.mines.lifetime.0;

    for slot in mines::existing(&sup.paths.dynamic(), sup.cfg.mines.slots) {
        let name = mines::name(slot);
        let dir = sup.paths.mine(&name);
        let node = sup.node(&name);
        let running = node.as_ref().is_some_and(|n| n.status() != Status::Stopped);

        match mines::reap_decision(
            mines::expires_from(&dir),
            now,
            lifetime,
            running,
            stop_running,
        ) {
            Reap::Keep { remaining } => {
                let m = remaining.as_secs() / 60;
                done.push(format!("{name}: noch {}h{:02}m", m / 60, m % 60));
            }
            Reap::SkipRunning => {
                done.push(format!(
                    "{name}: abgelaufen, laeuft aber noch - uebersprungen"
                ));
            }
            Reap::StopThenDelete | Reap::Delete => {
                if dry_run {
                    done.push(format!("{name}: wuerde geloescht"));
                    continue;
                }
                if running {
                    if let Some(n) = &node {
                        sup.stop_node(n);
                    }
                }
                match delete(sup, &name) {
                    Ok(()) => {
                        sup.log(format!("{name}: abgelaufen, geloescht"));
                        done.push(format!("{name}: geloescht"));
                    }
                    Err(e) => {
                        sup.log(format!("{name}: loeschen fehlgeschlagen: {e}"));
                        done.push(format!("{name}: {e}"));
                    }
                }
            }
        }
    }
    done
}

/// Raeumt einen Dungeon sofort ab, ohne auf seine Zeit zu warten.
///
/// Stoppen, verschieben, loeschen - dieselbe Reihenfolge wie beim Ablauf, nur
/// ohne die Frage nach dem Alter. Wer ihn wegwirft, hat sich das ueberlegt;
/// die Welt ist danach weg und der Platz frei.
pub fn reap_one(sup: &Arc<Supervisor>, slot: u8) -> Result<(), String> {
    let name = mines::name(slot);
    if !sup.paths.mine(&name).is_dir() {
        return Err(format!("{name} gibt es nicht"));
    }
    if let Some(n) = sup.node(&name) {
        if n.status() != Status::Stopped {
            sup.stop_node(&n);
        }
    }
    match delete(sup, &name) {
        Ok(()) => {
            sup.log(format!("{name}: abgeraeumt"));
            Ok(())
        }
        Err(e) => {
            sup.log(format!("{name}: abraeumen fehlgeschlagen: {e}"));
            Err(e.to_string())
        }
    }
}

fn delete(sup: &Arc<Supervisor>, name: &str) -> std::io::Result<()> {
    let dir = sup.paths.mine(name);
    let trash = sup.paths.trash();
    fs::create_dir_all(&trash)?;
    // Erst umbenennen: haelt noch jemand eine Datei offen, scheitert das
    // hier - und nicht mittendrin beim Loeschen.
    let grave = trash.join(format!("{name}-{}", mines::now_unix()));
    fs::rename(&dir, &grave)?;
    sup.remove_node(name);
    // Der Platz ist weg - dann soll auch der Proxy niemanden mehr dorthin
    // schicken.
    sup.sync_proxy_servers();
    fs::remove_dir_all(&grave)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Config;
    use crate::paths::Paths;
    use std::time::Duration;

    /// Der Entscheidungsteil ist in mines getestet; hier geht es um das
    /// Loeschen selbst.
    #[test]
    fn verschieben_und_loeschen() {
        let root = crate::testutil::tempdir("reap");
        let paths = Paths::new(&root);
        let dir = paths.mine("mining-2");
        fs::create_dir_all(dir.join("world")).unwrap();
        fs::write(dir.join("world").join("level.dat"), b"welt").unwrap();

        // Ohne Supervisor: nur die Dateibewegung nachstellen
        let trash = paths.trash();
        fs::create_dir_all(&trash).unwrap();
        let grave = trash.join("mining-2-1");
        fs::rename(&dir, &grave).unwrap();
        assert!(!dir.exists() && grave.join("world").join("level.dat").is_file());
        fs::remove_dir_all(&grave).unwrap();
        assert!(!grave.exists());
    }

    #[test]
    fn ein_offener_dungeon_bleibt() {
        let cfg = Config::parse(crate::testutil::EXAMPLE_CONFIG).unwrap();
        // frisch geoeffnet -> behalten
        assert!(matches!(
            mines::reap_decision(
                Some(mines::now_unix()),
                mines::now_unix(),
                cfg.mines.lifetime.0,
                true,
                false
            ),
            Reap::Keep { .. }
        ));
        // abgelaufen und laeuft -> nur mit stop_running
        let old = mines::now_unix() - 25 * 3600;
        assert_eq!(
            mines::reap_decision(
                Some(old),
                mines::now_unix(),
                Duration::from_secs(24 * 3600),
                true,
                false
            ),
            Reap::SkipRunning
        );
    }
}
