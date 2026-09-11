//! Ein sehr kleiner HTTP/1.1-Server und -Client - nur fuer 127.0.0.1.
//!
//! Warum selbst geschrieben: es gibt genau zwei Gegenstellen, die CLI und das
//! Dashboard, und beide sprechen auf der Loopback-Schnittstelle. Dafuer ein
//! Async-Laufzeitsystem samt Framework einzubinden waere mehr Abhaengigkeit
//! als Nutzen. Fertige kleine Server puffern ausserdem gern ein paar
//! Kilobyte, bevor sie etwas senden - fuer eine Live-Konsole ist das genau
//! das falsche Verhalten.
//!
//! Bewusst schlicht: ein Thread je Verbindung, immer `Connection: close`,
//! keine Umleitungen, kein chunked encoding.

use std::collections::BTreeMap;
use std::io::{self, BufRead, BufReader, Read, Write};
use std::net::{Shutdown, SocketAddr, TcpListener, TcpStream};
use std::sync::Arc;
use std::thread;
use std::time::Duration;

const MAX_HEAD: usize = 16 * 1024;
const MAX_BODY: usize = 1024 * 1024;

#[derive(Debug, Default)]
pub struct Request {
    pub method: String,
    pub path: String,
    pub query: BTreeMap<String, String>,
    /// Feldnamen klein geschrieben
    pub headers: BTreeMap<String, String>,
    pub body: Vec<u8>,
}

impl Request {
    pub fn header(&self, name: &str) -> Option<&str> {
        self.headers.get(name).map(String::as_str)
    }

    pub fn query(&self, name: &str) -> Option<&str> {
        self.query.get(name).map(String::as_str)
    }

    /// Der Pfad in Teilen: "/api/nodes/main/console" -> [api, nodes, main, console]
    pub fn segments(&self) -> Vec<&str> {
        self.path.split('/').filter(|s| !s.is_empty()).collect()
    }
}

pub struct Response {
    pub status: u16,
    pub content_type: String,
    pub headers: Vec<(String, String)>,
    pub body: Vec<u8>,
}

impl Response {
    pub fn new(status: u16, content_type: &str, body: impl Into<Vec<u8>>) -> Response {
        Response {
            status,
            content_type: content_type.into(),
            headers: Vec::new(),
            body: body.into(),
        }
    }

    pub fn json(body: impl Into<Vec<u8>>) -> Response {
        Response::new(200, "application/json; charset=utf-8", body)
    }

    pub fn text(status: u16, body: impl Into<String>) -> Response {
        Response::new(status, "text/plain; charset=utf-8", body.into().into_bytes())
    }

    pub fn html(body: impl Into<Vec<u8>>) -> Response {
        Response::new(200, "text/html; charset=utf-8", body)
    }

    pub fn with_header(mut self, name: &str, value: &str) -> Response {
        self.headers.push((name.to_string(), value.to_string()));
        self
    }
}

/// Was ein Handler zurueckgibt: eine fertige Antwort oder ein offener Strom
/// fuer Server-Sent Events.
pub enum Reply {
    Done(Response),
    Stream(Box<dyn FnOnce(&mut dyn Write) -> io::Result<()> + Send>),
}

fn reason(status: u16) -> &'static str {
    match status {
        200 => "OK",
        201 => "Created",
        204 => "No Content",
        400 => "Bad Request",
        401 => "Unauthorized",
        403 => "Forbidden",
        404 => "Not Found",
        405 => "Method Not Allowed",
        409 => "Conflict",
        500 => "Internal Server Error",
        503 => "Service Unavailable",
        _ => "Status",
    }
}

