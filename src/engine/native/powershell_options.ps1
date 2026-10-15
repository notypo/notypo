# Classic completion omits functions' dynamic parameters. Bind the known
# static context, discover metadata, then validate each name with all of the
# original literal arguments. Get-Command never invokes the command body.
function __notypo_literal_argument($value) {
    $literal = [PSObject]::AsPSObject($value)
    if (-not $literal.PSObject.Properties['__notypo_literal']) {
        $literal.PSObject.Properties.Add([System.Management.Automation.PSNoteProperty]::new('__notypo_literal', $true))
    }
    return ,$literal
}
function __notypo_classic_options($command, $lookup, [string]$line, [int]$cursor) {
    $ast = [System.Management.Automation.Language.Parser]::ParseInput($line, [ref]$null, [ref]$null).Find({ $args[0] -is [System.Management.Automation.Language.CommandAst] }, $true)
    if (-not $ast) { return }
    $slot = -1
    for ($i = 1; $i -lt $ast.CommandElements.Count; $i++) {
        $element = $ast.CommandElements[$i]
        if ($element.Extent.EndOffset -eq $cursor -and $element.Extent.Text -eq '-') { $slot = $i }
    }
    if ($slot -lt 0) { return }
    $binding = [System.Management.Automation.Language.StaticParameterBinder]::BindCommand($ast, $true)
    $static = @()
    foreach ($entry in $binding.BoundParameters.GetEnumerator()) {
        $parameter = $command.Parameters[$entry.Key]
        if (-not $parameter -or $parameter.IsDynamic) { continue }
        $value = $entry.Value.Value
        if ($value -isnot [System.Management.Automation.Language.StringConstantExpressionAst] -and $value -isnot [System.Management.Automation.Language.ConstantExpressionAst]) { return }
        if ($parameter.ParameterType -eq [System.Management.Automation.SwitchParameter]) {
            if ($value.Value -isnot [bool] -or -not $value.Value) { return }
            $static += '-' + $parameter.Name
        } else {
            $static += '-' + $parameter.Name
            $static += ,(__notypo_literal_argument $value.Value)
        }
    }
    $metadata = Microsoft.PowerShell.Core\Get-Command @lookup -ArgumentList $static -ErrorAction Stop | Microsoft.PowerShell.Utility\Select-Object -First 1
    $matches = @(foreach ($parameter in $metadata.Parameters.Values) {
        if (-not $parameter.IsDynamic) { continue }
        $arguments = @()
        $literal = $true
        for ($i = 1; $i -lt $ast.CommandElements.Count; $i++) {
            $element = $ast.CommandElements[$i]
            if ($i -eq $slot) { $arguments += '-' + $parameter.Name }
            elseif ($element -is [System.Management.Automation.Language.CommandParameterAst]) {
                if ($element.Argument) {
                    if ($element.Argument -isnot [System.Management.Automation.Language.StringConstantExpressionAst] -and $element.Argument -isnot [System.Management.Automation.Language.ConstantExpressionAst]) { $literal = $false; break }
                    $arguments += '-' + $element.ParameterName + ':' + [string]$element.Argument.Value
                } else { $arguments += '-' + $element.ParameterName }
            } elseif ($element -is [System.Management.Automation.Language.StringConstantExpressionAst] -or $element -is [System.Management.Automation.Language.ConstantExpressionAst]) {
                $arguments += ,(__notypo_literal_argument $element.Value)
            } else { $literal = $false; break }
        }
        if (-not $literal) { return }
        try {
            $checked = Microsoft.PowerShell.Core\Get-Command @lookup -ArgumentList $arguments -ErrorAction Stop | Microsoft.PowerShell.Utility\Select-Object -First 1
            if ($checked.Parameters.ContainsKey($parameter.Name)) {
                [System.Management.Automation.CompletionResult]::new(('-' + $parameter.Name), $parameter.Name, 'ParameterName', $parameter.Name)
            }
        } catch [System.Management.Automation.ParameterBindingException] { }
    })
    if ($matches.Count) {
        @{ start = $ast.CommandElements[$slot].Extent.StartOffset; length = 1; matches = $matches }
    }
}
