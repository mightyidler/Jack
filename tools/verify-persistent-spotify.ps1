param(
    [ValidateRange(1, 5)][int]$Cycles = 3,
    [string]$Report = "$PSScriptRoot\..\artifacts\spotify-persistent-restarts.json"
)
$ErrorActionPreference = 'Stop'
$workspace = Split-Path -Parent $PSScriptRoot
$probe = Join-Path $workspace 'target\release\jack-audio-probe.exe'
function Read-Audio { (& $probe | ConvertFrom-Json) }
$initial = Read-Audio
$spotifySession = $initial.sessions | Where-Object executable_path -match '\\Spotify.exe$' | Sort-Object state -Descending | Select-Object -First 1
$spotifyExe = if ($spotifySession) { $spotifySession.executable_path } else { (Get-Process Spotify | Select-Object -First 1).Path }
if (-not $spotifyExe) { throw 'Open Spotify before running this opt-in diagnostic.' }
$mediaEndpoint = ($initial.endpoints | Where-Object { $_.flow -eq 'render' -and $_.name -like '*Sonar - Media*' } | Select-Object -First 1).id
if (-not $mediaEndpoint) { throw 'Sonar Media is unavailable.' }
$jackProcess = Get-Process jack-desktop -ErrorAction SilentlyContinue
if (-not $jackProcess -or $jackProcess.Path -ne (Join-Path $workspace 'target\debug\jack-desktop.exe')) {
    throw 'Run the current native development build first.'
}
# Send Play to Spotify alone, without toggling or controlling another media app.
Add-Type @'
using System;
using System.Runtime.InteropServices;
public static class SpotifyPlaybackDiagnostic {
    [DllImport("user32.dll", SetLastError=true)]
    public static extern IntPtr SendMessageTimeout(IntPtr hwnd, uint msg, UIntPtr wParam, IntPtr lParam, uint flags, uint timeout, out UIntPtr result);
}
'@
$runs = @()
for ($cycle = 1; $cycle -le $Cycles; $cycle++) {
    $oldProcesses = @(Get-Process Spotify | Where-Object Path -eq $spotifyExe)
    if (-not $oldProcesses) { throw 'Verified Spotify executable is no longer running.' }
    $oldProcesses | Stop-Process
    if ($spotifyExe -like '*\WindowsApps\*') {
        Start-Process -FilePath explorer.exe -ArgumentList 'shell:AppsFolder\SpotifyAB.SpotifyMusic_zpdnekdrzrea0!Spotify' -WindowStyle Hidden
    } else {
        Start-Process -FilePath $spotifyExe -WindowStyle Hidden
    }
    $watchClock = [Diagnostics.Stopwatch]::StartNew()
    $samples = @()
    $playRequested = $false
    while ($watchClock.Elapsed.TotalSeconds -lt 18) {
        $snapshot = Read-Audio
        $sessions = @($snapshot.sessions | Where-Object executable_path -eq $spotifyExe)
        $samples += [ordered]@{
            elapsed_ms = $watchClock.ElapsedMilliseconds
            sessions = @($sessions | Select-Object pid, endpoint_id, state)
        }
        if (-not $playRequested -and $watchClock.Elapsed.TotalSeconds -gt 4) {
            $spotifyWindow = Get-Process Spotify -ErrorAction SilentlyContinue | Where-Object { $_.Path -eq $spotifyExe -and $_.MainWindowHandle -ne 0 } | Select-Object -First 1
            if ($spotifyWindow) {
                $playResult = [UIntPtr]::Zero
                [void][SpotifyPlaybackDiagnostic]::SendMessageTimeout($spotifyWindow.MainWindowHandle, 0x319, [UIntPtr]::Zero, [IntPtr](46 -shl 16), 2, 1000, [ref]$playResult)
                $playRequested = $true
            }
        }
        Start-Sleep -Milliseconds 200
    }
    $observed = @($samples | ForEach-Object { $_.sessions } | Where-Object state -eq 1)
    $wrong = @($observed | Where-Object endpoint_id -ne $mediaEndpoint)
    $runs += [ordered]@{
        cycle = $cycle
        passed = ($observed.Count -gt 0 -and $wrong.Count -eq 0)
        active_samples = $observed.Count
        wrong_endpoint_samples = $wrong.Count
        process_ids = @($observed.pid | Sort-Object -Unique)
        samples = $samples
    }
    [ordered]@{passed=(@($runs | Where-Object { -not $_.passed }).Count -eq 0); jack_executable=$jackProcess.Path; runs=$runs} | ConvertTo-Json -Depth 9 | Set-Content -LiteralPath $Report
    Write-Output "Spotify restart $cycle : active samples=$($observed.Count), non-Media samples=$($wrong.Count)"
    if (-not $runs[-1].passed) { throw 'Spotify did not remain on Media; inspect the native trace.' }
}
