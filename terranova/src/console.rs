//! Die Konsole eines Servers: ein Ring der letzten Zeilen, dem beliebig viele
//! Zuschauer folgen koennen - die CLI, das Dashboard, das Startfenster.
//!
//! Wichtigste Eigenschaft: `push` haelt den Mutex nur fuer ein paar
//! Anweisungen und wartet nie auf einen Zuschauer. Der Thread, der die Pipe
//! eines Servers leert, darf nicht haengen bleiben: laeuft die Pipe voll,
//! blockiert Paper beim Schreiben seiner Konsole irgendwann den Server-Thread.

use std::collections::VecDeque;
use std::io::{self, Read};
use std::sync::{Condvar, Mutex, MutexGuard};
use std::time::Duration;

use serde::Serialize;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Kind {
    /// stdout des Prozesses
    Out,
    /// stderr des Prozesses
    Err,
    /// Meldungen von Terranova selbst ("gestartet", "gestoppt", ...)
    Sys,
}

impl Kind {
    pub fn as_str(self) -> &'static str {
        match self {
            Kind::Out => "out",
            Kind::Err => "err",
            Kind::Sys => "sys",
        }
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct Line {
    pub seq: u64,
    pub kind: Kind,
    pub text: String,
}

/// Was ein Zuschauer bei einem Abruf bekommt.
#[derive(Debug)]
pub struct Batch {
    pub lines: Vec<Line>,
    /// Zeilen, die er verpasst hat, weil der Ring schneller war als er:
    /// (erste verlorene seq, erste noch vorhandene seq)
    pub gap: Option<(u64, u64)>,
    /// Ab hier geht es beim naechsten Abruf weiter.
    pub next: u64,
}

pub struct LineRing {
    inner: Mutex<Inner>,
    cond: Condvar,
}

struct Inner {
    lines: VecDeque<Line>,
    next: u64,
    cap: usize,
}

impl LineRing {
    pub fn new(cap: usize) -> LineRing {
        LineRing {
            inner: Mutex::new(Inner {
                lines: VecDeque::with_capacity(cap),
                next: 1,
                cap: cap.max(1),
            }),
            cond: Condvar::new(),
        }
    }

    fn lock(&self) -> MutexGuard<'_, Inner> {
        // Ein Panic eines anderen Threads darf die Konsole nicht mitreissen.
        self.inner.lock().unwrap_or_else(|e| e.into_inner())
    }

    pub fn push(&self, kind: Kind, text: impl Into<String>) -> u64 {
        let mut g = self.lock();
        let seq = g.next;
        g.next += 1;
        if g.lines.len() == g.cap {
            g.lines.pop_front();
        }
        g.lines.push_back(Line {
            seq,
            kind,
            text: text.into(),
        });
        drop(g);
        self.cond.notify_all();
        seq
    }

    pub fn next_seq(&self) -> u64 {
        self.lock().next
    }

    fn collect(g: &Inner, since: u64) -> Batch {
        let oldest = g.lines.front().map_or(g.next, |l| l.seq);
        let gap = (since < oldest && since < g.next).then_some((since, oldest));
        let start = since.max(oldest);
        // Die Folge ist lueckenlos, also laesst sich der Index ausrechnen.
        let skip = usize::try_from(start - oldest).unwrap_or(usize::MAX);
        Batch {
            lines: g.lines.iter().skip(skip).cloned().collect(),
            gap,
            next: g.next,
        }
    }

    /// Alles ab `since`.
    pub fn since(&self, since: u64) -> Batch {
        Self::collect(&self.lock(), since)
    }

    /// Wartet hoechstens `timeout` darauf, dass es Zeilen ab `since` gibt.
    pub fn wait_since(&self, since: u64, timeout: Duration) -> Batch {
        let g = self.lock();
        let (g, _) = self
            .cond
            .wait_timeout_while(g, timeout, |g| g.next <= since)
            .unwrap_or_else(|e| e.into_inner());
        Self::collect(&g, since)
    }

    /// Die letzten `n` Zeilen - fuer jemanden, der neu zuschaut.
    pub fn tail(&self, n: usize) -> Batch {
        let g = self.lock();
        let since = g.next.saturating_sub(n as u64).max(1);
        let mut b = Self::collect(&g, since);
        b.gap = None;
        b
    }
}

