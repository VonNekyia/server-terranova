//! Die lokale Schnittstelle.
//!
//! Alles, was die CLI kann, geht hierdurch - und das Dashboard benutzt
//! dieselben Wege. Damit kann das Dashboard nichts, was die CLI nicht auch
//! kann, und beide zeigen denselben Zustand.
//!
//! Gebunden nur an 127.0.0.1, abgesichert mit einem Token aus
//! runtime/terranova/api.token. Zusaetzlich wird der Host-Kopf geprueft:
//! sonst koennte eine Webseite im Browser ueber einen auf 127.0.0.1
//! zeigenden Namen hierher sprechen.

use std::io::{self, Write};
use std::net::TcpListener;
use std::sync::Arc;
use std::thread;
use std::time::Duration;

use serde_json::json;

use crate::config::NodeKind;
use crate::console::Batch;
use crate::http::{self, Reply, Request, Response};
use crate::supervisor::{Node, Status, Supervisor};
use crate::{mines, secrets};

/// Wie lange auf neue Konsolenzeilen gewartet wird, bevor ein Lebenszeichen
/// geht. Ohne das merkt der Server nie, dass ein Zuschauer weg ist.
const SSE_TICK: Duration = Duration::from_secs(15);

pub struct Api {
    sup: Arc<Supervisor>,
    token: String,
}

/// Bindet den Port und bedient ihn in einem eigenen Thread.
pub fn serve(sup: Arc<Supervisor>) -> io::Result<(u16, String)> {
    let (token, _) = secrets::load_or_create(&sup.paths.api_token())?;
    let port = sup.cfg.dashboard.port;
    let listener = TcpListener::bind(("127.0.0.1", port))?;
    let api = Arc::new(Api {
        sup,
        token: token.clone(),
    });
    thread::Builder::new()
        .name("terranova-api".into())
        .spawn(move || http::serve(listener, move |req| api.route(req)))?;
    Ok((port, token))
}

fn kind_str(k: NodeKind) -> &'static str {
    match k {
        NodeKind::MariaDb => "mariadb",
        NodeKind::Redis => "redis",
        NodeKind::Proxy => "proxy",
        NodeKind::Server => "server",
        NodeKind::Mine(_) => "mine",
    }
}

fn bad(status: u16, msg: &str) -> Reply {
    Reply::Done(Response::new(
        status,
        "application/json; charset=utf-8",
        json!({ "error": msg }).to_string().into_bytes(),
    ))
}

fn ok_json(v: serde_json::Value) -> Reply {
    Reply::Done(Response::json(v.to_string().into_bytes()))
}

impl Api {
    fn authorized(&self, req: &Request) -> bool {
        // Bearer-Token der CLI ...
        if let Some(h) = req.header("authorization") {
            if h.strip_prefix("Bearer ").is_some_and(|t| t == self.token) {
                return true;
            }
        }
        // ... oder das Sitzungsplaetzchen des Dashboards.
        if let Some(c) = req.header("cookie") {
            if c.split(';')
                .filter_map(|p| p.trim().strip_prefix("tn_token="))
                .any(|t| t == self.token)
            {
                return true;
            }
        }
        false
    }

    /// Nur 127.0.0.1 und localhost. Ein Name, der auf 127.0.0.1 zeigt, aber
    /// anders heisst, gehoert einer fremden Seite - die soll hier nicht
    /// mitreden.
    fn host_ok(&self, req: &Request) -> bool {
        let Some(host) = req.header("host") else {
            return false;
        };
        let host = host.rsplit_once(':').map_or(host, |(h, _)| h);
        matches!(host, "127.0.0.1" | "localhost" | "[::1]")
    }

