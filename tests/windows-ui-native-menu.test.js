const test = require('node:test');
const assert = require('node:assert/strict');
const { mkdtempSync, writeFileSync, rmSync } = require('node:fs');
const { tmpdir } = require('node:os');
const { join, resolve } = require('node:path');
const { spawnSync } = require('node:child_process');
const powershell = process.env.MAILBEACON_TEST_POWERSHELL || (process.platform === 'win32'
  ? join(process.env.WINDIR, 'System32', 'WindowsPowerShell', 'v1.0', 'powershell.exe') : null);

test('actual Win32 menu declarations match x64 ABI and preserve Unicode, state and screen coordinates', {
  skip: !powershell && 'Requires Windows PowerShell or an explicitly selected local PowerShell runtime',
}, () => {
  const directory = mkdtempSync(join(tmpdir(), 'native-menu-layout-'));
  try {
    const path = join(directory, 'test.ps1');
    writeFileSync(path, '\uFEFF' + `
$ErrorActionPreference='Stop'
$tokens=$null; $errors=$null
$ast=[System.Management.Automation.Language.Parser]::ParseFile($env:MAILBEACON_UI_SMOKE_SOURCE,[ref]$tokens,[ref]$errors)
if($errors.Count -ne 0){throw 'PowerShell parse failed.'}
$fn=$ast.Find({param($node) $node -is [System.Management.Automation.Language.FunctionDefinitionAst] -and $node.Name -eq 'Convert-Win32PopupItems'},$true)
if($null -eq $fn){throw 'Missing real Win32 popup conversion.'}
. ([ScriptBlock]::Create($fn.Extent.Text))
$definitions=$ast.FindAll({param($node) $node -is [System.Management.Automation.Language.StringConstantExpressionAst] -and $node.Value.Contains('public static class MailBeaconWindowVisibility')},$true)
if($definitions.Count -ne 1){throw 'Expected one native reader definition.'}
Add-Type -TypeDefinition $definitions[0].Value
if([IntPtr]::Size -ne 8){throw 'Installer and this ABI test require a 64-bit process.'}
foreach($layout in @(@{name='MenuBarInfo';size=48;fields=@{Menu=24;MenuWindow=32;Flags=40}},@{name='MenuItemInfo';size=80;fields=@{SubMenu=24;Text=56;Length=64;Bitmap=72}})){
  $type=[MailBeaconWindowVisibility].GetNestedType($layout.name,[Reflection.BindingFlags]::NonPublic)
  if($null -eq $type -or [Runtime.InteropServices.Marshal]::SizeOf([Activator]::CreateInstance($type)) -ne $layout.size){throw "Incorrect x64 size: $($layout.name)"}
  foreach($field in $layout.fields.Keys){if([Runtime.InteropServices.Marshal]::OffsetOf($type,$field).ToInt64() -ne $layout.fields[$field]){throw "Incorrect offset: $field"}}
}
$method=[MailBeaconWindowVisibility].GetMethod('GetMenuItemInfoW',([Reflection.BindingFlags]::Static -bor [Reflection.BindingFlags]::NonPublic))
$attribute=$method.GetCustomAttributes([Runtime.InteropServices.DllImportAttribute],$false)[0]
if($attribute.EntryPoint -cne 'GetMenuItemInfoW' -or $attribute.CharSet -ne [Runtime.InteropServices.CharSet]::Unicode){throw 'Menu text must use the Unicode Win32 API.'}
$raw=[pscustomobject]@{Menu=[IntPtr]123456;Window=[IntPtr]987;Thread=77;OwnerThread=77;Items=@(
  [pscustomobject]@{Name='文件';Enabled=$true;Separator=$false;Bounds=@{Left=18;Top=80;Right=178;Bottom=100}},
  [pscustomobject]@{Name='不可用';Enabled=$false;Separator=$false;Bounds=@{Left=18;Top=100;Right=178;Bottom=120}}
)}
$sample=Convert-Win32PopupItems $raw
if($sample.win32.menuHandle -cne '123456' -or $sample.win32.windowHandle -cne '987' -or $sample.win32.threadId -ne 77){throw 'Popup identity was lost.'}
if($sample.win32Items[0].name -cne '文件' -or $sample.win32Items[0].bounds.x -ne 18 -or $sample.win32Items[0].bounds.width -ne 160){throw 'Unicode or screen rectangle was lost.'}
if($sample.win32Items[1].enabled -ne $false){throw 'Disabled native item became enabled.'}
Write-Output 'native menu ABI and conversion passed'
`, 'utf8');
    const result = spawnSync(powershell, ['-NoProfile', '-NonInteractive', '-File', path], {
      encoding: 'utf8', timeout: 20000,
      env: { ...process.env, MAILBEACON_UI_SMOKE_SOURCE: resolve(__dirname, '../scripts/windows-ui-smoke.ps1') },
    });
    assert.equal(result.status, 0, result.stderr || String(result.error));
    assert.match(result.stdout, /native menu ABI and conversion passed/);
  } finally { rmSync(directory, { recursive: true, force: true }); }
});
