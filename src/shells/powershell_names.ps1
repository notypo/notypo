# The parent already knows its commands; avoid a second cold discovery shell.
$env:NOTYPO_POWERSHELL_NAMES = try {
    $seenNames = @{}
    $names = @(foreach ($command in (Microsoft.PowerShell.Core\Get-Command -CommandType Alias,Function,Filter,Cmdlet -ErrorAction Ignore)) {
        if ($command.Name -match '^[A-Za-z0-9_][A-Za-z0-9_.-]{0,127}$' -and -not $seenNames.ContainsKey($command.Name)) {
            $seenNames[$command.Name] = $true
            [string]$command.Name
        }
    })
    $text = Microsoft.PowerShell.Utility\ConvertTo-Json -InputObject @{ version = 1; names = $names } -Depth 3 -Compress
    $limit = if ([Environment]::OSVersion.Platform -eq 'Win32NT') { 32000 } else { 65536 }
    if ($text.Length -le $limit) { $text }
} catch { }
