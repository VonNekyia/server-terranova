//! Terranova - startet, beaufsichtigt und stoppt das Netzwerk.
//!
//! Die CLI ist die eigentliche Schnittstelle; start.bat ruft sie nur auf, und
//! das Dashboard spricht dieselbe API wie sie.

// Faellt weg, sobald die CLI vollstaendig ist - bis dahin sind die Bausteine
// da, aber noch nicht alle verdrahtet.
#![allow(dead_code)]

use std::process::ExitCode;

mod api;
mod backend;
mod cli;
mod client;
mod config;
mod console;
mod deps;
mod docker;
mod doctor;
mod http;
mod mines;
mod paperyml;
mod paths;
mod props;
mod rcon;
mod reaper;
mod sched;
mod secrets;
mod supervisor;
mod sync;
mod win;

#[cfg(test)]
mod testutil;

pub const VERSION: &str = env!("CARGO_PKG_VERSION");

/// Was `terranova init` hinlegt - dieselbe Datei, die im Repository liegt.
pub const DEFAULT_CONFIG: &str = include_str!("../../terranova.yml");

/// Das Dashboard steckt in der Programmdatei: keine zweite Auslieferung,
/// kein Bauschritt, nichts was auseinanderlaufen kann.
pub const DASHBOARD: &str = include_str!("dashboard.html");

fn main() -> ExitCode {
    cli::run()
}
