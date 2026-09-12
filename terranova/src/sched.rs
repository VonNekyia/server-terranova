//! Zeitplan: taeglicher Neustart und das Abraeumen abgelaufener Dungeons.
//!
//! Beides lief bisher als geplante Aufgabe von Windows. Jetzt macht es der
//! Supervisor selbst - er weiss ohnehin, was laeuft, und braucht dafuer
//! keine Rechte und kein zweites Skript.

use std::sync::Arc;
use std::thread;
use std::time::{Duration, Instant};

use crate::supervisor::{Status, Supervisor};
use crate::{reaper, win};

const TICK: Duration = Duration::from_secs(30);

pub fn run(sup: Arc<Supervisor>) {
    let mut last_reap = Instant::now();
    // Tag, an dem der Neustart schon lief - damit er in seiner Minute nicht
    // mehrfach ausgeloest wird.
    let mut restarted_on: Option<(u16, u16, u16)> = None;

    loop {
        thread::sleep(TICK);
        if sup.shutting_down() {
            return;
        }

        let every = sup.cfg.schedule.reap_every.0;
        if !every.is_zero() && last_reap.elapsed() >= every {
            last_reap = Instant::now();
            for line in reaper::reap(&sup, false, false) {
                // Nur das Abraeumen selbst ist eine Meldung wert, nicht jeder
                // Dungeon, der noch Zeit hat.
                if line.contains("geloescht") {
                    sup.log(format!("[abraeumen] {line}"));
                }
            }
        }

        let Some(plan) = &sup.cfg.schedule.daily_restart else {
            continue;
        };
        let Ok((h, m)) = plan.time() else { continue };
        let now = win::local_time();
        let today = (now.year, now.month, now.day);
        if now.hour != u16::from(h) || now.minute != u16::from(m) || restarted_on == Some(today) {
            continue;
        }
        restarted_on = Some(today);

        let which: Vec<String> = if plan.servers.is_empty() {
            sup.cfg.servers.keys().cloned().collect()
        } else {
            plan.servers.clone()
        };
        sup.log(format!("Taeglicher Neustart: {}", which.join(", ")));

        for (i, name) in which.iter().enumerate() {
            if sup.shutting_down() {
                return;
            }
            if i > 0 {
                // Nacheinander, damit nie alle gleichzeitig unten sind.
                thread::sleep(plan.gap.0);
            }
            let Some(node) = sup.node(name) else { continue };
            // Was nicht laeuft, wird auch nicht neu gestartet. Wer mit
            // "terranova start main" bewusst wenig hochgefahren hat, soll um
            // vier Uhr nicht ploetzlich alles im Speicher haben.
            if node.status() == Status::Stopped {
                continue;
            }
            if !plan.message.is_empty() && !plan.warn.0.is_zero() {
                let _ = sup.send_command(&node, &format!("say {}", plan.message));
                thread::sleep(plan.warn.0);
            }
            sup.restart_node(&node);
        }
        sup.log("Taeglicher Neustart erledigt");
    }
}

#[cfg(test)]
mod tests {
    use crate::config::Config;
    use crate::testutil::EXAMPLE_CONFIG;

    #[test]
    fn der_plan_aus_der_config_ist_lesbar() {
        let cfg = Config::parse(EXAMPLE_CONFIG).unwrap();
        let plan = cfg
            .schedule
            .daily_restart
            .as_ref()
            .expect("taeglicher Neustart");
        assert_eq!(plan.time(), Ok((4, 0)));
        assert_eq!(plan.servers, ["main", "build", "farm"]);
        assert_eq!(plan.gap.0.as_secs(), 120);
        assert_eq!(plan.warn.0.as_secs(), 30);
        assert!(!plan.message.is_empty());
    }
}
