//! Die Dungeon-Eintraege in velocity.toml.
//!
//! Velocity braucht fuer jeden Server, auf den jemand wechseln kann, einen
//! Eintrag unter [servers]. Bisher standen dort alle acht Dungeon-Plaetze
//! fest drin, auch wenn keiner lief: eine Liste mit acht toten Adressen, und
//! wer die Zahl der Plaetze in terranova.yml aenderte, musste daran denken,
//! velocity.toml von Hand nachzuziehen.
//!
//! Velocity kann Server zur Laufzeit an- und abmelden. "velocity reload"
//! liest die Datei neu und gleicht [servers] ab: was dazugekommen ist, wird
//! angemeldet, was fehlt, abgemeldet - wer auf einem abgemeldeten Server
//! steht, landet auf der Ausweichliste (try). Genau das brauchen wir.
//!
//! Deshalb gehoert der Dungeon-Teil der Datei ab jetzt Terranova. Er steht
//! zwischen zwei Markierungszeilen; alles ausserhalb bleibt unangetastet -
//! bind, motd, die festen Server und was sonst noch jemand eintraegt.

const BEGIN: &str = "# von terranova verwaltet - offene Dungeons, nicht von Hand aendern";
const END: &str = "# Ende der Dungeons";

/// Der Block, wie er in die Datei soll.
fn block(indent: &str, mines: &[(String, u16)]) -> Vec<String> {
    let mut v = vec![format!("{indent}{BEGIN}")];
    for (name, port) in mines {
        v.push(format!("{indent}{name} = \"127.0.0.1:{port}\""));
    }
    if mines.is_empty() {
        v.push(format!("{indent}# gerade keiner offen"));
    }
    v.push(format!("{indent}{END}"));
    v
}

/// Schreibt die offenen Dungeons in den [servers]-Abschnitt.
///
/// Gibt `None` zurueck, wenn sich nichts aendert - dann muss weder die Datei
/// geschrieben noch der Proxy behelligt werden.
pub fn with_mines(text: &str, mines: &[(String, u16)]) -> Option<String> {
    let crlf = text.contains("\r\n");
    let nl = if crlf { "\r\n" } else { "\n" };
    let lines: Vec<&str> = text.lines().collect();

    // Den Abschnitt [servers] finden. Ohne ihn ist die Datei nicht das, was
    // wir erwarten - lieber nichts anfassen.
    let start = lines.iter().position(|l| l.trim() == "[servers]")?;
    let end = lines[start + 1..]
        .iter()
        .position(|l| l.trim_start().starts_with('['))
        .map_or(lines.len(), |i| start + 1 + i);

    // Der Einzug richtet sich nach dem, was schon dasteht: Velocity schreibt
    // die Datei mit Tabulatoren, von Hand wird es gern anders.
    let indent = lines[start + 1..end]
        .iter()
        .find(|l| !l.trim().is_empty())
        .map_or("\t", |l| &l[..l.len() - l.trim_start().len()]);

    // Alles heraus, was uns gehoert: der eigene Block und - beim ersten Mal -
    // die von Hand eingetragenen Dungeon-Zeilen.
    let mut kept: Vec<String> = Vec::new();
    let mut inside = false;
    let mut was = None;
    for line in &lines[start + 1..end] {
        let t = line.trim();
        if t == BEGIN {
            inside = true;
            was = Some(kept.len());
            continue;
        }
        if t == END {
            inside = false;
            continue;
        }
        if inside {
            continue;
        }
        let key = t.split_once('=').map_or(t, |(k, _)| k.trim());
        if crate::mines::parse_name(key).is_some() {
            continue;
        }
        kept.push((*line).to_string());
    }

    // Wo der Block schon stand, steht er wieder - so bleibt die Datei beim
    // Schreiben in Ruhe, und wer ihn verschiebt, darf das.
    //
    // Beim ersten Mal gibt es ihn noch nicht: dann vor die Ausweichliste, denn
    // try schliesst den Abschnitt ab, und ein Server dahinter liest sich, als
    // gehoere er in die Liste.
    let at = was.unwrap_or_else(|| {
        kept.iter()
            .position(|l| l.trim_start().starts_with("try"))
            .unwrap_or(kept.len())
    });
    let mut body = kept;
    body.splice(at..at, block(indent, mines));

    let mut out: Vec<String> = lines[..=start].iter().map(|l| (*l).to_string()).collect();
    out.extend(body);
    out.extend(lines[end..].iter().map(|l| (*l).to_string()));

    let mut new = out.join(nl);
    if text.ends_with('\n') {
        new.push_str(nl);
    }
    (new != text).then_some(new)
}

#[cfg(test)]
mod tests {
    use super::*;