/// Liest eine Pipe bis zu ihrem Ende und reicht jede Zeile an `f` weiter.
///
/// \r\n und \n werden entfernt, ungueltiges UTF-8 ersetzt, ANSI-Farbcodes
/// verworfen. Eine Zeile ohne Umbruch wird nach 64 KiB zwangsweise
/// abgeschnitten, damit der Puffer nicht unbegrenzt waechst.
pub fn pump(mut r: impl Read, mut f: impl FnMut(String)) {
    const MAX_LINE: usize = 64 * 1024;
    let mut line = Vec::with_capacity(512);
    let mut chunk = [0u8; 8192];
    loop {
        let n = match r.read(&mut chunk) {
            Ok(0) => break,
            Ok(n) => n,
            Err(e) if e.kind() == io::ErrorKind::Interrupted => continue,
            Err(_) => break,
        };
        for &b in &chunk[..n] {
            if b == b'\n' {
                emit(&mut line, &mut f);
            } else {
                line.push(b);
                if line.len() >= MAX_LINE {
                    emit(&mut line, &mut f);
                }
            }
        }
    }
    if !line.is_empty() {
        emit(&mut line, &mut f);
    }
}

fn emit(line: &mut Vec<u8>, f: &mut impl FnMut(String)) {
    if line.last() == Some(&b'\r') {
        line.pop();
    }
    let text = strip_ansi(&String::from_utf8_lossy(line));
    line.clear();
    f(text);
}

/// Entfernt ANSI-Steuerfolgen (ESC [ ... Endzeichen). Paper und Velocity
/// faerben ihre Konsole, auch wenn stdout gar kein Terminal ist.
pub fn strip_ansi(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut chars = s.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '\u{1b}' {
            if chars.peek() == Some(&'[') {
                chars.next();
                // Parameter bis zum Endzeichen 0x40-0x7E
                for c in chars.by_ref() {
                    if ('\u{40}'..='\u{7e}').contains(&c) {
                        break;
                    }
                }
            } else {
                chars.next();
            }
            continue;
        }
        out.push(c);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;
    use std::thread;
    use std::time::Instant;

    #[test]
    fn ueberlauf_meldet_eine_luecke() {
        let r = LineRing::new(3);
        for i in 1..=5 {
            r.push(Kind::Out, format!("z{i}"));
        }
        let b = r.since(1);
        assert_eq!(b.gap, Some((1, 3)));
        let texts: Vec<_> = b.lines.iter().map(|l| l.text.as_str()).collect();
        assert_eq!(texts, ["z3", "z4", "z5"]);
        assert_eq!(b.next, 6);
        let b = r.since(5);
        assert!(b.gap.is_none());
        assert_eq!(b.lines.len(), 1);
        assert!(r.since(6).lines.is_empty());
    }

    #[test]
    fn tail_ohne_luecke() {
        let r = LineRing::new(10);
        for i in 1..=4 {
            r.push(Kind::Out, format!("z{i}"));
        }
        let b = r.tail(2);
        assert!(b.gap.is_none());
        assert_eq!(b.lines.iter().map(|l| l.seq).collect::<Vec<_>>(), [3, 4]);
    }

    #[test]
    fn zuschauer_wird_geweckt() {
        let r = Arc::new(LineRing::new(10));
        let r2 = r.clone();
        let t = thread::spawn(move || r2.wait_since(1, Duration::from_secs(10)));
        thread::sleep(Duration::from_millis(50));
        r.push(Kind::Sys, "hallo");
        let b = t.join().unwrap();
        assert_eq!(b.lines[0].text, "hallo");
    }

    #[test]
    fn warten_endet_nach_zeitablauf() {
        let r = LineRing::new(10);
        let t0 = Instant::now();
        assert!(r.wait_since(1, Duration::from_millis(30)).lines.is_empty());
        assert!(t0.elapsed() >= Duration::from_millis(25));
    }

    #[test]
    fn langsamer_zuschauer_bremst_nicht() {
        // Ein Zuschauer, der nie abholt, darf das Schreiben nicht aufhalten.
        let r = LineRing::new(100);
        let t0 = Instant::now();
        for i in 0..200_000 {
            r.push(Kind::Out, format!("zeile {i}"));
        }
        assert!(t0.elapsed() < Duration::from_secs(5));
        assert_eq!(r.since(1).lines.len(), 100);
    }

    #[test]
    fn zeilen_aus_der_pipe() {
        let data = b"eins\r\nzwei\n\x1b[32mgr\xc3\xbcn\x1b[0m\ndrei ohne umbruch".to_vec();
        let mut got = Vec::new();
        pump(io::Cursor::new(data), |l| got.push(l));
        assert_eq!(got, ["eins", "zwei", "grün", "drei ohne umbruch"]);
    }

    #[test]
    fn kaputtes_utf8_wird_ersetzt() {
        let mut got = Vec::new();
        pump(io::Cursor::new(b"a\xffb\n".to_vec()), |l| got.push(l));
        assert_eq!(got, ["a\u{fffd}b"]);
    }
}
