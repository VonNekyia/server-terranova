# server-terranova

Das Terranova-Netzwerk. Das Repository **ist** das Netzwerk: Serverconfigs,
Plugin-Configs, Plugin-Jars, die Vorlagen und das Programm, das alles startet,
liegen hier und werden versioniert. Wer klont, hat ein lauffähiges Netzwerk.

## Starten

```
start.bat
```

Doppelklick genügt. Beim ersten Start lädt Terranova einmalig MariaDB
(ca. 87 MB) und Redis (ca. 4 MB) herunter, prüft ihre Prüfsummen, richtet die
Datenverzeichnisse ein und legt die Datenbanken an. Danach bestückt es die
Server aus `templates/` und fährt Proxy, main, build und farm hoch.

Das Fenster ist nur **Zuschauer**: es zeigt, was der Supervisor tut. **Strg+C**
fährt das Netzwerk sauber herunter; noch einmal Strg+C schließt nur das Fenster
und lässt das Netzwerk weiterlaufen. Was beim Schließen des Fensters passiert,
steht in `terranova.yml` unter `dashboard.on_window_close`.

Die Server selbst haben **kein eigenes Fenster** mehr. An ihre Konsole kommt
man mit `terranova console <name>` oder über das Dashboard — von überall, auch
aus einem zweiten Terminal.

Voraussetzung ist eine Java-Laufzeit. Getestet mit Java 25 und 26. Mit
`TERRANOVA_JAVA=<Pfad zu java.exe>` lässt sich eine bestimmte erzwingen.

### Linux

```
./start.sh
```

oder gleich `bin/terranova start`. Dieselben Befehle, dieselbe
`terranova.yml`, derselbe Supervisor — nur darunter arbeitet eine andere
Prozessverwaltung: statt der Windows-API liest Terranova dort `/proc`, und
zum Stoppen gibt es mit `SIGTERM` einen Weg, den es unter Windows nicht gibt.

Anders als unter Windows lädt Terranova hier **nichts** herunter. MariaDB und
Redis kommen aus der Distribution:

```
sudo apt install mariadb-server redis-server
```

Fehlt eines von beiden, sagt Terranova beim Start, wie es hereinkommt. Das
Datenverzeichnis unter `runtime/` gehört trotzdem Terranova; eine
Systeminstanz auf 3306 bleibt unberührt.

Unter `bin/` liegt nur die Windows-Programmdatei — eine je System einzuchecken
hieße, sie bei jeder Änderung doppelt zu pflegen. `start.sh` baut die
Linux-Fassung deshalb beim ersten Mal selbst, sofern Rust installiert ist.
Fertige Binaries für x86_64 und aarch64 fallen ansonsten in der CI an.

### Nur das Nötigste

```
start_minimal.bat
```

Datenbanken, Proxy und `main` — sonst nichts. `build`, `farm` und die Dungeons
bleiben aus; zusammen belegen die rund acht Gigabyte, und auf einem knappen
Rechner ist das der Unterschied zwischen spielbar und nicht. Dahinter steckt
kein zweiter Startweg, nur `terranova start main`: jeder Servername hinter
`start` grenzt ein, was hochfährt.

Nachziehen geht jederzeit und ohne Neustart des Netzwerks:

```
terranova restart build
```

Der tägliche Neustart um vier rührt nur an, was auch läuft — aus einem
schmalen Start wird über Nacht also kein vollständiger.

## Die Server

| Server | Port | Speicher | Was drauf liegt |
| --- | --- | --- | --- |
| Proxy | **25565** | 512 MB | Velocity, der Eingang |
| `main` | 25566 | 4 GB | die gewachsene Welt: Nations, BetonQuest, Nexo, Pl3xMap, BountyfulSeas, Citizens, Proficisci |
| `build` | 25567 | 2 GB | Bauserver, Kreativ, flache Welt |
| `farm` | 25568 | 2 GB | Farmserver |
| `mining-1` … `mining-8` | 25571 … | 2 GB | Dungeons, auf Zuruf geöffnet, im Proxy zur Laufzeit eingetragen |

