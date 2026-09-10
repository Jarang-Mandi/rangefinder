param(
    [switch]$SkipBuild
)

$ErrorActionPreference = "Stop"
$taskName = "Rangefinder Engine"
$repoRoot = (Resolve-Path (Join-Path $PSScriptRoot "..")).Path
$localRoot = Join-Path ([Environment]::GetFolderPath("LocalApplicationData")) "Rangefinder"
$binDir = Join-Path $localRoot "bin"
$engineSource = Join-Path $repoRoot "target\release\rangefinder.exe"
$engineTarget = Join-Path $binDir "rangefinder.exe"
$launcher = Join-Path $binDir "run-engine.ps1"
$logPath = Join-Path $localRoot "engine.log"
$configPath = Join-Path $localRoot "config.toml"

if (-not $SkipBuild) {
    & cargo build --release -p rangefinder-engine --manifest-path (Join-Path $repoRoot "Cargo.toml")
    if ($LASTEXITCODE -ne 0) { throw "build engine gagal dengan exit code $LASTEXITCODE" }
}
if (-not (Test-Path -LiteralPath $engineSource)) {
    throw "biner release tidak ditemukan di $engineSource"
}

New-Item -ItemType Directory -Path $binDir -Force | Out-Null
Stop-ScheduledTask -TaskName $taskName -ErrorAction SilentlyContinue
Get-Process -Name "rangefinder" -ErrorAction SilentlyContinue | Stop-Process -Force
if (Test-Path -LiteralPath $logPath) {
    for ($attempt = 1; $attempt -le 20; $attempt++) {
        try {
            [System.IO.File]::WriteAllText($logPath, "", [System.Text.UTF8Encoding]::new($false))
            break
        } catch [System.IO.IOException] {
            if ($attempt -eq 20) { throw }
            Start-Sleep -Milliseconds 250
        }
    }
}
Copy-Item -LiteralPath $engineSource -Destination $engineTarget -Force

if (Test-Path -LiteralPath $configPath) {
    $configText = Get-Content -LiteralPath $configPath -Raw
    $configText = $configText -replace '(?m)^auto_lock_minutes\s*=\s*\d+\s*$', 'auto_lock_minutes = 0'
    $configText = $configText -replace '(?m)^use_credential_manager\s*=\s*(true|false)\s*$', 'use_credential_manager = true'
    Set-Content -LiteralPath $configPath -Value $configText -Encoding UTF8
}

$launcherText = @'
$ErrorActionPreference = "Continue"
$env:RANGEFINDER_REPO_ROOT = "__REPO_ROOT__"
while ($true) {
    if ((Test-Path -LiteralPath "__LOG__") -and (Get-Item -LiteralPath "__LOG__").Length -gt 5MB) {
        Move-Item -LiteralPath "__LOG__" -Destination "__LOG__.1" -Force
    }
    & "__ENGINE__" 2>&1 | Out-File -LiteralPath "__LOG__" -Append -Encoding utf8
    Start-Sleep -Seconds 3
}
'@
$launcherText = $launcherText.Replace("__REPO_ROOT__", $repoRoot.Replace('"', '""'))
$launcherText = $launcherText.Replace("__ENGINE__", $engineTarget.Replace('"', '""'))
$launcherText = $launcherText.Replace("__LOG__", $logPath.Replace('"', '""'))
Set-Content -LiteralPath $launcher -Value $launcherText -Encoding UTF8

$powershell = (Get-Command powershell.exe).Source
$argument = "-NoLogo -NoProfile -NonInteractive -WindowStyle Hidden -ExecutionPolicy Bypass -File `"$launcher`""
$action = New-ScheduledTaskAction -Execute $powershell -Argument $argument -WorkingDirectory $repoRoot
$trigger = New-ScheduledTaskTrigger -AtLogOn -User ([System.Security.Principal.WindowsIdentity]::GetCurrent().Name)
$settings = New-ScheduledTaskSettingsSet `
    -AllowStartIfOnBatteries `
    -DontStopIfGoingOnBatteries `
    -StartWhenAvailable `
    -MultipleInstances IgnoreNew `
    -RestartCount 99 `
    -RestartInterval (New-TimeSpan -Minutes 1) `
    -ExecutionTimeLimit ([TimeSpan]::Zero)

Register-ScheduledTask `
    -TaskName $taskName `
    -Action $action `
    -Trigger $trigger `
    -Settings $settings `
    -User ([System.Security.Principal.WindowsIdentity]::GetCurrent().Name) `
    -RunLevel Limited `
    -Force | Out-Null

Start-ScheduledTask -TaskName $taskName
Write-Output "Rangefinder Engine dipasang dan dijalankan dari $engineTarget"
