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
$OutputDirectory = (Resolve-Path -LiteralPath $OutputDirectory).Path
$result = [ordered]@{
  installed = $false; upgradeRetainedLocation = $false; launched = $false
  window = $false; webview2 = $false; uiReady = $false; uninstalled = $false
  installDirectory = $installDir
  installDrive = [IO.Path]::GetPathRoot($installDir)
  defaultDrive = [IO.Path]::GetPathRoot($env:LOCALAPPDATA)
  shortcutChecks = @()
  accountReady = $false; accountResumed = $false; appExits = @()
}

function Format-AppExitCode([int]$Code) {
  return ('0x{0:X8}' -f [BitConverter]::ToUInt32([BitConverter]::GetBytes($Code), 0))
}

function Read-PeStackReserve([IO.Stream]$Stream) {
  # PE32+ SizeOfStackReserve is an unsigned 64-bit value at optional-header +72.
  # https://learn.microsoft.com/en-us/windows/win32/debug/pe-format
  # Seek over the image; only the 22 required header bytes are read, once each.
  $reader = [IO.BinaryReader]::new($Stream, [Text.Encoding]::UTF8, $true)
  try {
    if ($Stream.Length -lt 64) { throw 'Installed executable has a truncated DOS header.' }
    $Stream.Position = 0
    if ($reader.ReadUInt16() -ne 0x5a4d) { throw 'Installed executable is not a PE image (DOS signature).' }
    $Stream.Position = 0x3c
    $peOffset = [long]$reader.ReadUInt32()
    if ($peOffset -lt 64 -or $peOffset -gt $Stream.Length - 24) { throw 'Installed executable has an invalid PE header offset.' }
    $Stream.Position = $peOffset
    if ($reader.ReadUInt32() -ne 0x4550) { throw 'Installed executable is not a PE image (PE signature).' }
    $Stream.Position = $peOffset + 20
    $optionalSize = $reader.ReadUInt16()
    $optionalOffset = $peOffset + 24
    if ($optionalSize -lt 80 -or $optionalOffset + $optionalSize -gt $Stream.Length) {
      throw 'Installed executable has a truncated optional header.'
    }
    $Stream.Position = $optionalOffset
    if ($reader.ReadUInt16() -ne 0x20b) { throw 'Installed executable must use PE32+ format.' }
    $Stream.Position = $optionalOffset + 72
    return $reader.ReadUInt64()
  } finally { $reader.Dispose() }
}

function Get-PeStackReserve([string]$Path) {
  $stream = [IO.File]::Open($Path, [IO.FileMode]::Open, [IO.FileAccess]::Read, [IO.FileShare]::Read)
  try { return (Read-PeStackReserve $stream) } finally { $stream.Dispose() }
}

function Read-MailFixtureReport {
  # The Node fixture atomically replaces this report while it handles IMAP.
  # FILE_SHARE_DELETE lets replacement succeed even during this short read.
  $sharing = [IO.FileShare]::ReadWrite -bor [IO.FileShare]::Delete
  $stream = [IO.File]::Open($fixtureReportPath, [IO.FileMode]::Open, [IO.FileAccess]::Read, $sharing)
  $reader = [IO.StreamReader]::new($stream, [Text.Encoding]::UTF8)
  try { return ($reader.ReadToEnd() | ConvertFrom-Json) } finally { $reader.Dispose() }
}

function Assert-AppAlive($Process, [string]$Stage) {
  $Process.Refresh()
  if ($Process.HasExited) {
    $Process.WaitForExit()
    $exit = [ordered]@{ stage = $Stage; code = $Process.ExitCode; hex = Format-AppExitCode $Process.ExitCode }
    $result.appExits += $exit
    throw "Application exited at ${Stage}: $($exit.code) ($($exit.hex))."
  }
}

