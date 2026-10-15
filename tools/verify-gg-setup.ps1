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
Write-Output 'GG setup: syntax, official-host restrictions, consent, duplicate-install and downgrade guards passed.'
