@echo off
setlocal enabledelayedexpansion
cd /d "%~dp0"

rem ---------------------------------------------------------------------------
rem Startet das Terranova-Netzwerk: MariaDB, Redis und danach CloudNet.
rem Doppelklick genuegt. Der erste Start laedt MariaDB und Redis einmalig
rem herunter.
rem
rem Die Datenbanken laufen bewusst hier und nicht in einem Plugin: Paper liest
rem server.properties und die Plugins ihre Configs, bevor ein Plugin ueberhaupt
rem laden koennte. CloudNet startet die Dienste, sobald es selbst oben ist -
rem beide Datenbanken muessen also schon vorher stehen.
rem ---------------------------------------------------------------------------

rem --- MariaDB ---------------------------------------------------------------
set "MARIADB_VERSION=11.4.5"
set "MARIADB_HOME=runtime\mariadb"
set "MARIADB_BASE=%MARIADB_HOME%\mariadb-%MARIADB_VERSION%-winx64"
set "MARIADB_DATA=%CD%\%MARIADB_HOME%\data"
set "MARIADB_PORT=13306"
set "DB_USER=minecraft"
set "DB_PASS=minecraft"
set "MYSQLD=%MARIADB_BASE%\bin\mysqld.exe"
set "MYSQL=%MARIADB_BASE%\bin\mysql.exe"

rem Datenbanken, die angelegt werden. Wer ein Plugin mit eigener Datenbank
rem ergaenzt, traegt den Namen hier ein - sonst startet es mit
rem "Unknown database". Der Name muss zu dem passen, was das Plugin in
rem seiner Config erwartet:
rem   nations          Nations/config.yml -> name   (auch PlayerActionAdapter)
rem   betonquest       BetonQuest/config.yml -> base
rem   chatcontrol      ChatControl/database.yml -> Database
rem   interactivechat  InteractiveChat/storage.yml -> Database
rem   luckperms        LuckPerms/config.yml -> database
rem   proficisci       Proficisci/config.yml -> database
rem   bountyfulseas    BountyfulSeas/config.yml -> name
rem   husksync         HuskSync/config.yml -> database.credentials.database
rem
rem Die frueher angelegte Datenbank "network" ist entfallen. Sie stammte aus
rem dem abgeloesten Interconnect-Plugin und blieb immer leer; die vorhandene
rem leere Datenbank kann von Hand geloescht werden.
set "DATABASES=nations betonquest chatcontrol interactivechat luckperms proficisci bountyfulseas husksync"

rem --- Redis -----------------------------------------------------------------
rem HuskSync braucht neben MariaDB zwingend einen Redis ab 5.0. Dort liegen die
rem Spielerdaten waehrend eines Serverwechsels und die Pub/Sub-Nachrichten.
rem Offiziell gibt es Redis fuer Windows nicht; verwendet wird der native Port
rem von zkteco-home. Bewusst auf den Commit von 8.10.1 festgenagelt und nicht
rem auf master: lose Dateien am Zweigende sind kein stabiles Artefakt.
rem
rem Ohne Persistenz (--save ""). Der massgebliche Speicher ist MariaDB, Redis
rem ist nur Zwischenlage - so kann auch keine kaputte RDB-Datei den Start
rem blockieren.
set "REDIS_HOME=runtime\redis"
set "REDIS_PORT=6379"
set "REDIS_COMMIT=794a883a083a317bc23fbbabc83a3c54abab0799"
set "REDIS_RAW=https://raw.githubusercontent.com/zkteco-home/redis-windows/%REDIS_COMMIT%"
set "REDIS_SERVER=%REDIS_HOME%\redis-server.exe"
set "REDIS_CLI=%REDIS_HOME%\redis-cli.exe"

rem --- CloudNet --------------------------------------------------------------
rem Der Launcher laedt CloudNet selbst nach und startet den Knoten. Der
rem Speicher hier gilt nur fuer den Launcher, nicht fuer den Knoten und erst
rem recht nicht fuer die Server - das steht in network\launcher.cnl bzw. in
rem den Tasks unter network\tasks.
set "CLOUDNET_HOME=network"

rem --- 1. MariaDB einmalig besorgen ------------------------------------------
if not exist "%MYSQLD%" (
    echo [start] MariaDB %MARIADB_VERSION% wird einmalig heruntergeladen ^(ca. 87 MB^)...
    if not exist "%MARIADB_HOME%" mkdir "%MARIADB_HOME%"
    curl -L --fail -o "%MARIADB_HOME%\mariadb.zip" "https://archive.mariadb.org/mariadb-%MARIADB_VERSION%/winx64-packages/mariadb-%MARIADB_VERSION%-winx64.zip"
    if errorlevel 1 (
        echo [start] FEHLER: Download fehlgeschlagen. Internetverbindung pruefen.
        pause
        exit /b 1
    )
    echo [start] Wird entpackt...
    tar -xf "%MARIADB_HOME%\mariadb.zip" -C "%MARIADB_HOME%"
    del "%MARIADB_HOME%\mariadb.zip"
)
if not exist "%MYSQLD%" (
    echo [start] FEHLER: MariaDB nicht gefunden unter %MYSQLD%
    pause
    exit /b 1
)

