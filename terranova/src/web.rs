//! Was im Browser liegt: die oeffentliche Seite und die Weltkarte.
//!
//! Die Seite in `website/` ist ein fertiger Build ohne eigenen Server.
//! Terranova liefert sie deshalb selbst aus - mit einem Webserver daneben
//! gaebe es ein zweites Ding, das jemand starten, aktuell halten und
//! ueberwachen muesste, und genau das soll hier ja gerade nicht sein.
//!
//! Die Karte dagegen gehoert uns nicht: Pl3xMap bringt seinen eigenen
//! Webserver mit und laeuft im Server-Prozess von main. Wir koennen sie
//! also nicht starten oder stoppen, nur nachsehen, ob sie antwortet - und
//! genau das ist die Auskunft, die fehlt, wenn jemand fragt "warum geht die
//! Karte nicht".

use std::fs;
use std::io;
use std::net::{SocketAddr, TcpListener, TcpStream, ToSocketAddrs};
use std::path::{Path, PathBuf};
use std::time::Duration;

use crate::http::{self, Reply, Request, Response};

/// Laenger warten wir nicht auf die Karte: sie horcht auf demselben Rechner.
const PROBE: Duration = Duration::from_millis(400);

// --- Die oeffentliche Seite ----------------------------------------------

/// Bindet den Port und liefert `root` aus, in einem eigenen Thread.
///
/// `Ok(None)` heisst: abgeschaltet (Port 0) oder es gibt nichts auszuliefern.
pub fn serve_site(root: PathBuf, bind: &str, port: u16) -> io::Result<Option<SocketAddr>> {
    if port == 0 || !root.join("index.html").is_file() {
        return Ok(None);
    }
    let listener = TcpListener::bind((bind, port))?;
    let addr = listener.local_addr()?;
    std::thread::Builder::new()
        .name("terranova-site".into())
        .spawn(move || http::serve(listener, move |req| reply(&root, &req)))?;
    Ok(Some(addr))
}

fn reply(root: &Path, req: &Request) -> Reply {
    if req.method != "GET" && req.method != "HEAD" {
        return Reply::Done(Response::text(405, "nur GET\n"));
    }
    let Some(rel) = safe_path(&req.path) else {
        return Reply::Done(Response::text(403, "kein gueltiger Pfad\n"));
    };

    // Erst die Datei selbst, dann index.html im Ordner, dann - fuer Wege wie
    // /impressum, die es nur im Browser gibt - die Startseite. Nur wenn im
    // letzten Stueck kein Punkt steht: ein fehlendes Bild soll 404 bleiben
    // und nicht als HTML zurueckkommen.
    let direct = root.join(&rel);
    let file = if direct.is_file() {
        Some(direct)
    } else if direct.join("index.html").is_file() {
        Some(direct.join("index.html"))
    } else if rel.extension().is_none() {
        Some(root.join("index.html"))
    } else {
        None
    };

    let Some(file) = file else {
        return Reply::Done(Response::text(404, "nicht gefunden\n"));
    };
    match send(&file, req) {
        Ok(r) => Reply::Done(r),
        Err(_) => Reply::Done(Response::text(500, "nicht lesbar\n")),
    }
}

fn send(file: &Path, req: &Request) -> io::Result<Response> {
    let meta = fs::metadata(file)?;
    let stamp = meta
        .modified()
        .ok()
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map_or(0, |d| d.as_secs());
    let tag = format!("\"{:x}-{:x}\"", meta.len(), stamp);

    // Das Hintergrundbild der Seite sind acht Megabyte. Ohne das hier
    // schickt der Server sie bei jedem Seitenwechsel neu.
    if req.header("if-none-match") == Some(tag.as_str()) {
        return Ok(Response::new(304, mime(file), Vec::new()).with_header("ETag", &tag));
    }

    let body = if req.method == "HEAD" {
        Vec::new()
    } else {
        fs::read(file)?
    };
    // Alles unter static/ traegt seinen Inhalt im Namen (main.c335e661.js) -
    // aendert sich der Inhalt, aendert sich der Name.
    let cache = if file
        .components()
        .any(|c| c.as_os_str().eq_ignore_ascii_case("static"))
    {
        "public, max-age=31536000, immutable"
    } else {
        "no-cache"
    };
    Ok(Response::new(200, mime(file), body)
        .with_header("ETag", &tag)
        .with_header("Cache-Control", cache)
        // Die Seite laedt nichts von uns nach, was in einem Rahmen haengen
        // muesste, und raten soll der Browser auch nicht.
        .with_header("X-Content-Type-Options", "nosniff")
        .with_header("X-Frame-Options", "SAMEORIGIN"))
}

/// Aus dem Pfad der Anfrage einen Pfad unterhalb des Wurzelverzeichnisses -
/// oder gar keinen.
///
/// Der Pfad ist zu diesem Zeitpunkt schon prozentdekodiert, `%2e%2e` kommt
/// hier also als `..` an und faellt durch.
fn safe_path(path: &str) -> Option<PathBuf> {
    let mut out = PathBuf::new();
    for seg in path.split('/') {
        match seg {
            "" | "." => continue,
            ".." => return None,
            s if s.contains('\\') || s.contains(':') || s.contains('\0') => return None,
            // Alternative Datenstroeme und Kurznamen gibt es hier nicht.
            s if s.ends_with(' ') || s.ends_with('.') => return None,
            s => out.push(s),
        }
    }
    Some(out)
}

