# server-terranova

Der Entwicklungsserver von Terranova. Das Repository **ist** der Server:
Plugin-Jars, Plugin-Configs, Serverconfig und der Starter liegen unter
`server/` und werden versioniert. Wer klont, hat einen lauffähigen Server.

## Starten

```
server\start.bat
```

Doppelklick genügt. Beim ersten Start lädt das Skript einmalig MariaDB
herunter (ca. 87 MB), richtet das Datenverzeichnis ein und legt die
Datenbanken an. Danach startet Paper.

Voraussetzung ist eine installierte Java-Laufzeit. Getestet mit Java 25
und Java 26.

## Was start.bat übernimmt

Die Datenbank läuft bewusst im Starter und nicht in einem Plugin. Paper
liest `server.properties` und die Plugins ihre eigenen Configs, bevor ein
Plugin überhaupt geladen werden könnte — nur so steht MariaDB schon beim
allerersten Start bereit.

1. MariaDB besorgen (einmalig) und das Datenverzeichnis initialisieren
2. MariaDB auf Port `13306` starten
3. Datenbanken und den Benutzer `minecraft` anlegen
4. Paper starten
5. MariaDB nach dem Beenden sauber herunterfahren

Port `13306` statt `3306`, damit eine lokal installierte MySQL/MariaDB
nicht kollidiert. Zugangsdaten für lokale Entwicklung: `minecraft` /
`minecraft`. Datenbankzugriff mit einem externen Client wie HeidiSQL oder
DBeaver auf `127.0.0.1:13306`.

## Eine Datenbank hinzufügen

Eine Zeile in `server/start.bat`:

```bat
set "DATABASES=network nations betonquest chatcontrol interactivechat luckperms proficisci"
```

Der Name muss zu dem passen, was das Plugin in seiner Config erwartet.

## Mitarbeiten

Änderungen an Plugin-Configs, Plugin-Jars und Serverconfig werden ganz
normal committet — sie liegen alle unter `server/`.

Nicht im Repository, weil zur Laufzeit erzeugt oder heruntergeladen:

| Pfad | Warum |
| --- | --- |
| `server/world*/` | Weltdaten, ständig in Bewegung |
| `server/mariadb/` | lädt `start.bat` selbst |
| `server/libraries/`, `server/cache/`, `server/versions/` | lädt Paper selbst |
| `server/logs/` | Laufzeitausgabe |
| `server/plugins/**/libs/`, `**/translations/` | laden die Plugins selbst |
| `server/plugins/**/*.db`, `*.mv.db` | lokale Dateidatenbanken; die echten Daten liegen in MariaDB |

Faustregel: was `start.bat`, Paper oder ein Plugin selbst wiederherstellen
kann, gehört nicht ins Repository.

## Historie

Das Repository hieß früher `Interconnect`, nach einem eigenen Plugin, das
MariaDB startete und Plugins aus versionierten Ordnern in den Server
kopierte. Beides erledigt jetzt `start.bat`, bevor die JVM läuft. Damit
entfielen das 244 MB große Shaded-Jar samt Git-LFS, der Gradle-Build und
der Zwang, den Server nach dem Einspielen neuer Jars zweimal zu starten.