    fn route(&self, req: Request) -> Reply {
        if !self.host_ok(&req) {
            return bad(403, "nur ueber 127.0.0.1");
        }
        let seg = req.segments();
        let post = req.method == "POST";

        if !self.authorized(&req) {
            return bad(401, "Token fehlt oder stimmt nicht");
        }

        match (post, seg.as_slice()) {
            (false, ["api", "health"]) => ok_json(json!({
                "version": crate::VERSION,
                "pid": std::process::id(),
                "runtime": self.sup.cfg.runtime().to_string(),
            })),

            (false, ["api", "status"]) => self.status(),

            (false, ["api", "events"]) => self.stream(self.sup.events.clone(), &req),

            (false, ["api", "nodes", name, "console"]) => match self.sup.node(name) {
                Some(n) => self.stream(n.console.clone(), &req),
                None => bad(404, "unbekannt"),
            },

            (false, ["api", "nodes", name, "logs"]) => match self.sup.node(name) {
                Some(n) => {
                    let count: usize = req.query("n").and_then(|v| v.parse().ok()).unwrap_or(200);
                    let b = n.console.tail(count.min(2000));
                    ok_json(json!({ "lines": lines_json(&b), "next": b.next }))
                }
                None => bad(404, "unbekannt"),
            },

            (true, ["api", "nodes", name, action]) => self.node_action(name, action, &req),

            (true, ["api", "network", "start"]) => {
                let sup = self.sup.clone();
                thread::spawn(move || {
                    if let Err(e) = sup.start_network() {
                        sup.log(format!("Start fehlgeschlagen: {e}"));
                    }
                });
                ok_json(json!({ "started": true }))
            }

            (true, ["api", "network", "stop"]) => {
                let sup = self.sup.clone();
                thread::spawn(move || {
                    sup.stop_network();
                    // Der Supervisor hat seine Aufgabe erfuellt.
                    std::process::exit(0);
                });
                ok_json(json!({ "stopping": true }))
            }

            (false, ["api", "mines"]) => self.mines(),
            (true, ["api", "mines", "open"]) => self.open_mines(&req),
            (true, ["api", "mines", "reap"]) => self.reap(&req),
            (true, ["api", "mines", slot, "close"]) => match slot
                .parse::<u8>()
                .ok()
                .and_then(|s| self.sup.node(&mines::name(s)))
            {
                Some(n) => {
                    let sup = self.sup.clone();
                    let ok = sup.stop_node(&n);
                    ok_json(json!({ "stopped": ok }))
                }
                None => bad(404, "kein solcher Dungeon"),
            },

            _ => bad(404, "unbekannter Weg"),
        }
    }

    fn status(&self) -> Reply {
        let nodes: Vec<serde_json::Value> = self
            .sup
            .all()
            .iter()
            .map(|n| {
                json!({
                    "name": n.spec.name,
                    "kind": kind_str(n.spec.kind),
                    "status": n.status(),
                    "control": n.control(),
                    "pid": n.pid(),
                    "port": n.spec.port,
                    "uptime_s": n.uptime().map(|d| d.as_secs()),
                    "restarts": n.restarts(),
                })
            })
            .collect();
        ok_json(json!({
            "runtime": self.sup.cfg.runtime().to_string(),
            "shutting_down": self.sup.shutting_down(),
            "nodes": nodes,
        }))
    }

    fn node_action(&self, name: &str, action: &str, req: &Request) -> Reply {
        let Some(node) = self.sup.node(name) else {
            return bad(404, "unbekannt");
        };
        let sup = self.sup.clone();
        match action {
            "start" => match sup.start_node(&node) {
                Ok(()) => ok_json(json!({ "started": true })),
                Err(e) => bad(500, &e.to_string()),
            },
            "stop" => {
                let ok = sup.stop_node(&node);
                ok_json(json!({ "stopped": ok }))
            }
            "restart" => {
                let node2 = node.clone();
                thread::spawn(move || sup.restart_node(&node2));
                ok_json(json!({ "restarting": true }))
            }
            "command" => {
                let line = serde_json::from_slice::<serde_json::Value>(&req.body)
                    .ok()
                    .and_then(|v| v.get("line").and_then(|l| l.as_str()).map(String::from));
                let Some(line) = line else {
                    return bad(400, "line fehlt");
                };
                match sup.send_command(&node, &line) {
                    Ok(reply) => ok_json(json!({ "reply": reply })),
                    Err(e) => bad(409, &e),
                }
            }
            _ => bad(404, "unbekannte Aktion"),
        }
    }

    fn mines(&self) -> Reply {
        let now = mines::now_unix();
        let lifetime = self.sup.cfg.mines.lifetime.0;
        let list: Vec<serde_json::Value> = self
            .sup
            .mine_nodes()
            .iter()
            .map(|n| {
                let opened = mines::opened_at(&n.spec.dir);
                json!({
                    "name": n.spec.name,
                    "port": n.spec.port,
                    "status": n.status(),
                    "opened_at": opened,
                    "age_s": opened.map(|o| now.saturating_sub(o)),
                    "expires_in_s": opened.map(|o| lifetime.as_secs().saturating_sub(now.saturating_sub(o))),
                })
            })
            .collect();
        ok_json(json!({ "slots": self.sup.cfg.mines.slots, "mines": list }))
    }

