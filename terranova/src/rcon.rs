//! Source-RCON: einem Server einen Befehl schicken, ohne seine Konsole zu
//! besitzen.
//!
//! Gebraucht wird es nur noch als Ersatzweg - fuer Server, die der Supervisor
//! nach einem eigenen Absturz uebernommen hat und deren stdin er deshalb
//! nicht in der Hand haelt. Server, die er selbst gestartet hat, bekommen ihre
//! Befehle ueber stdin.
//!
//! Paket, little endian: i32 laenge | i32 id | i32 typ | body | 0 | 0.
//! Die Laenge zaehlt ab dem id-Feld. Typ 3 meldet an, Typ 2 fuehrt aus; eine
//! abgelehnte Anmeldung beantwortet der Server mit id = -1.

use std::io::{self, Read, Write};
use std::net::{SocketAddr, TcpStream};
use std::time::Duration;

pub const AUTH: i32 = 3;
pub const EXEC: i32 = 2;
pub const AUTH_RESPONSE: i32 = 2;

/// Mehr schickt Minecraft in einem Paket nicht; alles darueber ist Unsinn
/// oder kein RCON.
const MAX_LEN: i32 = 1 << 16;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Packet {
    pub id: i32,
    pub kind: i32,
    pub body: String,
}

pub fn encode(id: i32, kind: i32, body: &str) -> Vec<u8> {
    let body = body.as_bytes();
    // id (4) + typ (4) + body + zwei Nullbytes
    let len = 10 + body.len() as i32;
    let mut out = Vec::with_capacity(4 + len as usize);
    out.extend_from_slice(&len.to_le_bytes());
    out.extend_from_slice(&id.to_le_bytes());
    out.extend_from_slice(&kind.to_le_bytes());
    out.extend_from_slice(body);
    out.extend_from_slice(&[0, 0]);
    out
}

/// Liest genau ein Paket. TCP liefert in beliebigen Stuecken - read_exact
/// setzt sie zusammen.
pub fn read_packet(r: &mut impl Read) -> io::Result<Packet> {
    let mut head = [0u8; 4];
    r.read_exact(&mut head)?;
    let len = i32::from_le_bytes(head);
    if !(10..=MAX_LEN).contains(&len) {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!("unplausible RCON-Paketlaenge {len}"),
        ));
    }
    let mut buf = vec![0u8; len as usize];
    r.read_exact(&mut buf)?;
    let id = i32::from_le_bytes([buf[0], buf[1], buf[2], buf[3]]);
    let kind = i32::from_le_bytes([buf[4], buf[5], buf[6], buf[7]]);
    let body = String::from_utf8_lossy(&buf[8..buf.len() - 2]).into_owned();
    Ok(Packet { id, kind, body })
}

/// Meldet sich an und fuehrt einen Befehl aus. Zurueck kommt die Antwort des
/// Servers.
pub fn command(port: u16, password: &str, command: &str, timeout: Duration) -> io::Result<String> {
    let addr = SocketAddr::from(([127, 0, 0, 1], port));
    let mut s = TcpStream::connect_timeout(&addr, timeout)?;
    s.set_read_timeout(Some(timeout))?;
    s.set_write_timeout(Some(timeout))?;

    s.write_all(&encode(1, AUTH, password))?;
    loop {
        let p = read_packet(&mut s)?;
        if p.kind == AUTH_RESPONSE {
            if p.id == -1 {
                return Err(io::Error::new(
                    io::ErrorKind::PermissionDenied,
                    "RCON-Anmeldung abgelehnt",
                ));
            }
            break;
        }
    }

    s.write_all(&encode(2, EXEC, command))?;
    match read_packet(&mut s) {
        Ok(p) => Ok(p.body),
        // "stop" beendet den Server, bevor er antwortet. Eine geschlossene
        // Verbindung heisst hier: der Befehl ist angekommen.
        Err(e)
            if matches!(
                e.kind(),
                io::ErrorKind::UnexpectedEof
                    | io::ErrorKind::ConnectionReset
                    | io::ErrorKind::ConnectionAborted
            ) =>
        {
            Ok(String::new())
        }
        Err(e) => Err(e),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::TcpListener;
    use std::thread;

    #[test]
    fn kodierung_wie_rcon_ps1() {
        // len = 10 + 4 Zeichen
        assert_eq!(
            encode(1, AUTH, "abcd"),
            [14, 0, 0, 0, 1, 0, 0, 0, 3, 0, 0, 0, b'a', b'b', b'c', b'd', 0, 0]
        );
    }

    /// Liefert jeweils nur ein Byte - wie ein zaeher TCP-Strom.
    struct Trickle(Vec<u8>, usize);
    impl Read for Trickle {
        fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
            if self.1 >= self.0.len() || buf.is_empty() {
                return Ok(0);
            }
            buf[0] = self.0[self.1];
            self.1 += 1;
            Ok(1)
        }
    }

    #[test]
    fn zerstueckelte_pakete_werden_zusammengesetzt() {
        let mut bytes = encode(7, 0, "There are 0 of a max of 20 players online");
        bytes.extend(encode(8, 0, "zweites"));
        let mut r = Trickle(bytes, 0);
        assert_eq!(read_packet(&mut r).unwrap().id, 7);
        assert_eq!(read_packet(&mut r).unwrap().body, "zweites");
    }

    #[test]
    fn unsinnige_laenge_wird_abgelehnt() {
        let mut r = io::Cursor::new(vec![0xff, 0xff, 0xff, 0x7f, 0, 0, 0, 0]);
        assert!(read_packet(&mut r).is_err());
    }

    /// Ein Mini-RCON-Server: nimmt "richtig" als Passwort und antwortet auf
    /// jeden Befehl mit "ok:<befehl>".
    fn fake_server() -> u16 {
        let l = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = l.local_addr().unwrap().port();
        thread::spawn(move || {
            for conn in l.incoming().take(2) {
                let mut s = conn.unwrap();
                let auth = read_packet(&mut s).unwrap();
                let id = if auth.body == "richtig" { auth.id } else { -1 };
                s.write_all(&encode(id, AUTH_RESPONSE, "")).unwrap();
                if id == -1 {
                    continue;
                }
                let cmd = read_packet(&mut s).unwrap();
                s.write_all(&encode(cmd.id, 0, &format!("ok:{}", cmd.body))).unwrap();
            }
        });
        port
    }

    #[test]
    fn anmelden_und_ausfuehren() {
        let port = fake_server();
        let t = Duration::from_secs(5);
        assert_eq!(command(port, "richtig", "list", t).unwrap(), "ok:list");
        let err = command(port, "falsch", "list", t).unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::PermissionDenied);
    }
}
