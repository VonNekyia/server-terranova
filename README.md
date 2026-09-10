# server-terranova

Das Terranova-Netzwerk. Das Repository **ist** das Netzwerk: Serverconfigs,
Plugin-Configs, Plugin-Jars und die Skripte liegen hier und werden versioniert.
Wer klont, hat ein lauffähiges Netzwerk.

## Starten

```
start.bat
```

Doppelklick genügt. Beim ersten Start lädt das Skript einmalig MariaDB
(ca. 87 MB) und Redis (ca. 4 MB) herunter, richtet die Datenverzeichnisse ein
und legt die Datenbanken an. Danach bestückt es die Server aus `templates/`
und startet Proxy, main, build und farm.

Jeder Server bekommt sein eigenes Konsolenfenster — dort lässt sich wie gewohnt
`stop`, `op …` oder `reload` eintippen. Das `start.bat`-Fenster bleibt als
Aufsicht: es prüft alle 20 Sekunden, ob noch alle laufen, und startet neu, was
abgestürzt ist. **Strg+C** fährt das ganze Netzwerk sauber herunter.

Voraussetzung ist eine Java-Laufzeit. Getestet mit Java 25 und 26. Mit
`TERRANOVA_JAVA=<Pfad zu java.exe>` lässt sich eine bestimmte erzwingen.

## Die Server

| Server | Port | Speicher | Was drauf liegt |
| --- | --- | --- | --- |
| Proxy | **25565** | 512 MB | Velocity, der Eingang |
| `main` | 25566 | 4 GB | die gewachsene Welt: Nations, BetonQuest, Nexo, Pl3xMap, BountyfulSeas, Citizens, Proficisci |
| `build` | 25567 | 2 GB | Bauserver, Kreativ, flache Welt |
| `farm` | 25568 | 2 GB | Farmserver |
| `mining-1` … `mining-8` | 25571 … | 2 GB | Dungeons, auf Zuruf geöffnet |

Nur der Proxy ist von außen erreichbar. Die Server binden auf `127.0.0.1` —
wer direkt auf 25566 will, müsste schon auf der Maschine sein.

### Dungeons

```
powershell -File scripts\dungeon.ps1 open 3
powershell -File scripts\dungeon.ps1 list
powershell -File scripts\dungeon.ps1 close 2
```

Ein Dungeon ist eine Kopie von `templates\mining` auf einem eigenen Port. Die
acht Plätze stehen fest in `proxy\velocity.toml`; geöffnet wird nur, was
gebraucht wird. Velocity stört ein eingetragener, nicht laufender Server nicht —
ein Verbindungsversuch gibt dann nur eine Fehlermeldung.

Ein Dungeon bleibt **24 Stunden** offen und überlebt in dieser Zeit auch einen
Neustart: `dungeon.ps1 open` kopiert nur, wenn das Verzeichnis noch fehlt.
Abgeräumt wird er von

```
powershell -File scripts\reap-mining.ps1 -WhatIf
powershell -File scripts\reap-mining.ps1
```

Gelöscht heißt: Verzeichnis weg, das nächste `open` legt eine frische Welt an.
Ein noch laufender Dungeon wird übersprungen — `-StopRunning` beendet ihn
vorher sauber.

## Aufbau

```
start.bat                   MariaDB, Redis, dann Proxy und Server
scripts/
  network.ps1               startet Proxy und Server, beaufsichtigt sie
  sync-servers.ps1          Paper, Configs, Plugin-Jars, server.properties
  dungeon.ps1               Dungeons öffnen, schliessen, auflisten
  reap-mining.ps1           abgelaufene Dungeons abräumen
  restart-daily.ps1         täglicher Neustart um 04:00
  rcon.ps1                  RCON-Client, damit "stop" wirklich stoppt
proxy/
  velocity.toml             versioniert
  velocity-*.jar            versioniert
  forwarding.secret         nicht versioniert
servers/
  main/  build/  farm/      versioniert: nur die Plugin-Configs
  mining-*/                 nicht versioniert
templates/
  common/                   Paper, gemeinsame Configs, gemeinsame Plugin-Jars
  main/plugins/             was nur main braucht
  mining/                   die Dungeon-Vorlage
runtime/                    MariaDB und Redis, nicht versioniert
```

### Wo ein Plugin hingehört

Jedes Jar liegt **genau einmal** im Repository, nämlich unter `templates/`.
`sync-servers.ps1` kopiert es bei jedem Start in die Server. Ein Plugin-Update
ist damit eine Datei, kein viermaliges Kopieren.

**`templates/common/plugins/`** — was auf jedem Server laufen soll:
TerranovaLib, LuckPerms, HuskSync, PlaceholderAPI samt Expansions, Vault, TAB,
ChatControl, InteractiveChat, packetevents, FastAsyncWorldEdit, WorldGuard.

**`templates/main/plugins/`** — was an mains Welt und seinen Tabellen hängt:
Nations, Proficisci, PlayerActionAdapter, BountyfulSeas, Nexo, Citizens,
Pl3xMap, BetonQuest. Pl3xMap (Port 8080) und Nexos Packserver (8082) binden
feste Ports und können ohnehin nur einmal laufen.

**`templates/mining/plugins/`** — bewusst dünn, damit ein Dungeon schnell
startet. Hier landet BountyfulMining aus dem Schwesterrepository:

```
cd ..\BountyfulMining
gradle deployToTestServer
```

Versioniert ist unter `servers/` nur, was ein Server wirklich selbst besitzt:
die Configs seiner Plugins. Paper, die gemeinsamen Configs, alle Jars und auch
`server.properties` entstehen beim Start aus `templates/` und stehen in
`.gitignore` — in `server.properties` landet das RCON-Passwort.

