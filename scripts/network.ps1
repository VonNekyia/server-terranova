# ---------------------------------------------------------------------------
# Startet Proxy und die drei festen Server und haelt sie am Laufen.
#
# Jeder Server bekommt sein eigenes Konsolenfenster - dort laesst sich wie
# gewohnt "stop", "op ..." oder "reload" eintippen. Dieses Fenster hier bleibt
# als Aufsicht: es prueft im Takt, ob noch alle laufen, und startet neu, was
# abgestuerzt ist. Mit Strg+C faehrt es alles wieder herunter.
#
# Dungeons laufen bewusst nicht unter dieser Aufsicht. Ein Dungeon ist auf 24
# Stunden angelegt und darf einfach enden; siehe dungeon.ps1 und
# reap-mining.ps1.
# ---------------------------------------------------------------------------
param(
    [string]$Root = (Split-Path -Parent $PSScriptRoot),
    [int]$CheckSeconds = 20,
    [switch]$NoWatch
)

$ErrorActionPreference = 'Stop'
. (Join-Path $PSScriptRoot 'rcon.ps1')

# RCON laeuft auf Serverport + 100; das Passwort legt sync-servers.ps1 an.
$RconPorts = @{ main = 25666; build = 25667; farm = 25668 }
$rconFile = Join-Path $Root 'runtime/rcon.secret'
$rconPass = if (Test-Path $rconFile) { [System.IO.File]::ReadAllText($rconFile).Trim() } else { '' }

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

# name, Verzeichnis, Speicher in MB
$Nodes = @(
    @{ Name = 'proxy'; Dir = (Join-Path $Root 'proxy');          Mem = 512;  Proxy = $true }
    @{ Name = 'main';  Dir = (Join-Path $Root 'servers\main');   Mem = 4096; Proxy = $false }
    @{ Name = 'build'; Dir = (Join-Path $Root 'servers\build');  Mem = 2048; Proxy = $false }
    @{ Name = 'farm';  Dir = (Join-Path $Root 'servers\farm');   Mem = 2048; Proxy = $false }
)

$java = if ($env:TERRANOVA_JAVA) { $env:TERRANOVA_JAVA } else { 'java' }
$running = @{}

function Start-Node($node) {
    $pattern = if ($node.Proxy) { 'velocity-*.jar' } else { 'paper-*.jar' }
    $jar = Get-ChildItem (Join-Path $node.Dir $pattern) -ErrorAction SilentlyContinue | Select-Object -First 1
    if (-not $jar) { Write-Warning "[netz] $($node.Name): kein $pattern in $($node.Dir)"; return $null }

    $a = @("-Xms$($node.Mem)M", "-Xmx$($node.Mem)M")
    if (-not $node.Proxy) { $a += $AikarFlags }
    $a += @('-jar', $jar.Name)
    if (-not $node.Proxy) { $a += '--nogui' }

    $p = Start-Process -FilePath $java -ArgumentList $a -WorkingDirectory $node.Dir -PassThru
    Write-Output ("[netz] {0,-6} gestartet (PID {1}, {2} MB)" -f $node.Name, $p.Id, $node.Mem)
    return $p
}

function Stop-All {
    Write-Output '[netz] Netzwerk wird heruntergefahren...'
    # Erst der Proxy: dann bekommt niemand mehr eine Verbindung auf einen
    # Server, der gerade speichert. Der Proxy haelt keine Welt, ihn darf man
    # hart beenden; die Server bekommen ein echtes "stop" ueber RCON.
    $p = $running['proxy']
    if ($p -and -not $p.HasExited) {
        Stop-Process -Id $p.Id -Force -ErrorAction SilentlyContinue
        Write-Output '[netz] proxy beendet'
    }
    foreach ($name in @('main', 'build', 'farm')) {
        $p = $running[$name]
        if (-not $p -or $p.HasExited) { continue }
        Stop-ServerGracefully -Name $name -RconPort $RconPorts[$name] `
            -Password $rconPass -ProcessId $p.Id -WaitSeconds 120 | Out-Null
    }
}

try {
    foreach ($n in $Nodes) {
        $p = Start-Node $n
        if ($p) { $running[$n.Name] = $p }
        # Der Proxy darf vor den Servern oben sein, die Reihenfolge ist ihm
        # egal - er verbindet sich erst, wenn jemand einen Server anwaehlt.
        Start-Sleep -Seconds 2
    }

    if ($NoWatch) { Write-Output '[netz] gestartet, keine Aufsicht (-NoWatch)'; return }

    Write-Output ''
    Write-Output "[netz] Alles oben. Proxy auf 25565. Strg+C beendet das Netzwerk."
    Write-Output "[netz]   Dungeons:  scripts\dungeon.ps1 open 3"
    Write-Output ''

    while ($true) {
        Start-Sleep -Seconds $CheckSeconds
        foreach ($n in $Nodes) {
            $p = $running[$n.Name]
            if ($p -and -not $p.HasExited) { continue }
            Write-Warning ("[netz] {0} laeuft nicht mehr (Exitcode {1}) - Neustart" -f `
                $n.Name, $(if ($p) { $p.ExitCode } else { '?' }))
            $np = Start-Node $n
            if ($np) { $running[$n.Name] = $np }
        }
    }
} finally {
    Stop-All
}
