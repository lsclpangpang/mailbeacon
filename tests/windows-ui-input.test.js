const test = require('node:test');
const assert = require('node:assert/strict');
const { mkdtempSync, writeFileSync, rmSync } = require('node:fs');
const { tmpdir } = require('node:os');
const { join, resolve } = require('node:path');
const { spawnSync } = require('node:child_process');

const powershell = process.env.MAILBEACON_TEST_POWERSHELL || (process.platform === 'win32'
  ? join(process.env.WINDIR, 'System32', 'WindowsPowerShell', 'v1.0', 'powershell.exe') : null);
test('actual PowerShell input confirmation waits for delayed UIA values but rejects stale values and never reads passwords', {
  skip: !powershell && 'Windows PowerShell required; optional local parser path can run pure helpers elsewhere',
}, () => {
  const directory = mkdtempSync(join(tmpdir(), 'ui-input-'));
  const script = join(directory, 'test.ps1');
  try {
    writeFileSync(script, '\uFEFF' + `
$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest
$tokens = $null; $errors = $null
$ast = [System.Management.Automation.Language.Parser]::ParseFile($env:MAILBEACON_UI_SMOKE_SOURCE, [ref]$tokens, [ref]$errors)
if ($errors.Count -ne 0) { throw 'UI smoke source did not parse.' }
# Minimal fake COM provider: SetValue returns before Current.Value is updated.
# Import the real setter and, when present, its polling helper without invoking UIA.
foreach ($name in @('Wait-AccountEditValue', 'Set-AccountEdit')) {
  $function = $ast.Find({ param($node) $node -is [System.Management.Automation.Language.FunctionDefinitionAst] -and $node.Name -eq $name }, $true)
  if ($null -ne $function) { . ([ScriptBlock]::Create($function.Extent.Text)) }
}
Add-Type -TypeDefinition @'
using System;
using System.Diagnostics;
namespace System.Windows.Automation {
  public sealed class ValuePattern {
    public static object Pattern = new object();
    private readonly FakeEdit edit;
    public ValuePattern(FakeEdit owner) { edit = owner; }
    public FakeCurrent Current { get { return new FakeCurrent(edit); } }
    public void SetValue(string value) { edit.Written = value; edit.Sets++; edit.Elapsed.Restart(); }
  }
}
public sealed class FakeCurrent {
  private readonly FakeEdit edit;
  public FakeCurrent(FakeEdit owner) { edit = owner; }
  public string Value { get {
    edit.Reads++;
    if (edit.Password) throw new Exception("Password was read");
    return edit.Elapsed.ElapsedMilliseconds >= edit.DelayMs ? edit.Written : "old value";
  } }
}
public sealed class FakeEdit {
  public bool Password;
  public int DelayMs;
  public int Reads;
  public int Sets;
  public int Patterns;
  public string Written;
  public Stopwatch Elapsed = new Stopwatch();
  public System.Windows.Automation.ValuePattern GetCurrentPattern(object unused) {
    Patterns++;
    return new System.Windows.Automation.ValuePattern(this);
  }
}
'@
function Get-UiElement($Element) {
  return @{ offscreen=$false; enabled=$true; readOnly=$false; bounds=@{width=200}; isPassword=$Element.Password }
}
$report = @{ actions=@() }
$delayed = [FakeEdit]::new(); $delayed.DelayMs = 150
Set-AccountEdit $delayed 'Windows Smoke' 'name'
if ($delayed.Reads -lt 2 -or $delayed.Patterns -lt 2 -or $delayed.Sets -ne 1) { throw 'Input confirmation did not poll fresh values after one write.' }
if ($report.actions.Count -ne 1) { throw 'Confirmed input was not recorded.' }
$stale = [FakeEdit]::new(); $stale.DelayMs = 60000
$failed = $false
try { Wait-AccountEditValue $stale 'PRIVATE_VALUE_SENTINEL' 'email' 150 }
catch {
  if ($_.Exception.Message -cnotmatch 'Account input did not update: email' -or $_.Exception.Message -match 'PRIVATE_VALUE_SENTINEL') { throw }
  $failed = $true
}
if (-not $failed -or $stale.Reads -lt 2) { throw 'Stale input was accepted.' }
$password = [FakeEdit]::new(); $password.Password = $true
Set-AccountEdit $password 'SYNTHETIC_PASSWORD' 'password'
if ($password.Reads -ne 0 -or $password.Sets -ne 1) { throw 'Password readback is forbidden.' }
if (($report | ConvertTo-Json -Depth 5) -match 'SYNTHETIC_PASSWORD|PRIVATE_VALUE_SENTINEL|Windows Smoke') { throw 'Input values leaked into diagnostics.' }
Write-Output 'delayed input confirmation passed'
`, 'utf8');
    const result = spawnSync(powershell, ['-NoProfile', '-NonInteractive', '-File', script], {
      encoding: 'utf8', timeout: 20000,
      env: { ...process.env, MAILBEACON_UI_SMOKE_SOURCE: resolve(__dirname, '../scripts/windows-ui-smoke.ps1') },
    });
    assert.equal(result.status, 0, result.stderr || String(result.error));
    assert.match(result.stdout, /delayed input confirmation passed/);
  } finally { rmSync(directory, { recursive: true, force: true }); }
});