    fn open_mines(&self, req: &Request) -> Reply {
        let body: serde_json::Value = serde_json::from_slice(&req.body).unwrap_or(json!({}));
        let count = body.get("count").and_then(|v| v.as_u64()).unwrap_or(1) as u8;
        let slot = body.get("slot").and_then(|v| v.as_u64()).map(|v| v as u8);
        let running = |n: u8| {
            self.sup
                .node(&mines::name(n))
                .is_some_and(|x| x.status() != Status::Stopped)
        };
        let picked = match mines::pick_slots(count, slot, self.sup.cfg.mines.slots, running) {
            Ok(p) => p,
            Err(e) => return bad(409, &e),
        };
        let mut opened = Vec::new();
        for s in picked {
            match self.sup.open_mine(s) {
                Ok(n) => opened.push(json!({ "name": n.spec.name, "port": n.spec.port })),
                Err(e) => return bad(500, &format!("mining-{s}: {e}")),
            }
        }
        ok_json(json!({ "opened": opened }))
    }

    fn reap(&self, req: &Request) -> Reply {
        let body: serde_json::Value = serde_json::from_slice(&req.body).unwrap_or(json!({}));
        let dry = body.get("dry_run").and_then(|v| v.as_bool()).unwrap_or(false);
        let stop_running = body
            .get("stop_running")
            .and_then(|v| v.as_bool())
            .unwrap_or(false);
        ok_json(json!({ "reaped": crate::reaper::reap(&self.sup, dry, stop_running) }))
    }

    /// Ein Ereignisstrom aus einem Konsolenring.
    fn stream(&self, ring: Arc<crate::console::LineRing>, req: &Request) -> Reply {
        // Wer wieder anknuepft, sagt wo er war; wer neu dazukommt, bekommt
        // den letzten Bildschirm voll.
        let since = req
            .header("last-event-id")
            .and_then(|v| v.parse::<u64>().ok())
            .map(|v| v + 1)
            .or_else(|| req.query("since").and_then(|v| v.parse().ok()));

        Reply::Stream(Box::new(move |w| {
            let mut next = match since {
                Some(s) => s,
                None => {
                    let b = ring.tail(200);
                    write_batch(w, &b)?;
                    b.next
                }
            };
            loop {
                let b = ring.wait_since(next, SSE_TICK);
                if b.lines.is_empty() && b.gap.is_none() {
                    http::write_keepalive(w)?;
                    continue;
                }
                write_batch(w, &b)?;
                next = b.next;
            }
        }))
    }
}

fn write_batch(w: &mut dyn Write, b: &Batch) -> io::Result<()> {
    if let Some((from, to)) = b.gap {
        http::write_event(
            w,
            None,
            "gap",
            &format!("{} Zeilen verpasst ({from}-{to})", to - from),
        )?;
    }
    for l in &b.lines {
        http::write_event(w, Some(l.seq), l.kind.as_str(), &l.text)?;
    }
    Ok(())
}

fn lines_json(b: &Batch) -> Vec<serde_json::Value> {
    b.lines
        .iter()
        .map(|l| json!({ "seq": l.seq, "kind": l.kind, "text": l.text }))
        .collect()
}

/// Was `Node` fuer die Anzeige hergibt - hier gebuendelt, damit die CLI
/// dieselben Felder benutzt wie das Dashboard.
pub fn node_line(n: &Node) -> String {
    let status = format!("{:?}", n.status()).to_lowercase();
    let pid = n.pid().map_or_else(|| "-".to_string(), |p| p.to_string());
    let up = n.uptime().map_or_else(
        || "-".to_string(),
        |d| {
            let s = d.as_secs();
            if s < 90 {
                format!("{s}s")
            } else if s < 5400 {
                format!("{}m", s / 60)
            } else {
                format!("{}h{}m", s / 3600, (s % 3600) / 60)
            }
        },
    );
    let ctrl = if n.control() == crate::supervisor::Control::Adopted {
        " (uebernommen)"
    } else {
        ""
    };
    format!(
        "{:<10} {:<10} Port {:<6} PID {:<8} {:>6}{}",
        n.spec.name, status, n.spec.port, pid, up, ctrl
    )
}
