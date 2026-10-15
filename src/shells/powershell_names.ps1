# The parent already knows its commands; avoid a second cold discovery shell.
$notypo_names_file = $null
$env:NOTYPO_POWERSHELL_NAMES_FILE = $null
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
    $bytes = [Text.UTF8Encoding]::new($false).GetBytes($text)
    if ($bytes.Length -le $limit) { $text }
    elseif ($bytes.Length -le 1048576) {
        # Large module inventories exceed one environment variable's limit.
        # This temporary file holds command names only, never argument data.
        $notypo_names_file = [IO.Path]::Combine([IO.Path]::GetTempPath(), ('notypo-powershell-names-' + [guid]::NewGuid().ToString('N') + '.json'))
        $stream = [IO.File]::Open($notypo_names_file, [IO.FileMode]::CreateNew, [IO.FileAccess]::Write, [IO.FileShare]::None)
        try { $stream.Write($bytes, 0, $bytes.Length) } finally { $stream.Dispose() }
        $env:NOTYPO_POWERSHELL_NAMES_FILE = $notypo_names_file
    }
} catch {
    if ($notypo_names_file -and [IO.File]::Exists($notypo_names_file)) { [IO.File]::Delete($notypo_names_file) }
    $notypo_names_file = $null
    $env:NOTYPO_POWERSHELL_NAMES_FILE = $null
}
