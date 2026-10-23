const test = require('node:test');
const assert = require('node:assert/strict');
const { mkdtempSync, writeFileSync, rmSync } = require('node:fs');
const { tmpdir } = require('node:os');
const { join, resolve } = require('node:path');
const { spawnSync } = require('node:child_process');
const powershell = process.env.MAILBEACON_TEST_POWERSHELL || (process.platform === 'win32'
  ? join(process.env.WINDIR, 'System32', 'WindowsPowerShell', 'v1.0', 'powershell.exe') : null);

test('actual PowerShell menu ancestry preserves system menus sharing the top-level HWND', {
  skip: !powershell && 'Requires Windows PowerShell or an explicitly selected local PowerShell runtime',
}, () => {
  const directory = mkdtempSync(join(tmpdir(), 'uia-menu-'));
  try {
    const path = join(directory, 'test.ps1');
    writeFileSync(path, '\uFEFF' + `
$ErrorActionPreference='Stop'
$tokens=$null; $errors=$null
$ast=[System.Management.Automation.Language.Parser]::ParseFile($env:MAILBEACON_UI_SMOKE_SOURCE,[ref]$tokens,[ref]$errors)
$fn=$ast.Find({param($node) $node -is [System.Management.Automation.Language.FunctionDefinitionAst] -and $node.Name -eq 'Test-InTitleBar'},$true)
if($null -eq $fn){throw 'Missing actual menu ancestry function.'}
. ([ScriptBlock]::Create($fn.Extent.Text))
Add-Type -TypeDefinition @'
namespace System.Windows.Automation {
  public static class ControlType { public const string TitleBar="TitleBar", Window="Window"; }
  public sealed class TreeWalker {
    public static TreeWalker ControlViewWalker=new TreeWalker();
    public FakeElement GetParent(FakeElement e){return e.Parent;}
  }
}
public sealed class FakeCurrent { public string ControlType; public long NativeWindowHandle; }
public sealed class FakeElement {
  public FakeCurrent Current=new FakeCurrent(); public FakeElement Parent;
  public FakeElement(string type,long handle,FakeElement parent){Current.ControlType=type;Current.NativeWindowHandle=handle;Parent=parent;}
}
'@
$WindowHandle=1234
$root=[FakeElement]::new('Window',1234,$null)
$title=[FakeElement]::new('TitleBar',1234,$root)
$system=[FakeElement]::new('MenuBar',1234,$title)
if(-not (Test-InTitleBar $system)){throw 'System menu sharing the native HWND was misclassified.'}
$app=[FakeElement]::new('MenuBar',1234,$root)
if(Test-InTitleBar $app){throw 'Application menu bar was incorrectly exempted.'}
Write-Output 'native menu ancestry passed'
`, 'utf8');
    const result = spawnSync(powershell, ['-NoProfile', '-NonInteractive', '-File', path], {
      encoding: 'utf8', timeout: 20000,
      env: { ...process.env, MAILBEACON_UI_SMOKE_SOURCE: resolve(__dirname, '../scripts/windows-ui-smoke.ps1') },
    });
    assert.equal(result.status, 0, result.stderr || String(result.error));
    assert.match(result.stdout, /native menu ancestry passed/);
  } finally { rmSync(directory, { recursive: true, force: true }); }
});

