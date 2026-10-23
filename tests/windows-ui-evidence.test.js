const test = require('node:test');
const assert = require('node:assert/strict');
const { mkdtempSync, writeFileSync, rmSync } = require('node:fs');
const { tmpdir } = require('node:os');
const { join, resolve } = require('node:path');
const { spawnSync } = require('node:child_process');
const powershell = process.env.MAILBEACON_TEST_POWERSHELL || (process.platform === 'win32'
  ? join(process.env.WINDIR, 'System32', 'WindowsPowerShell', 'v1.0', 'powershell.exe') : null);

test('actual native-menu flow retains successful raw stage evidence even if a later stage fails', {
  skip: !powershell && 'Requires Windows PowerShell or an explicitly selected local PowerShell runtime',
}, () => {
  const directory = mkdtempSync(join(tmpdir(), 'uia-stage-evidence-'));
  try {
    const path = join(directory, 'test.ps1');
    writeFileSync(path, '\uFEFF' + `
$ErrorActionPreference='Stop'
Set-StrictMode -Version Latest
$tokens=$null; $errors=$null
$ast=[System.Management.Automation.Language.Parser]::ParseFile($env:MAILBEACON_UI_SMOKE_SOURCE,[ref]$tokens,[ref]$errors)
foreach($name in @('Write-JsonFile','Save-StageEvidence','Test-NativeMenu')) {
  $fn=$ast.Find({param($node) $node -is [System.Management.Automation.Language.FunctionDefinitionAst] -and $node.Name -eq $name},$true)
  if($null -eq $fn){throw "Missing real evidence function: $name"}
  . ([ScriptBlock]::Create($fn.Extent.Text))
}
$utf8=[Text.UTF8Encoding]::new($false)
$OutputDirectory=Join-Path $PSScriptRoot 'success';$null=New-Item -ItemType Directory $OutputDirectory
$report=@{states=@()};$script:keys=@();$script:waits=@();$script:failCompose=$false
function Wait-Stage([string]$Stage,[int]$Seconds){
  $script:waits+=@{stage=$Stage;seconds=$Seconds}
  if($Stage -eq 'compose-open' -and $script:failCompose){throw 'compose timeout'}
  return @{snapshot=@{window=@{handle='1234'};nativeMenu=@{windows=@(@{elements=@();win32=@{menuHandle='5678'};win32Items=@(@{name='文件';enabled=$true})})}};assessment=@{ready=$true;menuButton='应用菜单'}}
}
function Open-ApplicationMenu([string]$Name){if($Name -cne '应用菜单'){throw 'Unexpected menu button'}}
function Send-AppKeys([string]$Keys,[string]$Label){$script:keys+=$Keys}
function Save-UiScreenshot([string]$Name){}
Test-NativeMenu
$report.states=@() # Simulate rolling diagnostic snapshots being overwritten.
$expected=@{
  'native-menu-initial'='menu-closed';'native-menu-open'='menu-open';'native-menu-escaped'='menu-closed'
  'native-menu-compose'='compose-open';'native-menu-compose-closed'='menu-closed';'native-menu-restored'='inbox'
}
foreach($name in $expected.Keys){
  $saved=[IO.File]::ReadAllText((Join-Path $OutputDirectory ($name+'.json')),$utf8)|ConvertFrom-Json
  if($saved.stage -cne $expected[$name] -or $saved.assessment.ready -ne $true -or $saved.snapshot.window.handle -cne '1234'){throw 'Successful stage evidence was not preserved.'}
}
$open=[IO.File]::ReadAllText((Join-Path $OutputDirectory 'native-menu-open.json'),$utf8)|ConvertFrom-Json
if($open.snapshot.nativeMenu.windows[0].elements.Count -ne 0 -or $open.snapshot.nativeMenu.windows[0].win32Items[0].name -cne '文件'){throw 'Raw Unicode Win32/UIA evidence changed.'}
if(($script:keys -join ',') -cne '{ESC},^n,{ESC}' -or $script:waits.Count -ne 6 -or ($script:waits|Where-Object seconds -ne 10)){throw 'Menu inputs, stages or timeouts changed.'}
$OutputDirectory=Join-Path $PSScriptRoot 'failed';$null=New-Item -ItemType Directory $OutputDirectory
$report=@{states=@()};$script:failCompose=$true
try{Test-NativeMenu;throw 'Expected later-stage failure.'}catch{if($_.Exception.Message -cne 'compose timeout'){throw}}
if(-not (Test-Path (Join-Path $OutputDirectory 'native-menu-open.json')) -or (Test-Path (Join-Path $OutputDirectory 'native-menu-compose.json'))){throw 'Failure lost prior evidence or fabricated a completed stage.'}
$resume=$ast.Find({param($node) $node -is [System.Management.Automation.Language.IfStatementAst] -and $node.Clauses[0].Item1.Extent.Text -ceq '$ResumeAccount'},$true)
if($null -eq $resume){throw 'Missing actual restart flow.'}
$OutputDirectory=Join-Path $PSScriptRoot 'restart';$null=New-Item -ItemType Directory $OutputDirectory
$fixtureConfig=@{version=1};$report=@{};$script:waits=@()
. ([ScriptBlock]::Create(($resume.Clauses[0].Item2.Statements | ForEach-Object {$_.Extent.Text}) -join [Environment]::NewLine))
foreach($name in @('account-resumed-inbox','native-menu-resumed')){
  $saved=[IO.File]::ReadAllText((Join-Path $OutputDirectory ($name+'.json')),$utf8)|ConvertFrom-Json
  if($saved.assessment.ready -ne $true -or $saved.snapshot.window.handle -cne '1234'){throw 'Restart evidence was not preserved.'}
}
if($script:waits.Count -ne 2 -or $script:waits[0].stage -cne 'inbox' -or $script:waits[0].seconds -ne 60 -or $script:waits[1].stage -cne 'menu-closed' -or $script:waits[1].seconds -ne 10){throw 'Restart assertions or timeouts changed.'}
Write-Output 'persistent native stage evidence passed'
`, 'utf8');
    const result = spawnSync(powershell, ['-NoProfile', '-NonInteractive', '-File', path], {
      encoding: 'utf8', timeout: 20000,
      env: { ...process.env, MAILBEACON_UI_SMOKE_SOURCE: resolve(__dirname, '../scripts/windows-ui-smoke.ps1') },
    });
    assert.equal(result.status, 0, result.stderr || String(result.error));
    assert.match(result.stdout, /persistent native stage evidence passed/);
  } finally { rmSync(directory, { recursive: true, force: true }); }
});
