# Pure dependency-setup regressions. No installer, browser, audio or GUI actions.
$ErrorActionPreference = 'Stop'
$path = Join-Path $PSScriptRoot '../desktop/src-tauri/windows/steelseries-gg.ps1'
$tokens = $null; $errors = $null
$ast = [Management.Automation.Language.Parser]::ParseFile($path, [ref]$tokens, [ref]$errors)
if ($errors.Count) { throw ($errors | Out-String) }
foreach ($definition in $ast.FindAll({ param($node) $node -is [Management.Automation.Language.FunctionDefinitionAst] }, $false)) {
    Invoke-Expression $definition.Extent.Text
}
function Must-Fail([scriptblock]$Action) {
    $failed = $false
    try { & $Action } catch { $failed = $true }
    if (!$failed) { throw 'An unsafe or unconsented operation was accepted.' }
}
Assert-OfficialUri ([Uri]'https://steelseries.com/gg/downloads/gg/latest/windows')
Assert-OfficialUri ([Uri]'https://engine.steelseriescdn.com/SteelSeriesGG121.0.0Setup.exe')
foreach ($url in @('http://steelseries.com/gg', 'https://steelseries.com.example.com/a', 'https://example.com/a', 'https://user@steelseries.com/a', 'https://steelseries.com:444/a', 'file:///C:/setup.exe')) {
    Must-Fail { Assert-OfficialUri ([Uri]$url) }
}
$Consent = $false
Must-Fail { Install-LatestGg }
$Consent = $true
function Get-LatestGg { @{version='121.0.0';url='https://example.invalid/must-not-download'} }
foreach ($installedVersion in @('121.0.0', '122.0.0')) {
    function Get-GgInstallation { @{installed=$true;version=$installedVersion} }
    $result = Install-LatestGg
    if ($result.result -ne 'already_installed' -or $result.version -ne $installedVersion) {
        throw 'Existing current/newer GG must be reused without a download or downgrade.'
    }
}
function Get-GgInstallation { @{installed=$true;version=$null} }
Must-Fail { Install-LatestGg }
foreach ($signatureCase in @(
    @{status='Valid';subject='CN=Other, O=Unrelated Company, C=US';accept=$false},
    @{status='NotTrusted';subject='CN=GN Hearing A/S, O=GN Hearing A/S, C=DK';accept=$false},
    @{status='Valid';subject='CN=GN Hearing A/S, O=GN Hearing A/S, C=DK';accept=$true},
    @{status='Valid';subject='CN=SteelSeries ApS, O=SteelSeries ApS, C=DK';accept=$true}
)) {
    function Get-AuthenticodeSignature {
        @{Status=$signatureCase.status;SignerCertificate=@{Subject=$signatureCase.subject}}
    }
    if ($signatureCase.accept) { Assert-SteelSeriesSignature 'unused.exe' }
    else { Must-Fail { Assert-SteelSeriesSignature 'unused.exe' } }
}
# Exercise the real download/validation/cleanup flow using inert compiled metadata
# and in-memory HTTP responses. Start-Process is replaced; no fixture is run.
Add-Type -AssemblyName System.Net.Http
$fixtureDirectory = Join-Path ([IO.Path]::GetTempPath()) ('Jack-GG-tests-' + [Guid]::NewGuid().ToString('N'))
$null = New-Item -ItemType Directory -Path $fixtureDirectory
$goodFixture = Join-Path $fixtureDirectory 'good.dll'
$wrongFixture = Join-Path $fixtureDirectory 'wrong.dll'
$compiler = Join-Path $env:SystemRoot 'Microsoft.NET/Framework64/v4.0.30319/csc.exe'
try {
    foreach ($fixture in @(@{path=$goodFixture;product='SteelSeries GG';name='Good'}, @{path=$wrongFixture;product='Unrelated';name='Wrong'})) {
        $sourcePath = $fixture.path + '.cs'
        [IO.File]::WriteAllText($sourcePath, @"
using System.Reflection;
[assembly: AssemblyProduct("$($fixture.product)")]
[assembly: AssemblyInformationalVersion("130.0.0")]
[assembly: AssemblyFileVersion("130.0.0.0")]
public class $($fixture.name)MetadataFixture { }
"@)
        & $compiler /nologo /target:library "/out:$($fixture.path)" $sourcePath
        if ($LASTEXITCODE -ne 0) { throw 'Metadata fixture compilation failed.' }
    }
    function Get-LatestGg { @{version='130.0.0';url='https://engine.steelseriescdn.com/SteelSeriesGG130.0.0Setup.exe'} }
    function Get-GgInstallation {
        if ($script:launched -and $script:scenario.confirm) { return @{installed=$true;version='130.0.0'} }
        @{installed=!!$script:scenario.before;version=$script:scenario.before}
    }
    function Assert-SteelSeriesSignature([string]$Path) {
        $script:downloadPath = $Path
        if (!$script:scenario.signature) { throw 'Test signature rejection.' }
    }
    function New-DownloadClient {
        $script:fetches++
        $client = [pscustomobject]@{}
        $client | Add-Member ScriptMethod GetAsync {
            param($Url, $Completion)
            $response = [Net.Http.HttpResponseMessage]::new([Net.HttpStatusCode]$script:scenario.http)
            $response.Content = [Net.Http.ByteArrayContent]::new([byte[]]$script:fixtureBytes)
            $task = [Threading.Tasks.TaskCompletionSource[Net.Http.HttpResponseMessage]]::new()
            $task.SetResult($response)
            $task.Task
        }
        $client | Add-Member ScriptMethod Dispose { }
        $client
    }
    function Start-Process {
        param($FilePath, $Verb, $WindowStyle, [switch]$Wait, [switch]$PassThru)
        if ($Verb -ne 'RunAs' -or $WindowStyle -ne 'Normal' -or !$Wait -or !$PassThru) { throw 'Interactive consent/UAC flags changed.' }
        $script:launched = $true
        [pscustomobject]@{ExitCode=$script:scenario.exit}
    }
    foreach ($script:scenario in @(
        @{name='fresh';before=$null;exit=0;confirm=$true;signature=$true;http=200;success=$true;start=$true},
        @{name='upgrade';before='120.0.0';exit=0;confirm=$true;signature=$true;http=200;success=$true;start=$true},
        @{name='reboot';before=$null;exit=3010;confirm=$true;signature=$true;http=200;success=$true;start=$true},
        @{name='cancel';before=$null;exit=1602;confirm=$false;signature=$true;http=200;success=$false;start=$true},
        @{name='unconfirmed';before=$null;exit=0;confirm=$false;signature=$true;http=200;success=$false;start=$true},
        @{name='signature';before=$null;signature=$false;http=200;success=$false;start=$false},
        @{name='product';before=$null;signature=$true;http=200;success=$false;start=$false},
        @{name='http';before=$null;signature=$true;http=503;success=$false;start=$false}
    )) {
        $script:launched = $false; $script:fetches = 0; $script:downloadPath = $null
        $fixturePath = if ($scenario.name -eq 'product') { $wrongFixture } else { $goodFixture }
        $script:fixtureBytes = [IO.File]::ReadAllBytes($fixturePath)
        if ($scenario.success) {
            $result = Install-LatestGg
            if ($result.result -ne 'installed' -or $result.version -ne '130.0.0' -or $result.reboot_required -ne ($scenario.exit -eq 3010)) { throw "Incorrect result: $($scenario.name)" }
        } else { Must-Fail { Install-LatestGg } }
        if ($script:fetches -ne 1 -or $script:launched -ne $scenario.start) { throw "Unexpected download/launch: $($scenario.name)" }
        if ($script:downloadPath -and (Test-Path -LiteralPath (Split-Path $script:downloadPath -Parent))) { throw 'Installer temporary directory was not cleaned.' }
    }
    $Consent = $false; $script:fetches = 0; $script:launched = $false
    Must-Fail { Install-LatestGg }
    if ($script:fetches -or $script:launched) { throw 'Consent refusal performed a download or launch.' }
} finally {
    foreach ($file in @($goodFixture, $wrongFixture, ($goodFixture + '.cs'), ($wrongFixture + '.cs'))) {
        if (Test-Path -LiteralPath $file) { Remove-Item -LiteralPath $file -Force }
    }
    Remove-Item -LiteralPath $fixtureDirectory -Force
}
Write-Output 'GG setup: host/signature/consent/reuse guards and mocked install/upgrade/reboot/failure/cleanup passed. No installer was executed.'
