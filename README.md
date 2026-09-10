# server-terranova

Das Terranova-Netzwerk. Das Repository **ist** das Netzwerk: Plugin-Jars,
Plugin-Configs, Serverconfig, die CloudNet-Tasks und der Starter liegen hier
und werden versioniert. Wer klont, hat ein lauffähiges Netzwerk.

## Starten

```
start.bat
```

Doppelklick genügt. Beim ersten Start lädt das Skript einmalig MariaDB
(ca. 87 MB) und Redis (ca. 4 MB) herunter, richtet die Datenverzeichnisse ein,
legt die Datenbanken an und startet danach CloudNet. CloudNet fährt Proxy,
main, build und farm selbst hoch.

Voraussetzung ist **Java 25**. CloudNet 4.0.0-RC17 besteht darauf und
verweigert den Start unter Java 26. `start.bat` sucht sich selbst ein Java 25
(Adoptium, Corretto, `%USERPROFILE%\.jdks`); mit `TERRANOVA_JAVA25=<JDK-Pfad>`
lässt sich die Suche übergehen.

## Die Server

| Server | Art | Speicher | Was drauf liegt |
| --- | --- | --- | --- |
| `Proxy-1` | Velocity | 512 MB | der Eingang, Port 25565 |
| `main-1` | Paper | 4 GB | die gewachsene Welt: Nations, BetonQuest, Nexo, Pl3xMap, BountyfulSeas, Citizens, Proficisci |
| `build-1` | Paper | 2 GB | Bauserver |
| `farm-1` | Paper | 2 GB | Farmserver |
| `mining-N` | Paper | 2 GB | Dungeons, auf Zuruf erzeugt |

CloudNet hängt an jeden Dienstnamen eine Nummer. Es heißt also `main-1`, nicht
`main` — auch dort, wo nur ein Dienst läuft.

### Dungeons öffnen

In der CloudNet-Konsole:

```
create by mining 3 --start
```

Das legt `mining-1`, `mining-2` und `mining-3` an und startet sie. Die Zahl ist
frei; begrenzt wird nur durch den Arbeitsspeicher (siehe unten), nicht durch
die Konfiguration.

Ein Dungeon ist **24 Stunden offen**. Weil der `mining`-Task statisch ist,
überlebt seine Welt einen Neustart innerhalb dieser Zeit. Abgeräumt wird er von
`scripts\reap-mining.ps1`:

```
powershell -File scripts\reap-mining.ps1 -WhatIf    # zeigt nur an
powershell -File scripts\reap-mining.ps1            # löscht Abgelaufene
```

Gelöscht heißt: Verzeichnis weg. Das nächste `create by mining` legt es aus
`network\local\templates\mining\default` neu an, also mit frischer Welt. Ein
noch laufender Dungeon wird übersprungen — mit `-StopRunning` wird er vorher
über die REST-Schnittstelle gestoppt.

## Aufbau

```
start.bat                                MariaDB + Redis + CloudNet
scripts/
  sync-forwarding-secret.ps1             Velocity-Secret erzeugen und verteilen
  reap-mining.ps1                        abgelaufene Dungeons abräumen
network/
  launcher.jar  config.json              CloudNet
  local/
    tasks/*.json                         die fünf Tasks
    templates/
      Global/default/plugins/            Plugins, die jeder Server bekommt
      Backend/default/                   gemeinsame Servereinstellungen
      Proxy/default/                     Velocity
      mining/default/                    Dungeon-Vorlage
    services/
      main-1/  build-1/  farm-1/         die festen Server, versioniert
      mining-*/                          Dungeons, nicht versioniert
runtime/                                 MariaDB und Redis, nicht versioniert
```

### Wo ein Plugin hingehört

**`network/local/templates/Global/default/plugins/`** — alles, was auf jedem
Server laufen soll: TerranovaLib, LuckPerms, HuskSync, PlaceholderAPI samt
Expansions, Vault, TAB, ChatControl, InteractiveChat, packetevents,
FastAsyncWorldEdit, WorldGuard. Dieses Template wird bei **jedem** Start über
die statischen Dienste kopiert (`alwaysCopyToStaticServices`), ein neues Jar
wirkt also überall, ohne dass server-eigene Configs angefasst werden. Deshalb
liegen hier ausschließlich Jars und keine Configs.

**`network/local/services/main-1/plugins/`** — was an mains Welt und seine
Tabellen gebunden ist: Nations, Proficisci, PlayerActionAdapter, BountyfulSeas,
Nexo, Citizens, Pl3xMap, BetonQuest. Pl3xMap (Port 8080) und Nexos Packserver
(8082) binden feste Ports und können ohnehin nur einmal laufen.

