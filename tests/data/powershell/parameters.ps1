using namespace System.Management.Automation
using namespace System.Management.Automation.Language

class FixtureCompleter : IArgumentCompleter {
    [System.Collections.Generic.IEnumerable[CompletionResult]] CompleteArgument(
        [string] $commandName, [string] $parameterName, [string] $word,
        [CommandAst] $ast, [System.Collections.IDictionary] $bound
    ) {
        return [CompletionResult[]] @([CompletionResult]::new('typed-one', 'typed-one', 'ParameterValue', 'A typed callback.'))
    }
}

class FixtureFactory : ArgumentCompleterFactoryAttribute {
    [IArgumentCompleter] Create() { return [FixtureCompleter]::new() }
}

function Get-FixtureChoices {
    param($scope)
    if ($scope -eq 'east') { return @('east-one', 'east-two') }
    @('west-one', 'team one', 'it''s;touch operation-marker', 'õun/üks', 'zone:blue', '-dash-value')
}

function Invoke-Fixture {
    [CmdletBinding()]
    param(
        [Parameter(Position=0)][Alias('s')][ValidateSet('west', 'east')][string] $Scope,
        [Parameter(Position=1)][Alias('t')][ArgumentCompleter({
            param($commandName, $parameterName, $word, $ast, $bound)
            if ($word -ne '') { throw 'completion must enumerate with an empty prefix' }
            [IO.File]::AppendAllText((Join-Path $env:NOTYPO_TEST_PS_MARKERS 'callbacks'), $bound['Scope'] + [char]10)
            foreach ($value in (Get-FixtureChoices $bound['Scope'])) {
                $quoted = "'" + $value.Replace("'", "''") + "'"
                [CompletionResult]::new($quoted, $value, 'ParameterValue', 'Resource from ' + $bound['Scope'])
            }
        })][string] $Target,
        [ArgumentCompletions('alpha', 'beta')][string] $Static,
        [ArgumentCompleter([FixtureCompleter])][string] $Typed,
        [FixtureFactory()][string] $Factory,
        [ArgumentCompleter({ param($a, $b, $c, $d, $e) })][string] $Empty,
        [ArgumentCompleter({ param($a, $b, $c, $d, $e); throw 'callback failed' })][string] $Broken,
        [ArgumentCompleter({ param($a, $b, $c, $d, $e); Start-Sleep -Seconds 10; 'too-late' })][string] $Slow,
        [ArgumentCompleter({
            param($a, $b, $c, $d, $e)
            foreach ($value in @('$secret', '$(touch operation-marker)', 'a;touch operation-marker', 'two words')) {
                [CompletionResult]::new($value, $value, 'ParameterValue', 'Unsafe code')
            }
            [CompletionResult]::new('file-one', 'file-one', 'ProviderItem', 'Filesystem fallback')
        })][string] $Unsafe,
        [ArgumentCompleter({ param($a, $b, $c, $d, $e); 1..20 | ForEach-Object { 'item-' + $_ } })][string] $Large,
        [ArgumentCompleter({ param($a, $b, $c, $d, $e); [Console]::Out.WriteLine('noise'); 'west-one' })][string] $Noise,
        [ArgumentCompleter({
            param($a, $b, $c, $d, $e)
            if ($env:HTTP_PROXY -ne 'http://127.0.0.1:9' -or $env:AWS_ACCESS_KEY_ID -or $env:AZURE_CLIENT_SECRET) { throw 'offline environment was not isolated' }
            'offline-correct'
        })][string] $Offline,
        [Parameter(Position=2)][ValidateSet('json', 'yaml')][string] $Format,
        [switch] $Loud,
        [bool] $Enabled
    )
    [IO.File]::WriteAllText((Join-Path $env:NOTYPO_TEST_PS_MARKERS 'operation-marker'), 'executed')
}

function Invoke-SetFixture {
    [CmdletBinding(DefaultParameterSetName='First')]
    param(
        [Parameter(Position=0,ParameterSetName='First')][ArgumentCompletions('first-one')][string] $First,
        [Parameter(Position=0,ParameterSetName='Second')][ArgumentCompletions('second-one')][string] $Second,
        [Parameter(ParameterSetName='Second')][switch] $ChooseSecond
    )
    [IO.File]::WriteAllText((Join-Path $env:NOTYPO_TEST_PS_MARKERS 'operation-marker'), 'executed')
}

function Invoke-AmbiguousFixture {
    [CmdletBinding()]
    param(
        [Parameter(Position=0,ParameterSetName='First')][ArgumentCompletions('first-one')][string] $First,
        [Parameter(Position=0,ParameterSetName='Second')][ArgumentCompletions('second-one')][string] $Second
    )
    [IO.File]::WriteAllText((Join-Path $env:NOTYPO_TEST_PS_MARKERS 'operation-marker'), 'executed')
}

# These statements run only in the parent session, never during completion.
Set-Alias -Name fx -Value Invoke-Fixture
[IO.File]::AppendAllText((Join-Path $env:NOTYPO_TEST_PS_MARKERS 'parent-statements'), 'parent' + [char]10)
