#!/usr/bin/env sh
# Startet das Terranova-Netzwerk. Alles Weitere macht terranova selbst;
# diese Datei ist nur die Abkuerzung - das Gegenstueck zu start.bat.
#
# bin/terranova liegt fertig im Repository: statisch gebaut (musl), laeuft
# also auf jeder x86_64-Distribution, ohne dass Rust installiert sein muss.
# Auf anderen Architekturen - etwa einem ARM-Rechner - wird einmalig aus den
# Quellen gebaut und als bin/terranova-<arch> abgelegt.
set -e
cd "$(dirname "$0")"

arch=$(uname -m)
bin=bin/terranova
[ "$arch" = "x86_64" ] || bin="bin/terranova-$arch"

if [ ! -x "$bin" ]; then
    if [ -f "$bin" ]; then
        # Aus einem Klon unter Windows kommt die Datei ohne Ausfuehrungsrecht.
        chmod +x "$bin"
    elif ! command -v cargo >/dev/null 2>&1; then
        echo "$bin fehlt, und cargo ist nicht installiert." >&2
        echo "Fuer $arch liegt keine fertige Programmdatei bei - Rust einrichten" >&2
        echo "(https://rustup.rs) und noch einmal starten." >&2
        exit 1
    else
        echo "[start] $bin fehlt - wird einmalig gebaut..."
        cargo build --release --manifest-path terranova/Cargo.toml
        mkdir -p bin
        cp terranova/target/release/terranova "$bin"
    fi
fi

exec "$bin" start "$@"
