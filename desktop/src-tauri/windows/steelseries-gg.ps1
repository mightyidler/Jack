# Shared by the installer and Jack. Never changes audio settings or Windows defaults.
[CmdletBinding()]
param(
    [ValidateSet('Inspect', 'Latest', 'Install', 'Open', 'Start', 'EnsureInstalled', 'RequireInstalled', 'StopJack')]
    [string]$Mode = 'Inspect',
    [switch]$Consent,
    [switch]$Korean,
    [string]$JackPath
)
$ErrorActionPreference = 'Stop'
$ProgressPreference = 'SilentlyContinue'
[Console]::OutputEncoding = New-Object System.Text.UTF8Encoding($false)
Add-Type -AssemblyName System.Net.Http

function Text([string]$Ko, [string]$En) { if ($Korean) { $Ko } else { $En } }

function Get-GgInstallation {
    $candidates = New-Object 'System.Collections.Generic.List[string]'
    foreach ($hive in @([Microsoft.Win32.RegistryHive]::LocalMachine, [Microsoft.Win32.RegistryHive]::CurrentUser)) {
        foreach ($view in @([Microsoft.Win32.RegistryView]::Registry64, [Microsoft.Win32.RegistryView]::Registry32)) {
            $base = [Microsoft.Win32.RegistryKey]::OpenBaseKey($hive, $view)
            try {
                $uninstall = $base.OpenSubKey('SOFTWARE\Microsoft\Windows\CurrentVersion\Uninstall')
                if (!$uninstall) { continue }
                try {
                    foreach ($name in $uninstall.GetSubKeyNames()) {
                        $entry = $uninstall.OpenSubKey($name)
                        if (!$entry) { continue }
                        try {
                            if ([string]$entry.GetValue('DisplayName') -notmatch '^SteelSeries GG(?:\s|$)') { continue }
                            $location = [string]$entry.GetValue('InstallLocation')
                            if ($location) { $candidates.Add((Join-Path $location 'SteelSeriesGG.exe')) }
                            $icon = [string]$entry.GetValue('DisplayIcon')
                            if ($icon) { $candidates.Add(($icon -replace ',\s*-?\d+$', '').Trim('"')) }
                            $uninstaller = [string]$entry.GetValue('UninstallString')
                            if ($uninstaller -match '^"([^"\r\n]+\.exe)"') {
                                $candidates.Add((Join-Path (Split-Path $Matches[1]) 'SteelSeriesGG.exe'))
                            }
                        } finally { $entry.Dispose() }
                    }
                } finally { $uninstall.Dispose() }
            } finally { $base.Dispose() }
        }
    }
    foreach ($folder in @($env:ProgramW6432, $env:ProgramFiles, ${env:ProgramFiles(x86)}, $env:LOCALAPPDATA)) {
        if ($folder) { $candidates.Add((Join-Path $folder 'SteelSeries\GG\SteelSeriesGG.exe')) }
    }
    $exe = $null
    foreach ($candidate in ($candidates | Select-Object -Unique)) {
        if ((Split-Path $candidate -Leaf) -ieq 'SteelSeriesGG.exe' -and (Test-Path -LiteralPath $candidate -PathType Leaf)) {
            $exe = [IO.Path]::GetFullPath($candidate)
            break
        }
    }
    $version = $null
    if ($exe) {
        $raw = [Diagnostics.FileVersionInfo]::GetVersionInfo($exe).ProductVersion
        if ($raw -match '(\d+\.\d+\.\d+)') { $version = $Matches[1] }
    }
    [pscustomobject]@{
        installed = [bool]$exe
        version = $version
        executable = $exe
        running = [bool](Get-Process -Name SteelSeriesGG -ErrorAction SilentlyContinue)
    }
}

function Assert-OfficialUri([Uri]$Uri) {
    if ($Uri.Scheme -ne 'https' -or !$Uri.IsDefaultPort -or $Uri.UserInfo -or
        $Uri.Host -notin @('steelseries.com', 'engine.steelseriescdn.com', 'downloads.steelseriescdn.com')) {
        throw 'The GG download did not resolve to an approved official HTTPS host.'
    }
}

function New-DownloadClient {
    $handler = New-Object Net.Http.HttpClientHandler
    $handler.AllowAutoRedirect = $false
    $client = New-Object Net.Http.HttpClient($handler)
    $client.Timeout = [TimeSpan]::FromMinutes(15)
    $client.DefaultRequestHeaders.UserAgent.ParseAdd('Jack/0.1 (Windows; dependency setup)')
    $client
}

