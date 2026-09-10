# ---------------------------------------------------------------------------
# Legt die abgeleiteten Teile eines Servers an: Paper-Jar, Configs, Plugin-Jars
# und server.properties - samt Velocity-Secret und RCON-Passwort.
#
# Warum ueberhaupt Templates statt alles direkt im Serververzeichnis:
# jedes Jar liegt so genau einmal im Repository. Ein Plugin-Update ist eine
# Datei in templates\common\plugins, kein viermaliges Kopieren.
#
# Und warum auch server.properties: dort steht das RCON-Passwort, und das
# gehoert so wenig ins Repository wie das Forwarding-Secret. Die versionierte
# Fassung liegt unter templates\<name>\server.properties, die echte entsteht
# hier daraus.
#
# Was hier hineinkopiert wird, ist in .gitignore ausgeschlossen. Versioniert
# ist an einem Server nur, was ihm wirklich gehoert: die Configs seiner
# Plugins und seine Welt-Metadaten.
#
# Aufruf:
#   scripts\sync-servers.ps1                      alle festen Server
#   scripts\sync-servers.ps1 -Target servers\mining-3 -Port 25573
# ---------------------------------------------------------------------------
param(
    [string]$Root = (Split-Path -Parent $PSScriptRoot),
    [string[]]$Target,
    [int]$Port = 0,
    [string]$Motd
)

$ErrorActionPreference = 'Stop'

$common = Join-Path $Root 'templates\common'
$secretFile = Join-Path $Root 'proxy\forwarding.secret'
$rconFile = Join-Path $Root 'runtime\rcon.secret'

# Die festen Server und ihre Ports. Dungeons bringen ihren Port selbst mit.
$Ports = @{ main = 25566; build = 25567; farm = 25568 }

function New-Secret {
    $chars = 'abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789'.ToCharArray()
    $rng = [System.Security.Cryptography.RandomNumberGenerator]::Create()
    $bytes = New-Object byte[] 32
    $rng.GetBytes($bytes)
    -join ($bytes | ForEach-Object { $chars[$_ % $chars.Length] })
}

# --- Geheimnisse: einmal erzeugen, danach nur noch lesen -------------------
if (-not (Test-Path $secretFile)) {
    New-Item -ItemType Directory -Force (Split-Path -Parent $secretFile) | Out-Null
    # Ohne Zeilenumbruch: Velocity liest die Datei roh.
    [System.IO.File]::WriteAllText($secretFile, (New-Secret))
    Write-Output '[sync] neues Forwarding-Secret erzeugt'
}
if (-not (Test-Path $rconFile)) {
    New-Item -ItemType Directory -Force (Split-Path -Parent $rconFile) | Out-Null
    [System.IO.File]::WriteAllText($rconFile, (New-Secret))
    Write-Output '[sync] neues RCON-Passwort erzeugt'
}
$secret = [System.IO.File]::ReadAllText($secretFile).Trim()
$rconPass = [System.IO.File]::ReadAllText($rconFile).Trim()

if (-not $Target) {
    $Target = @('main', 'build', 'farm') | ForEach-Object { Join-Path $Root "servers\$_" }
}

