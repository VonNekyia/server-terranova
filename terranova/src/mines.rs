//! Dungeons: nummerierte Kopien einer Vorlage, jede auf ihrem eigenen Port,
//! eine feste Zeit lang offen.
//!
//! Die Logik hier ist bewusst ohne Prozesse und Netz - welcher Platz frei ist
//! und wann ein Dungeon abgelaufen ist, laesst sich so vollstaendig testen.

use std::fs;
use std::io;
use std::path::Path;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

/// Merkt sich, wann ein Dungeon geoeffnet wurde.
pub const MARKER: &str = ".terranova-mine.json";

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Marker {
    pub slot: u8,
    /// Sekunden seit 1970
    pub opened_at: u64,
}

pub fn name(slot: u8) -> String {
    format!("mining-{slot}")
}

pub fn parse_name(name: &str) -> Option<u8> {
    let n: u8 = name.strip_prefix("mining-")?.parse().ok()?;
    (n >= 1).then_some(n)
}

pub fn now_unix() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_secs())
}

pub fn write_marker(dir: &Path, slot: u8, opened_at: u64) -> io::Result<()> {
    let m = Marker { slot, opened_at };
    fs::write(dir.join(MARKER), serde_json::to_vec_pretty(&m).expect("Marker"))
}

/// Wann wurde der Dungeon geoeffnet?
///
/// Aus der Markierungsdatei. Fehlt sie - ein Dungeon noch aus der Zeit von
/// dungeon.ps1 -, aus dem Anlegedatum des Verzeichnisses. Das allein waere
/// unzuverlaessig: NTFS gibt einem Verzeichnis, das binnen 15 Sekunden unter
/// gleichem Namen neu entsteht, das alte Anlegedatum zurueck ("tunneling").
/// Ein gerade abgeraeumter und sofort neu geoeffneter Dungeon waere sonst
/// schon beim Oeffnen abgelaufen.
pub fn opened_at(dir: &Path) -> Option<u64> {
    if let Ok(text) = fs::read_to_string(dir.join(MARKER)) {
        if let Ok(m) = serde_json::from_str::<Marker>(&text) {
            return Some(m.opened_at);
        }
    }
    fs::metadata(dir)
        .and_then(|m| m.created())
        .ok()
        .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
        .map(|d| d.as_secs())
}

/// Nummern aller vorhandenen Dungeon-Verzeichnisse, aufsteigend.
pub fn existing(servers_dir: &Path, slots: u8) -> Vec<u8> {
    let mut v: Vec<u8> = fs::read_dir(servers_dir)
        .into_iter()
        .flatten()
        .flatten()
        .filter(|e| e.path().is_dir())
        .filter_map(|e| parse_name(&e.file_name().to_string_lossy()))
        .filter(|&n| n <= slots)
        .collect();
    v.sort_unstable();
    v
}

