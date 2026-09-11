//! server.properties aus einer Vorlage erzeugen.
//!
//! Dieselbe Regel wie bisher in sync-servers.ps1: vorhandene Schluessel
//! werden an Ort und Stelle ersetzt, fehlende hinten angehaengt. Kommentare
//! und Reihenfolge bleiben stehen - so bleibt ein Diff gegen die Vorlage
//! lesbar, und Paper sortiert die Datei beim naechsten Start ohnehin selbst.

/// Was Terranova in jeder server.properties setzt.
///
/// Die Reihenfolge bestimmt, wie fehlende Schluessel angehaengt wuerden;
/// in den heutigen Vorlagen sind alle schon vorhanden.
pub fn wants(
    port: u16,
    rcon_port: u16,
    rcon_password: &str,
    motd: Option<&str>,
) -> Vec<(String, String)> {
    let mut w: Vec<(String, String)> = vec![
        // Hinter dem Proxy: Velocity prueft gegen Mojang, der Server nicht.
        ("online-mode".into(), "false".into()),
        // Nur ueber den Proxy erreichbar - das ist bei online-mode=false der
        // eigentliche Schutz.
        ("server-ip".into(), "127.0.0.1".into()),
        ("prevent-proxy-connections".into(), "false".into()),
        ("enforce-secure-profile".into(), "false".into()),
        // RCON ist der Ersatzweg fuer Server, deren Konsole Terranova nicht
        // besitzt - etwa nach einem Absturz des Supervisors.
        ("enable-rcon".into(), "true".into()),
        ("rcon.password".into(), rcon_password.into()),
        ("server-port".into(), port.to_string()),
        ("query.port".into(), port.to_string()),
        ("rcon.port".into(), rcon_port.to_string()),
    ];
    if let Some(motd) = motd {
        w.push(("motd".into(), motd.into()));
    }
    w
}

pub fn render(template: &str, want: &[(String, String)]) -> String {
    let eol = if template.contains("\r\n") { "\r\n" } else { "\n" };
    let mut lines: Vec<String> = template.lines().map(String::from).collect();
    let mut seen = vec![false; want.len()];

    for line in &mut lines {
        let Some(key) = key_of(line) else { continue };
        if let Some(i) = want.iter().position(|(k, _)| k == key) {
            *line = format!("{}={}", want[i].0, want[i].1);
            seen[i] = true;
        }
    }
    for (i, (k, v)) in want.iter().enumerate() {
        if !seen[i] {
            lines.push(format!("{k}={v}"));
        }
    }

    let mut out = lines.join(eol);
    out.push_str(eol);
    out
}

/// Der Schluessel einer Zeile - alles vor dem ersten `=`, sofern kein `#`
/// darin vorkommt. Damit bleiben Kommentarzeilen wie `#server-port=25565`
/// unangetastet.
fn key_of(line: &str) -> Option<&str> {
    let eq = line.find('=')?;
    let key = &line[..eq];
    (!key.is_empty() && !key.contains('#')).then_some(key)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testutil::same_text;

    const RCON_PASS: &str = "RCONPASSRCONPASSRCONPASSRCON1234";

    // Byteweise Fixtures: von sync-servers.ps1 erzeugt, bevor es abgeloest
    // wurde. Was hier abweicht, waere eine Verhaltensaenderung am Server.
    const IN_MAIN: &str = include_str!("../tests/fixtures/sync/input/main/server.properties");
    const OUT_MAIN: &str = include_str!("../tests/fixtures/sync/expected/main/server.properties");
    const IN_BUILD: &str = include_str!("../tests/fixtures/sync/input/build/server.properties");
    const OUT_BUILD: &str = include_str!("../tests/fixtures/sync/expected/build/server.properties");
    const IN_FARM: &str = include_str!("../tests/fixtures/sync/input/farm/server.properties");
    const OUT_FARM: &str = include_str!("../tests/fixtures/sync/expected/farm/server.properties");
    // Der Dungeon wurde von dungeon.ps1 aus der gemeinsamen Vorlage erzeugt.
    const IN_COMMON: &str = include_str!("../tests/fixtures/sync/input/common/server.properties");
    const OUT_MINE3: &str =
        include_str!("../tests/fixtures/sync/expected/mining-3/server.properties");

    #[test]
    fn feste_server_wie_das_powershell_skript() {
        for (tpl, expected, port, rcon) in [
            (IN_MAIN, OUT_MAIN, 25566u16, 25666u16),
            (IN_BUILD, OUT_BUILD, 25567, 25667),
            (IN_FARM, OUT_FARM, 25568, 25668),
        ] {
            let got = render(tpl, &wants(port, rcon, RCON_PASS, None));
            assert_eq!(got, expected, "Port {port}");
        }
    }

    #[test]
    fn dungeon_bekommt_zusaetzlich_eine_motd() {
        let got = render(
            IN_COMMON,
            &wants(25573, 25673, RCON_PASS, Some("Terranova Mine 3")),
        );
        assert_eq!(got, OUT_MINE3);
    }

    #[test]
    fn zweimal_anwenden_aendert_nichts_mehr() {
        let w = wants(25566, 25666, RCON_PASS, None);
        let once = render(IN_MAIN, &w);
        assert_eq!(render(&once, &w), once);
    }

    #[test]
    fn fehlende_schluessel_werden_angehaengt() {
        let got = render(
            "# Kommentar\nlevel-name=world\n",
            &[("motd".into(), "Hallo".into())],
        );
        assert_eq!(got, "# Kommentar\nlevel-name=world\nmotd=Hallo\n");
    }

    #[test]
    fn kommentare_und_gleichheitszeichen_im_wert() {
        let tpl = "#motd=alt\nmotd=a=b\nlevel-type=minecraft\\:flat\n";
        let got = render(tpl, &[("motd".into(), "neu".into())]);
        // Die auskommentierte Zeile bleibt, der Wert mit = wird ersetzt,
        // die maskierte Zeile bleibt unberuehrt.
        assert_eq!(got, "#motd=alt\nmotd=neu\nlevel-type=minecraft\\:flat\n");
    }

    #[test]
    fn zeilenenden_bleiben_wie_in_der_vorlage() {
        assert!(render("a=1\r\nb=2\r\n", &[]).ends_with("b=2\r\n"));
        let lf = render("a=1\nb=2\n", &[]);
        assert!(lf.ends_with("b=2\n") && !lf.contains('\r'));
        assert!(same_text("a=1\r\n", "a=1\n"));
    }
}