rem --- 2. Redis einmalig besorgen --------------------------------------------
if not exist "%REDIS_SERVER%" (
    echo [start] Redis wird einmalig heruntergeladen ^(ca. 4 MB^)...
    if not exist "%REDIS_HOME%" mkdir "%REDIS_HOME%"
    curl -L --fail -o "%REDIS_SERVER%" "%REDIS_RAW%/redis-server.exe"
    if errorlevel 1 (
        echo [start] FEHLER: Redis-Download fehlgeschlagen. Internetverbindung pruefen.
        pause
        exit /b 1
    )
    curl -L --fail -o "%REDIS_CLI%" "%REDIS_RAW%/redis-cli.exe"
    if errorlevel 1 (
        echo [start] FEHLER: Redis-Download fehlgeschlagen. Internetverbindung pruefen.
        pause
        exit /b 1
    )
)

rem --- 3. Datenverzeichnis initialisieren ------------------------------------
if not exist "%MARIADB_DATA%\mysql" (
    echo [start] Datenverzeichnis wird initialisiert...
    "%MARIADB_BASE%\bin\mysql_install_db.exe" --datadir="%MARIADB_DATA%"
)

rem --- 4. Reste eines abgestuerzten Laufs beenden ----------------------------
rem Kein taskkill mehr auf mysqld.exe: auf dieser Maschine laeuft daneben ein
rem separat installierter MariaDB-Dienst auf Port 3306, den ein pauschales
rem taskkill mit erwischt haette. Stattdessen gezielt die eigene Instanz auf
rem Port %MARIADB_PORT% ansprechen.
"%MYSQL%" -h 127.0.0.1 -P %MARIADB_PORT% -u root --protocol=tcp -e "SELECT 1" >nul 2>&1
if not errorlevel 1 (
    echo [start] Eine MariaDB auf Port %MARIADB_PORT% laeuft noch, wird beendet...
    "%MYSQL%" -h 127.0.0.1 -P %MARIADB_PORT% -u root --protocol=tcp -e "SHUTDOWN" >nul 2>&1
    ping -n 4 127.0.0.1 >nul
)
if exist "%REDIS_CLI%" "%REDIS_CLI%" -h 127.0.0.1 -p %REDIS_PORT% shutdown nosave >nul 2>&1

rem --- 5. MariaDB starten ----------------------------------------------------
echo [start] MariaDB wird gestartet ^(Port %MARIADB_PORT%^)...
start "MariaDB" /B "%MYSQLD%" --no-defaults --console --port=%MARIADB_PORT% --datadir="%MARIADB_DATA%" --max_allowed_packet=64M

set /a WAITED=0
:waitdb
"%MYSQL%" -h 127.0.0.1 -P %MARIADB_PORT% -u root --protocol=tcp -e "SELECT 1" >nul 2>&1
if not errorlevel 1 goto dbready
set /a WAITED+=1
if %WAITED% GEQ 60 (
    echo [start] FEHLER: MariaDB antwortet nicht.
    pause
    exit /b 1
)
ping -n 2 127.0.0.1 >nul
goto waitdb
:dbready
echo [start] MariaDB laeuft.

rem --- 6. Datenbanken und Benutzer sicherstellen ------------------------------
for %%D in (%DATABASES%) do (
    "%MYSQL%" -h 127.0.0.1 -P %MARIADB_PORT% -u root --protocol=tcp -e "CREATE DATABASE IF NOT EXISTS `%%D` CHARACTER SET utf8mb4 COLLATE utf8mb4_unicode_ci;" >nul 2>&1
)

rem Alle Dienste laufen auf dieser Maschine, deshalb reichen localhost und
rem 127.0.0.1 - MySQL behandelt beide als verschiedene Hosts, es braucht also
rem beide Eintraege. Frueher stand hier zusaetzlich '%%'@ALL PRIVILEGES ON *.*
rem WITH GRANT OPTION; fuer einen Einzelserver war das egal, fuer ein Netzwerk
rem mit mehreren verbindenden Diensten ist es das nicht mehr.
for %%H in (localhost 127.0.0.1) do (
    "%MYSQL%" -h 127.0.0.1 -P %MARIADB_PORT% -u root --protocol=tcp -e "CREATE USER IF NOT EXISTS '%DB_USER%'@'%%H' IDENTIFIED BY '%DB_PASS%';" >nul 2>&1
    for %%D in (%DATABASES%) do (
        "%MYSQL%" -h 127.0.0.1 -P %MARIADB_PORT% -u root --protocol=tcp -e "GRANT ALL PRIVILEGES ON `%%D`.* TO '%DB_USER%'@'%%H';" >nul 2>&1
    )
)
"%MYSQL%" -h 127.0.0.1 -P %MARIADB_PORT% -u root --protocol=tcp -e "FLUSH PRIVILEGES;" >nul 2>&1
echo [start] Datenbanken bereit.

