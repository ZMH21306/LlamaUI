# Minimal TCP-based HTTP proxy (bypasses http.sys)
param($Port = 8888)

$listener = [System.Net.Sockets.TcpListener]::new([System.Net.IPAddress]::Loopback, $Port)
$listener.Start()
Write-Output "[proxy] TCP Listening on 127.0.0.1:$Port -> https://rsproxy.cn"

function Rewrite-ConfigJson([byte[]]$body) {
    $s = [System.Text.Encoding]::UTF8.GetString($body)
    $s = $s -replace '"https://rsproxy.cn/api/v1/crates"', "`"http://127.0.0.1:$Port/api/v1/crates`""
    $s = $s -replace '"https://rsproxy.cn"', "`"http://127.0.0.1:$Port`""
    return [System.Text.Encoding]::UTF8.GetBytes($s)
}

function Read-Request($stream) {
    $ms = New-Object System.IO.MemoryStream
    $buf = New-Object byte[] 8192
    $headerEnd = $null
    while ($true) {
        $n = $stream.Read($buf, 0, $buf.Length)
        if ($n -le 0) { break }
        $ms.Write($buf, 0, $n)
        $data = $ms.ToArray()
        $text = [System.Text.Encoding]::ASCII.GetString($data)
        $idx = $text.IndexOf("`r`n`r`n")
        if ($idx -ge 0) {
            $headerEnd = $idx + 4
            break
        }
        if ($ms.Length -gt 1MB) { break }
    }
    $all = $ms.ToArray()
    if ($headerEnd -eq $null) { return $null }
    $headerText = [System.Text.Encoding]::ASCII.GetString($all, 0, $headerEnd)
    $lines = $headerText -split "`r`n"
    $first = $lines[0] -split ' ', 3
    if ($first.Length -lt 2) { return $null }
    $method = $first[0]
    $path = $first[1]
    $headers = @{}
    $contentLength = 0
    for ($i = 1; $i -lt $lines.Length; $i++) {
        $l = $lines[$i]
        if ($l -eq '') { continue }
        $colon = $l.IndexOf(':')
        if ($colon -gt 0) {
            $k = $l.Substring(0, $colon).Trim()
            $v = $l.Substring($colon + 1).Trim()
            $headers[$k.ToLowerInvariant()] = $v
            if ($k.ToLowerInvariant() -eq 'content-length') {
                [int]::TryParse($v, [ref]$contentLength) | Out-Null
            }
        }
    }
    $bodyBytes = $null
    $already = $all.Length - $headerEnd
    if ($contentLength -gt 0) {
        $need = $contentLength - $already
        $combined = New-Object System.IO.MemoryStream
        if ($already -gt 0) { $combined.Write($all, $headerEnd, $already) }
        if ($need -gt 0) {
            $buf2 = New-Object byte[] $need
            $read = 0
            while ($read -lt $need) {
                $n = $stream.Read($buf2, $read, $need - $read)
                if ($n -le 0) { break }
                $read += $n
            }
            $combined.Write($buf2, 0, $read)
        }
        $bodyBytes = $combined.ToArray()
    }
    return [PSCustomObject]@{
        Method = $method
        Path = $path
        Headers = $headers
        Body = $bodyBytes
    }
}

function Send-Response($stream, $statusCode, $statusText, $contentType, [byte[]]$body) {
    if ($body -eq $null) { $body = @() }
    $h = "HTTP/1.1 $statusCode $statusText`r`n"
    $h += "Content-Type: $contentType`r`n"
    $h += "Content-Length: $($body.Length)`r`n"
    $h += "Connection: close`r`n"
    $h += "`r`n"
    $headerBytes = [System.Text.Encoding]::ASCII.GetBytes($h)
    $stream.Write($headerBytes, 0, $headerBytes.Length)
    if ($body.Length -gt 0) { $stream.Write($body, 0, $body.Length) }
    $stream.Flush()
}

$count = 0
while ($count -lt 5000) {
    $count++
    $client = $null
    $stream = $null
    try {
        $client = $listener.AcceptTcpClient()
        $client.ReceiveTimeout = 30000
        $client.SendTimeout = 600000
        $stream = $client.GetStream()

        $req = Read-Request $stream
        if ($req -eq $null) { continue }

        $upstream = "https://rsproxy.cn$($req.Path)"
        Write-Output "[proxy] $($req.Method) $($req.Path)"

        $bodyBytes = $null
        $status = 200
        $statusText = "OK"
        $contentType = "application/octet-stream"

        try {
            $iwrParams = @{
                Uri = $upstream
                Method = $req.Method
                TimeoutSec = 600
                UseBasicParsing = $true
                ErrorAction = 'Stop'
            }
            if ($req.Body -ne $null -and $req.Body.Length -gt 0) {
                $iwrParams['Body'] = $req.Body
                if ($req.Headers['content-type']) {
                    $iwrParams['ContentType'] = $req.Headers['content-type']
                } else {
                    $iwrParams['ContentType'] = 'application/octet-stream'
                }
            }
            $result = Invoke-WebRequest @iwrParams
            $status = [int]$result.StatusCode
            $statusText = "OK"
            if ($result.Headers['Content-Type']) { $contentType = $result.Headers['Content-Type'] }
            if ($result.Content -is [byte[]]) {
                $bodyBytes = $result.Content
            } else {
                $bodyBytes = [System.Text.Encoding]::UTF8.GetBytes([string]$result.Content)
            }
        } catch {
            $webEx = $_.Exception
            $resp2 = $webEx.Response
            if ($resp2 -ne $null) {
                $status = [int]$resp2.StatusCode
                $statusText = [string]$resp2.StatusCode
                try {
                    $rs = $resp2.GetResponseStream()
                    $rdr = New-Object System.IO.StreamReader($rs)
                    $t = $rdr.ReadToEnd()
                    $rdr.Close()
                    $bodyBytes = [System.Text.Encoding]::UTF8.GetBytes($t)
                } catch {
                    $bodyBytes = [System.Text.Encoding]::UTF8.GetBytes($webEx.ToString())
                }
            } else {
                $status = 502
                $statusText = "Bad Gateway"
                $bodyBytes = [System.Text.Encoding]::UTF8.GetBytes($webEx.ToString())
            }
            Write-Output "[proxy]   upstream-err: $status"
        }

        if ($req.Path -like "/index/config.json" -and $bodyBytes -ne $null) {
            $bodyBytes = Rewrite-ConfigJson $bodyBytes
            $contentType = "application/json"
        }

        Send-Response $stream $status $statusText $contentType $bodyBytes
        $nbytes = if ($bodyBytes) { $bodyBytes.Length } else { 0 }
        Write-Output "[proxy]   -> $status $nbytes B"
    } catch {
        $msg = $_.Exception.Message
        Write-Output "[proxy] err (req#$count): $msg"
    } finally {
        if ($stream -ne $null) { try { $stream.Close() } catch {} }
        if ($client -ne $null) { try { $client.Close() } catch {} }
    }
}

$listener.Stop()
Write-Output "[proxy] done after $count requests"
