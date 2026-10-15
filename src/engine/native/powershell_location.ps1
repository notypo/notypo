# Restore only validated filesystem drive data in this child process.
if ($env:NOTYPO_PS_DRIVES) {
    if ($env:NOTYPO_PS_DRIVES.Length -gt 65536) { throw 'PowerShell drive context exceeds limits' }
    $driveContext = Microsoft.PowerShell.Utility\ConvertFrom-Json -InputObject $env:NOTYPO_PS_DRIVES
    if ($driveContext.version -ne 1 -or @($driveContext.PSObject.Properties.Name).Count -ne 2 -or $driveContext.drives -isnot [array]) { throw 'invalid PowerShell drive context' }
    $seenDrives = @{}
    foreach ($drive in $driveContext.drives) {
        $name = $drive.name
        $root = $drive.root
        if (@($drive.PSObject.Properties.Name).Count -ne 2 -or $name -isnot [string] -or $root -isnot [string]) { throw 'invalid PowerShell drive record' }
        if ($name -notmatch '^[A-Za-z][A-Za-z0-9_]{0,31}$' -or $seenDrives.ContainsKey($name) -or $root.Length -gt 4096 -or $root -match '[\x00-\x1f\x7f]') { throw 'invalid PowerShell drive data' }
        $seenDrives[$name] = $true
        if ($root.StartsWith('\\') -or $root.StartsWith('//')) { throw 'remote PowerShell drive context is unsupported' }
        if ([Environment]::OSVersion.Platform -eq 'Win32NT') {
            if ($root -notmatch '^[A-Za-z]:[\\/]') { throw 'PowerShell drive root must be absolute' }
        } elseif (-not $root.StartsWith('/')) { throw 'PowerShell drive root must be absolute' }
        $existing = Microsoft.PowerShell.Management\Get-PSDrive -Name $name -ErrorAction Ignore
        if ($existing) {
            if ($existing.Provider.Name -ne 'FileSystem' -or $existing.Root -ne $root) { throw 'PowerShell drive conflicts with the captured session' }
        } else {
            $null = Microsoft.PowerShell.Management\New-PSDrive -Name $name -PSProvider FileSystem -Root $root -Scope Global
        }
    }
}
[Environment]::SetEnvironmentVariable('NOTYPO_PS_DRIVES', $null)
if ($env:NOTYPO_PS_LOCATION) { Microsoft.PowerShell.Management\Set-Location -LiteralPath $env:NOTYPO_PS_LOCATION }