/// Nimmt Verbindungen an, bis der Lauscher geschlossen wird. Laeuft im
/// eigenen Thread.
pub fn serve<F>(listener: TcpListener, handler: F)
where
    F: Fn(Request) -> Reply + Send + Sync + 'static,
{
    let handler = Arc::new(handler);
    for conn in listener.incoming() {
        let Ok(stream) = conn else { continue };
        let handler = handler.clone();
        // Ein Thread je Verbindung: es sind eine Handvoll, und ein
        // haengender Zuschauer darf die anderen nicht aufhalten.
        thread::Builder::new()
            .name("terranova-http".into())
            .spawn(move || {
                let _ = handle(stream, handler.as_ref());
            })
            .ok();
    }
}

fn handle<F>(mut stream: TcpStream, handler: &F) -> io::Result<()>
where
    F: Fn(Request) -> Reply,
{
    stream.set_read_timeout(Some(Duration::from_secs(30)))?;
    stream.set_nodelay(true)?;

    let req = match read_request(&mut stream) {
        Ok(Some(r)) => r,
        Ok(None) => return Ok(()),
        Err(e) => {
            let _ = write_response(&mut stream, &Response::text(400, format!("{e}\n")));
            return Ok(());
        }
    };

    match handler(req) {
        Reply::Done(r) => write_response(&mut stream, &r)?,
        Reply::Stream(f) => {
            // Kein Content-Length und kein chunked: der Strom endet, wenn die
            // Verbindung endet. Fuer SSE auf Loopback ist das genau richtig.
            let head = "HTTP/1.1 200 OK\r\n\
                        Content-Type: text/event-stream; charset=utf-8\r\n\
                        Cache-Control: no-store\r\n\
                        X-Accel-Buffering: no\r\n\
                        Connection: close\r\n\r\n";
            stream.write_all(head.as_bytes())?;
            stream.flush()?;
            // Schreibfehler heisst: der Zuschauer ist weg. Das ist normal.
            let _ = f(&mut stream);
        }
    }
    let _ = stream.shutdown(Shutdown::Both);
    Ok(())
}

fn read_request(stream: &mut TcpStream) -> io::Result<Option<Request>> {
    let mut reader = BufReader::new(stream.try_clone()?);
    let mut head = Vec::new();
    loop {
        let mut line = Vec::new();
        let n = read_line(&mut reader, &mut line, MAX_HEAD - head.len().min(MAX_HEAD))?;
        if n == 0 {
            return Ok(None); // Verbindung ohne Anfrage
        }
        let done = line == b"\r\n" || line == b"\n";
        head.extend_from_slice(&line);
        if done {
            break;
        }
        if head.len() >= MAX_HEAD {
            return Err(io::Error::new(io::ErrorKind::InvalidData, "Kopf zu gross"));
        }
    }

    let text = String::from_utf8_lossy(&head).into_owned();
    let mut lines = text.lines();
    let start = lines
        .next()
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, "leere Anfrage"))?;
    let mut parts = start.split_whitespace();
    let method = parts.next().unwrap_or_default().to_string();
    let target = parts.next().unwrap_or_default().to_string();

    let mut req = Request {
        method,
        ..Request::default()
    };
    let (path, query) = target.split_once('?').unwrap_or((target.as_str(), ""));
    req.path = percent_decode(path);
    for pair in query.split('&').filter(|s| !s.is_empty()) {
        let (k, v) = pair.split_once('=').unwrap_or((pair, ""));
        req.query.insert(percent_decode(k), percent_decode(v));
    }
    for line in lines {
        if let Some((k, v)) = line.split_once(':') {
            req.headers
                .insert(k.trim().to_ascii_lowercase(), v.trim().to_string());
        }
    }

    let len: usize = req
        .header("content-length")
        .and_then(|v| v.parse().ok())
        .unwrap_or(0);
    if len > MAX_BODY {
        return Err(io::Error::new(io::ErrorKind::InvalidData, "Rumpf zu gross"));
    }
    if len > 0 {
        req.body = vec![0; len];
        reader.read_exact(&mut req.body)?;
    }
    Ok(Some(req))
}