**`network/local/templates/mining/default/plugins/`** — bewusst dünn, damit ein
Dungeon schnell startet. Hier landet BountyfulMining aus dem
Schwesterrepository:

```
cd ..\BountyfulMining
gradle deployToTestServer
```

## Eine Datenbank hinzufügen

Eine Zeile in `start.bat`:

```bat
set "DATABASES=nations betonquest chatcontrol interactivechat luckperms proficisci bountyfulseas husksync"
```

Der Name muss zu dem passen, was das Plugin in seiner Config erwartet. Die
Zuordnung steht als Kommentar direkt darüber.

Zugangsdaten für lokale Entwicklung: `minecraft` / `minecraft` auf
`127.0.0.1:13306`. Redis läuft ohne Passwort auf `127.0.0.1:6379`.

## Weiterleitung und das Forwarding-Secret

CloudNet richtet die Weiterleitung **selbst** ein: es schreibt `server-ip`,
`server-port` und `online-mode=false` in die `server.properties` jedes Backends
und benutzt dabei standardmäßig *legacy forwarding* — den alten
BungeeCord-Weg. Die von CloudNet erzeugte `velocity.toml` steht entsprechend
auf `player-info-forwarding-mode = "legacy"`, und `paper-global.yml` wird bei
jedem Start wieder auf `proxies.velocity.enabled: false` gesetzt.

`scripts\sync-forwarding-secret.ps1` pflegt deshalb nur die Secret-Datei unter
`network\local	emplates\Proxy\defaultorwarding.secret`, die in
`.gitignore` steht. In die Server-Configs schreibt es nichts — dagegen
anzuschreiben würde beim nächsten Start ohnehin zurückgesetzt und hinterließe
in der Zwischenzeit ein Geheimnis in einer versionierten Datei.

Solange alle Dienste auf `127.0.0.1` gebunden sind, ist das vertretbar: die
Backends glauben zwar jedem, der sie erreicht, aber erreichen kann sie nur, wer
schon auf der Maschine ist. **Sobald ein Backend darüber hinaus erreichbar
wird, gilt das nicht mehr** — dann auf modern forwarding umstellen:

1. in `network/local/tasks/*.json` der Backends `"disableIpRewrite": true`
   setzen, damit CloudNet `paper-global.yml` in Ruhe lässt
2. in der `velocity.toml` des Proxy-Templates
   `player-info-forwarding-mode = "modern"`
3. in `paper-global.yml` jedes Backends `proxies.velocity.enabled: true` und
   `secret` auf den Inhalt von `forwarding.secret`

Das ist bewusst nicht vorkonfiguriert: es will einmal von Hand mit einem echten
Verbindungsversuch geprüft werden.

## Speicher

32 GB stehen zur Verfügung, CloudNet darf 24 GB davon vergeben
(`maxMemory` in `network/config.json`) — der Rest bleibt für MariaDB, Redis und
Windows. Proxy 0,5 + main 4 + build 2 + farm 2 macht 8,5 GB Grundlast, also
rund **sieben Dungeons** gleichzeitig. `create by mining 20` würde die Maschine
ins Swappen treiben; eine Obergrenze steht bewusst nicht in der Konfiguration
(`maxServiceCount: -1`).

## Mitarbeiten

Änderungen an Plugin-Configs, Plugin-Jars, Serverconfig und den Tasks werden
ganz normal committet.

Nicht im Repository, weil zur Laufzeit erzeugt oder heruntergeladen:

| Pfad | Warum |
| --- | --- |
| `runtime/` | MariaDB und Redis, lädt `start.bat` selbst |
| `network/launcher/`, `network/modules/`, `network/temp/` | lädt CloudNet selbst |
| `network/local/services/mining-*/` | Dungeons, nach 24 h ohnehin weg |
| `network/.../forwarding.secret` | Geheimnis |
| `**/world/`, `**/logs/`, `**/cache/`, `**/libraries/`, `**/versions/` | Laufzeitdaten |
| `**/plugins/**/libs/`, `**/translations/` | laden die Plugins selbst |
| `**/plugins/**/*.db`, `*.mv.db` | lokale Dateidatenbanken; die echten Daten liegen in MariaDB |

Faustregel: was `start.bat`, CloudNet, Paper oder ein Plugin selbst
wiederherstellen kann, gehört nicht ins Repository.

## Historie

Bis September 2026 war das hier ein einzelner Paper-Server unter `server/`,
gestartet von `server/start.bat`. Der Baum liegt jetzt unter
`network/local/services/main-1/` und ist derselbe geblieben — die Umstellung
war eine reine Umbenennung.

Davor hieß das Repository `Interconnect`, nach einem eigenen Plugin, das
MariaDB startete und Plugins aus versionierten Ordnern in den Server kopierte.
Beides erledigt jetzt `start.bat` bzw. CloudNet.