/// Welche Plaetze `mine open` benutzt - dieselbe Regel wie in dungeon.ps1:
/// ab `slot` (sonst 1) aufwaerts, laufende werden uebersprungen. Ist ein
/// Platz ausdruecklich angegeben und laeuft er schon, ist dort Schluss.
///
/// Ein vorhandener, gestoppter Dungeon zaehlt als frei und wird
/// wiederverwendet - er behaelt dabei seine Welt.
pub fn pick_slots(
    count: u8,
    slot: Option<u8>,
    slots: u8,
    running: impl Fn(u8) -> bool,
) -> Result<Vec<u8>, String> {
    if count == 0 {
        return Err("mindestens ein Dungeon".into());
    }
    if let Some(s) = slot {
        if s == 0 || s > slots {
            return Err(format!("Platz {s} gibt es nicht (1-{slots})"));
        }
    }
    let mut picked = Vec::new();
    for n in slot.unwrap_or(1)..=slots {
        if picked.len() == usize::from(count) {
            break;
        }
        if running(n) {
            if slot.is_some() {
                if picked.is_empty() {
                    return Err(format!("{} laeuft bereits", name(n)));
                }
                break;
            }
            continue;
        }
        picked.push(n);
    }
    if picked.is_empty() {
        return Err(format!("kein freier Platz (mining-1..{slots} laufen alle)"));
    }
    Ok(picked)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Reap {
    /// Noch nicht abgelaufen
    Keep { remaining: Duration },
    /// Abgelaufen, laeuft aber noch - naechstes Mal
    SkipRunning,
    /// Abgelaufen und laeuft: erst sauber stoppen, dann weg
    StopThenDelete,
    /// Abgelaufen, gestoppt: weg damit
    Delete,
}

pub fn reap_decision(
    opened_at: Option<u64>,
    now: u64,
    lifetime: Duration,
    running: bool,
    stop_running: bool,
) -> Reap {
    // Ohne bekanntes Alter nichts loeschen - lieber einmal zu lang als einmal
    // eine Welt zu frueh.
    let Some(at) = opened_at else {
        return Reap::Keep {
            remaining: lifetime,
        };
    };
    // Ein Zeitstempel in der Zukunft (Uhr verstellt) zaehlt als frisch.
    let age = now.saturating_sub(at);
    if age < lifetime.as_secs() {
        return Reap::Keep {
            remaining: Duration::from_secs(lifetime.as_secs() - age),
        };
    }
    match (running, stop_running) {
        (false, _) => Reap::Delete,
        (true, false) => Reap::SkipRunning,
        (true, true) => Reap::StopThenDelete,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const DAY: Duration = Duration::from_secs(24 * 3600);

    #[test]
    fn namen() {
        assert_eq!(name(3), "mining-3");
        assert_eq!(parse_name("mining-3"), Some(3));
        assert_eq!(parse_name("mining-0"), None);
        assert_eq!(parse_name("mining-x"), None);
        assert_eq!(parse_name("main"), None);
    }

    #[test]
    fn freie_plaetze_von_unten() {
        let running = |n: u8| n == 1 || n == 3;
        assert_eq!(pick_slots(3, None, 8, running), Ok(vec![2, 4, 5]));
    }

    #[test]
    fn ausdruecklicher_platz_stoppt_am_ersten_laufenden() {
        let running = |n: u8| n == 7;
        assert_eq!(pick_slots(3, Some(5), 8, running), Ok(vec![5, 6]));
        assert!(pick_slots(1, Some(7), 8, running).unwrap_err().contains("laeuft bereits"));
        assert!(pick_slots(1, Some(9), 8, running).is_err());
    }

    #[test]
    fn alle_belegt() {
        assert!(pick_slots(1, None, 2, |_| true).is_err());
    }

    #[test]
    fn nicht_mehr_als_verlangt_und_nicht_ueber_die_plaetze() {
        assert_eq!(pick_slots(10, None, 3, |_| false), Ok(vec![1, 2, 3]));
    }

    #[test]
    fn abraeumen() {
        let now = 1_000_000;
        let fresh = now - 3600;
        let old = now - DAY.as_secs();
        assert_eq!(
            reap_decision(Some(fresh), now, DAY, false, false),
            Reap::Keep {
                remaining: DAY - Duration::from_secs(3600)
            }
        );
        // genau 24 h: abgelaufen
        assert_eq!(reap_decision(Some(old), now, DAY, false, false), Reap::Delete);
        assert_eq!(reap_decision(Some(old), now, DAY, true, false), Reap::SkipRunning);
        assert_eq!(reap_decision(Some(old), now, DAY, true, true), Reap::StopThenDelete);
        // Zukunft zaehlt als frisch, Unbekanntes wird behalten
        assert!(matches!(reap_decision(Some(now + 99), now, DAY, false, false), Reap::Keep { .. }));
        assert!(matches!(reap_decision(None, now, DAY, false, false), Reap::Keep { .. }));
    }

    #[test]
    fn markierung_schlaegt_verzeichnisdatum() {
        let dir = crate::testutil::tempdir("mine-marker");
        write_marker(&dir, 4, 12345).unwrap();
        assert_eq!(opened_at(&dir), Some(12345));
        fs::remove_file(dir.join(MARKER)).unwrap();
        // ohne Markierung: Anlegedatum, also ungefaehr jetzt
        let at = opened_at(&dir).unwrap();
        assert!(now_unix().abs_diff(at) < 600);
    }

    #[test]
    fn vorhandene_dungeons() {
        let dir = crate::testutil::tempdir("mine-existing");
        for d in ["mining-2", "mining-10", "main", "mining-x"] {
            fs::create_dir_all(dir.join(d)).unwrap();
        }
        fs::write(dir.join("mining-5"), "keine Datei als Dungeon").unwrap();
        assert_eq!(existing(&dir, 8), [2]);
        assert_eq!(existing(&dir, 10), [2, 10]);
    }
}