## Eine Datenbank hinzufügen

Eine Zeile in `start.bat`:

```bat
set "DATABASES=nations betonquest chatcontrol interactivechat luckperms proficisci bountyfulseas husksync"
```

Der Name muss zu dem passen, was das Plugin in seiner Config erwartet — die
Zuordnung steht als Kommentar direkt darüber.

Zugangsdaten für lokale Entwicklung: `minecraft` / `minecraft` auf
`127.0.0.1:13306`. Redis läuft ohne Passwort auf `127.0.0.1:6379`.

## Weiterleitung

Velocity läuft mit **modern forwarding**: der Proxy prüft gegen Mojang und
reicht UUID und Skin signiert weiter, das Backend prüft die Signatur gegen
`proxy\forwarding.secret`. Die Server selbst laufen deshalb auf
`online-mode=false`.

Das Secret erzeugt `sync-servers.ps1` beim ersten Start und trägt es in
`config\paper-global.yml` jedes Servers ein. Beide sind in `.gitignore` — das
Secret ist der einzige Schutz der Backends und gehört nicht ins Repository.
Deshalb sind `servers/*/config/` und `servers/*/server.properties` nicht
versioniert; die Vorlagen dafür liegen in `templates/common/config/` und
`templates/<name>/server.properties`.

## Sauber stoppen

Einen Paper-Server unter Windows sauber herunterzufahren geht nur über RCON.
Die JVM hat kein Fenster mit Nachrichtenschleife, `CloseMainWindow()` läuft ins
Leere, und `taskkill` ohne `/F` antwortet *"Die Beendigung dieses Prozesses muss
erzwungen werden"*. Bliebe der harte Abschuss — und der kostet bei mains
500-MB-Welt irgendwann Chunks.

`scripts\rcon.ps1` schickt deshalb ein echtes `stop`. Im Log steht danach
`All chunks are saved` und `All RegionFile I/O tasks to complete`. RCON läuft je
Server auf Port + 100 (main 25666, build 25667, farm 25668, Dungeons 25671+),
gebunden an `127.0.0.1`; das Passwort steht in `runtime\rcon.secret`.

### Täglicher Neustart

`scripts\restart-daily.ps1` stoppt main, build und farm nacheinander mit zwei
Minuten Abstand — hochfahren tut sie die Aufsicht in `network.ps1`. So gibt es
genau eine Stelle, die Server startet. Dungeons bleiben unberührt.

Als geplante Aufgabe einrichten, einmalig in einer Konsole als Administrator —
`schtasks /Create /TN "Terranova Neustart" /SC DAILY /ST 04:00 /TR "..."` mit
dem vollen Pfad zu `restart-daily.ps1`; der genaue Aufruf steht als Kommentar
im Skript.

## Speicher

Proxy 0,5 + main 4 + build 2 + farm 2 macht 8,5 GB Grundlast. Bei 32 GB im
Rechner bleiben etwa 20 GB für Dungeons, also rund **acht** gleichzeitig — was
genau den acht Plätzen in `velocity.toml` entspricht.

## Mitarbeiten

Änderungen an Serverconfigs, Plugin-Configs und Plugin-Jars werden ganz normal
committet.

Nicht im Repository, weil zur Laufzeit erzeugt oder heruntergeladen:

| Pfad | Warum |
| --- | --- |
| `runtime/` | MariaDB und Redis, lädt `start.bat` selbst |
| `servers/*/*.jar`, `servers/*/plugins/*.jar` | Kopien aus `templates/` |
| `servers/*/config/`, `server.properties`, `eula.txt`, `bukkit.yml`, `spigot.yml` | dito; enthalten Forwarding-Secret und RCON-Passwort |
| `servers/mining-*/` | Dungeons, nach 24 h ohnehin weg |
| `proxy/forwarding.secret`, `runtime/rcon.secret` | Geheimnisse |
| `**/world/`, `**/logs/`, `**/cache/`, `**/libraries/`, `**/versions/` | Laufzeitdaten |
| `**/plugins/**/libs/`, `**/translations/` | laden die Plugins selbst |
| `**/plugins/**/*.db`, `*.mv.db` | lokale Dateidatenbanken; die echten Daten liegen in MariaDB |

Faustregel: was `start.bat`, `sync-servers.ps1`, Paper oder ein Plugin selbst
wiederherstellen kann, gehört nicht ins Repository.

## Historie

Bis September 2026 war das hier ein einzelner Paper-Server unter `server/`.
Daraus wurde ein Netzwerk aus Velocity plus vier Servern; mains Welt und
Configs sind dabei unverändert mitgewandert.

Zwischendurch stand CloudNet 4 als Orchestrierung dahinter. Es ist wieder
herausgeflogen: seine Stärken sind Cluster über mehrere Maschinen und
Autoscaling nach Spielerzahl, und beides braucht dieses Netzwerk nicht. Übrig
blieben die Nachteile — es gibt keine stabile 4.x, der Launcher aktualisiert
sich bei jedem Start aus dem *beta*-Zweig, er besteht auf genau Java 25, und er
schreibt die Weiterleitung bei jedem Start auf *legacy* zurück. Einen Dungeon
aus einer Vorlage zu kopieren und auf einem Port zu starten sind drei Zeilen
PowerShell; dafür braucht es keinen Orchestrator.

Davor hieß das Repository `Interconnect`, nach einem eigenen Plugin, das
MariaDB startete und Plugins aus versionierten Ordnern in den Server kopierte.
Beides erledigen jetzt `start.bat` und `sync-servers.ps1`.
