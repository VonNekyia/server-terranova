# ---------------------------------------------------------------------------
# Taeglicher Neustart der drei festen Server.
#
# Der Trick: hier wird nur gestoppt, nicht gestartet. Die Aufsicht in
# network.ps1 merkt innerhalb von 20 Sekunden, dass ein Server weg ist, und
# faehrt ihn wieder hoch. Damit gibt es genau eine Stelle, die Server startet.
#
# Gestoppt wird ueber RCON, also mit einem echten "stop" - der Server speichert
# und beendet sich selbst. Nacheinander und mit Abstand, damit nie alle drei
# gleichzeitig unten sind.
#
# Dungeons bleiben unberuehrt. Sie haben ihre eigenen 24 Stunden.
#
# Als geplante Aufgabe einrichten (einmalig, in einer Konsole als
# Administrator):
#
#   schtasks /Create /TN "Terranova Neustart" /SC DAILY /ST 04:00 ^
#     /TR "powershell -NoProfile -ExecutionPolicy Bypass -File \"C:\Pfad\zu\scripts\restart-daily.ps1\""
#
# ---------------------------------------------------------------------------
param(
    [string]$Root = (Split-Path -Parent $PSScriptRoot),
    [string[]]$Servers = @('main', 'build', 'farm'),
    [int]$GapSeconds = 120
)

$ErrorActionPreference = 'Stop'
. (Join-Path $PSScriptRoot 'rcon.ps1')

$RconPorts = @{ main = 25666; build = 25667; farm = 25668 }
$Ports = @{ main = 25566; build = 25567; farm = 25568 }

$rconFile = Join-Path $Root 'runtime/rcon.secret'
if (-not (Test-Path $rconFile)) {
    Write-Warning "[neustart] kein RCON-Passwort unter $rconFile - laeuft das Netzwerk?"
    return
}
$pass = [System.IO.File]::ReadAllText($rconFile).Trim()

$first = $true
foreach ($name in $Servers) {
    if (-not $first) {
        Write-Output "[neustart] $GapSeconds s Pause, damit $name nicht zeitgleich mit dem vorigen unten ist"
        Start-Sleep -Seconds $GapSeconds
    }
    $first = $false

    $conn = Get-NetTCPConnection -LocalPort $Ports[$name] -State Listen -ErrorAction SilentlyContinue |
            Select-Object -First 1
    if (-not $conn) { Write-Warning "[neustart] $name laeuft nicht, uebersprungen"; continue }

    # Erst ansagen, dann stoppen - wer drauf ist, wird vom Proxy sowieso
    # rausgeworfen, soll es aber vorher lesen.
    try {
        Send-RconCommand -Port $RconPorts[$name] -Password $pass `
            -Command 'say Neustart in 30 Sekunden.' | Out-Null
        Start-Sleep -Seconds 30
    } catch { }

    Stop-ServerGracefully -Name $name -RconPort $RconPorts[$name] -Password $pass `
        -ProcessId $conn.OwningProcess -WaitSeconds 120 | Out-Null
    Write-Output "[neustart] $name gestoppt - die Aufsicht startet ihn gleich neu"
}

Write-Output '[neustart] fertig'
