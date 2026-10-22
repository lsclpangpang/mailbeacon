# Run only on an empty, disposable GitHub Windows runner. Never use a real mailbox.
param(
  [Parameter(Mandatory = $true)][string]$Installer,
  [string]$OutputDirectory = (Join-Path (Get-Location) 'windows-smoke-results')
)
$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest
if (-not $IsWindows -or $env:GITHUB_ACTIONS -ne 'true' -or -not $env:RUNNER_TEMP) {
  throw 'Installer smoke requires a disposable GitHub Actions Windows runner.'
}
foreach ($base in @($env:APPDATA, $env:LOCALAPPDATA)) {
  foreach ($id in @('com.lsclpangpang.mailbeacon', 'com.lsclpangpang.mailpulse')) {
    if (Test-Path (Join-Path $base $id)) { throw "Refusing to use existing application data: $id" }
  }
}
foreach ($key in @('HKCU:\Software\Classes\mailbeacon', 'HKCU:\Software\Microsoft\Windows\CurrentVersion\Uninstall\MailBeacon')) {
  if (Test-Path $key) { throw "Refusing to replace existing installation registration: $key" }
}
$Installer = (Resolve-Path -LiteralPath $Installer).Path
$installDir = Join-Path $env:RUNNER_TEMP ('自选目录 MailBeacon ' + [guid]::NewGuid().ToString('N'))
if (Test-Path $installDir) { throw 'Smoke installation target must be new.' }
$shortcutPath = Join-Path ([Environment]::GetFolderPath('Programs')) 'MailBeacon.lnk'
if (Test-Path -LiteralPath $shortcutPath) { throw 'Refusing to replace an existing MailBeacon shortcut.' }
$uninstallKey = 'HKCU:\Software\Microsoft\Windows\CurrentVersion\Uninstall\MailBeacon'
$protocolKey = 'HKCU:\Software\Classes\mailbeacon'
New-Item -ItemType Directory -Force -Path $OutputDirectory | Out-Null
$result = [ordered]@{
  installed = $false; upgradeRetainedLocation = $false; launched = $false
  window = $false; webview2 = $false; uninstalled = $false
  installDirectory = $installDir
  installDrive = [IO.Path]::GetPathRoot($installDir)
  defaultDrive = [IO.Path]::GetPathRoot($env:LOCALAPPDATA)
  shortcutChecks = @()
}

function Assert-InstallLocation {
  param([string]$ExpectedDirectory, [string]$ExpectedBinary)
  $registered = (Get-ItemProperty -LiteralPath $uninstallKey -Name InstallLocation).InstallLocation.Trim('"')
  if ($registered -ne $ExpectedDirectory) { throw "Installer registered the wrong directory: $registered" }
  $command = (Get-Item -LiteralPath "$protocolKey\shell\open\command").GetValue('')
  if ($command -ne ('"' + $ExpectedBinary + '" "%1"')) { throw 'Protocol command does not quote the chosen executable and URL separately.' }
  if (-not (Test-Path -LiteralPath $shortcutPath)) { throw 'Installer did not create the Start menu shortcut.' }
  $shell = New-Object -ComObject WScript.Shell
  $shortcut = $null
  try {
    $shortcut = $shell.CreateShortcut($shortcutPath)
    $check = [ordered]@{ actualTarget = $shortcut.TargetPath; expectedTarget = $ExpectedBinary }
    $result.shortcutChecks += $check
    Copy-Item -LiteralPath $shortcutPath -Destination (Join-Path $OutputDirectory 'installed-shortcut.lnk') -Force
    $comparison = [MailBeaconFileIdentity]::Compare($ExpectedBinary, $shortcut.TargetPath)
    $check['identity'] = $comparison
    if (-not $comparison.SameFile) { throw "Shortcut targets a different file: actual '$($shortcut.TargetPath)', expected '$ExpectedBinary'." }
  } finally {
    if ($null -ne $shortcut) { [Runtime.InteropServices.Marshal]::FinalReleaseComObject($shortcut) | Out-Null }
    [Runtime.InteropServices.Marshal]::FinalReleaseComObject($shell) | Out-Null
  }
  if (-not (Test-Path -LiteralPath $ExpectedBinary)) { throw 'The installed executable is missing from the chosen directory.' }
  $defaultDirectory = Join-Path $env:LOCALAPPDATA 'MailBeacon'
  if ($ExpectedDirectory -ne $defaultDirectory -and (Test-Path -LiteralPath $defaultDirectory)) {
    throw 'Installer unexpectedly created the default installation directory.'
  }
}

