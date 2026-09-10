# ---------------------------------------------------------------------------
# Minimaler RCON-Client, um Servern Befehle zu schicken - vor allem "stop".
#
# Warum ueberhaupt: einen Paper-Server unter Windows sauber herunterzufahren
# ist ohne RCON nicht moeglich. Die JVM hat kein Fenster mit Nachrichtenschleife,
# also laeuft CloseMainWindow() ins Leere, und taskkill ohne /F antwortet
# "Die Beendigung dieses Prozesses muss erzwungen werden". Bliebe nur der harte
# Abschuss - und der kostet bei einer 500-MB-Welt irgendwann Chunks. Mit RCON
# bekommt der Server ein echtes "stop", speichert und beendet sich selbst.
#
# Das Protokoll ist simpel (Valve Source RCON):
#   int32 laenge | int32 id | int32 typ | body als ASCII, 0-terminiert | 0
#   typ 3 = anmelden, typ 2 = Befehl. Auf eine fehlgeschlagene Anmeldung
#   antwortet der Server mit id = -1.
# ---------------------------------------------------------------------------

function Send-RconCommand {
    param(
        [Parameter(Mandatory = $true)][int]$Port,
        [Parameter(Mandatory = $true)][string]$Password,
        [Parameter(Mandatory = $true)][string]$Command,
        [string]$RconHost = '127.0.0.1',
        [int]$TimeoutMs = 5000
    )

    $client = New-Object System.Net.Sockets.TcpClient
    try {
        $iar = $client.BeginConnect($RconHost, $Port, $null, $null)
        if (-not $iar.AsyncWaitHandle.WaitOne($TimeoutMs)) { throw "keine Verbindung zu ${RconHost}:$Port" }
        $client.EndConnect($iar)
        $client.ReceiveTimeout = $TimeoutMs
        $client.SendTimeout = $TimeoutMs
        $stream = $client.GetStream()

        function Write-Packet([int]$id, [int]$type, [string]$body) {
            $bytes = [System.Text.Encoding]::ASCII.GetBytes($body)
            # laenge zaehlt ab dem id-Feld: 4 (id) + 4 (typ) + body + 2 Nullbytes
            $len = 10 + $bytes.Length
            $w = New-Object System.IO.MemoryStream
            $bw = New-Object System.IO.BinaryWriter($w)
            $bw.Write([int]$len); $bw.Write([int]$id); $bw.Write([int]$type)
            $bw.Write($bytes); $bw.Write([byte]0); $bw.Write([byte]0)
            $bw.Flush()
            $buf = $w.ToArray()
            $stream.Write($buf, 0, $buf.Length)
            $stream.Flush()
        }

        function Read-Packet {
            $head = New-Object byte[] 4
            $read = 0
            while ($read -lt 4) {
                $n = $stream.Read($head, $read, 4 - $read)
                if ($n -le 0) { return $null }
                $read += $n
            }
            $len = [BitConverter]::ToInt32($head, 0)
            if ($len -lt 8 -or $len -gt 8192) { return $null }
            $body = New-Object byte[] $len
            $read = 0
            while ($read -lt $len) {
                $n = $stream.Read($body, $read, $len - $read)
                if ($n -le 0) { return $null }
                $read += $n
            }
            [pscustomobject]@{
                Id   = [BitConverter]::ToInt32($body, 0)
                Type = [BitConverter]::ToInt32($body, 4)
                Body = [System.Text.Encoding]::ASCII.GetString($body, 8, [Math]::Max(0, $len - 10))
            }
        }

        Write-Packet 1 3 $Password
        $auth = Read-Packet
        if ($null -eq $auth -or $auth.Id -eq -1) { throw 'RCON-Anmeldung abgelehnt' }

        Write-Packet 2 2 $Command
        $resp = Read-Packet
        if ($resp) { return $resp.Body } else { return '' }
    } finally {
        $client.Close()
    }
}

# ---------------------------------------------------------------------------
# Faehrt einen Server ueber RCON herunter und wartet, bis der Prozess weg ist.
# Nur wenn er danach immer noch laeuft, wird hart beendet - dann ist ohnehin
# etwas kaputt.
# ---------------------------------------------------------------------------
function Stop-ServerGracefully {
    param(
        [Parameter(Mandatory = $true)][string]$Name,
        [Parameter(Mandatory = $true)][int]$RconPort,
        [Parameter(Mandatory = $true)][string]$Password,
        [int]$ProcessId = 0,
        [int]$WaitSeconds = 120
    )

    try {
        Send-RconCommand -Port $RconPort -Password $Password -Command 'stop' | Out-Null
        Write-Output "[stop] $Name : stop gesendet"
    } catch {
        Write-Warning "[stop] $Name : RCON nicht erreichbar ($($_.Exception.Message))"
    }

    if ($ProcessId -gt 0) {
        $p = Get-Process -Id $ProcessId -ErrorAction SilentlyContinue
        if ($p) {
            if ($p.WaitForExit($WaitSeconds * 1000)) {
                Write-Output "[stop] $Name : beendet"
                return $true
            }
            Write-Warning "[stop] $Name : nach $WaitSeconds s noch da, wird hart beendet"
            Stop-Process -Id $ProcessId -Force -ErrorAction SilentlyContinue
            return $false
        }
    }
    return $true
}