function Get-LatestGg {
    $client = New-DownloadClient
    $client.Timeout = [TimeSpan]::FromSeconds(25)
    try {
        $uri = [Uri]'https://steelseries.com/gg/downloads/gg/latest/windows'
        for ($index = 0; $index -lt 8; $index++) {
            Assert-OfficialUri $uri
            $request = New-Object Net.Http.HttpRequestMessage([Net.Http.HttpMethod]::Head, $uri)
            try { $response = $client.SendAsync($request).GetAwaiter().GetResult() } finally { $request.Dispose() }
            try {
                if ([int]$response.StatusCode -in @(301, 302, 303, 307, 308)) {
                    if (!$response.Headers.Location) { throw 'GG download redirect has no destination.' }
                    $uri = New-Object Uri($uri, $response.Headers.Location)
                    continue
                }
                $null = $response.EnsureSuccessStatusCode()
                if ($uri.AbsolutePath -notmatch '/SteelSeriesGG(\d+\.\d+\.\d+)Setup\.exe$') {
                    throw 'The official latest GG version could not be identified.'
                }
                return [pscustomobject]@{ version = $Matches[1]; url = $uri.AbsoluteUri }
            } finally { $response.Dispose() }
        }
        throw 'Too many GG download redirects.'
    } finally { $client.Dispose() }
}

function Assert-SteelSeriesSignature([string]$Path) {
    $signature = Get-AuthenticodeSignature -LiteralPath $Path
    if ($signature.Status -ne 'Valid' -or !$signature.SignerCertificate -or
        $signature.SignerCertificate.Subject -notmatch '(?:^|,\s*)O="?(?:SteelSeries (?:ApS|A/S)|GN Hearing A/S)"?(?:,|$)') {
        throw 'GG signature verification failed. A valid SteelSeries publisher signature is required.'
    }
}

function Start-GgIfStopped {
    $installation = Get-GgInstallation
    if (!$installation.installed) { throw 'SteelSeries GG is not installed. Install GG and enable Sonar, then retry.' }
    if ($installation.running) { return @{result='already_running'} }
    Assert-SteelSeriesSignature $installation.executable
    # Signature verification can take time; avoid launching again if GG started meanwhile.
    if ((Get-GgInstallation).running) { return @{result='already_running'} }
    $null = Start-Process -FilePath $installation.executable -WorkingDirectory (Split-Path $installation.executable -Parent) -WindowStyle Normal
    @{result='starting'}
}

function Install-LatestGg {
    if (!$Consent) { throw 'Explicit consent is required to install or update SteelSeries GG.' }
    $latest = Get-LatestGg
    $before = Get-GgInstallation
    if ($before.installed -and !$before.version) {
        throw 'The installed GG version is unknown. Open GG and use its update option, then retry.'
    }
    if ($before.version -and [version]$before.version -ge [version]$latest.version) {
        return [pscustomobject]@{ result = 'already_installed'; version = $before.version; reboot_required = $false }
    }
    # An unpredictable private subdirectory avoids a shared fixed installer filename.
    $directory = Join-Path ([IO.Path]::GetTempPath()) ('Jack-GG-' + [Guid]::NewGuid().ToString('N'))
    $null = New-Item -ItemType Directory -Path $directory
    $installer = Join-Path $directory 'SteelSeriesGGSetup.exe'
    $client = New-DownloadClient
    try {
        Assert-OfficialUri ([Uri]$latest.url)
        $response = $client.GetAsync($latest.url, [Net.Http.HttpCompletionOption]::ResponseHeadersRead).GetAwaiter().GetResult()
        try {
            $null = $response.EnsureSuccessStatusCode()
            if ([int]$response.StatusCode -ne 200) { throw 'Unexpected GG download response.' }
            $source = $response.Content.ReadAsStreamAsync().GetAwaiter().GetResult()
            $destination = [IO.File]::Open($installer, [IO.FileMode]::CreateNew, [IO.FileAccess]::Write, [IO.FileShare]::None)
            try {
                $buffer = New-Object byte[] 1048576
                [long]$total = 0
                while (($count = $source.Read($buffer, 0, $buffer.Length)) -gt 0) {
                    $total += $count
                    if ($total -gt 1GB) { throw 'GG installer exceeds the download size limit.' }
                    $destination.Write($buffer, 0, $count)
                }
            } finally { $destination.Dispose(); $source.Dispose() }
        } finally { $response.Dispose() }
        Assert-SteelSeriesSignature $installer
        $product = [Diagnostics.FileVersionInfo]::GetVersionInfo($installer)
        if ($product.ProductName -notmatch 'SteelSeries' -or $product.ProductVersion -notmatch ('^' + [regex]::Escape($latest.version) + '(?:\D|$)')) {
            throw 'The downloaded installer does not match the requested SteelSeries GG version.'
        }
        $current = Get-GgInstallation
        if ($current.version -and [version]$current.version -ge [version]$latest.version) {
            return [pscustomobject]@{ result = 'already_installed'; version = $current.version; reboot_required = $false }
        }
        # Use the official interactive installer. Do not suppress its terms or UAC.
        $process = Start-Process -FilePath $installer -Verb RunAs -WindowStyle Normal -Wait -PassThru
        if ($process.ExitCode -notin @(0, 1641, 3010)) { throw "GG installation was cancelled or failed (exit $($process.ExitCode))." }
        $after = Get-GgInstallation
        if (!$after.installed -or !$after.version -or [version]$after.version -lt [version]$latest.version) {
            throw 'GG installation is not confirmed. Restart Windows if requested, then retry setup.'
        }
        [pscustomobject]@{ result = 'installed'; version = $after.version; reboot_required = ($process.ExitCode -in @(1641, 3010)) }
    } finally {
        $client.Dispose()
        # Delete only the two paths created above. Never remove arbitrary temp contents.
        if (Test-Path -LiteralPath $installer) { Remove-Item -LiteralPath $installer -Force }
        if (Test-Path -LiteralPath $directory) { Remove-Item -LiteralPath $directory -Force }
    }
}

