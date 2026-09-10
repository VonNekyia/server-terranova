# ---------------------------------------------------------------------------
# Raeumt abgelaufene Dungeons ab.
#
# Ein Dungeon ist 24 Stunden offen. Die Uhr laeuft ab dem Anlegen des
# Verzeichnisses unter servers\mining-*. Ein Neustart in dieser Zeit behaelt
# die Welt - dungeon.ps1 kopiert nur, wenn das Verzeichnis noch fehlt.
#
# Loeschen heisst: Verzeichnis weg. Das naechste "dungeon.ps1 open" legt es
# aus templates\mining neu an, also mit frischer Welt.
#
# Ein laufender Dungeon wird nicht angefasst. Der Prozess haelt Dateien offen,
# und ein halb geloeschtes Verzeichnis ist schlimmer als eines, das eine
# Viertelstunde zu lang steht - beim naechsten Lauf ist er gestoppt und faellt
# dann weg. Mit -StopRunning wird er vorher sauber beendet.
#
# Aufruf (Testlauf, aendert nichts):
#   powershell -File scripts\reap-mining.ps1 -WhatIf
# ---------------------------------------------------------------------------
[CmdletBinding(SupportsShouldProcess = $true)]
param(
    [string]$Root = (Split-Path -Parent $PSScriptRoot),
    [int]$MaxAgeHours = 24,
    [switch]$StopRunning
)

$ErrorActionPreference = 'Stop'

$servicesDir = Join-Path $Root 'servers'
if (-not (Test-Path $servicesDir)) {
    Write-Output "[reap] kein servers-Verzeichnis unter $servicesDir"
    return
}

$now = Get-Date
$dungeons = Get-ChildItem $servicesDir -Directory -Filter 'mining-*' -ErrorAction SilentlyContinue

if (-not $dungeons) {
    Write-Output "[reap] kein Dungeon vorhanden"
    return
}

foreach ($d in $dungeons) {
    $ageHours = [math]::Round(($now - $d.CreationTime).TotalHours, 1)

    if ($ageHours -lt $MaxAgeHours) {
        Write-Output ("[reap] {0}: {1} h alt, laeuft noch bis {2:HH:mm}" -f `
            $d.Name, $ageHours, $d.CreationTime.AddHours($MaxAgeHours))
        continue
    }

    # Laeuft der Dungeon noch? Er haelt dann die Weltdateien offen. Erkannt wird
    # er am Port 25570+n - die Kommandozeile taugt nicht dafuer, weil der Pfad
    # nur im Arbeitsverzeichnis steht.
    $n = [int]($d.Name -replace '^mining-', '')
    $running = Get-NetTCPConnection -LocalPort (25570 + $n) -State Listen -ErrorAction SilentlyContinue

    if ($running) {
        if ($StopRunning) {
            Write-Output ("[reap] {0}: {1} h alt, wird beendet" -f $d.Name, $ageHours)
            & (Join-Path $PSScriptRoot 'dungeon.ps1') close -Slot $n -Root $Root | Out-Null
            Start-Sleep -Seconds 5
        } else {
            Write-Output ("[reap] {0}: {1} h alt, laeuft aber noch - uebersprungen. Mit -StopRunning erzwingen." -f $d.Name, $ageHours)
            continue
        }
    }

    if ($PSCmdlet.ShouldProcess($d.FullName, "Dungeon loeschen ($ageHours h alt)")) {
        Remove-Item $d.FullName -Recurse -Force
        Write-Output ("[reap] {0}: geloescht ({1} h alt)" -f $d.Name, $ageHours)
    }
}
