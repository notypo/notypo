# Native registrations retain their bounded data scope and private helpers.
$env:NOTYPO_POWERSHELL_COMPLETIONS = try {
    $keys = @(foreach ($call in [System.Management.Automation.Language.Parser]::ParseInput($history, [ref]$null, [ref]$null).FindAll({ $args[0] -is [System.Management.Automation.Language.CommandAst] }, $true)) {
        $name = $call.GetCommandName()
        if (-not $name) { continue }
        $name
        $command = Microsoft.PowerShell.Core\Get-Command -Name ([WildcardPattern]::Escape($name)) -ErrorAction Ignore | Microsoft.PowerShell.Utility\Select-Object -First 1
        if ($command -and $command.CommandType -eq 'Alias') { $command = $command.ResolvedCommand }
        if ($command -and $command.CommandType -eq 'Application') { $command.Name; [IO.Path]::GetFileNameWithoutExtension($command.Name) }
    })
    $flags = [System.Reflection.BindingFlags]'NonPublic,Instance'
    $context = $ExecutionContext.GetType().GetField('_context', $flags).GetValue($ExecutionContext)
    $table = $context.GetType().GetProperty('NativeArgumentCompleters', $flags).GetValue($context)
    $seen = @{}
    $registrations = @(foreach ($name in $keys) {
        $block = $null
        if (-not $table -or $seen.ContainsKey($name)) { continue }
        if (-not $table.TryGetValue($name, [ref]$block)) {
            # PowerShell 7.6's NativeFallback registration uses this table key.
            # Older shells have no entry and simply contribute no fallback.
            $null = $table.TryGetValue('___ps::<native_fallback_key>@@___', [ref]$block)
        }
        if (-not $block) { continue }
        $seen[$name] = $true
        $saved = __notypo_capture $block
        @{ name = $name; body = $saved.body; portable = $saved.portable; definitions = $saved.definitions; capture = $saved.capture }
    })
    if ($registrations.Count) {
        $text = Microsoft.PowerShell.Utility\ConvertTo-Json -InputObject @{ version = 2; registrations = $registrations } -Depth 30 -Compress
        $limit = if ([Environment]::OSVersion.Platform -eq 'Win32NT') { 32000 } else { 65536 }
        if ($text.Length -le $limit -and -not $text.Contains([char]0)) { $text }
    }
} catch { }