function Confirm-Gg([string]$Message) {
    Add-Type -AssemblyName System.Windows.Forms
    [Windows.Forms.MessageBox]::Show($Message, 'Jack - SteelSeries GG', 'YesNo', 'Information') -eq 'Yes'
}

try {
    $result = switch ($Mode) {
        'Inspect' { Get-GgInstallation }
        'Latest' { Get-LatestGg }
        'Install' { Install-LatestGg }
        'Start' { Start-GgIfStopped }
        'Open' {
            $installation = Get-GgInstallation
            if (!$installation.installed) { throw 'SteelSeries GG is not installed.' }
            Assert-SteelSeriesSignature $installation.executable
            $null = Start-Process -FilePath $installation.executable -WindowStyle Normal
            @{ result = 'opened' }
        }
        'RequireInstalled' {
            $installation = Get-GgInstallation
            if (!$installation.installed) { throw 'Install SteelSeries GG interactively before installing Jack silently.' }
            Assert-SteelSeriesSignature $installation.executable
            $installation
        }
        'StopJack' {
            if (!$JackPath -or (Split-Path $JackPath -Leaf) -ine 'jack-desktop.exe') { throw 'Invalid Jack installation path.' }
            $resolved = [IO.Path]::GetFullPath($JackPath)
            $processes = @(Get-Process -Name jack-desktop -ErrorAction SilentlyContinue | Where-Object { $_.Path -ieq $resolved })
            if ($processes.Count) {
                $null = Start-Process -FilePath $resolved -ArgumentList '--exit-for-install' -WindowStyle Hidden -Wait
                foreach ($process in $processes) {
                    if (!$process.WaitForExit(20000)) { throw 'Jack could not restore its audio routes. Close Jack from settings and retry.' }
                }
            }
            @{result='stopped'}
        }
        'EnsureInstalled' {
            $installation = Get-GgInstallation
            if (!$installation.installed) {
                $accepted = Confirm-Gg (Text 'Jack을 사용하려면 SteelSeries GG와 Sonar가 필요합니다. 공식 최신 GG를 다운로드하여 설치할까요? GG 약관 동의와 관리자 승인이 필요할 수 있습니다. 거절하면 Jack 설치가 중단됩니다.' 'Jack requires SteelSeries GG and Sonar. Download and install the latest official GG? Its terms and administrator approval may be required. Declining cancels Jack installation.')
                if (!$accepted) { exit 20 }
                $Consent = $true
                Install-LatestGg
            } else {
                $latest = $null
                try { $latest = Get-LatestGg } catch {
                    Add-Type -AssemblyName System.Windows.Forms
                    $null = [Windows.Forms.MessageBox]::Show((Text 'GG의 최신 버전을 확인할 수 없습니다. 기존 설치를 유지합니다. GG에서 업데이트를 확인해 주세요.' 'The latest GG version could not be checked. Your existing installation will be kept. Check for updates in GG.'), 'Jack')
                }
                if ($latest -and $installation.version -and [version]$installation.version -lt [version]$latest.version) {
                    $accepted = Confirm-Gg (Text "GG $($installation.version)이 설치되어 있습니다. 공식 최신 $($latest.version) 버전으로 업데이트할까요? 기존 설치를 사용하려면 아니요를 선택하세요." "GG $($installation.version) is installed. Update to the latest official version $($latest.version)? Choose No to keep the existing installation.")
                    if ($accepted) { $Consent = $true; Install-LatestGg } else { $installation }
                } else { $installation }
            }
        }
    }
    $result | ConvertTo-Json -Compress -Depth 4
} catch {
    if ($Mode -eq 'EnsureInstalled') {
        Add-Type -AssemblyName System.Windows.Forms
        $null = [Windows.Forms.MessageBox]::Show($_.Exception.Message, 'Jack - GG setup', 'OK', 'Error')
    }
    [Console]::Error.WriteLine($_.Exception.Message)
    exit 1
}