test('actual PowerShell samples and expands menu providers without relaxing ordinary button invocation', {
  skip: !powershell && 'Requires Windows PowerShell or an explicitly selected local PowerShell runtime',
}, () => {
  const directory = mkdtempSync(join(tmpdir(), 'uia-menu-pattern-'));
  try {
    const path = join(directory, 'test.ps1');
    writeFileSync(path, '\uFEFF' + `
$ErrorActionPreference='Stop'
Set-StrictMode -Version Latest
$tokens=$null; $errors=$null
$ast=[System.Management.Automation.Language.Parser]::ParseFile($env:MAILBEACON_UI_SMOKE_SOURCE,[ref]$tokens,[ref]$errors)
foreach($name in @('Get-UiExpandCollapse','Open-ApplicationMenu','Invoke-MailboxButton')) {
  $fn=$ast.Find({param($node) $node -is [System.Management.Automation.Language.FunctionDefinitionAst] -and $node.Name -eq $name},$true)
  if($null -eq $fn){throw "Missing actual provider function: $name"}
  . ([ScriptBlock]::Create($fn.Extent.Text))
}
Add-Type -TypeDefinition @'
namespace System.Windows.Automation {
  public enum ExpandCollapseState { Collapsed, Expanded, PartiallyExpanded, LeafNode }
  public sealed class ExpandState { public ExpandCollapseState ExpandCollapseState; }
  public sealed class ExpandCollapsePattern {
    public static readonly object Pattern=new object();
    public ExpandState Current=new ExpandState(); public int Calls; public bool Fail;
    public void Expand(){Calls++;if(Fail)throw new System.InvalidOperationException("Expansion failed");}
  }
  public sealed class InvokePattern {
    public static readonly object Pattern=new object(); public int Calls;
    public void Invoke(){Calls++;}
  }
}
public sealed class FakeMenuButton {
  public System.Windows.Automation.ExpandCollapsePattern Expand;
  public System.Windows.Automation.InvokePattern Invoke;
  public bool TryGetCurrentPattern(object id,out object pattern){
    pattern=object.ReferenceEquals(id,System.Windows.Automation.ExpandCollapsePattern.Pattern)?(object)Expand:Invoke;
    return pattern!=null;
  }
  public object GetCurrentPattern(object id){object pattern;if(!TryGetCurrentPattern(id,out pattern))throw new System.InvalidOperationException("Pattern unavailable");return pattern;}
}
'@
function Get-VisibleButton([string]$Name) { return $script:button }
function Write-JsonFile($Path,$Value) {}
$WindowHandle=1234; $reportPath='unused'; $report=@{actions=@()}
$button=[FakeMenuButton]::new()
$button.Expand=[System.Windows.Automation.ExpandCollapsePattern]::new()
$sample=Get-UiExpandCollapse $button
if($sample.available -ne $true -or $sample.state -cne 'Collapsed'){throw 'Actual menu pattern was not recorded.'}
Open-ApplicationMenu '应用菜单'
if($button.Expand.Calls -ne 1 -or $report.actions[-1].method -cne 'ExpandCollapsePattern.Expand' -or $report.actions[-1].phase -cne 'invoked'){throw 'Menu was not expanded through its actual provider.'}
try { Invoke-MailboxButton '登录并添加'; throw 'Login accepted an expand-only provider.' } catch { if($_.Exception.Message -like '*Login accepted*'){throw} }
foreach($state in @('Expanded','PartiallyExpanded','LeafNode')) {
  $button.Expand.Current.ExpandCollapseState=[System.Windows.Automation.ExpandCollapseState]$state
  $before=$button.Expand.Calls
  try { Open-ApplicationMenu '应用菜单'; throw 'Invalid menu state was accepted.' } catch { if($_.Exception.Message -like '*Invalid menu state*'){throw} }
  if($button.Expand.Calls -ne $before){throw 'Invalid provider state activated.'}
}
$button.Expand=$null
$sample=Get-UiExpandCollapse $button
if($sample.available -ne $false -or $null -ne $sample.state){throw 'Absent menu provider was inferred.'}
try { Open-ApplicationMenu '应用菜单'; throw 'Missing provider was accepted.' } catch { if($_.Exception.Message -like '*Missing provider was accepted*'){throw} }
$button.Invoke=[System.Windows.Automation.InvokePattern]::new()
Open-ApplicationMenu '应用菜单'
Invoke-MailboxButton '登录并添加'
if($button.Invoke.Calls -ne 2){throw 'Ordinary buttons no longer invoke.'}
$button.Expand=[System.Windows.Automation.ExpandCollapsePattern]::new();$button.Expand.Fail=$true
try { Open-ApplicationMenu '应用菜单'; throw 'Failed expansion was ignored.' } catch { if($_.Exception.Message -like '*Failed expansion was ignored*'){throw} }
if($button.Invoke.Calls -ne 2){throw 'Failed expansion silently fell back to Invoke.'}
Write-Output 'native menu provider passed'
`, 'utf8');
    const result = spawnSync(powershell, ['-NoProfile', '-NonInteractive', '-File', path], {
      encoding: 'utf8', timeout: 20000,
      env: { ...process.env, MAILBEACON_UI_SMOKE_SOURCE: resolve(__dirname, '../scripts/windows-ui-smoke.ps1') },
    });
    assert.equal(result.status, 0, result.stderr || String(result.error));
    assert.match(result.stdout, /native menu provider passed/);
  } finally { rmSync(directory, { recursive: true, force: true }); }
});
