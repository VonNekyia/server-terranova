@echo off
setlocal enabledelayedexpansion
cd /d "%~dp0"

rem ---------------------------------------------------------------------------
rem Startet das Terranova-Netzwerk: MariaDB, Redis und danach Proxy und Server.
rem Doppelklick genuegt. Der erste Start laedt MariaDB und Redis einmalig
rem herunter.
rem
rem Die Datenbanken laufen bewusst hier und nicht in einem Plugin: Paper liest
rem server.properties und die Plugins ihre Configs, bevor ein Plugin ueberhaupt
rem laden koennte. Beide muessen also schon stehen, bevor der erste Server
rem hochfaehrt.
rem
rem Die Server selbst startet scripts\network.ps1. Jeder bekommt sein eigenes
rem Konsolenfenster, dieses hier bleibt als Aufsicht und startet neu, was
rem abstuerzt. Strg+C faehrt alles wieder herunter.
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
rem Kein taskkill auf mysqld.exe: auf dieser Maschine laeuft daneben ein
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

rem Alle Server laufen auf dieser Maschine, deshalb reichen localhost und
rem 127.0.0.1 - MySQL behandelt beide als verschiedene Hosts, es braucht also
rem beide Eintraege. Frueher stand hier ALL PRIVILEGES ON *.* fuer '%%'; fuer
rem einen Einzelserver war das egal, fuer ein Netzwerk ist es das nicht mehr.
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
    goto shutdown
)
ping -n 2 127.0.0.1 >nul
goto waitredis
:redisready
echo [start] Redis laeuft.

rem --- 8. Paper, Configs und Plugins in die Server legen ----------------------
rem Jedes Jar liegt genau einmal im Repository, naemlich unter templates\.
rem Hier wird es in die Serververzeichnisse kopiert - und dabei das
rem Velocity-Secret eingetragen, das ausserhalb des Repositories bleibt.
echo [start] Server werden bestueckt...
powershell -NoProfile -ExecutionPolicy Bypass -File "scripts\sync-servers.ps1"
if errorlevel 1 (
    echo [start] FEHLER: Server konnten nicht bestueckt werden.
    goto shutdown
)

rem --- 9. Proxy und Server starten -------------------------------------------
rem network.ps1 bleibt im Vordergrund und beaufsichtigt die Server, bis es mit
rem Strg+C beendet wird. Danach laeuft dieses Skript hier weiter und raeumt
rem die Datenbanken ab.
powershell -NoProfile -ExecutionPolicy Bypass -File "scripts\network.ps1"

rem --- 10. Redis und MariaDB sauber herunterfahren ----------------------------
:shutdown
echo [start] Redis wird heruntergefahren...
"%REDIS_CLI%" -h 127.0.0.1 -p %REDIS_PORT% shutdown nosave >nul 2>&1

echo [start] MariaDB wird heruntergefahren...
"%MYSQL%" -h 127.0.0.1 -P %MARIADB_PORT% -u root --protocol=tcp -e "SHUTDOWN" >nul 2>&1
ping -n 4 127.0.0.1 >nul

pause
