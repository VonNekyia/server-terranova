@echo off
rem Startet das Terranova-Netzwerk. Alles Weitere macht terranova selbst;
rem diese Datei ist nur die Abkuerzung fuer den Doppelklick.
cd /d "%~dp0"
bin\terranova.exe start %*
if errorlevel 1 pause
