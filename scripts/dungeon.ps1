# ---------------------------------------------------------------------------
# Oeffnet und schliesst Mining-Dungeons.
#
# Ein Dungeon ist eine Kopie von templates\mining, die auf einem eigenen Port
# laeuft. Die Plaetze mining-1 bis mining-8 stehen fest in proxy\velocity.toml;
# geoeffnet wird nur, was gebraucht wird. Velocity stoert ein eingetragener,
# nicht laufender Server nicht.
#
# Ein Dungeon bleibt 24 Stunden offen und ueberlebt in dieser Zeit auch einen
# Neustart - abgeraeumt wird er von reap-mining.ps1.
#
#   scripts\dungeon.ps1 open 3      die naechsten drei freien Plaetze oeffnen
#   scripts\dungeon.ps1 open 3 -Slot 5   gezielt mining-5, -6, -7
#   scripts\dungeon.ps1 close 2     mining-2 beenden (Welt bleibt liegen)
#   scripts\dungeon.ps1 list        was laeuft, wie alt, wann Schluss ist
# ---------------------------------------------------------------------------
param(
    [Parameter(Mandatory = $true)][ValidateSet('open', 'close', 'list')][string]$Action,
    [int]$Count = 1,
    [int]$Slot = 0,
    [string]$Root = (Split-Path -Parent $PSScriptRoot),
    [int]$Memory = 2048,
    [int]$MaxAgeHours = 24
)

$ErrorActionPreference = 'Stop'
. (Join-Path $PSScriptRoot 'rcon.ps1')

$BasePort = 25570          # mining-N laeuft auf BasePort + N
$MaxSlots = 8              # so viele stehen in velocity.toml
$serversDir = Join-Path $Root 'servers'

# Aikar-Flags, dieselben wie fuer die festen Server.
$AikarFlags = @(
    '-XX:+AlwaysPreTouch', '-XX:+DisableExplicitGC', '-XX:+ParallelRefProcEnabled',
    '-XX:+PerfDisableSharedMem', '-XX:+UnlockExperimentalVMOptions', '-XX:+UseG1GC',
    '-XX:G1HeapRegionSize=8M', '-XX:G1HeapWastePercent=5', '-XX:G1MaxNewSizePercent=40',
    '-XX:G1MixedGCCountTarget=4', '-XX:G1MixedGCLiveThresholdPercent=90',
    '-XX:G1NewSizePercent=30', '-XX:G1RSetUpdatingPauseTimePercent=5',
    '-XX:G1ReservePercent=20', '-XX:InitiatingHeapOccupancyPercent=15',
    '-XX:MaxGCPauseMillis=200', '-XX:MaxTenuringThreshold=1', '-XX:SurvivorRatio=32',
    '-Dusing.aikars.flags=https://mcflags.emc.gs', '-Daikars.new.flags=true'
)

function Get-DungeonProcess([int]$n) {
    # Ueber den Port und nicht ueber die Kommandozeile: die Server werden mit
    # -WorkingDirectory gestartet, der Pfad steht also gar nicht in den
    # Argumenten. Wer auf 25570+n horcht, ist der Dungeon.
    $c = Get-NetTCPConnection -LocalPort ($BasePort + $n) -State Listen -ErrorAction SilentlyContinue |
         Select-Object -First 1
    if (-not $c) { return $null }
    Get-Process -Id $c.OwningProcess -ErrorAction SilentlyContinue
}

switch ($Action) {

    'list' {
        $any = $false
        for ($n = 1; $n -le $MaxSlots; $n++) {
            $dir = Join-Path $serversDir "mining-$n"
            if (-not (Test-Path $dir)) { continue }
            $any = $true
            $age = [math]::Round(((Get-Date) - (Get-Item $dir).CreationTime).TotalHours, 1)
            $proc = Get-DungeonProcess $n
            $state = if ($proc) { "laeuft (PID $($proc.Id))" } else { 'gestoppt' }
            $bis = (Get-Item $dir).CreationTime.AddHours($MaxAgeHours)
            "{0,-9} Port {1}  {2,-22} {3} h alt, offen bis {4:dd.MM. HH:mm}" -f `
                "mining-$n", ($BasePort + $n), $state, $age, $bis
        }
        if (-not $any) { Write-Output 'Kein Dungeon offen.' }
    }

    'close' {
        $n = if ($Slot -gt 0) { $Slot } else { $Count }
        $proc = Get-DungeonProcess $n
        if (-not $proc) { Write-Output "mining-$n laeuft nicht."; break }
        # Ueber RCON, nicht ueber das Fenster: die JVM hat keine
        # Nachrichtenschleife, CloseMainWindow laeuft ins Leere und taskkill
        # ohne /F verweigert den Dienst. Siehe rcon.ps1.
        $rconFile = Join-Path $Root 'runtime/rcon.secret'
        $pass = if (Test-Path $rconFile) { [System.IO.File]::ReadAllText($rconFile).Trim() } else { '' }
        Stop-ServerGracefully -Name "mining-$n" -RconPort ($BasePort + $n + 100) `
            -Password $pass -ProcessId $proc.Id -WaitSeconds 90 | Out-Null
        Write-Output "mining-$n beendet. Die Welt bleibt liegen bis reap-mining.ps1 sie abraeumt."
    }

    'open' {
        $opened = 0
        $start = if ($Slot -gt 0) { $Slot } else { 1 }
        for ($n = $start; $n -le $MaxSlots -and $opened -lt $Count; $n++) {
            $dir = Join-Path $serversDir "mining-$n"
            $port = $BasePort + $n

            if (Get-DungeonProcess $n) {
                if ($Slot -gt 0) { Write-Warning "mining-$n laeuft bereits"; break }
                continue
            }

            $fresh = -not (Test-Path $dir)
            if ($fresh) {
                # Frische Welt aus der Vorlage. Nur hier wird kopiert - ein
                # Neustart eines bestehenden Dungeons behaelt seine Welt.
                Copy-Item (Join-Path $Root 'templates\mining') $dir -Recurse
                Write-Output "[dungeon] mining-$n aus der Vorlage angelegt"
            }

            # Paper, Configs, Plugins, server.properties samt Port, RCON und
            # Forwarding-Secret - alles an einer Stelle.
            & (Join-Path $PSScriptRoot 'sync-servers.ps1') -Root $Root -Target $dir -Port $port | Out-Null

            $args = @("-Xms${Memory}M", "-Xmx${Memory}M") + $AikarFlags +
                    @('-jar', (Get-ChildItem (Join-Path $dir 'paper-*.jar') | Select-Object -First 1).Name, '--nogui')
            $java = if ($env:TERRANOVA_JAVA) { $env:TERRANOVA_JAVA } else { 'java' }
            Start-Process -FilePath $java -ArgumentList $args -WorkingDirectory $dir | Out-Null

            Write-Output "[dungeon] mining-$n gestartet auf Port $port"
            $opened++
        }
        if ($opened -eq 0) { Write-Warning "Kein freier Platz gefunden (mining-1..$MaxSlots)." }
        else { Write-Output "$opened Dungeon(s) offen. In Velocity: /server mining-<n>" }
    }
}