/// Liest eine Zeile einschliesslich \n, hoechstens `max` Bytes.
fn read_line(r: &mut impl BufRead, out: &mut Vec<u8>, max: usize) -> io::Result<usize> {
    let mut n = 0;
    while n < max {
        let mut b = [0u8; 1];
        match r.read(&mut b) {
            Ok(0) => break,
            Ok(_) => {
                out.push(b[0]);
                n += 1;
                if b[0] == b'\n' {
                    break;
                }
            }
            Err(e) if e.kind() == io::ErrorKind::Interrupted => continue,
            Err(e) => return Err(e),
        }
    }
    Ok(n)
}

fn write_response(stream: &mut TcpStream, r: &Response) -> io::Result<()> {
    let mut head = format!(
        "HTTP/1.1 {} {}\r\nContent-Type: {}\r\nContent-Length: {}\r\nConnection: close\r\n",
        r.status,
        reason(r.status),
        r.content_type,
        r.body.len()
    );
    for (k, v) in &r.headers {
        head.push_str(&format!("{k}: {v}\r\n"));
    }
    head.push_str("\r\n");
    stream.write_all(head.as_bytes())?;
    stream.write_all(&r.body)?;
    stream.flush()
}

pub fn percent_decode(s: &str) -> String {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'%' && i + 2 < b.len() {
            if let Ok(v) = u8::from_str_radix(&s[i + 1..i + 3], 16) {
                out.push(v);
                i += 3;
                continue;
            }
        }
        out.push(if b[i] == b'+' { b' ' } else { b[i] });
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

// --- Server-Sent Events -------------------------------------------------------

/// Schreibt ein Ereignis. Mehrzeilige Daten werden auf mehrere data-Zeilen
/// verteilt, wie es das Format verlangt.
pub fn write_event(w: &mut dyn Write, id: Option<u64>, name: &str, data: &str) -> io::Result<()> {
    if let Some(id) = id {
        write!(w, "id: {id}\n")?;
    }
    if !name.is_empty() {
        write!(w, "event: {name}\n")?;
    }
    for line in data.split('\n') {
        write!(w, "data: {line}\n")?;
    }
    w.write_all(b"\n")?;
    w.flush()
}

/// Haelt die Verbindung am Leben und merkt, wenn der Zuschauer weg ist.
pub fn write_keepalive(w: &mut dyn Write) -> io::Result<()> {
    w.write_all(b": keepalive\n\n")?;
    w.flush()
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Event {
    pub id: Option<u64>,
    pub name: String,
    pub data: String,
}

// --- Client ---------------------------------------------------------------------

fn connect(port: u16, timeout: Duration) -> io::Result<TcpStream> {
    let addr = SocketAddr::from(([127, 0, 0, 1], port));
    let s = TcpStream::connect_timeout(&addr, timeout)?;
    s.set_nodelay(true)?;
    Ok(s)
}

fn send_request(
    s: &mut TcpStream,
    port: u16,
    method: &str,
    path: &str,
    token: &str,
    body: Option<&[u8]>,
    extra: &[(&str, &str)],
) -> io::Result<()> {
    let mut head = format!(
        "{method} {path} HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nConnection: close\r\n"
    );
    if !token.is_empty() {
        head.push_str(&format!("Authorization: Bearer {token}\r\n"));
    }
    for (k, v) in extra {
        head.push_str(&format!("{k}: {v}\r\n"));
    }
    if let Some(b) = body {
        head.push_str("Content-Type: application/json; charset=utf-8\r\n");
        head.push_str(&format!("Content-Length: {}\r\n", b.len()));
    }
    head.push_str("\r\n");
    s.write_all(head.as_bytes())?;
    if let Some(b) = body {
        s.write_all(b)?;
    }
    s.flush()
}

fn read_status_and_headers(r: &mut impl BufRead) -> io::Result<u16> {
    let mut line = String::new();
    r.read_line(&mut line)?;
    let status: u16 = line
        .split_whitespace()
        .nth(1)
        .and_then(|s| s.parse().ok())
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, format!("keine Antwort: {line:?}")))?;
    loop {
        let mut h = String::new();
        if r.read_line(&mut h)? == 0 || h == "\r\n" || h == "\n" {
            break;
        }
    }
    Ok(status)
}

