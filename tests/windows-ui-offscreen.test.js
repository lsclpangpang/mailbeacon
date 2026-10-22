const test = require('node:test');
const assert = require('node:assert/strict');
const { mkdtempSync, writeFileSync, rmSync } = require('node:fs');
const { tmpdir } = require('node:os');
const { join, resolve } = require('node:path');
const { spawnSync } = require('node:child_process');

const powershell = process.env.MAILBEACON_TEST_POWERSHELL || (process.platform === 'win32'
  ? join(process.env.WINDIR, 'System32', 'WindowsPowerShell', 'v1.0', 'powershell.exe') : null);
test('actual PowerShell UIA conversion preserves true, false and the NotSupported sentinel', {
  skip: !powershell && 'Windows PowerShell required; optional local parser path can run the pure helper elsewhere',
}, () => {
  const directory = mkdtempSync(join(tmpdir(), 'ui-offscreen-'));
  const script = join(directory, 'test.ps1');
  try {
    // Import just the pure functions by AST, not the Windows application probe.
    writeFileSync(script, '\uFEFF' + `
$ErrorActionPreference = 'Stop'
$tokens = $null; $errors = $null
$ast = [System.Management.Automation.Language.Parser]::ParseFile($env:MAILBEACON_UI_SMOKE_SOURCE, [ref]$tokens, [ref]$errors)
if ($errors.Count -ne 0) { throw 'UI smoke source did not parse.' }
foreach ($name in @('Convert-UiOffscreenValue', 'Test-UiOffscreenValue')) {
  $function = $ast.Find({ param($node) $node -is [System.Management.Automation.Language.FunctionDefinitionAst] -and $node.Name -eq $name }, $true)
  if ($null -eq $function) { throw "Missing pure UIA function: $name" }
  . ([ScriptBlock]::Create($function.Extent.Text))
}
$sentinel = [object]::new()
$result = Test-UiOffscreenValue $sentinel
if (-not $result.preservesTrue -or -not $result.preservesFalse -or -not $result.preservesUnknown) { throw 'Incomplete three-state verification.' }
Write-Output 'three-state UIA conversion passed'
`, 'utf8');
    const result = spawnSync(powershell, ['-NoProfile', '-NonInteractive', '-File', script], {
      encoding: 'utf8', timeout: 20000,
      env: { ...process.env, MAILBEACON_UI_SMOKE_SOURCE: resolve(__dirname, '../scripts/windows-ui-smoke.ps1') },
    });
    assert.equal(result.status, 0, result.stderr || String(result.error));
    assert.match(result.stdout, /three-state UIA conversion passed/);
  } finally { rmSync(directory, { recursive: true, force: true }); }
});