rem --- 7. Redis starten ------------------------------------------------------
echo [start] Redis wird gestartet ^(Port %REDIS_PORT%^)...
start "Redis" /B "%REDIS_SERVER%" --port %REDIS_PORT% --bind 127.0.0.1 --save "" --appendonly no

set /a WAITED=0
:waitredis
"%REDIS_CLI%" -h 127.0.0.1 -p %REDIS_PORT% ping >nul 2>&1
if not errorlevel 1 goto redisready
set /a WAITED+=1
if %WAITED% GEQ 30 (
    echo [start] FEHLER: Redis antwortet nicht.
    pause
    exit /b 1
)
ping -n 2 127.0.0.1 >nul
goto waitredis
:redisready
echo [start] Redis laeuft.

rem --- 8. Java 25 suchen -----------------------------------------------------
rem CloudNet 4.0.0-RC17 besteht auf genau Java 25 und weigert sich unter 26 zu
rem starten. Auf dieser Maschine zeigt "java" auf Corretto 26, deshalb wird
rem hier gezielt ein Java 25 gesucht statt sich auf den PATH zu verlassen.
rem Mit TERRANOVA_JAVA25=<JDK-Verzeichnis> laesst sich die Suche uebergehen.
set "JAVA25="

if defined TERRANOVA_JAVA25 (
    if exist "%TERRANOVA_JAVA25%\bin\java.exe" set "JAVA25=%TERRANOVA_JAVA25%\bin\java.exe"
)

if not defined JAVA25 call :findjava25 "C:\Program Files\Eclipse Adoptium\jdk-25*"
if not defined JAVA25 call :findjava25 "C:\Program Files\Amazon Corretto\jdk25*"
if not defined JAVA25 call :findjava25 "C:\Program Files\Java\jdk-25*"
if not defined JAVA25 call :findjava25 "C:\Program Files\Microsoft\jdk-25*"
if not defined JAVA25 call :findjava25 "%USERPROFILE%\.jdks\*25*"

rem Zuletzt: vielleicht ist das Java im PATH ohnehin schon eine 25.
if not defined JAVA25 (
    java -version 2>&1 | findstr /C:"version \"25" >nul
    if not errorlevel 1 set "JAVA25=java"
)

if not defined JAVA25 (
    echo [start] FEHLER: CloudNet braucht Java 25, es wurde keines gefunden.
    echo [start]        Ein Java 25 installieren ^(Adoptium, Corretto, ...^) oder
    echo [start]        TERRANOVA_JAVA25 auf das JDK-Verzeichnis setzen.
    goto :shutdown
)
echo [start] Java 25: %JAVA25%

rem --- 9. Velocity-Forwarding-Secret sicherstellen ----------------------------
rem Erzeugt beim ersten Mal ein Secret und traegt es in jeden Backend-Server
rem ein. Laeuft bei jedem Start, damit ein neu angelegter Dienst es ebenfalls
rem bekommt. Das Secret selbst bleibt ausserhalb des Repositories.
powershell -NoProfile -ExecutionPolicy Bypass -File "scripts\sync-forwarding-secret.ps1"
if errorlevel 1 (
    echo [start] FEHLER: Forwarding-Secret konnte nicht gesetzt werden.
    goto :shutdown
)

rem --- 10. CloudNet starten --------------------------------------------------
rem Der Knoten startet Proxy, main, build und farm selbst. Dungeons entstehen
rem erst auf Zuruf:  create by mining <anzahl> --start
echo [start] CloudNet wird gestartet...
pushd "%CLOUDNET_HOME%"
"%JAVA25%" -Xms128M -Xmx128M -XX:+UseZGC -XX:+PerfDisableSharedMem -jar launcher.jar
popd

rem --- 11. Redis und MariaDB sauber herunterfahren ----------------------------
:shutdown
echo [start] Redis wird heruntergefahren...
"%REDIS_CLI%" -h 127.0.0.1 -p %REDIS_PORT% shutdown nosave >nul 2>&1

echo [start] MariaDB wird heruntergefahren...
"%MYSQL%" -h 127.0.0.1 -P %MARIADB_PORT% -u root --protocol=tcp -e "SHUTDOWN" >nul 2>&1
ping -n 4 127.0.0.1 >nul

pause
exit /b 0

rem ---------------------------------------------------------------------------
rem Sucht im uebergebenen Verzeichnismuster ein JDK und uebernimmt es nur, wenn
rem java -version wirklich eine 25 meldet. Der Verzeichnisname allein reicht
rem nicht: .jdks\corretto-25.0.2 heisst so, muss es aber nicht sein.
rem ---------------------------------------------------------------------------
:findjava25
for /d %%D in (%~1) do (
    if not defined JAVA25 (
        if exist "%%~fD\bin\java.exe" (
            "%%~fD\bin\java.exe" -version 2>&1 | findstr /C:"version \"25" >nul
            if not errorlevel 1 set "JAVA25=%%~fD\bin\java.exe"
        )
    )
)
goto :eof
