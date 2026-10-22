# This script runs in system Windows PowerShell 5.1, whose .NET Framework includes
# UIAutomationClient. Keep the source ASCII; JSON files carry Unicode explicitly.
param(
  [Parameter(Mandatory = $true)][long]$WindowHandle,
  [Parameter(Mandatory = $true)][int]$AppProcessId,
  [Parameter(Mandatory = $true)][string]$OutputDirectory,
  [Parameter(Mandatory = $true)][string]$NodeExecutable,
  [string]$AccountFixture,
  [switch]$ResumeAccount
)
$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest
if ($env:OS -ne 'Windows_NT' -or $env:GITHUB_ACTIONS -ne 'true' -or -not $env:RUNNER_TEMP) {
  throw 'UI smoke requires a disposable GitHub Actions Windows runner.'
}
$utf8 = [System.Text.UTF8Encoding]::new($false)
$reportPath = Join-Path $OutputDirectory 'ui-smoke-result.json'
$inputPath = Join-Path $OutputDirectory 'uia-snapshot.json'
$assessmentPath = Join-Path $OutputDirectory 'uia-assessment.json'
$policy = Join-Path $PSScriptRoot 'windows-ui-smoke.mjs'
$expected = @{ handle = [string]$WindowHandle; processId = $AppProcessId }
$report = [ordered]@{ passed = $false; inspection = 'Windows UI Automation'; states = @(); actions = @() }
$failure = $null
$fixtureConfig = $null
$fixturePublic = $null
$accountPlan = $null

