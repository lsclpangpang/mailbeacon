const test = require('node:test');
const assert = require('node:assert/strict');
const { mkdtempSync, writeFileSync, rmSync } = require('node:fs');
const { tmpdir } = require('node:os');
const { join, resolve } = require('node:path');
const { spawnSync } = require('node:child_process');

const powershell = process.env.MAILBEACON_TEST_POWERSHELL || (process.platform === 'win32'
  ? join(process.env.WINDIR, 'System32', 'WindowsPowerShell', 'v1.0', 'powershell.exe') : null);
test('PowerShell reads PE32+ stack reserve as uint64, rejects malformed headers and reads each required byte once', {
  skip: !powershell && 'Requires Windows PowerShell or an explicitly selected local PowerShell runtime',
}, () => {
  const directory = mkdtempSync(join(tmpdir(), 'pe-stack-'));
  const script = join(directory, 'test.ps1');
  const pe = Buffer.alloc(512);
  pe.writeUInt16LE(0x5a4d, 0); pe.writeUInt32LE(0x80, 0x3c);
  pe.writeUInt32LE(0x4550, 0x80); pe.writeUInt16LE(0x8664, 0x84);
  pe.writeUInt16LE(240, 0x94); pe.writeUInt16LE(0x20b, 0x98);
  pe.writeBigUInt64LE(0x100200000n, 0x98 + 72); // Must not truncate to 32 bits.
  try {
    writeFileSync(join(directory, 'valid.exe'), pe);
    writeFileSync(script, '\uFEFF' + `
$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest
$tokens = $null; $errors = $null
$ast = [System.Management.Automation.Language.Parser]::ParseFile($env:MAILBEACON_SMOKE_SOURCE, [ref]$tokens, [ref]$errors)
if ($errors.Count -ne 0) { throw 'Installer smoke source did not parse.' }
foreach ($name in @('Read-PeStackReserve', 'Get-PeStackReserve')) {
  $function = $ast.Find({ param($node) $node -is [System.Management.Automation.Language.FunctionDefinitionAst] -and $node.Name -eq $name }, $true)
  if ($null -eq $function) { throw "Missing PE function: $name" }
  . ([ScriptBlock]::Create($function.Extent.Text))
}
Add-Type -TypeDefinition @'
using System;
using System.IO;
using System.Collections.Generic;
public sealed class CountedPeStream : Stream {
  private readonly MemoryStream data;
  private readonly HashSet<long> positions = new HashSet<long>();
  public int ReadBytes; public bool ReadRepeated;
  public CountedPeStream(byte[] bytes) { data = new MemoryStream(bytes); }
  public override int Read(byte[] buffer,int offset,int count) {
    long begin=data.Position; int n=data.Read(buffer,offset,count);
    for(int i=0;i<n;i++) if(!positions.Add(begin+i)) ReadRepeated=true;
    ReadBytes+=n; return n;
  }
  public override bool CanRead {get{return true;}} public override bool CanSeek {get{return true;}}
  public override bool CanWrite {get{return false;}} public override long Length {get{return data.Length;}}
  public override long Position {get{return data.Position;}set{data.Position=value;}}
  public override long Seek(long offset,SeekOrigin origin){return data.Seek(offset,origin);}
  public override void Flush(){} public override void SetLength(long n){throw new NotSupportedException();}
  public override void Write(byte[] b,int o,int c){throw new NotSupportedException();}
}
'@
$path = Join-Path $PSScriptRoot 'valid.exe'
$bytes = [IO.File]::ReadAllBytes($path)
$stream = [CountedPeStream]::new($bytes)
try {
  $reserve = Read-PeStackReserve $stream
  if ($reserve -isnot [uint64] -or $reserve -ne [uint64]4297064448) { throw 'PE32+ reserve was truncated or read at the wrong offset.' }
  if ($stream.ReadRepeated -or $stream.ReadBytes -ne 22) { throw "PE inspection repeated or read unnecessary bytes: $($stream.ReadBytes)." }
} finally { $stream.Dispose() }
if ((Get-PeStackReserve $path) -ne [uint64]4297064448) { throw 'Installed executable path wrapper read a different value.' }
# Old 1 MiB and fixed 8 MiB values are both diagnostic evidence, never a pass gate.
foreach ($size in @([uint64]1048576, [uint64]8388608)) {
  [BitConverter]::GetBytes($size).CopyTo($bytes, 224)
  [IO.File]::WriteAllBytes($path, $bytes)
  if ((Get-PeStackReserve $path) -ne $size) { throw 'Valid stack reserve was rejected or changed.' }
}
foreach ($kind in @('dos', 'signature', 'pe32', 'offset', 'short-dos', 'short-optional', 'oversize-optional')) {
  $bad = [byte[]]$bytes.Clone()
  switch ($kind) {
    'dos' { $bad[0]=0 }
    'signature' { $bad[128]=0 }
    'pe32' { $bad[153]=1 }
    'offset' { [BitConverter]::GetBytes([uint32]4294967295).CopyTo($bad,60) }
    'short-dos' { $bad=[byte[]]$bad[0..31] }
    'short-optional' { $bad[148]=79; $bad[149]=0 }
    'oversize-optional' { $bad[148]=255; $bad[149]=255 }
  }
  $badPath=Join-Path $PSScriptRoot 'bad.exe'; [IO.File]::WriteAllBytes($badPath,$bad)
  $rejected=$false
  try { Get-PeStackReserve $badPath | Out-Null } catch { $rejected=$true }
  if (-not $rejected) { throw "Invalid PE accepted: $kind" }
}
Write-Output 'PE32+ stack reserve inspection passed'
`, 'utf8');
    const result = spawnSync(powershell, ['-NoProfile', '-NonInteractive', '-File', script], {
      encoding: 'utf8', timeout: 20000,
      env: { ...process.env, MAILBEACON_SMOKE_SOURCE: resolve(__dirname, '../scripts/windows-smoke.ps1') },
    });
    assert.equal(result.status, 0, result.stderr || String(result.error));
    assert.match(result.stdout, /PE32\+ stack reserve inspection passed/);
  } finally { rmSync(directory, { recursive: true, force: true }); }
});
