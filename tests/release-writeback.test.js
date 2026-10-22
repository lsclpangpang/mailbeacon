// This exercises the Bash + SSH writeback job, which deliberately runs only on macOS/Linux.
// Keep its real filesystem/permission tests there; never require Unix executables on Windows.
const nodeTest = require('node:test');
const test = process.platform === 'win32' ? (name, fn) => nodeTest(name, { skip: 'Unix-only release writeback job' }, fn) : nodeTest;
const assert = require('node:assert/strict');
const fs = require('node:fs');
const os = require('node:os');
const path = require('node:path');
const { execFileSync, spawnSync } = require('node:child_process');

const script = path.resolve(__dirname, '../scripts/release-writeback.sh');
const git = process.platform === 'win32' ? null : execFileSync('which', ['git'], { encoding: 'utf8' }).trim();

function fixture(t, options = {}) {
  const root = fs.mkdtempSync(path.join(os.tmpdir(), 'mb-writeback-'));
  t.after(() => fs.rmSync(root, { recursive: true, force: true }));
  const repo = path.join(root, 'repo');
  const remote = path.join(root, 'remote.git');
  const bin = path.join(root, 'bin');
  const temp = path.join(root, 'runner temp');
  for (const dir of [repo, bin, temp]) fs.mkdirSync(dir);
  const runGit = (...args) => execFileSync(git, args, { cwd: repo, encoding: 'utf8', stdio: ['ignore', 'pipe', 'pipe'] }).trim();
  runGit('init', '-b', 'main');
  runGit('config', 'user.name', 'Test');
  runGit('config', 'user.email', 'test@example.invalid');
  fs.mkdirSync(path.join(repo, 'src-tauri'));
  fs.mkdirSync(path.join(repo, 'scripts'));
  const versionFile = path.join(repo, 'src-tauri/tauri.conf.json');
  fs.writeFileSync(versionFile, '{"version":"0.1.0"}\n');
  fs.writeFileSync(path.join(repo, 'app.js'), '// app\n');
  fs.writeFileSync(path.join(repo, 'scripts/release.js'), `require('fs').writeFileSync('src-tauri/tauri.conf.json', '{"version":"0.1.1"}\\n'); console.log('0.1.1');\n`);
  runGit('add', '.');
  runGit('commit', '-qm', 'base');
  runGit('init', '--bare', remote);
  runGit('remote', 'add', 'origin', remote);
  runGit('push', '-qu', 'origin', 'main');
  if (!options.clean) fs.writeFileSync(versionFile, '{"version":"0.1.1"}\n');
  const logFile = path.join(root, 'calls.jsonl');
  const writeBin = (name, source) => fs.writeFileSync(path.join(bin, name), `#!${process.execPath}\n${source}`, { mode: 0o755 });
  writeBin('git', `
    const fs = require('fs');
    const args = process.argv.slice(2);
    const ssh = process.env.GIT_SSH_COMMAND || '';
    const key = ssh.match(/-i "([^"]+)"/);
    const record = { args, ssh, secretInEnv: !!process.env.RELEASE_DEPLOY_KEY };
    if (key) { record.keyMode = fs.statSync(key[1]).mode & 0o777; record.keyExists = true; }
    fs.appendFileSync(${JSON.stringify(logFile)}, JSON.stringify(record) + '\\n');
    if (args[0] === 'push') {
      const count = fs.readFileSync(${JSON.stringify(logFile)}, 'utf8').trim().split('\\n').map(JSON.parse).filter(x => x.args[0] === 'push').length;
      if (count <= ${options.failPushes || 0}) process.exit(1);
      args[args.indexOf('git@github.com:fixture/test.git')] = ${JSON.stringify(remote)};
    }
    const result = require('child_process').spawnSync(${JSON.stringify(git)}, args, { stdio: 'inherit' });
    process.exit(result.status ?? 1);
  `);
  writeBin('gh', `console.log('github.com ssh-ed25519 fixture-host-key');\n`);
  const env = {
    ...process.env, PATH: `${bin}:${process.env.PATH}`, GITHUB_EVENT_NAME: 'push',
    GITHUB_REF: 'refs/heads/main', GITHUB_REPOSITORY: 'fixture/test', RUNNER_TEMP: temp,
    GITHUB_OUTPUT: path.join(root, 'output'), RELEASE_DEPLOY_KEY: 'fixture-private-key',
  };
  return {
    repo, temp, env, runGit,
    run: (overrides = {}) => spawnSync('bash', [script], { cwd: repo, env: { ...env, ...overrides }, encoding: 'utf8' }),
    calls: () => fs.existsSync(logFile) ? fs.readFileSync(logFile, 'utf8').trim().split('\n').map(JSON.parse) : [],
  };
}

