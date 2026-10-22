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
$installDir = Join-Path $env:RUNNER_TEMP ('MailBeacon-Smoke-' + [guid]::NewGuid().ToString('N'))
if (Test-Path $installDir) { throw 'Smoke installation target must be new.' }
New-Item -ItemType Directory -Force -Path $OutputDirectory | Out-Null
$result = [ordered]@{ installed = $false; launched = $false; window = $false; webview2 = $false; uninstalled = $false }
$app = $null
$binaryPath = $null
$failure = $null
try {
  # NSIS requires /D last and without extra quotes, including paths containing spaces.
  $setup = Start-Process -FilePath $Installer -ArgumentList "/S /D=$installDir" -PassThru
  if (-not $setup.WaitForExit(180000)) { $setup.Kill(); throw 'Installer timed out.' }
  if ($setup.ExitCode -ne 0) { throw "Installer exited with $($setup.ExitCode)." }
  $binary = @(Get-ChildItem -LiteralPath $installDir -Filter '*.exe' -File | Where-Object { $_.Name -ne 'uninstall.exe' })
  if ($binary.Count -ne 1) { throw 'Expected one installed application executable.' }
  $binaryPath = $binary[0].FullName
  $result.installed = $true
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
      while ($binaryPath -and (Test-Path -LiteralPath $binaryPath) -and (Get-Date) -lt $deadline) { Start-Sleep -Milliseconds 300 }
      if ($binaryPath -and (Test-Path -LiteralPath $binaryPath)) { throw 'Uninstaller left the application executable behind.' }
      $result.uninstalled = $true
    } catch {
      $result['uninstallError'] = $_.Exception.Message
      if ($null -eq $failure) { $failure = $_ }
    }
  }
  $result | ConvertTo-Json | Set-Content -LiteralPath (Join-Path $OutputDirectory 'smoke-result.json') -Encoding utf8
}
$result | ConvertTo-Json
if ($null -ne $failure) { throw $failure }
if (-not $result.uninstalled) { throw 'Installer smoke did not complete uninstall.' }
