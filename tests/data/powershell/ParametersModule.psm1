using namespace System.Management.Automation

$script:Choices = @('private-one', 'private-two')
function Invoke-ModuleFixture {
    [CmdletBinding()]
    param([string] $Target)
    [IO.File]::WriteAllText((Join-Path $env:NOTYPO_TEST_PS_MARKERS 'operation-marker'), 'executed')
}
Register-ArgumentCompleter -CommandName Invoke-ModuleFixture -ParameterName Target -ScriptBlock {
        param($commandName, $parameterName, $word, $ast, $bound)
        [IO.File]::AppendAllText((Join-Path $env:NOTYPO_TEST_PS_MARKERS 'module-callbacks'), 'called' + [char]10)
        foreach ($value in $script:Choices) {
            [CompletionResult]::new($value, $value, 'ParameterValue', 'Private module state')
        }
}
Export-ModuleMember -Function Invoke-ModuleFixture
