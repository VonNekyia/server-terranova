//! Argumente lesen und den passenden Befehl ausfuehren.

use std::path::PathBuf;
use std::process::ExitCode;

use lexopt::prelude::*;

use crate::config::Config;
use crate::doctor;
use crate::paths::Paths;
use crate::sync::Syncer;
use crate::VERSION;

pub const HELP: &str = "\
terranova - startet, beaufsichtigt und stoppt das Terranova-Netzwerk

  terranova <befehl> [optionen]

Befehle
  sync [name...]      Server aus templates/ bestuecken (--dry-run zeigt nur)
  doctor              pruefen, ob alles startklar ist
  init                eine terranova.yml anlegen
  version             Fassung anzeigen

Optionen
  --root <pfad>       Netzwerkverzeichnis (sonst: ueber bin/, sonst aufwaerts
                      gesucht bis zu einer terranova.yml)
  --dry-run           nichts schreiben, nur zeigen
  -h, --help          diese Hilfe
";

#[derive(Debug, Default)]
struct Args {
    root: Option<PathBuf>,
    command: Option<String>,
    values: Vec<String>,
    dry_run: bool,
    help: bool,
}

fn parse() -> Result<Args, lexopt::Error> {
    let mut a = Args::default();
    let mut p = lexopt::Parser::from_env();
    while let Some(arg) = p.next()? {
        match arg {
            Short('h') | Long("help") => a.help = true,
            Long("root") => a.root = Some(p.value()?.into()),
            Long("dry-run") => a.dry_run = true,
            Value(v) if a.command.is_none() => a.command = Some(v.string()?),
            Value(v) => a.values.push(v.string()?),
            _ => return Err(arg.unexpected()),
        }
    }
    Ok(a)
}

pub fn run() -> ExitCode {
    let args = match parse() {
        Ok(a) => a,
        Err(e) => {
            eprintln!("terranova: {e}\n\n{HELP}");
            return ExitCode::from(2);
        }
    };

    if args.help || args.command.is_none() {
        print!("{HELP}");
        return ExitCode::SUCCESS;
    }
    let command = args.command.as_deref().unwrap_or_default();

    if command == "version" {
        println!("terranova {VERSION}");
        return ExitCode::SUCCESS;
    }

    // Alles Weitere braucht das Netzwerkverzeichnis.
    let paths = match Paths::discover(args.root.as_deref()) {
        Ok(p) => p,
        Err(e) => {
            // init darf auch dort laufen, wo es noch keine Config gibt.
            if command == "init" {
                return init(args.root.as_deref());
            }
            eprintln!("terranova: {e}");
            return ExitCode::FAILURE;
        }
    };

    if command == "init" {
        println!(
            "terranova: {} gibt es schon - nichts zu tun",
            paths.config().display()
        );
        return ExitCode::SUCCESS;
    }

    let cfg = match Config::load(&paths) {
        Ok(c) => c,
        Err(e) => {
            eprintln!("terranova: {e}");
            return ExitCode::FAILURE;
        }
    };

    match command {
        "sync" => sync(&paths, &cfg, &args.values, args.dry_run),
        "doctor" => {
            let checks = doctor::run(&paths, &cfg);
            print!("{}", doctor::render(&checks));
            match doctor::worst(&checks) {
                doctor::Level::Fail => ExitCode::FAILURE,
                _ => ExitCode::SUCCESS,
            }
        }
        other => {
            eprintln!("terranova: unbekannter Befehl '{other}'\n\n{HELP}");
            ExitCode::from(2)
        }
    }
}

fn sync(paths: &Paths, cfg: &Config, which: &[String], dry_run: bool) -> ExitCode {
    let syncer = match Syncer::new(paths, cfg) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("terranova: {e}");
            return ExitCode::FAILURE;
        }
    };

    // Ohne Angabe: die festen Server und jeder vorhandene Dungeon.
    let mut nodes = Vec::new();
    if which.is_empty() {
        nodes.extend(cfg.server_nodes(paths));
        for slot in crate::mines::existing(&paths.servers(), cfg.mines.slots) {
            nodes.push(cfg.mine_node(paths, slot));
        }
    } else {
        for name in which {
            match cfg.node(paths, name) {
                Some(n) => nodes.push(n),
                None => {
                    eprintln!("terranova: '{name}' gibt es nicht");
                    return ExitCode::FAILURE;
                }
            }
        }
    }

    let mut failed = false;
    for node in nodes {
        if node.template.is_none() {
            continue; // der Proxy wird nicht bestueckt
        }
        // Ein laufender Server haelt seine Jars offen - Windows laesst sie
        // dann nicht ersetzen.
        if !dry_run {
            if let Some(pid) = crate::win::port_owner(node.port) {
                let who = crate::win::image_name(pid).unwrap_or_else(|| format!("PID {pid}"));
                eprintln!(
                    "[sync] {}: laeuft ({who} auf Port {}) - uebersprungen",
                    node.name, node.port
                );
                continue;
            }
        }
        match syncer.sync(&node, dry_run) {
            Ok(r) => {
                let what = if r.nothing_to_do() {
                    "unveraendert".to_string()
                } else {
                    let mut parts = Vec::new();
                    if !r.copied.is_empty() {
                        parts.push(format!("{} kopiert", r.copied.len()));
                    }
                    if !r.pruned.is_empty() {
                        parts.push(format!("{} entfernt ({})", r.pruned.len(), r.pruned.join(", ")));
                    }
                    if r.props {
                        parts.push("server.properties".into());
                    }
                    if r.paper_global {
                        parts.push("paper-global.yml".into());
                    }
                    parts.join(", ")
                };
                let prefix = if dry_run { "[sync, Probelauf]" } else { "[sync]" };
                println!("{prefix} {:<10} {what}", node.name);
            }
            Err(e) => {
                eprintln!("[sync] {}: {e}", node.name);
                failed = true;
            }
        }
    }

    if failed {
        ExitCode::FAILURE
    } else {
        ExitCode::SUCCESS
    }
}

fn init(root: Option<&std::path::Path>) -> ExitCode {
    let dir = match root {
        Some(r) => r.to_path_buf(),
        None => match std::env::current_dir() {
            Ok(d) => d,
            Err(e) => {
                eprintln!("terranova: {e}");
                return ExitCode::FAILURE;
            }
        },
    };
    let file = dir.join(crate::paths::CONFIG_FILE);
    if file.exists() {
        println!("terranova: {} gibt es schon", file.display());
        return ExitCode::SUCCESS;
    }
    match std::fs::write(&file, crate::DEFAULT_CONFIG) {
        Ok(()) => {
            println!("terranova: {} angelegt", file.display());
            ExitCode::SUCCESS
        }
        Err(e) => {
            eprintln!("terranova: {}: {e}", file.display());
            ExitCode::FAILURE
        }
    }
}
