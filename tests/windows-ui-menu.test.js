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