function Write-JsonFile($Path, $Value) {
  [IO.File]::WriteAllText($Path, ($Value | ConvertTo-Json -Depth 12), $utf8)
}
function Get-PublicFixture($Fixture) {
  # Only this allowlist may cross the policy/artifact boundary. Keep credentials
  # and private filesystem paths in the parent process's fixture configuration.
  return @{
    version = $Fixture.version; host = $Fixture.host; imapPort = $Fixture.imapPort
    smtpHost = $Fixture.smtpHost; smtpPort = $Fixture.smtpPort; email = $Fixture.email; name = $Fixture.name
  }
}
function Read-SharedFixtureReport([string]$Path) {
  # The fixture replaces its report atomically. On Windows the reader must allow
  # delete sharing so a concurrent rename can replace the path while we read.
  $stream = [IO.FileStream]::new($Path, [IO.FileMode]::Open, [IO.FileAccess]::Read, ([IO.FileShare]::ReadWrite -bor [IO.FileShare]::Delete))
  try {
    $reader = [IO.StreamReader]::new($stream, $utf8)
    try { return ($reader.ReadToEnd() | ConvertFrom-Json) }
    finally { $reader.Dispose() }
  } finally { $stream.Dispose() }
}
function Get-Assessment($Snapshot, [string]$Stage) {
  $server = $null
  if ($null -ne $fixtureConfig -and (Test-Path -LiteralPath $fixtureConfig.reportPath)) {
    $server = Read-SharedFixtureReport $fixtureConfig.reportPath
  }
  Write-JsonFile $inputPath @{ snapshot = $Snapshot; expected = $expected; stage = $Stage; fixture = $fixturePublic; server = $server }
  & $NodeExecutable $policy $inputPath $assessmentPath
  if ($LASTEXITCODE -ne 0) { throw 'UI snapshot validation failed; see uia-snapshot.json.' }
  return ([IO.File]::ReadAllText($assessmentPath, $utf8) | ConvertFrom-Json)
}
function Get-Bounds($Element) {
  $rect = $Element.Current.BoundingRectangle
  if ($rect.IsEmpty) { return @{ x = 0; y = 0; width = 0; height = 0 } }
  return @{ x = $rect.X; y = $rect.Y; width = $rect.Width; height = $rect.Height }
}
function Convert-UiOffscreenValue($Value, $Unsupported) {
  # PowerShell -eq coerces its right operand to the left operand type: a real
  # boolean true would equal a non-null sentinel. Compare object identity only.
  if ([object]::ReferenceEquals($Value, $Unsupported)) { return $null }
  return $Value
}
function Test-UiOffscreenValue($Unsupported) {
  $on = Convert-UiOffscreenValue $true $Unsupported
  if ($on -isnot [bool] -or $on -ne $true) { throw 'A real offscreen=true value was lost.' }
  $off = Convert-UiOffscreenValue $false $Unsupported
  if ($off -isnot [bool] -or $off -ne $false) { throw 'A real offscreen=false value was lost.' }
  $unknown = Convert-UiOffscreenValue $Unsupported $Unsupported
  if ($null -ne $unknown) { throw 'NotSupported must remain unknown.' }
  return @{ preservesTrue = $true; preservesFalse = $true; preservesUnknown = $true }
}
function Get-UiElement($Element) {
  $current = $Element.Current
  $readOnly = $null
  if ($current.ControlType -eq [System.Windows.Automation.ControlType]::Edit) {
    $pattern = $null
    if ($Element.TryGetCurrentPattern([System.Windows.Automation.ValuePattern]::Pattern, [ref]$pattern)) {
      # Never read Value, especially from a password control.
      $readOnly = ([System.Windows.Automation.ValuePattern]$pattern).Current.IsReadOnly
    }
  }
  $offscreen = $Element.GetCurrentPropertyValue([System.Windows.Automation.AutomationElement]::IsOffscreenProperty, $true)
  $offscreen = Convert-UiOffscreenValue $offscreen ([System.Windows.Automation.AutomationElement]::NotSupported)
  return @{
    name = $current.Name; controlType = $current.ControlType.ProgrammaticName.Replace('ControlType.', '')
    bounds = Get-Bounds $Element; offscreen = $offscreen; enabled = $current.IsEnabled
    isPassword = $current.IsPassword; readOnly = $readOnly
    invoke = [bool]$Element.GetCurrentPropertyValue([System.Windows.Automation.AutomationElement]::IsInvokePatternAvailableProperty)
  }
}
function Get-AppElement {
  $root = [System.Windows.Automation.AutomationElement]::FromHandle([IntPtr]$WindowHandle)
  if ($null -eq $root -or $root.Current.ProcessId -ne $AppProcessId -or $root.Current.NativeWindowHandle -ne $WindowHandle) {
    throw 'UI Automation window identity changed.'
  }
  return $root
}
function Get-Snapshot {
  $root = Get-AppElement
  $window = Get-UiElement $root
  $window['handle'] = [string]$root.Current.NativeWindowHandle
  $window['processId'] = $root.Current.ProcessId
  # The top-level UIA provider may not implement IsOffscreen. Keep that unknown
  # value and collect independent Win32 evidence on the same verified HWND.
  $window['nativeVisible'] = [MailBeaconWindowVisibility]::IsWindowVisible([IntPtr]$WindowHandle)
  $window['minimized'] = [MailBeaconWindowVisibility]::IsIconic([IntPtr]$WindowHandle)
  $children = $root.FindAll([System.Windows.Automation.TreeScope]::Descendants, [System.Windows.Automation.Condition]::TrueCondition)
  if ($children.Count -gt 512) { throw 'Unexpectedly large onboarding accessibility tree.' }
  $elements = @()
  foreach ($element in $children) { $elements += (Get-UiElement $element) }
  return @{ window = $window; elements = $elements }
}
function Save-UiScreenshot([string]$Name) {
  $bounds = [System.Windows.Forms.SystemInformation]::VirtualScreen
  $bitmap = [System.Drawing.Bitmap]::new($bounds.Width, $bounds.Height)
  $graphics = [System.Drawing.Graphics]::FromImage($bitmap)
  try {
    $graphics.CopyFromScreen($bounds.Location, [System.Drawing.Point]::Empty, $bounds.Size)
    $bitmap.Save((Join-Path $OutputDirectory $Name), [System.Drawing.Imaging.ImageFormat]::Png)
  } finally { $graphics.Dispose(); $bitmap.Dispose() }
}
function Wait-Stage([string]$Stage, [int]$Seconds) {
  $deadline = [DateTime]::UtcNow.AddSeconds($Seconds)
  $assessment = @{ missing = @('accessibility nodes') }
  do {
    try { $snapshot = Get-Snapshot }
    catch {
      # Changing provider re-renders the form and can invalidate one UIA node.
      if ($_.Exception -is [System.Windows.Automation.ElementNotAvailableException] -or $_.Exception.InnerException -is [System.Windows.Automation.ElementNotAvailableException]) {
        $report['transientNote'] = 'Account form accessibility nodes were replaced.'
        Start-Sleep -Milliseconds 300
        continue
      }
      throw
    }
    $assessment = Get-Assessment $snapshot $Stage
    $report.states = @($report.states | Select-Object -Last 9) + @(@{ stage = $Stage; snapshot = $snapshot; assessment = $assessment })
    Write-JsonFile $reportPath $report
    if ($assessment.ready) { return @{ snapshot = $snapshot; assessment = $assessment } }
    Start-Sleep -Milliseconds 300
  } while ([DateTime]::UtcNow -lt $deadline)
  throw "Timed out waiting for visible $Stage welcome/account controls: $($assessment.missing -join ', ')."
}
function Invoke-MailboxButton([string]$Name) {
  $root = Get-AppElement
  $condition = [System.Windows.Automation.AndCondition]::new(
    [System.Windows.Automation.PropertyCondition]::new([System.Windows.Automation.AutomationElement]::ControlTypeProperty, [System.Windows.Automation.ControlType]::Button),
    [System.Windows.Automation.PropertyCondition]::new([System.Windows.Automation.AutomationElement]::NameProperty, $Name)
  )
  $buttons = $root.FindAll([System.Windows.Automation.TreeScope]::Descendants, $condition)
  if ($buttons.Count -ne 1) { throw 'Mailbox button must be unique in the installed application window.' }
  $button = $buttons[0]
  if ($button.Current.IsOffscreen -or -not $button.Current.IsEnabled -or $button.Current.BoundingRectangle.Width -le 1) {
    throw 'Mailbox button is not visible and enabled.'
  }
  $pattern = [System.Windows.Automation.InvokePattern]$button.GetCurrentPattern([System.Windows.Automation.InvokePattern]::Pattern)
  $action = @{ name = $Name; method = 'InvokePattern'; windowHandle = [string]$WindowHandle; phase = 'requested' }
  $report.actions += $action
  Write-JsonFile $reportPath $report
  $pattern.Invoke()
  $action.phase = 'invoked'
}
function Get-AccountEdit([string]$Name) {
  $root = Get-AppElement
  $condition = [System.Windows.Automation.AndCondition]::new(
    [System.Windows.Automation.PropertyCondition]::new([System.Windows.Automation.AutomationElement]::ControlTypeProperty, [System.Windows.Automation.ControlType]::Edit),
    [System.Windows.Automation.PropertyCondition]::new([System.Windows.Automation.AutomationElement]::NameProperty, $Name)
  )
  $found = $root.FindAll([System.Windows.Automation.TreeScope]::Descendants, $condition)
  if ($found.Count -ne 1) { throw "Expected one account edit control: $Name" }
  return $found[0]
}
function Get-AccountPort($HostEdit) {
  $hostBounds = $HostEdit.Current.BoundingRectangle
  $root = Get-AppElement
  $condition = [System.Windows.Automation.PropertyCondition]::new([System.Windows.Automation.AutomationElement]::ControlTypeProperty, [System.Windows.Automation.ControlType]::Edit)
  $found = @()
  foreach ($element in $root.FindAll([System.Windows.Automation.TreeScope]::Descendants, $condition)) {
    $r = $element.Current.BoundingRectangle
    if ($r.Width -ge 30 -and $r.X -ge $hostBounds.Right -and [Math]::Abs($r.Y - $hostBounds.Y) -le 2) { $found += $element }
  }
  if ($found.Count -ne 1) { throw 'Expected one port edit beside the server host.' }
  return $found[0]
}
function Set-AccountEdit($Element, [string]$Value, [string]$Field) {
  $state = Get-UiElement $Element
  if ($state.offscreen -ne $false -or -not $state.enabled -or $state.readOnly -ne $false -or $state.bounds.width -le 1) {
    throw "Account field is not visibly editable: $Field"
  }
  $pattern = [System.Windows.Automation.ValuePattern]$Element.GetCurrentPattern([System.Windows.Automation.ValuePattern]::Pattern)
  $pattern.SetValue($Value)
  # Read back only known synthetic non-password fields, never a password Value.
  if (-not $state.isPassword -and $pattern.Current.Value -cne $Value) { throw "Account input did not update: $Field" }
  $report.actions += @{ method = 'ValuePattern.SetValue'; field = $Field }
}
function Fill-FixtureAccount([string]$Password) {
  $controls = @{}
  foreach ($field in @('email', 'password', 'name', 'imapHost', 'smtpHost')) {
    $controls[$field] = Get-AccountEdit $accountPlan.fields.$field
  }
  $controls['imapPort'] = Get-AccountPort $controls.imapHost
  $controls['smtpPort'] = Get-AccountPort $controls.smtpHost
  $values = @{
    email = $fixtureConfig.email; password = $Password; name = $fixtureConfig.name
    imapHost = $fixtureConfig.host; imapPort = [string]$fixtureConfig.imapPort
    smtpHost = $fixtureConfig.smtpHost; smtpPort = [string]$fixtureConfig.smtpPort
  }
  foreach ($field in @('email', 'name', 'imapHost', 'imapPort', 'smtpHost', 'smtpPort', 'password')) {
    Set-AccountEdit $controls[$field] $values[$field] $field
  }
}
function Test-FixtureAccount {
  $scenario = [ordered]@{ rejected = $false; edited = $false; switchedProvider = $false; retried = $false; added = $false; inboxReady = $false }
  $report['accountScenario'] = $scenario
  Invoke-MailboxButton $accountPlan.provider
  $null = Wait-Stage 'custom' 15
  Fill-FixtureAccount $fixtureConfig.invalidPassword
  Save-UiScreenshot 'account-before-login.png'
  Invoke-MailboxButton $accountPlan.submit
  $null = Wait-Stage 'auth-failed' 25
  $scenario.rejected = $true
  Save-UiScreenshot 'account-authentication-failed.png'
  Set-AccountEdit (Get-AccountEdit $accountPlan.fields.name) ($fixtureConfig.name + ' Retry') 'name-after-failure'
  $scenario.edited = $true
  Invoke-MailboxButton $accountPlan.alternate
  $null = Wait-Stage 'gmail' 15
  Invoke-MailboxButton $accountPlan.provider
  $null = Wait-Stage 'custom' 15
  $scenario.switchedProvider = $true
  Fill-FixtureAccount $fixtureConfig.password
  Invoke-MailboxButton $accountPlan.submit
  $null = Wait-Stage 'inbox' 35
  $scenario.retried = $true
  $scenario.added = $true
  $scenario.inboxReady = $true
  Save-UiScreenshot 'account-inbox.png'
}
function Test-UiRuleBridge($Snapshot) {
  # Exercise actual Windows PowerShell JSON serialization and the Node policy
  # with the live UIA sample. These negative variants must never pass readiness.
  $checks = [ordered]@{}
  foreach ($case in @('blank', 'zeroWidth', 'noInvoke', 'hiddenWindow', 'minimizedWindow')) {
    $copy = ($Snapshot | ConvertTo-Json -Depth 12 | ConvertFrom-Json)
    if ($case -eq 'blank') { $copy.elements = @() }
    if ($case -eq 'hiddenWindow') { $copy.window.nativeVisible = $false }
    if ($case -eq 'minimizedWindow') { $copy.window.minimized = $true }
    if ($case -eq 'zeroWidth') {
      foreach ($element in $copy.elements) { if ($element.controlType -eq 'Edit') { $element.bounds.width = 0 } }
    }
    if ($case -eq 'noInvoke') {
      foreach ($element in $copy.elements) { if ($element.controlType -eq 'Button') { $element.invoke = $false } }
    }
    if ((Get-Assessment $copy 'qq').ready) { throw "UI rule bridge accepted invalid $case sample." }
    $checks[$case] = $true
  }
  return $checks
}
try {
  Add-Type -TypeDefinition @'
using System;
using System.Runtime.InteropServices;
public static class MailBeaconWindowVisibility {
    [DllImport("user32.dll")]
    [return: MarshalAs(UnmanagedType.Bool)]
    public static extern bool IsWindowVisible(IntPtr hWnd);
    [DllImport("user32.dll")]
    [return: MarshalAs(UnmanagedType.Bool)]
    public static extern bool IsIconic(IntPtr hWnd);
}
'@
  Add-Type -AssemblyName UIAutomationClient
  Add-Type -AssemblyName UIAutomationTypes
  Add-Type -AssemblyName WindowsBase
  Add-Type -AssemblyName System.Windows.Forms
  Add-Type -AssemblyName System.Drawing
  $identity = [Security.Principal.WindowsIdentity]::GetCurrent()
  $principal = [Security.Principal.WindowsPrincipal]::new($identity)
  $report['administrator'] = $principal.IsInRole([Security.Principal.WindowsBuiltInRole]::Administrator)
  $report['offscreenSelfTest'] = Test-UiOffscreenValue ([System.Windows.Automation.AutomationElement]::NotSupported)
  if ($AccountFixture) {
    $fixtureConfig = [IO.File]::ReadAllText($AccountFixture, $utf8) | ConvertFrom-Json
    $fixturePublic = Get-PublicFixture $fixtureConfig
    $accountPlan = Get-Assessment $null 'account-plan'
    if (-not $fixtureConfig.password -or -not $fixtureConfig.invalidPassword -or $fixtureConfig.password -eq $fixtureConfig.invalidPassword) {
      throw 'The isolated fixture must supply different valid and invalid synthetic passwords.'
    }
  }
  if ($ResumeAccount) {
    if ($null -eq $fixtureConfig) { throw 'Account resume requires the isolated fixture configuration.' }
    $null = Wait-Stage 'inbox' 60
    $report['accountScenario'] = @{ resumed = $true; inboxReady = $true }
    $report['ui'] = @{ ready = $true; interaction = $false; restored = $false }
    Save-UiScreenshot 'account-resumed-inbox.png'
  } else {
    $before = Wait-Stage 'qq' 45
    $report['ruleBridgeSelfTest'] = Test-UiRuleBridge $before.snapshot
    Save-UiScreenshot 'ui-qq.png'
    Invoke-MailboxButton $before.assessment.nextButton
    $gmail = Wait-Stage 'gmail' 15
    Save-UiScreenshot 'ui-gmail.png'
    Invoke-MailboxButton $gmail.assessment.nextButton
    $restored = Wait-Stage 'qq' 15
    Save-UiScreenshot 'ui-restored.png'
    $report['ui'] = @{ ready = $true; interaction = $true; restored = $true }
    if ($null -ne $fixtureConfig) { Test-FixtureAccount }
  }
  $report.passed = $true
} catch {
  $failure = $_
  $report['error'] = $_.Exception.Message
} finally {
  try { Save-UiScreenshot 'ui-final.png' } catch { $report['screenshotNote'] = $_.Exception.Message }
  Write-JsonFile $reportPath $report
}
if ($null -ne $failure) { throw $failure }
