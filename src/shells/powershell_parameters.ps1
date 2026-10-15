# Snapshot parameter completers and session command definitions, never the
# executable statements of the profile or dot-sourced script.
$env:NOTYPO_POWERSHELL_PARAMETERS = try {
    $flags = [System.Reflection.BindingFlags]'NonPublic,Instance'
    $context = $ExecutionContext.GetType().GetField('_context', $flags).GetValue($ExecutionContext)
    $table = $context.GetType().GetProperty('CustomArgumentCompleters', $flags).GetValue($context)
    $commands = [System.Collections.Generic.List[object]]::new()
    $seen = @{}
    $ast = [System.Management.Automation.Language.Parser]::ParseInput($history, [ref]$null, [ref]$null)
    foreach ($call in $ast.FindAll({ $args[0] -is [System.Management.Automation.Language.CommandAst] }, $true)) {
        $name = $call.GetCommandName()
        if (-not $name) { continue }
        $command = Microsoft.PowerShell.Core\Get-Command -Name ([WildcardPattern]::Escape($name)) -ErrorAction Ignore |
            Microsoft.PowerShell.Utility\Select-Object -First 1
        if ($command -and $command.CommandType -eq 'Alias') { $command = $command.ResolvedCommand }
        if (-not $command -or $command.CommandType -notin @('Function', 'Filter', 'Cmdlet') -or $seen.ContainsKey($command.Name)) { continue }
        $seen[$command.Name] = 1
        $roots = [System.Collections.Generic.List[object]]::new()
        $portable = $true
        $session = -not $command.Module -and $command.CommandType -in @('Function', 'Filter')
        if ($session) {
            $root = $command.ScriptBlock.Ast
            while ($root.Parent) { $root = $root.Parent }
            $roots.Add($root)
        }
        $registrations = @(if ($table) { foreach ($key in $table.Keys) {
            $parameter = $null
            $specific = $false
            if ($key.StartsWith($command.Name + ':', [StringComparison]::OrdinalIgnoreCase)) {
                $parameter = $key.Substring($command.Name.Length + 1)
                $specific = $true
            } elseif (-not $key.Contains(':')) { $parameter = $key }
            if (-not $parameter) { continue }
            $block = $table[$key]
            $saved = __notypo_capture $block
            $supported = $saved.portable
            $root = $block.Ast
            while ($root.Parent) { $root = $root.Parent }
            if ($supported -and -not $saved.capture) {
                if (-not $roots.Contains($root)) { $roots.Add($root) }
            }
            $namespaces = @(foreach ($using in $root.UsingStatements) {
                if ([string]$using.UsingStatementKind -eq 'Namespace') { $using.Extent.Text }
            })
            @{ parameter = $parameter; specific = $specific; body = $saved.body; portable = $supported; capture = $saved.capture }
        } })
        $usings = @(foreach ($root in $roots) { foreach ($using in $root.UsingStatements) {
            if ([string]$using.UsingStatementKind -eq 'Namespace') { $using.Extent.Text }
            else { $portable = $false }
        } }) | Microsoft.PowerShell.Utility\Select-Object -Unique
        $definitions = @(foreach ($root in $roots) { foreach ($definition in $root.FindAll({
            $args[0] -is [System.Management.Automation.Language.FunctionDefinitionAst] -or
            $args[0] -is [System.Management.Automation.Language.TypeDefinitionAst]
        }, $false)) {
            # Only top-level definitions. Nested definitions remain inside
            # their containing function or type, with their lexical scope.
            if ($definition.Parent -isnot [System.Management.Automation.Language.NamedBlockAst]) { continue }
            if ($session -and $definition -is [System.Management.Automation.Language.FunctionDefinitionAst] -and ($definition.Name -split ':')[-1] -eq $command.Name) {
                # Binding needs the signature and dynamic parameters. The
                # failed operation's begin/process/end bodies never run here.
                $signature = if ($definition.Body.ParamBlock) { ($definition.Body.ParamBlock.Attributes.Extent.Text -join ' ') + ' ' + $definition.Body.ParamBlock.Extent.Text } else { '' }
                $dynamic = if ($definition.Body.DynamicParamBlock) { $definition.Body.DynamicParamBlock.Extent.Text } else { '' }
                'function ' + $command.Name + ' {' + $signature + ' ' + $dynamic + ' process {} }'
                continue
            }
            $definition.Extent.Text
        } })
        $text = (@($usings) + $definitions) -join "`n"
        $commandCapture = $null
        if ($text) {
            $values = @{}
            $owner = if ($session) { $command.ScriptBlock } else { [scriptblock]::Create($text) }
            if (-not (__notypo_portable ([scriptblock]::Create($text)) $values $owner)) { $portable = $false }
            if ($session -and $values.Count) {
                $commandCapture = @{ module = ''; definitions = $text; variables = @($values.Values) }
            }
            if (-not $session -and $values.Count) { $portable = $false }
        }
        if ($session -or $registrations.Count) {
            $commands.Add(@{ name = $command.Name; session = $session; portable = $portable; definitions = $text; capture = $commandCapture; registrations = @($registrations) })
        }
    }
    if ($commands.Count) {
        $text = Microsoft.PowerShell.Utility\ConvertTo-Json -InputObject @{ version = 2; commands = @($commands.ToArray()) } -Depth 30 -Compress
        $limit = if ([Environment]::OSVersion.Platform -eq 'Win32NT') { 32000 } else { 65536 }
        if ($text.Length -le $limit -and -not $text.Contains([char]0)) { $text }
    }
} catch { }
