# Filesystem drive definitions contain data; credentials are never copied.
$env:NOTYPO_POWERSHELL_DRIVES = try {
    $drives = @(foreach ($drive in (Microsoft.PowerShell.Management\Get-PSDrive -PSProvider FileSystem -ErrorAction Ignore)) {
        if ($drive.Name -notmatch '^[A-Za-z][A-Za-z0-9_]{0,31}$' -or $drive.Credential.UserName) { continue }
        if ($drive.Root.StartsWith('\\') -or $drive.Root.StartsWith('//')) { continue }
        @{ name = [string]$drive.Name; root = [string]$drive.Root }
    })
    $text = Microsoft.PowerShell.Utility\ConvertTo-Json -InputObject @{ version = 1; drives = $drives } -Depth 4 -Compress
    if ($text.Length -le 16000) { $text }
} catch { }