function Assert-AccountSmokeEvidence($UiReport, $MailReport, [switch]$ResumeAccount, [long]$AcceptedBefore = 0, [long]$SelectedBefore = 0) {
  if ($UiReport.passed -ne $true) { throw 'Native account UI probe did not pass.' }
  $fields = if ($ResumeAccount) { @('resumed', 'inboxReady') } else { @('rejected', 'edited', 'switchedProvider', 'retried', 'added', 'inboxReady') }
  foreach ($field in $fields) {
    $property = $UiReport.accountScenario.PSObject.Properties[$field]
    if ($null -eq $property -or $property.Value -isnot [bool] -or -not $property.Value) {
      throw "Missing native account stage: $field."
    }
  }
  $menuFields = if ($ResumeAccount) { @('hidden', 'titleBar') } else { @('hidden', 'titleBar', 'opened', 'escaped', 'accelerator', 'composeClosed', 'usable') }
  foreach ($field in $menuFields) {
    $property = $UiReport.nativeMenuScenario.PSObject.Properties[$field]
    if ($null -eq $property -or $property.Value -isnot [bool] -or -not $property.Value) {
      throw "Missing native menu stage: $field."
    }
  }
  if ($MailReport.authRejected -lt 1 -or $MailReport.authAccepted -lt 2 -or $MailReport.selectAccepted -lt 1) {
    throw 'The isolated server has no completed rejection, login and inbox evidence.'
  }
  if ($ResumeAccount -and $MailReport.authAccepted -le $AcceptedBefore) {
    throw 'Restart produced no fresh authenticated mailbox connection.'
  }
  if ($ResumeAccount -and $MailReport.selectAccepted -le $SelectedBefore) {
    throw 'Restart produced no fresh successful INBOX selection.'
  }
}

function Wait-AppWindow($Process) {
  $deadline = (Get-Date).AddSeconds(45)
  do {
    Start-Sleep -Milliseconds 300
    Assert-AppAlive $Process 'waiting-for-window'
    if ($Process.MainWindowHandle -ne [IntPtr]::Zero -and $Process.MainWindowTitle -eq 'MailBeacon') { $result.window = $true; break }
  } while ((Get-Date) -lt $deadline)
  if ($Process.MainWindowHandle -eq [IntPtr]::Zero -or $Process.MainWindowTitle -ne 'MailBeacon') { throw 'No visible MailBeacon main window after startup.' }
  $deadline = (Get-Date).AddSeconds(15)
  do {
    Assert-AppAlive $Process 'waiting-for-webview'
    $webviews = @(Get-CimInstance Win32_Process -Filter "Name = 'msedgewebview2.exe'" | Where-Object { $_.ParentProcessId -eq $Process.Id })
    if ($webviews.Count -gt 0) { $result.webview2 = $true; return }
    Start-Sleep -Milliseconds 300
  } while ((Get-Date) -lt $deadline)
  throw 'Installed application did not start its WebView2 browser process.'
}

function Invoke-InstalledUiProbe($Process, [string]$Directory, [switch]$ResumeAccount) {
  New-Item -ItemType Directory -Force -Path $Directory | Out-Null
  $windowsPowerShell = Join-Path $env:WINDIR 'System32\WindowsPowerShell\v1.0\powershell.exe'
  $uiScript = Join-Path $PSScriptRoot 'windows-ui-smoke.ps1'
  $uiArgs = @('-NoProfile', '-NonInteractive', '-Mta', '-File', ('"' + $uiScript + '"'),
    '-WindowHandle', [string]$Process.MainWindowHandle.ToInt64(), '-AppProcessId', [string]$Process.Id,
    '-OutputDirectory', ('"' + $Directory + '"'), '-NodeExecutable', ('"' + $nodeExecutable + '"'),
    '-AccountFixture', ('"' + $fixtureConfigPath + '"'))
  if ($ResumeAccount) { $uiArgs += '-ResumeAccount' }
  $uiProbe = Start-Process -FilePath $windowsPowerShell -ArgumentList $uiArgs -WindowStyle Hidden -PassThru -RedirectStandardOutput (Join-Path $Directory 'ui-probe-stdout.log') -RedirectStandardError (Join-Path $Directory 'ui-probe-stderr.log')
  try {
    if (-not $uiProbe.WaitForExit(180000)) { throw 'Native UI Automation probe timed out.' }
    Assert-AppAlive $Process 'account-ui-probe'
    if ($uiProbe.ExitCode -ne 0) { throw 'Installed Windows account UI failed; see UI Automation tree, probe logs and screenshots.' }
    return (Get-Content -Raw -LiteralPath (Join-Path $Directory 'ui-smoke-result.json') | ConvertFrom-Json)
  } finally {
    $uiProbe.Refresh()
    if (-not $uiProbe.HasExited) { & taskkill.exe /PID $uiProbe.Id /T /F | Out-Null }
  }
}

function Wait-AccountEvidence($UiReport, [switch]$ResumeAccount, [long]$AcceptedBefore = 0, [long]$SelectedBefore = 0) {
  $deadline = (Get-Date).AddSeconds(15)
  do {
    Assert-AppAlive $app 'account-server-evidence'
    try {
      $mail = Read-MailFixtureReport
      Assert-AccountSmokeEvidence $UiReport $mail -ResumeAccount:$ResumeAccount -AcceptedBefore $AcceptedBefore -SelectedBefore $SelectedBefore
      return $mail
    } catch { $evidenceError = $_ }
    Start-Sleep -Milliseconds 200
  } while ((Get-Date) -lt $deadline)
  throw $evidenceError
}

