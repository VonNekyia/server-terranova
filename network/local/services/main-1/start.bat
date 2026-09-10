@echo off
setlocal enabledelayedexpansion
cd /d "%~dp0"

rem ---------------------------------------------------------------------------
rem Startet den Entwicklungsserver: lokale MariaDB und danach Paper.
rem Doppelklick genuegt. Der erste Start laedt MariaDB einmalig herunter.
rem
rem MariaDB laeuft bewusst hier und nicht in einem Plugin: Paper liest
rem server.properties und die Plugins ihre Configs, bevor ein Plugin
rem ueberhaupt laden koennte. Nur so steht die Datenbank schon beim
rem allerersten Start bereit.
rem ---------------------------------------------------------------------------

set "MARIADB_VERSION=11.4.5"
set "MARIADB_HOME=mariadb"
set "MARIADB_BASE=%MARIADB_HOME%\mariadb-%MARIADB_VERSION%-winx64"
set "MARIADB_DATA=%CD%\%MARIADB_HOME%\data"
set "MARIADB_PORT=13306"
set "DB_USER=minecraft"
set "DB_PASS=minecraft"

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
set "DATABASES=network nations betonquest chatcontrol interactivechat luckperms proficisci bountyfulseas"

set "PAPER_JAR=paper-26.2-121.jar"
set "MYSQLD=%MARIADB_BASE%\bin\mysqld.exe"
set "MYSQL=%MARIADB_BASE%\bin\mysql.exe"

rem --- 1. MariaDB einmalig besorgen -----------------------------------------
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

rem --- 2. Datenverzeichnis initialisieren ------------------------------------
if not exist "%MARIADB_DATA%\mysql" (
    echo [start] Datenverzeichnis wird initialisiert...
    "%MARIADB_BASE%\bin\mysql_install_db.exe" --datadir="%MARIADB_DATA%"
)

rem --- 3. Reste eines abgestuerzten Laufs beenden ----------------------------
taskkill /F /IM mysqld.exe >nul 2>&1

rem --- 4. MariaDB starten ----------------------------------------------------
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

rem --- 5. Datenbanken und Benutzer sicherstellen ------------------------------
for %%D in (%DATABASES%) do (
    "%MYSQL%" -h 127.0.0.1 -P %MARIADB_PORT% -u root --protocol=tcp -e "CREATE DATABASE IF NOT EXISTS `%%D` CHARACTER SET utf8mb4 COLLATE utf8mb4_unicode_ci;" >nul 2>&1
)
"%MYSQL%" -h 127.0.0.1 -P %MARIADB_PORT% -u root --protocol=tcp -e "CREATE USER IF NOT EXISTS '%DB_USER%'@'localhost' IDENTIFIED BY '%DB_PASS%'; CREATE USER IF NOT EXISTS '%DB_USER%'@'%%' IDENTIFIED BY '%DB_PASS%'; GRANT ALL PRIVILEGES ON *.* TO '%DB_USER%'@'localhost' WITH GRANT OPTION; GRANT ALL PRIVILEGES ON *.* TO '%DB_USER%'@'%%' WITH GRANT OPTION; FLUSH PRIVILEGES;" >nul 2>&1
echo [start] Datenbanken bereit.

rem --- 6. Paper starten ------------------------------------------------------
java -Xms4096M -Xmx4096M -XX:+AlwaysPreTouch -XX:+DisableExplicitGC -XX:+ParallelRefProcEnabled -XX:+PerfDisableSharedMem -XX:+UnlockExperimentalVMOptions -XX:+UseG1GC -XX:G1HeapRegionSize=8M -XX:G1HeapWastePercent=5 -XX:G1MaxNewSizePercent=40 -XX:G1MixedGCCountTarget=4 -XX:G1MixedGCLiveThresholdPercent=90 -XX:G1NewSizePercent=30 -XX:G1RSetUpdatingPauseTimePercent=5 -XX:G1ReservePercent=20 -XX:InitiatingHeapOccupancyPercent=15 -XX:MaxGCPauseMillis=200 -XX:MaxTenuringThreshold=1 -XX:SurvivorRatio=32 -Dusing.aikars.flags=https://mcflags.emc.gs -Daikars.new.flags=true -jar %PAPER_JAR% --nogui

rem --- 7. MariaDB sauber herunterfahren --------------------------------------
echo [start] MariaDB wird heruntergefahren...
"%MYSQL%" -h 127.0.0.1 -P %MARIADB_PORT% -u root --protocol=tcp -e "SHUTDOWN" >nul 2>&1
ping -n 4 127.0.0.1 >nul
taskkill /F /IM mysqld.exe >nul 2>&1

pause
