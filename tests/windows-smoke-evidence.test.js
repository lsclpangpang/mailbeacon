const test = require('node:test');
const assert = require('node:assert/strict');
const { mkdtempSync, writeFileSync, rmSync } = require('node:fs');
const { tmpdir } = require('node:os');
const { join, resolve } = require('node:path');
const { spawnSync } = require('node:child_process');
const powershell = process.env.MAILBEACON_TEST_POWERSHELL || (process.platform === 'win32'
  ? join(process.env.WINDIR, 'System32', 'WindowsPowerShell', 'v1.0', 'powershell.exe') : null);

test('native account smoke requires every UI stage, real authentication and fresh restart activity', {
  skip: !powershell && 'Requires Windows PowerShell or an explicitly selected local PowerShell runtime',
}, () => {
  const directory = mkdtempSync(join(tmpdir(), 'account-smoke-evidence-'));
  const script = join(directory, 'test.ps1');
  try {
    writeFileSync(script, '\uFEFF' + `
$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest
$tokens = $null; $errors = $null
$ast = [System.Management.Automation.Language.Parser]::ParseFile($env:MAILBEACON_SMOKE_SOURCE, [ref]$tokens, [ref]$errors)
if ($errors.Count -ne 0) { throw 'Installer smoke source did not parse.' }
foreach ($name in @('Format-AppExitCode', 'Assert-AccountSmokeEvidence')) {
  $function = $ast.Find({ param($node) $node -is [System.Management.Automation.Language.FunctionDefinitionAst] -and $node.Name -eq $name }, $true)
  if ($null -eq $function) { throw "Missing evidence function: $name" }
  . ([ScriptBlock]::Create($function.Extent.Text))
}
function Must-Reject([scriptblock]$Action) {
  $rejected = $false
  try { & $Action | Out-Null } catch { $rejected = $true }
  if (-not $rejected) { throw 'Incomplete account evidence was accepted.' }
}
$scenario = [ordered]@{ rejected=$true; edited=$true; switchedProvider=$true; retried=$true; added=$true; inboxReady=$true }
$ui = [pscustomobject]@{ passed=$true; accountScenario=[pscustomobject]$scenario }
$mail = [pscustomobject]@{ authRejected=1; authAccepted=2; selectAccepted=1; commands=[pscustomobject]@{ SELECT=1 } }
Assert-AccountSmokeEvidence $ui $mail | Out-Null
foreach ($field in $scenario.Keys) {
  $ui.accountScenario.$field = $false
  Must-Reject { Assert-AccountSmokeEvidence $ui $mail }
  $ui.accountScenario.$field = $true
}
$mail.authRejected=0; Must-Reject { Assert-AccountSmokeEvidence $ui $mail }; $mail.authRejected=1
$mail.authAccepted=1; Must-Reject { Assert-AccountSmokeEvidence $ui $mail }; $mail.authAccepted=2
$mail.selectAccepted=0; Must-Reject { Assert-AccountSmokeEvidence $ui $mail }; $mail.selectAccepted=1
$ui.passed=$false; Must-Reject { Assert-AccountSmokeEvidence $ui $mail }; $ui.passed=$true
$resume = [pscustomobject]@{ passed=$true; accountScenario=[pscustomobject]@{ resumed=$true; inboxReady=$true } }
Must-Reject { Assert-AccountSmokeEvidence $resume $mail -ResumeAccount -AcceptedBefore 2 -SelectedBefore 1 }
$mail.authAccepted=3
Must-Reject { Assert-AccountSmokeEvidence $resume $mail -ResumeAccount -AcceptedBefore 2 -SelectedBefore 1 }
$mail.selectAccepted=2
Assert-AccountSmokeEvidence $resume $mail -ResumeAccount -AcceptedBefore 2 -SelectedBefore 1 | Out-Null
$resume.accountScenario.resumed=$false
Must-Reject { Assert-AccountSmokeEvidence $resume $mail -ResumeAccount -AcceptedBefore 2 -SelectedBefore 1 }
if ((Format-AppExitCode -1073741571) -ne '0xC00000FD') { throw 'Stack overflow exit code lost its Windows status bits.' }
if ((Format-AppExitCode -1073741819) -ne '0xC0000005') { throw 'Access violation exit code lost its Windows status bits.' }
Write-Output 'account evidence and Windows exit codes passed'
`, 'utf8');
    const result = spawnSync(powershell, ['-NoProfile', '-NonInteractive', '-File', script], {
      encoding: 'utf8', timeout: 20000,
      env: { ...process.env, MAILBEACON_SMOKE_SOURCE: resolve(__dirname, '../scripts/windows-smoke.ps1') },
    });
    assert.equal(result.status, 0, result.stderr || String(result.error));
    assert.match(result.stdout, /account evidence and Windows exit codes passed/);
  } finally { rmSync(directory, { recursive: true, force: true }); }
});
