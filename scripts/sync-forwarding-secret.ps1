# ---------------------------------------------------------------------------
# Legt das Velocity-Forwarding-Secret an.
#
# Warum nur die Datei und nicht die Server-Configs:
#
# CloudNet richtet die Weiterleitung selbst ein. Es schreibt server-ip,
# server-port und online-mode in die server.properties jedes Backends und setzt
# in paper-global.yml proxies.velocity.enabled bei jedem Start wieder auf
# false - CloudNet arbeitet standardmaessig mit legacy forwarding (der alte
# BungeeCord-Weg), und die erzeugte velocity.toml steht entsprechend auf
# player-info-forwarding-mode = "legacy".
#
# Dagegen anzuschreiben bringt nichts: der naechste Start setzt es zurueck, und
# in der Zwischenzeit steht ein Geheimnis in einer versionierten Datei. Also
# wird hier nur die Secret-Datei gepflegt, die Velocity ohnehin liest.
#
# Solange alle Dienste auf 127.0.0.1 gebunden sind, ist legacy forwarding
# vertretbar: die Backends laufen zwar auf online-mode=false und glauben jedem,
# der sie erreicht, aber erreichen kann sie nur, wer schon auf der Maschine
# ist. Sobald ein Backend ueber 127.0.0.1 hinaus erreichbar wird, ist das nicht
# mehr wahr - dann auf modern forwarding umstellen:
#
#   1. in network/local/tasks/*.json der Backends "disableIpRewrite": true
#      setzen, damit CloudNet paper-global.yml in Ruhe laesst
#   2. in der velocity.toml des Proxy-Templates
#      player-info-forwarding-mode = "modern"
#   3. in paper-global.yml jedes Backends proxies.velocity.enabled: true und
#      secret auf den Inhalt von forwarding.secret
#
# Das ist bewusst nicht vorkonfiguriert - es will einmal von Hand mit einem
# echten Verbindungsversuch geprueft werden.
# ---------------------------------------------------------------------------
param(
    [string]$Root = (Split-Path -Parent $PSScriptRoot)
)

$ErrorActionPreference = 'Stop'

$secretFile = Join-Path $Root 'network\local\templates\Proxy\default\forwarding.secret'
$secretDir = Split-Path -Parent $secretFile
if (-not (Test-Path $secretDir)) { New-Item -ItemType Directory -Force $secretDir | Out-Null }

if (-not (Test-Path $secretFile)) {
    $chars = 'abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789'.ToCharArray()
    $rng = [System.Security.Cryptography.RandomNumberGenerator]::Create()
    $bytes = New-Object byte[] 32
    $rng.GetBytes($bytes)
    $secret = -join ($bytes | ForEach-Object { $chars[$_ % $chars.Length] })
    # Ohne abschliessenden Zeilenumbruch: Velocity liest die Datei roh ein.
    [System.IO.File]::WriteAllText($secretFile, $secret)
    Write-Output "[secret] neues Forwarding-Secret erzeugt"
} else {
    $secret = [System.IO.File]::ReadAllText($secretFile).Trim()
}

# Der laufende Proxy-Dienst bekommt dieselbe Datei - CloudNet kopiert das
# Template nur beim Anlegen, ein spaeter erzeugtes Secret kaeme sonst nie an.
$proxyServices = Get-ChildItem (Join-Path $Root 'network\local\services') -Directory `
    -Filter 'Proxy-*' -ErrorAction SilentlyContinue
foreach ($p in $proxyServices) {
    [System.IO.File]::WriteAllText((Join-Path $p.FullName 'forwarding.secret'), $secret)
}

Write-Output "[secret] Secret liegt bereit (Proxy-Template + $($proxyServices.Count) Proxy-Dienst(e))"