fn mime(p: &Path) -> &'static str {
    let ext = p
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("")
        .to_ascii_lowercase();
    match ext.as_str() {
        "html" | "htm" => "text/html; charset=utf-8",
        "css" => "text/css; charset=utf-8",
        "js" | "mjs" => "text/javascript; charset=utf-8",
        "json" | "map" => "application/json; charset=utf-8",
        "webmanifest" => "application/manifest+json; charset=utf-8",
        "txt" => "text/plain; charset=utf-8",
        "svg" => "image/svg+xml",
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "gif" => "image/gif",
        "webp" => "image/webp",
        "avif" => "image/avif",
        "ico" => "image/x-icon",
        "woff2" => "font/woff2",
        "woff" => "font/woff",
        "ttf" => "font/ttf",
        "otf" => "font/otf",
        "mp4" => "video/mp4",
        "webm" => "video/webm",
        "mp3" => "audio/mpeg",
        "ogg" => "audio/ogg",
        "wasm" => "application/wasm",
        "pdf" => "application/pdf",
        "zip" => "application/zip",
        _ => "application/octet-stream",
    }
}

// --- Die Karte ------------------------------------------------------------

/// Antwortet da jemand? Mehr laesst sich von aussen nicht sagen, und mehr
/// braucht es auch nicht.
pub fn reachable(port: u16) -> bool {
    if port == 0 {
        return false;
    }
    let Ok(mut addrs) = ("127.0.0.1", port).to_socket_addrs() else {
        return false;
    };
    addrs.any(|a| TcpStream::connect_timeout(&a, PROBE).is_ok())
}

/// Der Ordner, aus dem Pl3xMap seine Kacheln ausliefert - und wie viele
/// Welten darin liegen. Steht der auf 0, hat die Karte noch nie gerendert.
pub fn mapped_worlds(server_dir: &Path) -> usize {
    let tiles = server_dir.join("plugins/Pl3xMap/web/tiles");
    fs::read_dir(tiles).map_or(0, |d| d.flatten().filter(|e| e.path().is_dir()).count())
}

/// Was in `plugins/Pl3xMap/config.yml` unter `internal-webserver` steht:
/// eingeschaltet, und auf welchem Port.
///
/// Nur zum Vergleichen mit `web.map.port` in terranova.yml. Wir schreiben
/// die Datei nicht - sie gehoert dem Plugin.
pub fn pl3xmap_webserver(server_dir: &Path) -> Option<(bool, u16)> {
    let text = fs::read_to_string(server_dir.join("plugins/Pl3xMap/config.yml")).ok()?;
    let mut lines = text.lines();
    let outer = lines
        .by_ref()
        .find(|l| l.trim() == "internal-webserver:")
        .map(indent)?;

    let (mut on, mut port) = (true, 0u16);
    for line in lines {
        if line.trim().is_empty() || line.trim_start().starts_with('#') {
            continue;
        }
        // Der Block endet, sobald wieder etwas auf gleicher oder geringerer
        // Ebene steht.
        if indent(line) <= outer {
            break;
        }
        let Some((k, v)) = line.split_once(':') else {
            continue;
        };
        match k.trim() {
            "enabled" => on = v.trim() == "true",
            "port" => port = v.trim().parse().unwrap_or(0),
            _ => {}
        }
    }
    Some((on, port))
}

fn indent(l: &str) -> usize {
    l.len() - l.trim_start().len()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pfade_bleiben_im_verzeichnis() {
        assert_eq!(
            safe_path("/static/js/a.js"),
            Some(PathBuf::from("static/js/a.js"))
        );
        assert_eq!(safe_path("/"), Some(PathBuf::new()));
        assert_eq!(safe_path("/./a//b"), Some(PathBuf::from("a/b")));
        // Der Kern der Sache: nichts davon darf aus website/ herausfuehren.
        assert_eq!(safe_path("/../terranova.yml"), None);
        assert_eq!(safe_path("/a/../../b"), None);
        assert_eq!(safe_path("/C:/Windows/win.ini"), None);
        assert_eq!(safe_path("/a\\..\\b"), None);
        assert_eq!(safe_path("/index.html "), None);
    }

    #[test]
    fn dateitypen() {
        assert_eq!(mime(Path::new("a/index.html")), "text/html; charset=utf-8");
        assert_eq!(
            mime(Path::new("main.C335E661.JS")),
            "text/javascript; charset=utf-8"
        );
        assert_eq!(mime(Path::new("favicon.ico")), "image/x-icon");
        assert_eq!(mime(Path::new("robots")), "application/octet-stream");
    }

    #[test]
    fn pl3xmap_port_aus_der_echten_config() {
        // Die Datei im Netzwerk ist die Vorlage fuer diesen Test: aendert
        // Pl3xMap eines Tages seine Schluessel, faellt es hier auf.
        let main = Path::new(env!("CARGO_MANIFEST_DIR")).join("../servers/main");
        if !main.join("plugins/Pl3xMap/config.yml").is_file() {
            return;
        }
        let (on, port) = pl3xmap_webserver(&main).expect("config lesbar");
        assert!(on, "internal-webserver ist aus");
        assert_ne!(port, 0, "kein Port gefunden");
    }

    #[test]
    fn pl3xmap_block_endet_am_einzug() {
        let dir = std::env::temp_dir().join("tn-pl3x-test/plugins/Pl3xMap");
        fs::create_dir_all(&dir).unwrap();
        fs::write(
            dir.join("config.yml"),
            "settings:
  internal-webserver:
    # Kommentar
    enabled: true
    port: 8123
  performance:
    port: 99
",
        )
        .unwrap();
        let root = std::env::temp_dir().join("tn-pl3x-test");
        assert_eq!(pl3xmap_webserver(&root), Some((true, 8123)));
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn geschlossener_port_antwortet_nicht() {
        let l = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = l.local_addr().unwrap().port();
        assert!(reachable(port));
        drop(l);
        assert!(!reachable(port));
        assert!(!reachable(0));
    }
}
