#!/usr/bin/env sh
# Startet nur das Noetigste: Datenbanken, Proxy und main.
# Fuer Rechner, denen build, farm und die Dungeons zu viel sind -
# zusammen belegen die sonst rund acht Gigabyte.
#
# Der Rest kommt jederzeit nach, ohne Neustart:
#   bin/terranova restart build
#
# Das Gegenstueck zu start_minimal.bat. Welche Programmdatei es nimmt und ob
# sie erst gebaut werden muss, entscheidet start.sh - hier steht nur, was
# gestartet wird.
exec "$(dirname "$0")/start.sh" main "$@"