Nur der Proxy ist von außen erreichbar. Die Server binden auf `127.0.0.1` —
wer direkt auf 25566 will, müsste schon auf der Maschine sein.

## Befehle

```
terranova start [--detach]     hochfahren (ohne --detach: zusehen)
terranova start <name...>      nur diese Server hochfahren
terranova stop [name...]       alles oder einzelne Knoten herunterfahren
terranova restart <name>       stoppen, bestücken, starten
terranova status               was läuft
terranova console <name>       Konsole mitlesen und Befehle eintippen
terranova logs <name> [-n N]   die letzten Zeilen
terranova cmd <name> <befehl>  einen Befehl schicken
terranova dashboard            Oberfläche im Browser öffnen

terranova mine open [anzahl]   Dungeons öffnen (--slot N für einen bestimmten)
terranova mine close <n>       schließen
terranova mine list            was offen ist
terranova mine reap            abgelaufene abräumen (--dry-run zeigt nur)

terranova sync [name...]       Server aus templates/ bestücken
terranova doctor               prüfen, ob alles startklar ist
```

`bin\terranova.exe` liegt im Repository; wer es oft braucht, legt den Ordner in
den PATH. Ohne `--root` sucht Terranova die `terranova.yml` selbst — über `bin\`
oder vom Arbeitsverzeichnis aufwärts.

### Dungeons

```
terranova mine open 3
```

Ein Dungeon ist eine Kopie einer Vorlage auf einem eigenen Port — welcher,
sagt `--template`; `terranova mine templates` zeigt, was zur Wahl steht. Womit
er angelegt wurde, merkt er sich, und dabei bleibt es auch beim Bestücken.

Den Eintrag in `proxy\velocity.toml` legt Terranova selbst an und lässt den
Proxy mit `velocity reload` nachladen — Velocity meldet Server zur Laufzeit an
und ab. In der Datei gehört ihm nur der Block zwischen den beiden
Markierungszeilen; alles andere darin bleibt unangetastet. Wer `mines.slots`
ändert, muss dort also nichts nachziehen.

Ein Dungeon läuft, solange es ihn gibt: ein Neustart des Netzwerks bringt ihn
zurück, ein Absturz auch (`mines.resume`). Von selbst angelegt wird trotzdem
keiner — nur `mine open` legt einen an. Nach **24 Stunden** ist er weg; der
Supervisor räumt ihn im Takt von `schedule.reap_every` ab und fährt ihn dafür
herunter, auch wenn gerade jemand darin steht. Wer drin ist, landet auf `main`.

`Schließen` beendet ihn vorzeitig: er bleibt unten, der Proxy meldet ihn ab,
und die Uhr fängt von vorn an — nach weiteren 24 Stunden ist er weg. Bis dahin
holt `mine open --slot N` ihn samt seiner Welt zurück; im Dashboard heißt der
Knopf `Reopen`. Wer ihn sofort loswerden will, nimmt `Reaper` beziehungsweise
`mine reap --slot N` — danach ist die Welt weg und der Platz frei.

### Eine Welt in der Vorlage

Was in `templates/<name>/` liegt, wird beim **Anlegen** eines Dungeons einmal
kopiert — ein `world/` darin also auch. Kopiert wird aber nur, wenn das
Verzeichnis noch nicht existiert: ein Dungeon behält seine Welt, sonst wäre ein
Neustart des Netzwerks eine Landkarte weiter.

Wer die Vorlagenwelt ändert, sieht davon in einem **bestehenden** Dungeon
deshalb nichts. Erst räumen, dann öffnen:

```
terranova mine reap --slot 1
terranova mine open --slot 1
```

Worlds unter `templates/` sind nicht versioniert (`templates/*/world/` steht in
`.gitignore`). Auf einem anderen Klon fehlt eine Vorlagenwelt also — wer sie
teilen will, muss die Regel dort streichen.

Gelöscht heißt: Verzeichnis weg, das nächste `open` legt eine frische Welt an.
`terranova mine reap` von Hand überspringt einen laufenden Dungeon —
`--stop-running` beendet ihn vorher sauber. Der Supervisor und der Knopf im
Dashboard tun das von sich aus, sonst käme nie einer an die Reihe. Abgeräumt wird über `servers_dynamic\.trash`: erst umbenennen, dann
löschen — das Umbenennen scheitert, solange jemand Dateien offen hält, also
kann kein halb gelöschter Dungeon entstehen.

## Aufbau

```
start.bat                   ruft nur bin\terranova.exe start auf
start.sh                    dasselbe unter Linux
bin/terranova.exe           das Programm, versioniert
terranova.yml               was läuft, mit wie viel Speicher, auf welchem Port
terranova/                  sein Quelltext (Rust)
  src/win.rs                Prozesse, Ports, Zeit — über die Windows-API
  src/unix.rs               dasselbe über /proc und Signale
  src/docker.rs             dasselbe über Container
proxy/
  velocity.toml             versioniert
  velocity-*.jar            versioniert
  forwarding.secret         nicht versioniert
servers/                    die festen Server, vollstaendig versioniert
  main/  build/  farm/      Paper, Plugin-Jars, Configs - was da liegt, laeuft
    server.properties.dist  Quelle; die fertige Datei traegt das RCON-Passwort
    config/paper-global.yml.dist   Quelle; die fertige traegt das Secret
servers_dynamic/            die Dungeons, nicht versioniert
  mining-*/                 entstehen aus templates/, nach 24 h weg
