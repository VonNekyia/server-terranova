//! Das Velocity-Forwarding in config/paper-global.yml eintragen.
//!
//! Zeilenweise und nicht ueber einen YAML-Parser, genau wie bisher in
//! den alten Skripten: die Datei gehoert Paper, hat Kommentare und eine
//! gewachsene Reihenfolge. Ein Parser wuerde sie beim Schreiben neu
//! formatieren und jedes Paper-Update mit einem unlesbaren Diff quittieren.
//!
//! Angefasst wird ausschliesslich dieser Block:
//!
//! ```yaml
//! proxies:
//!   velocity:
//!     enabled: true
//!     online-mode: true
//!     secret: '...'
//! ```

/// Traegt Secret ein und schaltet die Weiterleitung an. Unveraendert, wenn es
//  den Block nicht gibt.
pub fn inject(text: &str, secret: &str) -> String {
    let eol = if text.contains("\r\n") { "\r\n" } else { "\n" };
    let mut lines: Vec<String> = text.lines().map(String::from).collect();

    if let Some(start) = find_velocity(&lines) {
        for line in &mut lines[start + 1..] {
            if line.trim().is_empty() {
                continue;
            }
            let indent = indent_of(line);
            if indent < 4 {
                break; // Block zu Ende
            }
            if indent > 4 {
                continue; // tiefer verschachtelt, geht uns nichts an
            }
            let key = line[4..].trim_start();
            if key.starts_with("enabled:") {
                *line = "    enabled: true".into();
            } else if key.starts_with("online-mode:") {
                *line = "    online-mode: true".into();
            } else if key.starts_with("secret:") {
                *line = format!("    secret: '{secret}'");
            }
        }
    }

    let mut out = lines.join(eol);
    out.push_str(eol);
    out
}

/// Steht im Block ein nicht leeres Secret? Fuer doctor.
pub fn has_secret(text: &str) -> bool {
    let lines: Vec<String> = text.lines().map(String::from).collect();
    let Some(start) = find_velocity(&lines) else {
        return false;
    };
    lines[start + 1..]
        .iter()
        .take_while(|l| l.trim().is_empty() || indent_of(l) >= 4)
        .filter(|l| indent_of(l) == 4)
        .any(|l| secret_value(&l[4..]).is_some_and(|v| !v.is_empty()))
}

/// Aus `secret: 'abc'` wird `abc`.
fn secret_value(after_indent: &str) -> Option<&str> {
    let v = after_indent.trim_start().strip_prefix("secret:")?.trim();
    Some(v.trim_matches(['\'', '"']))
}

fn indent_of(line: &str) -> usize {
    line.len() - line.trim_start_matches(' ').len()
}

/// Die Zeile `  velocity:` unterhalb des Schluessels `proxies:` auf oberster
/// Ebene. Ein `velocity:` woanders in der Datei bleibt unangetastet.
fn find_velocity(lines: &[String]) -> Option<usize> {
    let proxies = lines
        .iter()
        .position(|l| indent_of(l) == 0 && l.trim_end() == "proxies:")?;
    lines[proxies + 1..]
        .iter()
        .enumerate()
        .take_while(|(_, l)| l.trim().is_empty() || indent_of(l) >= 2)
        // trim() und nicht trim_end(): letzteres laesst die Einrueckung
        // stehen, und "  velocity:" ist nie gleich "velocity:".
        .find(|(_, l)| indent_of(l) == 2 && l.trim() == "velocity:")
        .map(|(i, _)| proxies + 1 + i)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testutil::same_text;

    const SECRET: &str = "FWDSECRETFWDSECRETFWDSECRET12345";
    const INPUT: &str = include_str!("../tests/fixtures/sync/input/common/config/paper-global.yml");
    const EXPECTED: &str =
        include_str!("../tests/fixtures/sync/expected/main/config/paper-global.yml");

    #[test]
    fn wie_das_powershell_skript() {
        // Die Fixture kam aus WriteAllLines und hat deshalb \r\n; wir behalten
        // die Zeilenenden der Vorlage. Inhaltlich muss es dasselbe sein.
        assert!(
            same_text(&inject(INPUT, SECRET), EXPECTED),
            "{}",
            inject(INPUT, SECRET)
        );
    }

    #[test]
    fn nur_der_velocity_block_aendert_sich() {
        let got = inject(INPUT, SECRET);
        let before: Vec<&str> = INPUT.lines().collect();
        let after: Vec<&str> = got.lines().collect();
        assert_eq!(before.len(), after.len(), "keine Zeile kommt dazu");
        let changed: Vec<_> = before
            .iter()
            .zip(&after)
            .filter(|(a, b)| a != b)
            .map(|(_, b)| b.trim())
            .collect();
        assert_eq!(changed, ["enabled: true", &format!("secret: '{SECRET}'")]);
    }

    #[test]
    fn zweimal_anwenden_aendert_nichts_mehr() {
        let once = inject(INPUT, SECRET);
        assert_eq!(inject(&once, SECRET), once);
        assert!(has_secret(&once));
        assert!(!has_secret(INPUT), "Vorlage hat secret: ''");
    }

    #[test]
    fn ein_velocity_ausserhalb_von_proxies_bleibt_stehen() {
        let text = "\
settings:
  velocity:
    enabled: false
    secret: ''
proxies:
  velocity:
    enabled: false
    online-mode: true
    secret: ''
scoreboards:
  x: 1
";
        let got = inject(text, "S");
        assert!(got.contains("settings:\n  velocity:\n    enabled: false\n    secret: ''\n"));
        assert!(got.contains("    secret: 'S'"));
        assert!(got.ends_with("scoreboards:\n  x: 1\n"));
    }

    #[test]
    fn ohne_block_bleibt_alles_wie_es_war() {
        let text = "a: 1\nb: 2\n";
        assert_eq!(inject(text, "S"), text);
    }
}