function Assert-InstallLocation {
  param([string]$ExpectedDirectory, [string]$ExpectedBinary)
  $registered = (Get-ItemProperty -LiteralPath $uninstallKey -Name InstallLocation).InstallLocation.Trim('"')
  if ($registered -ne $ExpectedDirectory) { throw "Installer registered the wrong directory: $registered" }
  $command = (Get-Item -LiteralPath "$protocolKey\shell\open\command").GetValue('')
  if ($command -ne ('"' + $ExpectedBinary + '" "%1"')) { throw 'Protocol command does not quote the chosen executable and URL separately.' }
  if (-not (Test-Path -LiteralPath $shortcutPath)) { throw 'Installer did not create the Start menu shortcut.' }
  Copy-Item -LiteralPath $shortcutPath -Destination (Join-Path $OutputDirectory 'installed-shortcut.lnk') -Force
  $actualTarget = [MailBeaconFileIdentity]::ReadShortcutTarget($shortcutPath)
  $check = [ordered]@{ reader = 'IShellLinkW'; actualTarget = $actualTarget; expectedTarget = $ExpectedBinary }
  $result.shortcutChecks += $check
  $comparison = [MailBeaconFileIdentity]::Compare($ExpectedBinary, $actualTarget)
  $check['identity'] = $comparison
  if (-not $comparison.SameFile) { throw "Shortcut targets a different file: actual '$actualTarget', expected '$ExpectedBinary'." }
  if (-not (Test-Path -LiteralPath $ExpectedBinary)) { throw 'The installed executable is missing from the chosen directory.' }
  $defaultDirectory = Join-Path $env:LOCALAPPDATA 'MailBeacon'
  if ($ExpectedDirectory -ne $defaultDirectory -and (Test-Path -LiteralPath $defaultDirectory)) {
    throw 'Installer unexpectedly created the default installation directory.'
  }
}

function Save-SmokeScreenshot {
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
}

