const test = require('node:test');
const assert = require('node:assert/strict');
const { mkdtempSync, writeFileSync, rmSync } = require('node:fs');
const { tmpdir } = require('node:os');
const { join, resolve } = require('node:path');
const { spawnSync } = require('node:child_process');
const powershell = process.env.MAILBEACON_TEST_POWERSHELL || (process.platform === 'win32'
  ? join(process.env.WINDIR, 'System32', 'WindowsPowerShell', 'v1.0', 'powershell.exe') : null);

test('actual UIA snapshot retries missing control types but never evaluates incomplete trees or hides unrelated errors', {
  skip: !powershell && 'Requires Windows PowerShell or an explicitly selected local PowerShell runtime',
}, () => {
  const directory = mkdtempSync(join(tmpdir(), 'uia-snapshot-'));
  try {
    const path = join(directory, 'test.ps1');
    writeFileSync(path, '\uFEFF' + `
$ErrorActionPreference='Stop'
Set-StrictMode -Version Latest
$tokens=$null; $errors=$null
$ast=[System.Management.Automation.Language.Parser]::ParseFile($env:MAILBEACON_UI_SMOKE_SOURCE,[ref]$tokens,[ref]$errors)
foreach($name in @('Get-UiControlType','Get-UiElement','Convert-UiOffscreenValue','Wait-Stage')) {
  $fn=$ast.Find({param($node) $node -is [System.Management.Automation.Language.FunctionDefinitionAst] -and $node.Name -eq $name},$true)
  if($null -eq $fn){throw "Missing actual snapshot function: $name"}
  . ([ScriptBlock]::Create($fn.Extent.Text))
}
Add-Type -TypeDefinition @'
namespace System.Windows.Automation {
  public sealed class ControlType {
    public static readonly ControlType Button=new ControlType("ControlType.Button"),Edit=new ControlType("ControlType.Edit");
    public string ProgrammaticName; private ControlType(string name){ProgrammaticName=name;}
  }
  public static class AutomationElement {
    public static readonly object NotSupported=new object();
    public const int ControlTypeProperty=1,IsOffscreenProperty=2,IsInvokePatternAvailableProperty=3;
  }
  public sealed class ElementNotAvailableException:System.Exception {
    public ElementNotAvailableException(string message):base(message){}
  }
}
public sealed class FakeCurrent {
  public string Name="Menu"; public bool IsEnabled=true,IsPassword=false;
  public object Type; public int TypeReads;
  public object ControlType {get{TypeReads++;return Type;}}
}
public sealed class FakeElement {
  public FakeCurrent Current=new FakeCurrent();
  public object Type {get{return Current.Type;}set{Current.Type=value;}}
  public object GetCurrentPropertyValue(int id,bool ignoreDefault){return false;}
  public object GetCurrentPropertyValue(int id){return id==3;}
}
'@
function Get-Bounds($Element){return @{x=10;y=10;width=30;height=30}}
function Get-UiExpandCollapse($Element){return @{available=$false;state=$null}}
function Write-JsonFile($Path,$Value){}
$reportPath='unused';$report=@{states=@()};$script:assessments=0;$script:samples=0
$script:element=[FakeElement]::new()
foreach($invalid in @($null,[System.Windows.Automation.AutomationElement]::NotSupported,'ControlType.Button')) {
  $element.Type=$invalid
  $before=$element.Current.TypeReads
  try { $null=Get-UiElement $element;throw 'Invalid type was accepted.' } catch {
    if($_.Exception -isnot [System.Windows.Automation.ElementNotAvailableException]){throw}
  }
  if($element.Current.TypeReads -ne $before+1){throw 'ControlType was not sampled once.'}
}
function Get-Assessment($Snapshot,[string]$Stage){
  if($Snapshot.elements.Count -ne 1 -or $Snapshot.elements[0].controlType -cne 'Button'){throw 'Incomplete snapshot reached assessment.'}
  $script:assessments++;return @{ready=$true;missing=@()}
}
function Get-Snapshot {
  $script:samples++
  if($script:samples -eq 1){$element.Type=$null}else{$element.Type=[System.Windows.Automation.ControlType]::Button}
  return @{elements=@(Get-UiElement $element)}
}
$before=$element.Current.TypeReads
$null=Wait-Stage 'menu-closed' 2
if($script:samples -ne 2 -or $script:assessments -ne 1 -or $report.states.Count -ne 1){throw 'Incomplete snapshot was assessed or not retried in full.'}
if($element.Current.TypeReads -ne $before+2){throw 'Valid ControlType was read again during snapshot construction.'}
$script:samples=0;$script:assessments=0
function Get-Snapshot {$script:samples++;$element.Type=$null;return @{elements=@(Get-UiElement $element)}}
try {$null=Wait-Stage 'menu-closed' 0;throw 'Persistent missing type passed.'}catch{if($_.Exception.Message -notlike 'Timed out waiting*'){throw}}
if($script:samples -ne 1 -or $script:assessments -ne 0){throw 'Persistent invalid snapshot escaped the deadline or reached assessment.'}
function Get-Snapshot {throw [InvalidOperationException]::new('unrelated snapshot failure')}
try {$null=Wait-Stage 'menu-closed' 2;throw 'Unrelated failure was ignored.'}catch{if($_.Exception.Message -cne 'unrelated snapshot failure'){throw}}
function Get-Snapshot {$unrelated=[pscustomobject]@{};$null=$unrelated.UnrelatedProperty}
try {$null=Wait-Stage 'menu-closed' 2;throw 'Unrelated missing property was ignored.'}catch{if($_.FullyQualifiedErrorId -notlike 'PropertyNotFoundStrict*'){throw}}
Write-Output 'invalid snapshot retry passed'
`, 'utf8');
    const result = spawnSync(powershell, ['-NoProfile', '-NonInteractive', '-File', path], {
      encoding: 'utf8', timeout: 20000,
      env: { ...process.env, MAILBEACON_UI_SMOKE_SOURCE: resolve(__dirname, '../scripts/windows-ui-smoke.ps1') },
    });
    assert.equal(result.status, 0, result.stderr || String(result.error));
    assert.match(result.stdout, /invalid snapshot retry passed/);
  } finally { rmSync(directory, { recursive: true, force: true }); }
});
