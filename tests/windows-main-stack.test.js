import test, { before, after } from 'node:test';
import assert from 'node:assert/strict';
import { mkdtempSync, writeFileSync, rmSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join, resolve } from 'node:path';
import { spawnSync } from 'node:child_process';

let directory;
let executable;

before(() => {
  directory = mkdtempSync(join(tmpdir(), 'mailbeacon-build-rules-'));
  executable = join(directory, process.platform === 'win32' ? 'probe.exe' : 'probe');
  const source = join(directory, 'probe.rs');
  // Execute the real build script; only resource generation is isolated, since
  // these tests exercise linker directives independently of a Tauri build.
  writeFileSync(source, `mod tauri_build { pub fn build() { println!("tauri-build-ran"); } }\ninclude!(${JSON.stringify(resolve('src-tauri/build.rs'))});\n`);
  const compilation = spawnSync('rustc', ['--edition=2024', source, '-o', executable], {
    encoding: 'utf8', timeout: 30_000,
  });
  assert.equal(compilation.status, 0, compilation.stderr || String(compilation.error));
});

after(() => {
  if (directory) rmSync(directory, { recursive: true, force: true });
});

function buildOutput(os, targetEnvironment, host, profile = 'release') {
  const execution = spawnSync(executable, [], {
    encoding: 'utf8', timeout: 10_000,
    env: {
      ...process.env,
      CARGO_CFG_TARGET_OS: os,
      CARGO_CFG_TARGET_ENV: targetEnvironment,
      HOST: host,
      PROFILE: profile,
    },
  });
  assert.equal(execution.status, 0, execution.stderr || String(execution.error));
  const lines = execution.stdout.trim().split(/\r?\n/);
  assert.equal(lines.filter(line => line === 'tauri-build-ran').length, 1);
  return lines.filter(line => /^cargo::?rustc-link-arg/.test(line));
}

test('Windows MSVC app reserves 8 MiB of main-thread stack without increasing initial commit', () => {
  assert.deepEqual(buildOutput('windows', 'msvc', 'aarch64-apple-darwin'), [
    'cargo:rustc-link-arg-bin=mailbeacon-app=/STACK:8388608',
  ]);
});

test('Windows debug app uses the same binary-scoped stack allocation', () => {
  assert.deepEqual(buildOutput('windows', 'msvc', 'x86_64-pc-windows-msvc', 'debug'), [
    'cargo:rustc-link-arg-bin=mailbeacon-app=/STACK:8388608',
  ]);
});

test('stack setting follows the compile target and does not leak into other platforms or linkers', () => {
  for (const [os, environment] of [['macos', ''], ['linux', 'gnu'], ['windows', 'gnu'], ['windows', 'gnullvm']]) {
    assert.deepEqual(buildOutput(os, environment, 'x86_64-pc-windows-msvc'), [], `${os}/${environment}`);
  }
});