templates/                  nur noch fuer Dungeons
  common/                   Paper, gemeinsame Configs, gemeinsame Plugin-Jars
  mining/                   die Dungeon-Vorlage
runtime/                    MariaDB, Redis, Geheimnisse, Zustand — nichts davon versioniert
```

### Wo ein Plugin hingehört

Bei einem **festen Server** dorthin, wo es laufen soll: `servers/main/plugins/`.
Das Jar wird committet, und damit ist es überall dort, wo das Repository ist.
Terranova kopiert nichts in einen festen Server hinein — was im Verzeichnis
liegt, ist was läuft. Wer ein Plugin aktualisiert, ersetzt die Datei und
committet sie.

Dass dasselbe Jar dann in `main`, `build` und `farm` liegt, kostet im
Repository nichts: Git speichert nach Inhalt, drei gleiche Dateien sind ein
Objekt. Auf der Platte lagen sie ohnehin schon dreimal.

Bei einem **Dungeon** dagegen weiter unter `templates/` — er entsteht ja bei
jedem Öffnen neu.

**`templates/common/plugins/`** — was in jedem Dungeon laufen soll, heute:
TerranovaLib, LuckPerms, HuskSync, PlaceholderAPI samt Expansions, Vault, TAB,
ChatControl, InteractiveChat, packetevents, FastAsyncWorldEdit, WorldGuard.

**`servers/main/plugins/`** — was an mains Welt und seinen Tabellen hängt:
Nations, Proficisci, PlayerActionAdapter, BountyfulSeas, Nexo, Citizens,
Pl3xMap, BetonQuest. Pl3xMap (Port 8080) und Nexos Packserver (8082) binden
feste Ports und können ohnehin nur einmal laufen.

**`templates/mining/plugins/`** — bewusst dünn, damit ein Dungeon schnell
startet. Hier landet BountyfulMining aus dem Schwesterrepository:

```
cd ..\BountyfulMining
gradle deployToTestServer
```

Was Terranova in einen Dungeon kopiert hat, steht dort in
`.terranova-sync.json`. Fällt ein Jar aus der Vorlage weg, verschwindet die
Kopie — sonst lägen nach einem Plugin-Update die alte und die neue Fassung
nebeneinander.

### Die zwei Dateien, die nicht ins Repository gehören

In `server.properties` steht das RCON-Passwort, in `config/paper-global.yml`
das Forwarding-Secret. Beide entstehen bei jedem Start neu — aus
`server.properties.dist` und `config/paper-global.yml.dist` daneben, die
versioniert sind. Wer den MOTD oder eine Paper-Einstellung dauerhaft ändern
will, ändert die `.dist`-Datei; Port, RCON-Port und das Geheimnis setzt
Terranova beim Schreiben selbst.

## Eine Datenbank hinzufügen

Eine Zeile in `terranova.yml`:

```yaml
deps:
  mariadb:
    databases: [nations, betonquest, ..., husksync]
