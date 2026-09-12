//! Kleine Helfer fuer Tests.

use std::path::PathBuf;
use std::sync::atomic::{AtomicU32, Ordering};

/// Ein frisches, leeres Verzeichnis unterhalb des Temp-Ordners.
///
/// Es bleibt danach liegen: bei einem fehlgeschlagenen Test will man
/// hineinsehen koennen. Der naechste Lauf raeumt es weg.
pub fn tempdir(tag: &str) -> PathBuf {
    static N: AtomicU32 = AtomicU32::new(0);
    let n = N.fetch_add(1, Ordering::Relaxed);
    let dir = std::env::temp_dir()
        .join("terranova-tests")
        .join(format!("{tag}-{}-{n}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("Testverzeichnis anlegen");
    dir
}

/// Die echte terranova.yml aus dem Repository - Tests laufen gegen das, was
/// auch wirklich ausgeliefert wird.
pub const EXAMPLE_CONFIG: &str = include_str!("../../terranova.yml");

/// Vergleicht ohne Ruecksicht auf \r\n gegen \n.
pub fn same_text(a: &str, b: &str) -> bool {
    a.replace("\r\n", "\n") == b.replace("\r\n", "\n")
}
