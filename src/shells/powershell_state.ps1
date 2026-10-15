# Bounded primitive state and helper definitions shared by both exporters.
function __notypo_data($value, [int]$depth, $limits) {
    $limits.nodes++
    if ($depth -gt 8 -or $limits.nodes -gt 1024) { throw 'captured state exceeds limits' }
    if ($null -eq $value) { return @{ kind = 'Null'; value = $null } }
    # Invoke Object.GetType directly, bypassing attached script methods.
    $type = [object].GetMethod('GetType').Invoke($value.PSObject.BaseObject, $null)
    $kind = if ($type -eq [string]) { 'String' }
        elseif ($type -eq [bool]) { 'Boolean' }
        elseif ($type -eq [int]) { 'Int32' }
        elseif ($type -eq [long]) { 'Int64' }
        elseif ($type -eq [double]) { if ([double]::IsNaN($value) -or [double]::IsInfinity($value)) { throw 'nonfinite state' }; 'Double' }
    if ($kind) {
        $raw = if ($kind -eq 'String') { [string]::new($value.PSBase.ToCharArray()) } else { $value.PSObject.BaseObject }
        return @{ kind = $kind; value = $raw }
    }
    if ($type -eq [object[]]) {
        $items = @(foreach ($item in $value) { __notypo_data $item ($depth + 1) $limits })
        return @{ kind = 'Array'; value = $items }
    }
    if ($type -eq [string[]] -or $type -eq [bool[]] -or $type -eq [int[]] -or $type -eq [long[]] -or $type -eq [double[]]) {
        $kind = switch ($type.GetElementType().FullName) { System.String {'StringArray'} System.Boolean {'BooleanArray'} System.Int32 {'Int32Array'} System.Int64 {'Int64Array'} System.Double {'DoubleArray'} }
        $items = @(foreach ($item in $value) { __notypo_data $item ($depth + 1) $limits })
        return @{ kind = $kind; value = $items }
    }
    if ($type -eq [hashtable] -or $type -eq [System.Collections.Specialized.OrderedDictionary]) {
        $seen = @{}
        $items = @(foreach ($item in $value.PSBase.GetEnumerator()) {
            if ($item.Key -isnot [string] -or $seen.ContainsKey($item.Key)) { throw 'unsupported captured dictionary key' }
            $seen[$item.Key] = $true
            @{ key = $item.Key; value = (__notypo_data $item.Value ($depth + 1) $limits) }
        })
        return @{ kind = $(if ($type -eq [hashtable]) { 'Map' } else { 'OrderedMap' }); value = $items }
    }
    throw 'captured objects are unsupported'
}
function __notypo_portable($block, $capture = $null, $owner = $block, $limits = $null) {
    if ($null -eq $limits) { $limits = @{ nodes = 0 } }
    if ($block.Module -and $null -eq $capture) { return $false }
    $ast = $block.Ast
    foreach ($variable in $ast.FindAll({ $args[0] -is [System.Management.Automation.Language.VariableExpressionAst] }, $true)) {
        $path = $variable.VariablePath.UserPath
        if ($path -like 'env:*' -or $path -in @('args', '_', 'PSItem', 'null', 'true', 'false', 'this', 'input', 'ErrorActionPreference', 'PSBoundParameters')) { continue }
        $name = $path
        $storage = 'script'
        if ($path.Contains(':')) {
            $parts = $path -split ':', 2
            if ($parts[0] -notin @('script','global')) { return $false }
            $storage = $parts[0].ToLowerInvariant()
            $name = $parts[1]
        }
        $scope = $variable.Parent
        while ($scope -and $scope -isnot [System.Management.Automation.Language.ScriptBlockAst]) { $scope = $scope.Parent }
        if (-not $scope) { return $false }
        $locals = @()
        while ($scope -and $scope.Extent.StartOffset -ge $ast.Extent.StartOffset -and $scope.Extent.EndOffset -le $ast.Extent.EndOffset) {
            $locals += @(foreach ($declaration in $scope.FindAll({
            $args[0] -is [System.Management.Automation.Language.ParameterAst] -or
            $args[0] -is [System.Management.Automation.Language.AssignmentStatementAst] -or
            $args[0] -is [System.Management.Automation.Language.ForEachStatementAst]
        }, $true)) {
            $declarationOwner = $declaration.Parent
            while ($declarationOwner -and $declarationOwner -isnot [System.Management.Automation.Language.ScriptBlockAst]) { $declarationOwner = $declarationOwner.Parent }
            if ($declarationOwner -ne $scope) { continue }
            if ($declaration -is [System.Management.Automation.Language.ParameterAst]) { $declaration.Name.VariablePath.UserPath }
            elseif ($declaration -is [System.Management.Automation.Language.ForEachStatementAst]) { $declaration.Variable.VariablePath.UserPath }
            else { foreach ($left in $declaration.Left.FindAll({ $args[0] -is [System.Management.Automation.Language.VariableExpressionAst] }, $true)) { $left.VariablePath.UserPath } }
            })
            $scope = $scope.Parent
            while ($scope -and $scope -isnot [System.Management.Automation.Language.ScriptBlockAst]) { $scope = $scope.Parent }
        }
        if (-not $path.Contains(':') -and $path -in $locals) { continue }
        if ($null -eq $capture -or $name -notmatch '^[A-Za-z0-9_]+$' -or $name -match '^(?:__)?notypo') { return $false }
        $key = $storage + ':' + $name
        if ($capture.ContainsKey($key)) { continue }
        $saved = if ($owner.Module) { $owner.Module.SessionState.PSVariable.Get($path) } else { $ExecutionContext.SessionState.PSVariable.Get($path) }
        if (-not $saved) { return $false }
        if ([object].GetMethod('GetType').Invoke($saved.PSObject.BaseObject, $null).Assembly -ne [System.Management.Automation.PSVariable].Assembly) { return $false }
        try { $data = __notypo_data $saved.PSBase.Value 0 $limits } catch { return $false }
        $capture[$key] = @{ name = $name; scope = $storage; value = $data }
    }
    return $true
}
function __notypo_capture($block) {
    $limits = @{ nodes = 0 }
    $values = @{}
    $supported = __notypo_portable $block $values $block $limits
    $root = $block.Ast
    while ($root.Parent) { $root = $root.Parent }
    if ($block.Module -and $block.Module.Definition) {
        $root = ([scriptblock]::Create($block.Module.Definition)).Ast
    }
    $namespaces = @(foreach ($using in $root.UsingStatements) {
        if ([string]$using.UsingStatementKind -eq 'Namespace') { $using.Extent.Text }
        else { $supported = $false }
    })
    $definitions = @(foreach ($definition in $root.FindAll({
        $args[0] -is [System.Management.Automation.Language.FunctionDefinitionAst] -or
        $args[0] -is [System.Management.Automation.Language.TypeDefinitionAst]
    }, $false)) {
        if ($definition.Parent -is [System.Management.Automation.Language.NamedBlockAst]) { $definition.Extent.Text }
    })
    if ($block.Module) {
        $definitions += @(foreach ($helper in (& $block.Module { Microsoft.PowerShell.Management\Get-ChildItem -LiteralPath Function: })) {
            if ($helper.Module -ne $block.Module) { continue }
            if ($helper.Name -notmatch '^[A-Za-z0-9_.-]{1,128}$') { $supported = $false; continue }
            'function ' + $helper.Name + ' {' + $helper.Definition + '}'
        })
    }
    $text = (@($namespaces) + $definitions) -join "`n"
    if ($text -and -not (__notypo_portable ([scriptblock]::Create($text)) $values $block $limits)) { $supported = $false }
    $capture = if ($block.Module -or $values.Count) {
        @{ module = $(if ($block.Module) { [string]$block.Module.Name } else { '' }); definitions = $text; variables = @($values.Values) }
    } else { $null }
    @{ portable = $supported; capture = $capture; definitions = $text; body = (@($namespaces) + $block.ToString()) -join "`n" }
}