foreach ($dir in $Target) {
    $name = Split-Path -Leaf $dir
    New-Item -ItemType Directory -Force $dir | Out-Null

    # --- Paper und die gemeinsamen Configs --------------------------------
    Copy-Item (Join-Path $common 'paper-*.jar') $dir -Force
    Copy-Item (Join-Path $common 'eula.txt') $dir -Force
    Copy-Item (Join-Path $common 'bukkit.yml') $dir -Force
    Copy-Item (Join-Path $common 'spigot.yml') $dir -Force
    New-Item -ItemType Directory -Force (Join-Path $dir 'config') | Out-Null
    Copy-Item (Join-Path $common 'config\*.yml') (Join-Path $dir 'config') -Force

    # --- Gemeinsame Plugin-Jars -------------------------------------------
    New-Item -ItemType Directory -Force (Join-Path $dir 'plugins') | Out-Null
    Copy-Item (Join-Path $common 'plugins\*.jar') (Join-Path $dir 'plugins') -Force
    if (Test-Path (Join-Path $common 'plugins\PlaceholderAPI\expansions')) {
        New-Item -ItemType Directory -Force (Join-Path $dir 'plugins\PlaceholderAPI\expansions') | Out-Null
        Copy-Item (Join-Path $common 'plugins\PlaceholderAPI\expansions\*.jar') `
                  (Join-Path $dir 'plugins\PlaceholderAPI\expansions') -Force
    }
    # HuskSync-Config nur anlegen, wenn noch keine da ist - eine bestehende
    # koennte von Hand angepasst sein.
    $hs = Join-Path $dir 'plugins\HuskSync\config.yml'
    if (-not (Test-Path $hs) -and (Test-Path (Join-Path $common 'plugins\HuskSync\config.yml'))) {
        New-Item -ItemType Directory -Force (Split-Path -Parent $hs) | Out-Null
        Copy-Item (Join-Path $common 'plugins\HuskSync\config.yml') $hs
    }

    # --- Serverspezifische Plugins ----------------------------------------
    $ownDir = Join-Path $Root "templates\$name"
    $ownPlugins = Join-Path $ownDir 'plugins'
    if (Test-Path $ownPlugins) { Copy-Item (Join-Path $ownPlugins '*.jar') (Join-Path $dir 'plugins') -Force }

    # --- server.properties aus der Vorlage --------------------------------
    $tplProps = Join-Path $ownDir 'server.properties'
    if (-not (Test-Path $tplProps)) { $tplProps = Join-Path $common 'server.properties' }
    $props = [System.IO.File]::ReadAllLines($tplProps)

    $srvPort = if ($Port -gt 0) { $Port } elseif ($Ports.ContainsKey($name)) { $Ports[$name] } else { 0 }
    if ($srvPort -eq 0) { Write-Warning "[sync] $name : kein Port bekannt, server.properties bleibt wie in der Vorlage" }

    $want = @{
        # Hinter dem Proxy: Velocity prueft gegen Mojang, der Server nicht.
        # Er bindet auf 127.0.0.1, damit ihn nur der Proxy erreicht.
        'online-mode'               = 'false'
        'server-ip'                 = '127.0.0.1'
        'prevent-proxy-connections' = 'false'
        'enforce-secure-profile'    = 'false'
        # RCON ist der einzige Weg, den Server unter Windows sauber zu stoppen.
        # Siehe scripts\rcon.ps1.
        'enable-rcon'               = 'true'
        'rcon.password'             = $rconPass
    }
    if ($Motd) { $want['motd'] = $Motd }
    if ($srvPort -gt 0) {
        $want['server-port'] = "$srvPort"
        $want['query.port'] = "$srvPort"
        $want['rcon.port'] = "$($srvPort + 100)"
    }

    $seen = @{}
    for ($i = 0; $i -lt $props.Length; $i++) {
        if ($props[$i] -match '^([^#=]+)=') {
            $k = $Matches[1]
            if ($want.ContainsKey($k)) { $props[$i] = "$k=$($want[$k])"; $seen[$k] = $true }
        }
    }
    $props = @($props) + @($want.Keys | Where-Object { -not $seen[$_] } | ForEach-Object { "$_=$($want[$_])" })
    [System.IO.File]::WriteAllLines((Join-Path $dir 'server.properties'), $props)

    # --- Velocity modern forwarding ---------------------------------------
    $pg = Join-Path $dir 'config\paper-global.yml'
    if (Test-Path $pg) {
        $lines = [System.IO.File]::ReadAllLines($pg)
        $inVelocity = $false
        for ($i = 0; $i -lt $lines.Length; $i++) {
            if ($lines[$i] -match '^\s{2}velocity:\s*$') { $inVelocity = $true; continue }
            if ($inVelocity) {
                if ($lines[$i] -match '^\s{4}enabled:') { $lines[$i] = '    enabled: true'; continue }
                if ($lines[$i] -match '^\s{4}online-mode:') { $lines[$i] = '    online-mode: true'; continue }
                if ($lines[$i] -match '^\s{4}secret:') { $lines[$i] = "    secret: '$secret'"; $inVelocity = $false; continue }
                if ($lines[$i] -notmatch '^\s{4}') { $inVelocity = $false }
            }
        }
        [System.IO.File]::WriteAllLines($pg, $lines)
    }

    if ($srvPort -gt 0) { Write-Output "[sync] $name (Port $srvPort, RCON $($srvPort + 100))" }
    else { Write-Output "[sync] $name" }
}