$app = $null
$binaryPath = $null
$failure = $null
try {
  . (Join-Path $PSScriptRoot 'windows-file-identity.ps1')
  $result['fileIdentitySelfTest'] = Test-WindowsFileIdentity
  # /S is only for this CI smoke. Normal double-click uses the standard NSIS wizard,
  # including its directory picker. /D must be last and unquoted, even with spaces.
  $setup = Start-Process -FilePath $Installer -ArgumentList "/S /D=$installDir" -PassThru
  if (-not $setup.WaitForExit(180000)) { $setup.Kill(); throw 'Installer timed out.' }
  if ($setup.ExitCode -ne 0) { throw "Installer exited with $($setup.ExitCode)." }
  $binary = @(Get-ChildItem -LiteralPath $installDir -Filter '*.exe' -File | Where-Object { $_.Name -ne 'uninstall.exe' })
  if ($binary.Count -ne 1) { throw 'Expected one installed application executable.' }
  $binaryPath = $binary[0].FullName
  Assert-InstallLocation -ExpectedDirectory $installDir -ExpectedBinary $binaryPath
  $result.installed = $true

  # Deliberately omit /D: NSIS must restore the user's prior directory, including
  # its drive, spaces and Unicode, instead of reverting to LOCALAPPDATA on update.
  $upgrade = Start-Process -FilePath $Installer -ArgumentList '/S /UPDATE' -PassThru
  if (-not $upgrade.WaitForExit(180000)) { $upgrade.Kill(); throw 'Installer update timed out.' }
  if ($upgrade.ExitCode -ne 0) { throw "Installer update exited with $($upgrade.ExitCode)." }
  Assert-InstallLocation -ExpectedDirectory $installDir -ExpectedBinary $binaryPath
  $result.upgradeRetainedLocation = $true

  $app = Start-Process -FilePath $binaryPath -PassThru -RedirectStandardOutput (Join-Path $OutputDirectory 'stdout.log') -RedirectStandardError (Join-Path $OutputDirectory 'stderr.log')
  $result.launched = $true
  $deadline = (Get-Date).AddSeconds(45)
  do {
    Start-Sleep -Milliseconds 300
    $app.Refresh()
    if ($app.HasExited) { throw "Application exited during startup: $($app.ExitCode)." }
    if ($app.MainWindowHandle -ne [IntPtr]::Zero -and $app.MainWindowTitle -eq 'MailBeacon') { $result.window = $true; break }
  } while ((Get-Date) -lt $deadline)
  if (-not $result.window) { throw 'No visible MailBeacon main window after startup.' }
  $deadline = (Get-Date).AddSeconds(15)
  do {
    $webviews = @(Get-CimInstance Win32_Process -Filter "Name = 'msedgewebview2.exe'" | Where-Object { $_.ParentProcessId -eq $app.Id })
    if ($webviews.Count -gt 0) { $result.webview2 = $true; break }
    Start-Sleep -Milliseconds 300
  } while ((Get-Date) -lt $deadline)
  if (-not $result.webview2) { throw 'Installed application did not start its WebView2 browser process.' }
  Start-Sleep -Seconds 3
  $app.Refresh()
  if ($app.HasExited) { throw 'Application crashed after window creation.' }
  try {
    Add-Type -AssemblyName System.Windows.Forms
    Add-Type -AssemblyName System.Drawing
    $bounds = [System.Windows.Forms.SystemInformation]::VirtualScreen
    $image = [System.Drawing.Bitmap]::new($bounds.Width, $bounds.Height)
    $graphics = [System.Drawing.Graphics]::FromImage($image)
    try {
      $graphics.CopyFromScreen($bounds.Location, [System.Drawing.Point]::Empty, $bounds.Size)
      $image.Save((Join-Path $OutputDirectory 'first-launch.png'), [System.Drawing.Imaging.ImageFormat]::Png)
    } finally { $graphics.Dispose(); $image.Dispose() }
  } catch { $result['screenshotNote'] = $_.Exception.Message }
} catch {
  $failure = $_
  $result['error'] = $_.Exception.Message
} finally {
  if ($null -ne $app) {
    $app.Refresh()
    if (-not $app.HasExited) { & taskkill.exe /PID $app.Id /T /F | Out-Null }
  }
  $uninstaller = Join-Path $installDir 'uninstall.exe'
  if (Test-Path -LiteralPath $uninstaller) {
    try {
      $remove = Start-Process -FilePath $uninstaller -ArgumentList '/S' -PassThru
      if (-not $remove.WaitForExit(60000)) { $remove.Kill(); throw 'Uninstaller timed out.' }
      $deadline = (Get-Date).AddSeconds(30)
      while ((($binaryPath -and (Test-Path -LiteralPath $binaryPath)) -or (Test-Path -LiteralPath $uninstallKey)) -and (Get-Date) -lt $deadline) { Start-Sleep -Milliseconds 300 }
      if ($binaryPath -and (Test-Path -LiteralPath $binaryPath)) { throw 'Uninstaller left the application executable behind.' }
      foreach ($item in @($uninstallKey, $protocolKey, $shortcutPath)) {
        if (Test-Path -LiteralPath $item) { throw "Uninstaller left installation registration behind: $item" }
      }
      $result.uninstalled = $true
    } catch {
      $result['uninstallError'] = $_.Exception.Message
      if ($null -eq $failure) { $failure = $_ }
    }
  }
  $result | ConvertTo-Json -Depth 6 | Set-Content -LiteralPath (Join-Path $OutputDirectory 'smoke-result.json') -Encoding utf8
}
$result | ConvertTo-Json -Depth 6
if ($null -ne $failure) { throw $failure }
if (-not $result.uninstalled) { throw 'Installer smoke did not complete uninstall.' }
