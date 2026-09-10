# ---------------------------------------------------------------------------
# Legt das Velocity-Forwarding-Secret an und traegt es in jeden Backend-Server
# ein. Wird von start.bat bei jedem Start aufgerufen: das ist billig und heilt
# sich selbst, sobald ein neuer Dienst dazukommt.
#
# Das Secret ist der einzige Schutz der Backends. Mit modern forwarding laufen
# sie auf online-mode=false und glauben jedem, der sie erreicht - nur wer das
# Secret kennt, darf Spieler durchreichen. Deshalb steht es in einer Datei,
# die .gitignore ausschliesst, und nicht im Repository.
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

# Alle Backends: die festen Dienste und die Templates, aus denen neue entstehen.
$targets = @()
$targets += Get-ChildItem (Join-Path $Root 'network\local\services') -Directory -ErrorAction SilentlyContinue |
    Where-Object { $_.Name -notlike 'Proxy*' }
$targets += Get-ChildItem (Join-Path $Root 'network\local\templates') -Directory -ErrorAction SilentlyContinue |
    Where-Object { $_.Name -ne 'Proxy' -and $_.Name -ne 'Global' } |
    ForEach-Object { Get-ChildItem $_.FullName -Directory -ErrorAction SilentlyContinue }

foreach ($t in $targets) {
    # --- paper-global.yml: den velocity-Block unter proxies: setzen ---------
    $pg = Join-Path $t.FullName 'config\paper-global.yml'
    if (Test-Path $pg) {
        $lines = [System.IO.File]::ReadAllLines($pg)
        $inVelocity = $false
        $changed = $false
        for ($i = 0; $i -lt $lines.Length; $i++) {
            if ($lines[$i] -match '^\s{2}velocity:\s*$') { $inVelocity = $true; continue }
            if ($inVelocity) {
                if ($lines[$i] -match '^\s{4}enabled:') { $lines[$i] = '    enabled: true'; $changed = $true; continue }
                if ($lines[$i] -match '^\s{4}online-mode:') { $lines[$i] = '    online-mode: true'; $changed = $true; continue }
                if ($lines[$i] -match '^\s{4}secret:') { $lines[$i] = "    secret: '$secret'"; $changed = $true; continue }
                # Ende des Blocks: alles, was flacher eingerueckt ist
                if ($lines[$i] -notmatch '^\s{4}') { $inVelocity = $false }
            }
        }
        if ($changed) { [System.IO.File]::WriteAllLines($pg, $lines) }
    }

    # --- server.properties: online-mode aus, Proxy darf verbinden ----------
    $sp = Join-Path $t.FullName 'server.properties'
    if (Test-Path $sp) {
        $lines = [System.IO.File]::ReadAllLines($sp)
        for ($i = 0; $i -lt $lines.Length; $i++) {
            if ($lines[$i] -match '^online-mode=') { $lines[$i] = 'online-mode=false' }
            if ($lines[$i] -match '^prevent-proxy-connections=') { $lines[$i] = 'prevent-proxy-connections=false' }
        }
        [System.IO.File]::WriteAllLines($sp, $lines)
    }
}

Write-Output "[secret] Forwarding in $($targets.Count) Backend(s) eingetragen"