$app = $null
$fixtureProcess = $null
$fixtureDirectory = $null
$fixtureStopPath = $null
$fixtureReportPath = Join-Path $OutputDirectory 'mail-fixture-report.json'
$previousExtraCa = [Environment]::GetEnvironmentVariable('MAILPULSE_EXTRA_CA', 'Process')
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
  # Record the actual installed binary, including old releases with smaller
  # reserves. Successful account operations remain the acceptance criterion.
  $result['pe'] = @{ format = 'PE32+'; sizeOfStackReserve = Get-PeStackReserve $binaryPath }

  # The only credentials used here belong to this loopback-only fixture. Keep
  # its private connection config outside artifacts; publish sanitized counts.
  $nodeExecutable = (Get-Command node -ErrorAction Stop).Source
  $fixtureDirectory = Join-Path $env:RUNNER_TEMP ('mailbeacon-mail-fixture-' + [guid]::NewGuid().ToString('N'))
  New-Item -ItemType Directory -Path $fixtureDirectory | Out-Null
  $fixtureConfigPath = Join-Path $fixtureDirectory 'config.json'
  $fixtureStopPath = Join-Path $fixtureDirectory 'stop'
  $fixtureScript = Join-Path $PSScriptRoot 'windows-mail-fixture.mjs'
  $fixtureArgs = @(('"' + $fixtureScript + '"'), '--config', ('"' + $fixtureConfigPath + '"'), '--report', ('"' + $fixtureReportPath + '"'), '--stop', ('"' + $fixtureStopPath + '"'))
  $fixtureProcess = Start-Process -FilePath $nodeExecutable -ArgumentList $fixtureArgs -WindowStyle Hidden -PassThru -RedirectStandardOutput (Join-Path $OutputDirectory 'mail-fixture-stdout.log') -RedirectStandardError (Join-Path $OutputDirectory 'mail-fixture-stderr.log')
  $deadline = (Get-Date).AddSeconds(15)
  while (-not (Test-Path -LiteralPath $fixtureConfigPath)) {
    $fixtureProcess.Refresh()
    if ($fixtureProcess.HasExited -or (Get-Date) -ge $deadline) { throw 'Loopback mailbox fixture did not become ready.' }
    Start-Sleep -Milliseconds 100
  }
  $fixtureConfig = Get-Content -Raw -LiteralPath $fixtureConfigPath | ConvertFrom-Json
  if ($fixtureConfig.host -ne '127.0.0.1' -or $fixtureConfig.smtpHost -ne '127.0.0.1' -or $fixtureConfig.email -notlike '*@example.invalid' -or -not (Test-Path -LiteralPath $fixtureConfig.caPath)) {
    throw 'Account smoke must use the isolated loopback fixture.'
  }
  $env:MAILPULSE_EXTRA_CA = $fixtureConfig.caPath
  $app = Start-Process -FilePath $binaryPath -PassThru -RedirectStandardOutput (Join-Path $OutputDirectory 'stdout.log') -RedirectStandardError (Join-Path $OutputDirectory 'stderr.log')
  $result.launched = $true
  Wait-AppWindow $app
  $uiReport = Invoke-InstalledUiProbe $app $OutputDirectory
  if (-not $uiReport.passed -or -not $uiReport.ui.ready -or -not $uiReport.ui.interaction -or -not $uiReport.ui.restored) { throw 'UI smoke returned no successful rendered-page and interaction evidence.' }
  $mailReport = Wait-AccountEvidence $uiReport
  $result.accountReady = $true
  $result['nativeMenu'] = $uiReport.nativeMenuScenario
  $result.uiReady = $true
  Assert-AppAlive $app 'after-account-added'
  Save-SmokeScreenshot

  # Reopen the same installed executable and actual encrypted local account.
  # A new login must appear in the fixture, not just a cached sidebar entry.
  & taskkill.exe /PID $app.Id /T /F | Out-Null
  if (-not $app.WaitForExit(10000)) { throw 'Could not stop app before account restart check.' }
  $beforeRestartReport = Read-MailFixtureReport
  $beforeRestart = $beforeRestartReport.authAccepted
  $selectedBeforeRestart = $beforeRestartReport.selectAccepted
  $restartDirectory = Join-Path $OutputDirectory 'restart'
  New-Item -ItemType Directory -Path $restartDirectory | Out-Null
  $app = Start-Process -FilePath $binaryPath -PassThru -RedirectStandardOutput (Join-Path $restartDirectory 'stdout.log') -RedirectStandardError (Join-Path $restartDirectory 'stderr.log')
  Wait-AppWindow $app
  $restartReport = Invoke-InstalledUiProbe $app $restartDirectory -ResumeAccount
  $resumedMailReport = Wait-AccountEvidence $restartReport -ResumeAccount -AcceptedBefore $beforeRestart -SelectedBefore $selectedBeforeRestart
  $result.accountResumed = $true
  $result['nativeMenuAfterRestart'] = $restartReport.nativeMenuScenario
  $result['mailboxEvidence'] = @{ authRejected = $resumedMailReport.authRejected; authAcceptedBeforeRestart = $beforeRestart; authAccepted = $resumedMailReport.authAccepted; selectedBeforeRestart = $selectedBeforeRestart; selected = $resumedMailReport.selectAccepted }
  Assert-AppAlive $app 'after-account-restart'
} catch {
  $failure = $_
  $result['error'] = $_.Exception.Message
} finally {
  if ($null -ne $app) {
    $app.Refresh()
    if (-not $app.HasExited) { Save-SmokeScreenshot; & taskkill.exe /PID $app.Id /T /F | Out-Null }
    elseif ($result.appExits.Count -eq 0) {
      $app.WaitForExit()
      $result.appExits += @{ stage = 'cleanup'; code = $app.ExitCode; hex = Format-AppExitCode $app.ExitCode }
    }
  }
  [Environment]::SetEnvironmentVariable('MAILPULSE_EXTRA_CA', $previousExtraCa, 'Process')
  if ($null -ne $fixtureProcess) {
    try {
      $fixtureProcess.Refresh()
      if (-not $fixtureProcess.HasExited) {
        Set-Content -LiteralPath $fixtureStopPath -Value 'stop' -Encoding ascii
        if (-not $fixtureProcess.WaitForExit(5000)) { & taskkill.exe /PID $fixtureProcess.Id /T /F | Out-Null; throw 'Loopback fixture did not stop cleanly.' }
      }
      if ($fixtureProcess.ExitCode -ne 0) { throw 'Loopback fixture exited unsuccessfully.' }
    } catch {
      $result['fixtureCleanupError'] = $_.Exception.Message
      if ($null -eq $failure) { $failure = $_ }
    }
  }
  if ($null -ne $fixtureDirectory -and (Test-Path -LiteralPath $fixtureDirectory)) {
    try { Remove-Item -LiteralPath $fixtureDirectory -Recurse -Force } catch {
      $result['fixtureConfigCleanupError'] = 'Unable to remove temporary fixture connection config.'
      if ($null -eq $failure) { $failure = $_ }
    }
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
