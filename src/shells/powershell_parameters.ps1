# Snapshot parameter completers and session command definitions, never the
# executable statements of the profile or dot-sourced script.
$env:NOTYPO_POWERSHELL_PARAMETERS = try {
    function __notypo_portable($block) {
        if ($block.Module) { return $false }
        $ast = $block.Ast
        foreach ($variable in $ast.FindAll({ $args[0] -is [System.Management.Automation.Language.VariableExpressionAst] }, $true)) {
            $path = $variable.VariablePath.UserPath
            if ($path -like 'env:*' -or $path -in @('args', '_', 'PSItem', 'null', 'true', 'false', 'this', 'input', 'ErrorActionPreference', 'PSBoundParameters')) { continue }
            if ($path.Contains(':')) { return $false }
            $scope = $variable.Parent
            while ($scope -and $scope -isnot [System.Management.Automation.Language.ScriptBlockAst]) { $scope = $scope.Parent }
            if (-not $scope) { return $false }
            $locals = @(foreach ($declaration in $scope.FindAll({
                $args[0] -is [System.Management.Automation.Language.ParameterAst] -or
                $args[0] -is [System.Management.Automation.Language.AssignmentStatementAst] -or
                $args[0] -is [System.Management.Automation.Language.ForEachStatementAst]
            }, $true)) {
                $owner = $declaration.Parent
                while ($owner -and $owner -isnot [System.Management.Automation.Language.ScriptBlockAst]) { $owner = $owner.Parent }
                if ($owner -ne $scope) { continue }
                if ($declaration -is [System.Management.Automation.Language.ParameterAst]) { $declaration.Name.VariablePath.UserPath }
                elseif ($declaration -is [System.Management.Automation.Language.ForEachStatementAst]) { $declaration.Variable.VariablePath.UserPath }
                else { foreach ($left in $declaration.Left.FindAll({ $args[0] -is [System.Management.Automation.Language.VariableExpressionAst] }, $true)) { $left.VariablePath.UserPath } }
            })
            if ($path -notin $locals) { return $false }
        }
        return $true
    }
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
            $supported = __notypo_portable $block
            $root = $block.Ast
            while ($root.Parent) { $root = $root.Parent }
            if ($supported) {
                if (-not $roots.Contains($root)) { $roots.Add($root) }
            }
            $namespaces = @(foreach ($using in $root.UsingStatements) {
                if ([string]$using.UsingStatementKind -eq 'Namespace') { $using.Extent.Text }
            })
            @{ parameter = $parameter; specific = $specific; body = (@($namespaces) + $block.ToString()) -join "`n"; portable = $supported }
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
            $definition.Extent.Text
        } })
        $text = (@($usings) + $definitions) -join "`n"
        if ($text -and -not (__notypo_portable ([scriptblock]::Create($text)))) { $portable = $false }
        if ($session -or $registrations.Count) {
            $commands.Add(@{ name = $command.Name; session = $session; portable = $portable; definitions = $text; registrations = @($registrations) })
        }
    }
    if ($commands.Count) {
        $text = Microsoft.PowerShell.Utility\ConvertTo-Json -InputObject @{ version = 1; commands = @($commands.ToArray()) } -Depth 6 -Compress
        $limit = if ([Environment]::OSVersion.Platform -eq 'Win32NT') { 32000 } else { 65536 }
        if ($text.Length -le $limit -and -not $text.Contains([char]0)) { $text }
    }
} catch { }