```

Der Name muss zu dem passen, was das Plugin in seiner Config erwartet — die
Zuordnung steht als Kommentar direkt darüber. Beim nächsten Start wird sie
angelegt.

Zugangsdaten für lokale Entwicklung: `minecraft` / `minecraft` auf
`127.0.0.1:13306`. Redis läuft ohne Passwort auf `127.0.0.1:6379`. Beide binden
nur auf `127.0.0.1`, und der Benutzer hat Rechte je Datenbank statt auf alles.

## Weiterleitung

Velocity läuft mit **modern forwarding**: der Proxy prüft gegen Mojang und
reicht UUID und Skin signiert weiter, das Backend prüft die Signatur gegen
`proxy\forwarding.secret`. Die Server laufen deshalb auf `online-mode=false` —
das Secret ist ihr einziger Schutz, und darum steht es nicht im Repository.
Terranova erzeugt es beim ersten Start und trägt es in jede
`config\paper-global.yml` ein.

## Sauber stoppen

Gestoppt wird über die Konsole des Servers, nicht über das Betriebssystem:
Terranova schreibt `stop` in seine Eingabe und wartet. Erst wenn das nicht
ankommt, geht es über RCON; erst danach hart — und das steht dann als `FEHLER`
im Protokoll, denn dabei gehen ungespeicherte Chunks verloren.

Ein sauberer Stopp hinterlässt im Serverlog `All chunks are saved` und
`All RegionFile I/O tasks to complete`.

RCON läuft je Server auf Port + 100 (main 25666, build 25667, farm 25668,
Dungeons 25671+), nur auf `127.0.0.1`; das Passwort steht in
`runtime\rcon.secret`.

Unter Linux kommt vor dem harten Abschuss noch ein Schritt dazu: ein `SIGTERM`.
Die JVM behandelt es über ihre Abschalthaken, Paper speichert die Welt und
beendet sich selbst — das klappt auch dann noch, wenn die Konsole schon nicht
mehr annimmt. Unter Windows gibt es dieses Mittel nicht: eine JVM hat dort kein
Fenster mit Nachrichtenschleife, `CloseMainWindow` läuft ins Leere, und
`taskkill` ohne `/F` verweigert den Dienst.

### Wenn der Supervisor abstürzt

Die Server laufen weiter. Das ist Absicht: sie mit ihm sterben zu lassen hieße,
jeden Absturz in einen gleichzeitigen harten Abschuss von main, den Dungeons
und MariaDB zu verwandeln.

Der nächste `terranova start` **übernimmt** sie: er erkennt sie am Port, prüft
PID, Erzeugungszeit und Programmdatei, redet dann über RCON mit ihnen und liest
ihre Konsole aus `logs/latest.log` mit. `terranova status` zeigt sie als
*übernommen*. Der nächste Neustart macht sie wieder zu eigenen.

Sitzt auf einem Port etwas Fremdes, wird es weder angefasst noch beendet — der
Knoten steht dann auf `conflict`.

### Täglicher Neustart

Macht der Supervisor selbst, laut `schedule.daily_restart` um 04:00: eine
Ansage in den Chat, dann nacheinander mit zwei Minuten Abstand stoppen,
bestücken und wieder starten. Dungeons bleiben unberührt. Eine geplante Aufgabe
in Windows braucht es dafür nicht mehr — falls noch eine von früher existiert,
warnt `terranova doctor` davor.

## Dashboard

```
terranova dashboard
```

Öffnet die Oberfläche im Browser: Zustand, Start und Stopp, Live-Konsole mit
Eingabe, Dungeons. Sie steckt in der Programmdatei und benutzt ausschließlich
dieselbe Schnittstelle wie die CLI — sie kann also nichts, was die CLI nicht
auch kann.

Ein Lesezeichen auf <http://127.0.0.1:25590/> tut es genauso — `terranova
dashboard` öffnet nur den Browser.

Die Schnittstelle hört nur auf `127.0.0.1` und verlangt ein Token aus
`runtime\terranova\api.token`. Die CLI schickt es als Kopfzeile, der Browser
bekommt beim Abruf der Seite ein Sitzungsplätzchen (`HttpOnly`,
`SameSite=Strict`, 30 Tage). Das Token steht nie in der Adresszeile. Wer die
Seite bekommt, sitzt an diesem Rechner: der `Host`-Kopf muss `127.0.0.1` oder
`localhost` sein, eine fremde Webseite kann die Antwort nicht lesen, und ihre
eigenen Anfragen tragen das Plätzchen wegen `SameSite=Strict` nicht mit.

## Website und Karte

```
terranova status
```

zeigt unter **Web** beides:

| | |
| --- | --- |
| `website` | die öffentliche Seite aus `website/`, ausgeliefert auf **8081** |
| `karte` | Pl3xMap in `main` auf **8080** |

Die Seite in `website/` ist ein fertiger Build ohne eigenen Server. Terranova
liefert sie deshalb selbst aus — daneben noch nginx zu pflegen wäre ein
zweites Ding, das jemand starten, aktuell halten und überwachen müsste. Pfade
wie `/impressum` gibt es nur im Browser: was keine Dateiendung hat und nicht
existiert, bekommt `index.html` (eine fehlende `.png` bleibt 404). Alles unter
`static/` trägt seinen Inhalt im Namen und wird als unveränderlich
ausgeliefert, der Rest über ETag — ohne das ginge das acht Megabyte große
Hintergrundbild bei jedem Seitenwechsel neu über die Leitung.

Voreingestellt ist `bind: 127.0.0.1`, also **nur dieser Rechner**. Für den
öffentlichen Betrieb steht in `terranova.yml` unter `web.site.bind` ein
`0.0.0.0` — davor gehört dann etwas, das TLS spricht und Last abfängt; dieser
Server kann beides nicht, und `terranova doctor` sagt das auch.

Die Karte gehört uns nicht: Pl3xMap bringt seinen eigenen Webserver mit und
läuft **im Prozess von `main`**. Terranova kann sie also nicht starten oder
stoppen, nur nachsehen — und dafür drei Zustände unterscheiden, die sonst
alle gleich aussehen:

| Zustand | Heißt |
| --- | --- |
| `ready` | antwortet, mit der Zahl der gerenderten Welten |
| `waiting` | `main` läuft (noch) nicht — kein Fehler |
| `down` | `main` läuft, aber auf 8080 antwortet niemand |

`terranova doctor` vergleicht außerdem `web.map.port` mit dem, was in
`servers/main/plugins/Pl3xMap/config.yml` unter `internal-webserver` steht.
Gehen die auseinander, zeigt das Dashboard sonst eine Karte als tot an, die
läuft.

## Docker

`terranova.yml` kennt `runtime: auto | native | docker`. `auto` heißt immer
`native` — unter Windows wie unter Linux. Docker wird nie gewählt, nur weil es
installiert ist: wer native Prozesse erwartet, soll nicht plötzlich Container
bekommen.

Für `runtime: docker` muss in Docker Desktop **host networking** eingeschaltet
sein (Settings → Resources → Network → Enable host networking → Apply &
restart). Ohne das landen die Container in einem eigenen Netz, in dem
`127.0.0.1:13306` auf den Container selbst zeigt, und keine Datenbank­verbindung
eines Plugins käme an. `terranova doctor` prüft das mit einem echten Container,
bevor irgendetwas startet.

Zweiter Punkt: die WSL2-Maschine bekommt ohne Zutun die Hälfte des
Arbeitsspeichers. Für mehr als etwa zwei Dungeons braucht es in
`%USERPROFILE%\.wslconfig`

```ini
[wsl2]
memory=28GB
```

und danach `wsl --shutdown`. Auch das rechnet `doctor` vor.

## Speicher

Proxy 0,5 + main 4 + build 2 + farm 2 macht 8,5 GB Grundlast. Bei 32 GB im
Rechner bleiben etwa 20 GB für Dungeons, also rund **acht** gleichzeitig — die
Obergrenze setzt `mines.slots` in `terranova.yml`.

## Mitarbeiten

Änderungen an Serverconfigs, Plugin-Configs, Plugin-Jars und `terranova.yml`
werden ganz normal committet. Wer am Quelltext arbeitet:

```
cd terranova
cargo test
cargo build --release
copy target\release\terranova.exe ..\bin\
```

Die gebaute Datei gehört mit ins Repository — wie die Jars auch, damit ein
Klon ohne Rust-Werkzeug lauffähig ist.

Nicht im Repository, weil zur Laufzeit erzeugt oder heruntergeladen:

| Pfad | Warum |
| --- | --- |
| `runtime/` | MariaDB, Redis, Geheimnisse, Zustand |
| `servers/*/server.properties` | enthält das RCON-Passwort; entsteht aus `.dist` |
| `servers/*/config/paper-global.yml` | enthält das Forwarding-Secret; entsteht aus `.dist` |
| `servers_dynamic/` | Dungeons, nach 24 h ohnehin weg |
| `proxy/forwarding.secret` | Geheimnis |
| `**/world/`, `**/logs/`, `**/cache/`, `**/libraries/`, `**/versions/` | Laufzeitdaten |
| `terranova/target/` | Bauverzeichnis |

Faustregel: was Terranova, Paper oder ein Plugin selbst wiederherstellen kann,
gehört nicht ins Repository.

## Historie

Bis September 2026 war das hier ein einzelner Paper-Server unter `server/`.
Daraus wurde ein Netzwerk aus Velocity plus vier Servern; mains Welt und
Configs sind unverändert mitgewandert.

Zwischendurch stand **CloudNet 4** als Orchestrierung dahinter. Es ist wieder
herausgeflogen: seine Stärken sind Cluster über mehrere Maschinen und
Autoscaling nach Spielerzahl, und beides braucht dieses Netzwerk nicht. Übrig
blieben die Nachteile — keine stabile 4.x, ein Launcher, der sich bei jedem
Start aus dem *beta*-Zweig aktualisiert, die Forderung nach genau Java 25, und
eine Weiterleitung, die bei jedem Start auf *legacy* zurückgeschrieben wurde.

Danach liefen sieben PowerShell-Skripte. Die taten es, aber die Logik steckte
in Windows-Skripten, jeder Server brauchte ein eigenes sichtbares Fenster, und
sauber herunterfahren ging nur über einen Umweg: die JVM hat kein Fenster mit
Nachrichtenschleife, `CloseMainWindow()` läuft ins Leere und `taskkill` ohne
`/F` verweigert den Dienst. Jetzt besitzt Terranova die Konsolen selbst, und
`stop` ist wieder das, was es sein sollte.

Davor hieß das Repository `Interconnect`, nach einem eigenen Plugin, das
MariaDB startete und Plugins aus versionierten Ordnern in den Server kopierte.
