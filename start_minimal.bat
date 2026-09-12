@echo off
rem Startet nur das Noetigste: Datenbanken, Proxy und main.
rem Fuer Rechner, denen build, farm und die Dungeons zu viel sind -
rem zusammen belegen die sonst rund acht Gigabyte.
rem
rem Der Rest kommt jederzeit nach, ohne Neustart:
rem   bin\terranova.exe restart build
cd /d "%~dp0"
bin\terranova.exe start main %*
if errorlevel 1 pause