    const TOML: &str = "bind = \"0.0.0.0:25565\"\n\
        \n\
        [servers]\n\
        \tmain = \"127.0.0.1:25566\"\n\
        \tmining-1 = \"127.0.0.1:25571\"\n\
        \tmining-2 = \"127.0.0.1:25572\"\n\
        \ttry = [\"main\"]\n\
        \n\
        [advanced]\n\
        \tcompression-level = -1\n";

    fn mines(slots: &[u8]) -> Vec<(String, u16)> {
        slots
            .iter()
            .map(|s| (format!("mining-{s}"), 25570 + u16::from(*s)))
            .collect()
    }

    #[test]
    fn handeintraege_weichen_dem_block() {
        let out = with_mines(TOML, &mines(&[3])).unwrap();
        assert!(out.contains("\tmining-3 = \"127.0.0.1:25573\""));
        assert!(!out.contains("mining-1"));
        assert!(!out.contains("mining-2"));
        // Alles andere bleibt, wie es war.
        assert!(out.contains("\tmain = \"127.0.0.1:25566\""));
        assert!(out.contains("bind = \"0.0.0.0:25565\""));
        assert!(out.contains("[advanced]"));
        assert!(out.contains("\tcompression-level = -1"));
    }

    #[test]
    fn der_block_steht_vor_der_ausweichliste() {
        let out = with_mines(TOML, &mines(&[1])).unwrap();
        let servers = out.find("mining-1").unwrap();
        let try_ = out.find("try = ").unwrap();
        assert!(servers < try_, "{out}");
    }

    #[test]
    fn zweimal_dasselbe_aendert_nichts() {
        let once = with_mines(TOML, &mines(&[1, 2])).unwrap();
        assert!(with_mines(&once, &mines(&[1, 2])).is_none());
        // Und ein anderer Stand aendert eben doch etwas.
        assert!(with_mines(&once, &mines(&[1])).is_some());
    }

    #[test]
    fn ohne_offenen_dungeon_bleibt_der_block_leer() {
        let out = with_mines(TOML, &[]).unwrap();
        assert!(!out.contains("mining-"));
        assert!(out.contains("gerade keiner offen"));
        assert!(out.contains("\tmain = \"127.0.0.1:25566\""));
    }

    #[test]
    fn zeilenenden_bleiben_wie_sie_sind() {
        let dos = TOML.replace('\n', "\r\n");
        let out = with_mines(&dos, &mines(&[1])).unwrap();
        assert!(!out.contains("\n\n"), "eine nackte Zeilenschaltung");
        assert!(out.ends_with("\r\n"));
    }

    #[test]
    fn ohne_servers_abschnitt_lieber_nichts() {
        assert!(with_mines("bind = \"0.0.0.0:25565\"\n", &mines(&[1])).is_none());
    }

    #[test]
    fn der_block_bleibt_stehen_wo_er_ist() {
        // Hier steht er ganz oben statt vor try - und soll dort bleiben.
        let moved = format!(
            "[servers]\n\
             \t{BEGIN}\n\
             \tmining-1 = \"127.0.0.1:25571\"\n\
             \t{END}\n\
             \tmain = \"127.0.0.1:25566\"\n\
             \ttry = [\"main\"]\n"
        );
        assert!(with_mines(&moved, &mines(&[1])).is_none(), "{moved}");
        // Und ein zweiter Dungeon kommt dort dazu, nicht unten.
        let out = with_mines(&moved, &mines(&[1, 2])).unwrap();
        assert!(
            out.find("mining-2").unwrap() < out.find("main =").unwrap(),
            "{out}"
        );
    }

    #[test]
    fn die_echte_velocity_toml_kennt_den_block() {
        // Gegen die Datei, die wirklich ausgeliefert wird. Welche Dungeons
        // gerade drinstehen, ist Laufzeitstand - darauf darf kein Test
        // bestehen. Dass der Block da ist und sitzt, schon.
        let text = include_str!("../../proxy/velocity.toml");
        assert!(
            text.contains(BEGIN) && text.contains(END),
            "der Block fehlt"
        );
        let leer = with_mines(text, &[]).unwrap_or_else(|| text.to_string());
        assert!(with_mines(&leer, &[]).is_none(), "{leer}");
        let out = with_mines(&leer, &mines(&[2])).unwrap();
        assert!(out.contains("	mining-2 = \"127.0.0.1:25572\""), "{out}");
        assert!(out.contains("	main = \"127.0.0.1:25566\""));
    }

    #[test]
    fn der_einzug_richtet_sich_nach_der_datei() {
        let spaces = TOML.replace('\t', "  ");
        let out = with_mines(&spaces, &mines(&[1])).unwrap();
        assert!(out.contains("\n  mining-1 = "), "{out}");
    }
}
