const test = require('node:test');
const assert = require('node:assert/strict');
const { mkdtempSync, readFileSync, writeFileSync, rmSync } = require('node:fs');
const { tmpdir } = require('node:os');
const { join, resolve } = require('node:path');
const { spawnSync } = require('node:child_process');

const powershell = process.env.MAILBEACON_TEST_POWERSHELL || (process.platform === 'win32'
  ? join(process.env.WINDIR, 'System32', 'WindowsPowerShell', 'v1.0', 'powershell.exe') : null);
test('actual PowerShell assessment artifacts whitelist fixture fields and exclude credentials and private paths', {
  skip: !powershell && 'Windows PowerShell required; optional local parser path can run pure helpers elsewhere',
}, () => {
  const directory = mkdtempSync(join(tmpdir(), 'ui-fixture-boundary-'));
  const script = join(directory, 'test.ps1');
  try {
    // Import the actual artifact writer and policy bridge; never run the native app probe.
    writeFileSync(script, '\uFEFF' + `
$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest
$tokens = $null; $errors = $null
$ast = [System.Management.Automation.Language.Parser]::ParseFile($env:MAILBEACON_UI_SMOKE_SOURCE, [ref]$tokens, [ref]$errors)
if ($errors.Count -ne 0) { throw 'UI smoke source did not parse.' }
foreach ($name in @('Write-JsonFile', 'Get-PublicFixture', 'Read-SharedFixtureReport', 'Get-Assessment')) {
  $function = $ast.Find({ param($node) $node -is [System.Management.Automation.Language.FunctionDefinitionAst] -and $node.Name -eq $name }, $true)
  if ($null -eq $function) { throw "Missing UIA function: $name" }
  . ([ScriptBlock]::Create($function.Extent.Text))
}
$utf8 = [Text.UTF8Encoding]::new($false)
$inputPath = Join-Path $PSScriptRoot 'uia-snapshot.json'
$assessmentPath = Join-Path $PSScriptRoot 'uia-assessment.json'
$NodeExecutable = $env:MAILBEACON_NODE
$policy = $env:MAILBEACON_UI_SMOKE_POLICY
$expected = @{ handle = '17'; processId = 42 }
$privateReport = Join-Path $PSScriptRoot 'PRIVATE_REPORT_SENTINEL.json'
[IO.File]::WriteAllText($privateReport, '{"authRejected":1,"authAccepted":0,"commands":{"LOGIN":1},"note":"本地测试"}', $utf8)
$fixtureConfig = [PSCustomObject]@{
  version = 1; host = '127.0.0.1'; imapPort = 12993; smtpHost = '127.0.0.1'; smtpPort = 9
  email = 'windows-smoke@example.invalid'; name = '本地测试'
  password = 'PASSWORD_SENTINEL'; invalidPassword = 'INVALID_PASSWORD_SENTINEL'
  caPath = 'PRIVATE_CA_SENTINEL'; reportPath = $privateReport; unexpected = 'UNEXPECTED_SENTINEL'
}
$fixturePublic = Get-PublicFixture $fixtureConfig
$result = Get-Assessment $null 'account-plan'
if ($result.provider -cne '其他') { throw 'Unicode policy response did not survive the PowerShell bridge.' }
Write-Output 'fixture assessment boundary passed'
`, 'utf8');
    const result = spawnSync(powershell, ['-NoProfile', '-NonInteractive', '-File', script], {
      encoding: 'utf8', timeout: 20000,
      env: {
        ...process.env,
        MAILBEACON_UI_SMOKE_SOURCE: resolve(__dirname, '../scripts/windows-ui-smoke.ps1'),
        MAILBEACON_UI_SMOKE_POLICY: resolve(__dirname, '../scripts/windows-ui-smoke.mjs'),
        MAILBEACON_NODE: process.execPath,
      },
    });
    assert.equal(result.status, 0, result.stderr || String(result.error));
    assert.match(result.stdout, /fixture assessment boundary passed/);
    const artifact = readFileSync(join(directory, 'uia-snapshot.json'), 'utf8');
    const snapshot = JSON.parse(artifact);
    assert.deepEqual(Object.keys(snapshot.fixture).sort(), ['email', 'host', 'imapPort', 'name', 'smtpHost', 'smtpPort', 'version']);
    assert.equal(snapshot.fixture.name, '本地测试');
    assert.equal(snapshot.server.note, '本地测试');
    assert.equal(snapshot.server.authRejected, 1);
    assert.doesNotMatch(artifact, /password|invalidPassword|caPath|reportPath|SENTINEL/i);
  } finally { rmSync(directory, { recursive: true, force: true }); }
});