/// Eine Anfrage, eine Antwort. Zurueck kommen Status und Rumpf.
pub fn request(
    port: u16,
    method: &str,
    path: &str,
    token: &str,
    body: Option<&[u8]>,
    timeout: Duration,
) -> io::Result<(u16, String)> {
    let mut s = connect(port, timeout)?;
    s.set_read_timeout(Some(timeout))?;
    s.set_write_timeout(Some(timeout))?;
    send_request(&mut s, port, method, path, token, body, &[])?;
    let mut r = BufReader::new(s);
    let status = read_status_and_headers(&mut r)?;
    let mut out = String::new();
    r.read_to_string(&mut out)?;
    Ok((status, out))
}

/// Verfolgt einen Ereignisstrom. `on_event` bekommt jedes Ereignis; gibt es
/// false zurueck, endet das Zuhoeren.
pub fn sse(
    port: u16,
    path: &str,
    token: &str,
    last_event_id: Option<u64>,
    mut on_event: impl FnMut(Event) -> bool,
) -> io::Result<()> {
    let mut s = connect(port, Duration::from_secs(10))?;
    // Kein Lesezeitlimit: ein stiller Strom ist normal, der Keepalive kommt.
    s.set_read_timeout(None)?;
    let id_header = last_event_id.map(|i| i.to_string());
    let extra: Vec<(&str, &str)> = match &id_header {
        Some(v) => vec![("Last-Event-ID", v.as_str())],
        None => vec![],
    };
    send_request(&mut s, port, "GET", path, token, None, &extra)?;

    let mut r = BufReader::new(s);
    let status = read_status_and_headers(&mut r)?;
    if status != 200 {
        return Err(io::Error::other(format!("Status {status}")));
    }

    let mut ev = Event::default();
    let mut data = Vec::new();
    loop {
        let mut line = String::new();
        if r.read_line(&mut line)? == 0 {
            return Ok(()); // Gegenstelle hat geschlossen
        }
        let line = line.trim_end_matches(['\r', '\n']);
        if line.is_empty() {
            if !data.is_empty() || !ev.name.is_empty() {
                ev.data = data.join("\n");
                if !on_event(std::mem::take(&mut ev)) {
                    return Ok(());
                }
                data.clear();
            }
            continue;
        }
        if let Some(rest) = line.strip_prefix("data:") {
            data.push(rest.strip_prefix(' ').unwrap_or(rest).to_string());
        } else if let Some(rest) = line.strip_prefix("event:") {
            ev.name = rest.trim().to_string();
        } else if let Some(rest) = line.strip_prefix("id:") {
            ev.id = rest.trim().parse().ok();
        }
        // Zeilen mit : am Anfang sind Kommentare (Keepalive)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::mpsc;

    fn start(handler: impl Fn(Request) -> Reply + Send + Sync + 'static) -> u16 {
        let l = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = l.local_addr().unwrap().port();
        thread::spawn(move || serve(l, handler));
        port
    }

    #[test]
    fn anfrage_und_antwort() {
        let port = start(|req| {
            Reply::Done(Response::json(
                format!(
                    "{{\"m\":\"{}\",\"p\":\"{}\",\"q\":\"{}\",\"auth\":\"{}\",\"body\":\"{}\"}}",
                    req.method,
                    req.path,
                    req.query("since").unwrap_or(""),
                    req.header("authorization").unwrap_or(""),
                    String::from_utf8_lossy(&req.body)
                )
                .into_bytes(),
            ))
        });
        let t = Duration::from_secs(5);
        let (s, b) = request(port, "POST", "/api/x?since=7", "geheim", Some(b"{\"a\":1}"), t).unwrap();
        assert_eq!(s, 200);
        assert!(b.contains("\"m\":\"POST\""), "{b}");
        assert!(b.contains("\"p\":\"/api/x\""), "{b}");
        assert!(b.contains("\"q\":\"7\""), "{b}");
        assert!(b.contains("Bearer geheim"), "{b}");
        assert!(b.contains("{\\\"a\\\":1}") || b.contains("{\"a\":1}"), "{b}");
    }

    #[test]
    fn pfad_und_status() {
        let port = start(|req| match req.segments().as_slice() {
            ["api", "nodes", name, "console"] => Reply::Done(Response::text(200, *name)),
            _ => Reply::Done(Response::text(404, "nichts da")),
        });
        let t = Duration::from_secs(5);
        assert_eq!(
            request(port, "GET", "/api/nodes/mining-3/console", "", None, t).unwrap(),
            (200, "mining-3".to_string())
        );
        assert_eq!(request(port, "GET", "/weg", "", None, t).unwrap().0, 404);
    }

    #[test]
    fn ereignisstrom_kommt_zeile_fuer_zeile_an() {
        let (tx, rx) = mpsc::channel::<()>();
        let tx = std::sync::Mutex::new(tx);
        let port = start(move |_req| {
            let tx = tx.lock().unwrap().clone();
            Reply::Stream(Box::new(move |w| {
                write_event(w, Some(1), "out", "erste Zeile")?;
                write_event(w, Some(2), "out", "zweite\nmit Umbruch")?;
                write_keepalive(w)?;
                write_event(w, Some(3), "sys", "Schluss")?;
                let _ = tx.send(());
                Ok(())
            }))
        });

        let mut got = Vec::new();
        sse(port, "/api/events", "", Some(0), |ev| {
            got.push((ev.id, ev.name.clone(), ev.data.clone()));
            ev.data != "Schluss"
        })
        .unwrap();
        let _ = rx.recv_timeout(Duration::from_secs(5));

        assert_eq!(
            got,
            [
                (Some(1), "out".to_string(), "erste Zeile".to_string()),
                (Some(2), "out".to_string(), "zweite\nmit Umbruch".to_string()),
                (Some(3), "sys".to_string(), "Schluss".to_string()),
            ]
        );
    }

    #[test]
    fn last_event_id_wird_mitgeschickt() {
        let port = start(|req| {
            let id = req.header("last-event-id").unwrap_or("-").to_string();
            Reply::Stream(Box::new(move |w| write_event(w, None, "x", &id)))
        });
        let mut seen = String::new();
        sse(port, "/e", "", Some(42), |ev| {
            seen = ev.data;
            false
        })
        .unwrap();
        assert_eq!(seen, "42");
    }

    #[test]
    fn prozentzeichen_im_pfad() {
        assert_eq!(percent_decode("a%20b"), "a b");
        assert_eq!(percent_decode("Gr%C3%BC%C3%9Fe"), "Grüße");
        assert_eq!(percent_decode("a+b"), "a b");
        assert_eq!(percent_decode("100%"), "100%");
    }

    #[test]
    fn zu_grosser_kopf_wird_abgewiesen() {
        let port = start(|_| Reply::Done(Response::text(200, "ok")));
        let mut s = connect(port, Duration::from_secs(5)).unwrap();
        s.write_all(b"GET / HTTP/1.1\r\n").unwrap();
        let junk = format!("X-Muell: {}\r\n", "a".repeat(1000));
        for _ in 0..20 {
            if s.write_all(junk.as_bytes()).is_err() {
                break;
            }
        }
        let _ = s.write_all(b"\r\n");
        let mut out = String::new();
        let _ = BufReader::new(s).read_to_string(&mut out);
        assert!(out.is_empty() || out.contains("400"), "{out}");
    }
}
