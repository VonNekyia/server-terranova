//! Geheimnisse: einmal erzeugen, danach nur noch lesen.
//!
//! Forwarding-Secret, RCON-Passwort, API-Token - alle 32 Zeichen aus
//! a-z A-Z 0-9, ohne Zeilenumbruch am Ende. Velocity liest das
//! Forwarding-Secret roh aus der Datei; ein angehaengtes \n waere Teil davon.

use std::fs;
use std::io::{self, Write};
use std::path::Path;

const ALPHABET: &[u8] = b"abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789";
pub const LEN: usize = 32;

pub fn generate() -> String {
    let mut out = String::with_capacity(LEN);
    let mut buf = [0u8; 64];
    while out.len() < LEN {
        crate::sys::random(&mut buf);
        for &b in &buf {
            // Verwerfen statt Modulo: erst unterhalb von 4 * 62 = 248 ist jedes
            // Zeichen gleich wahrscheinlich.
            if b < 248 {
                out.push(ALPHABET[usize::from(b % 62)] as char);
                if out.len() == LEN {
                    break;
                }
            }
        }
    }
    out
}

/// Liest ein Geheimnis oder legt es an; zurueck kommt es samt der Angabe, ob
/// es neu ist.
///
/// Ein vorhandenes wird nie ueberschrieben - wer das Forwarding-Secret
/// aendert, sperrt jeden Server aus, bis alle neu bestueckt sind. Nur eine
/// leere Datei gilt als nicht vorhanden: von ihr kann nichts abhaengen.
pub fn load_or_create(path: &Path) -> io::Result<(String, bool)> {
    match fs::read_to_string(path) {
        Ok(s) if !s.trim().is_empty() => return Ok((s.trim().to_string(), false)),
        Ok(_) => {}
        Err(e) if e.kind() == io::ErrorKind::NotFound => {}
        Err(e) => return Err(e),
    }
    if let Some(dir) = path.parent() {
        fs::create_dir_all(dir)?;
    }
    let secret = generate();
    let mut f = fs::File::create(path)?;
    f.write_all(secret.as_bytes())?;
    Ok((secret, true))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn form_stimmt() {
        let a = generate();
        let b = generate();
        assert_eq!(a.len(), LEN);
        assert!(a.bytes().all(|c| ALPHABET.contains(&c)));
        assert_ne!(a, b);
    }

    #[test]
    fn wird_nie_ueberschrieben() {
        let dir = crate::testutil::tempdir("secrets");
        let file = dir.join("sub").join("x.secret");
        let (first, created) = load_or_create(&file).unwrap();
        assert!(created);
        assert_eq!(
            fs::read(&file).unwrap(),
            first.as_bytes(),
            "ohne Zeilenumbruch"
        );
        let (second, created) = load_or_create(&file).unwrap();
        assert!(!created);
        assert_eq!(first, second);
    }

    #[test]
    fn bestehendes_wird_getrimmt_gelesen() {
        let dir = crate::testutil::tempdir("secrets-trim");
        let file = dir.join("y.secret");
        fs::write(&file, "abc123\r\n").unwrap();
        assert_eq!(
            load_or_create(&file).unwrap(),
            ("abc123".to_string(), false)
        );
    }

    #[test]
    fn leere_datei_gilt_als_fehlend() {
        let dir = crate::testutil::tempdir("secrets-empty");
        let file = dir.join("z.secret");
        fs::write(&file, "  \n").unwrap();
        let (s, created) = load_or_create(&file).unwrap();
        assert!(created);
        assert_eq!(s.len(), LEN);
    }
}
