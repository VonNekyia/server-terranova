# ---------------------------------------------------------------------------
# Raeumt abgelaufene Dungeons ab.
#
# Ein Dungeon ist 24 Stunden offen. Die Uhr laeuft ab dem Anlegen des
# Dienstverzeichnisses unter network\local\services\mining-*. Weil der mining-
# Task statisch ist, ueberlebt ein Dungeon Neustarts innerhalb dieser Zeit -
# abgeraeumt wird er nur hier.
#
# Loeschen heisst: Verzeichnis weg. Beim naechsten "create by mining" legt
# CloudNet es aus network\local\templates\mining\default neu an, also mit
# frischer Welt.
#
# Ein laufender Dungeon wird nicht angefasst. Der Prozess haelt Dateien offen,
# und ein halb geloeschtes Dienstverzeichnis ist schlimmer als eines, das eine
# Viertelstunde zu lang steht - beim naechsten Lauf ist er gestoppt und dann
# faellt er weg. Mit -StopRunning wird er vorher ueber die REST-Schnittstelle
# gestoppt (setzt "modules install CloudNet-Rest" voraus).
#
# Aufruf (Testlauf, aendert nichts):
#   powershell -File scripts\reap-mining.ps1 -WhatIf
# ---------------------------------------------------------------------------
[CmdletBinding(SupportsShouldProcess = $true)]
param(
    [string]$Root = (Split-Path -Parent $PSScriptRoot),
    [int]$MaxAgeHours = 24,
    [switch]$StopRunning,
    [string]$RestUrl = 'http://127.0.0.1:2812'
)

$ErrorActionPreference = 'Stop'

$servicesDir = Join-Path $Root 'network\local\services'
if (-not (Test-Path $servicesDir)) {
    Write-Output "[reap] kein services-Verzeichnis unter $servicesDir"
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

    # Laeuft der Dungeon noch? Ein Java-Prozess, dessen Arbeitsverzeichnis oder
    # Kommandozeile auf dieses Verzeichnis zeigt, haelt die Weltdateien offen.
    $running = Get-CimInstance Win32_Process -Filter "Name='java.exe'" -ErrorAction SilentlyContinue |
        Where-Object { $_.CommandLine -and $_.CommandLine -like "*$($d.Name)*" }

    if ($running) {
        if ($StopRunning) {
            Write-Output ("[reap] {0}: {1} h alt, wird ueber REST gestoppt" -f $d.Name, $ageHours)
            try {
                Invoke-RestMethod -Method Delete -Uri "$RestUrl/api/v3/service/$($d.Name)" -TimeoutSec 20 | Out-Null
                Start-Sleep -Seconds 15
            } catch {
                Write-Warning ("[reap] {0}: REST-Stopp fehlgeschlagen ({1}). Uebersprungen." -f $d.Name, $_.Exception.Message)
                continue
            }
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
