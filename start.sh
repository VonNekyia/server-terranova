#!/usr/bin/env sh
# Startet das Terranova-Netzwerk. Alles Weitere macht terranova selbst;
# diese Datei ist nur die Abkuerzung - das Gegenstueck zu start.bat.
#
# Im Repository liegt unter bin/ nur die Windows-Fassung: eine Programmdatei
# je System einzuchecken hiesse, sie bei jeder Aenderung doppelt zu pflegen.
# Fehlt die Linux-Fassung, wird sie hier einmalig aus den Quellen gebaut.
set -e
cd "$(dirname "$0")"

if [ ! -x bin/terranova ]; then
    if ! command -v cargo >/dev/null 2>&1; then
        echo "bin/terranova fehlt, und cargo ist nicht installiert." >&2
        echo "Entweder Rust einrichten (https://rustup.rs) und noch einmal" >&2
        echo "starten, oder die fertige Programmdatei aus den CI-Artefakten" >&2
        echo "nach bin/terranova legen und ausfuehrbar machen." >&2
        exit 1
    fi
    echo "[start] bin/terranova fehlt - wird einmalig gebaut..."
    cargo build --release --manifest-path terranova/Cargo.toml
    mkdir -p bin
    cp terranova/target/release/terranova bin/terranova
fi

exec bin/terranova start "$@"