test('version writeback uses the dedicated SSH key only for push and prevents another workflow run', (t) => {
  const f = fixture(t);
  const result = f.run();
  assert.equal(result.status, 0, result.stderr);
  assert.equal(f.runGit('log', '-1', '--format=%s', 'main'), 'chore(release): v0.1.1 [skip ci]');
  assert.equal(f.runGit('rev-parse', 'main'), f.runGit('ls-remote', 'origin', 'refs/heads/main').split('\t')[0]);
  assert.match(fs.readFileSync(f.env.GITHUB_OUTPUT, 'utf8'), /^sha=[a-f0-9]{40}\n$/);
  const pushes = f.calls().filter(x => x.args[0] === 'push');
  assert.equal(pushes.length, 1);
  assert.ok(pushes[0].args.includes('git@github.com:fixture/test.git'));
  assert.match(pushes[0].ssh, /StrictHostKeyChecking=yes/);
  assert.match(pushes[0].ssh, /IdentitiesOnly=yes/);
  assert.equal(pushes[0].keyMode, 0o600);
  assert.ok(f.calls().every(x => !x.secretInEnv));
  assert.ok(f.calls().filter(x => x.args[0] !== 'push').every(x => !x.ssh));
  assert.deepEqual(fs.readdirSync(f.temp), [], 'temporary credentials are removed');
  assert.doesNotMatch(result.stdout + result.stderr, /fixture-private-key/);
});

test('a rejected push fetches main and recalculates before retrying', (t) => {
  const f = fixture(t, { failPushes: 1 });
  const result = f.run();
  assert.equal(result.status, 0, result.stderr);
  assert.equal(f.calls().filter(x => x.args[0] === 'push').length, 2);
  assert.equal(f.calls().filter(x => x.args[0] === 'fetch').length, 1);
  assert.equal(f.runGit('rev-list', '--count', 'main'), '2');
});

test('persistent push failure stops after three attempts and removes the key', (t) => {
  const f = fixture(t, { failPushes: 3 });
  const result = f.run();
  assert.notEqual(result.status, 0);
  assert.equal(f.calls().filter(x => x.args[0] === 'push').length, 3);
  assert.deepEqual(fs.readdirSync(f.temp), []);
  assert.ok(!fs.existsSync(f.env.GITHUB_OUTPUT));
});

test('a clean version plan outputs its SHA without making a new commit', (t) => {
  const f = fixture(t, { clean: true });
  const result = f.run();
  assert.equal(result.status, 0, result.stderr);
  assert.equal(f.calls().filter(x => x.args[0] === 'push').length, 0);
  assert.equal(f.runGit('rev-list', '--count', 'main'), '1');
});

test('missing deploy secret fails instead of falling back to an ordinary token', (t) => {
  const f = fixture(t);
  const result = f.run({ RELEASE_DEPLOY_KEY: '' });
  assert.notEqual(result.status, 0);
  assert.match(result.stderr, /RELEASE_DEPLOY_KEY/);
  assert.equal(f.calls().filter(x => x.args[0] === 'push').length, 0);
});

test('writeback refuses unrelated code changes', (t) => {
  const f = fixture(t);
  fs.appendFileSync(path.join(f.repo, 'app.js'), '// unrelated change\n');
  const result = f.run();
  assert.notEqual(result.status, 0);
  assert.match(result.stderr, /拒绝回写非版本文件/);
  assert.equal(f.calls().filter(x => x.args[0] === 'push').length, 0);
  assert.deepEqual(fs.readdirSync(f.temp), []);
});

test('only a main push can invoke automated writeback', (t) => {
  for (const overrides of [{ GITHUB_EVENT_NAME: 'pull_request' }, { GITHUB_REF: 'refs/heads/topic' }]) {
    const f = fixture(t);
    const result = f.run(overrides);
    assert.notEqual(result.status, 0);
    assert.match(result.stderr, /仅允许 main/);
    assert.equal(f.calls().filter(x => x.args[0] === 'push').length, 0);
  }
});
