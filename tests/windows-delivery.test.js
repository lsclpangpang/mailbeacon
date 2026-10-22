const test = require('node:test');
const assert = require('node:assert/strict');
const fs = require('node:fs');
const path = require('node:path');
const root = path.resolve(__dirname, '..');
const read = file => fs.readFileSync(path.join(root, file), 'utf8');
const job = (source, id) => source.match(new RegExp(`^  ${id}:\\n([\\s\\S]*?)(?=^  [a-z][a-z_-]*:|$(?![\\s\\S]))`, 'm'))?.[1] || '';

test('Windows overrides the complete main window without creating a duplicate or exposing startup state', () => {
  const base = JSON.parse(read('src-tauri/tauri.conf.json'));
  const win = JSON.parse(read('src-tauri/tauri.windows.conf.json'));
  assert.equal(win.version, undefined, 'all platforms inherit the one version source');
  assert.equal(win.identifier, undefined, 'retain the stable app identity');
  assert.deepEqual(win.bundle.targets, ['nsis']);
  assert.ok(win.bundle.icon.includes('icons/icon.ico'));
  assert.equal(win.app.windows.length, 1);
  const main = win.app.windows[0];
  for (const field of ['label', 'create', 'visible', 'width', 'height', 'minWidth', 'minHeight']) assert.equal(main[field], base.app.windows[0][field], field);
  assert.equal(main.decorations, true);
  assert.notEqual(main.titleBarStyle, 'Overlay');
  assert.equal(main.trafficLightPosition, undefined);
  assert.equal(win.bundle.windows.nsis.installMode, 'currentUser');
  assert.equal(win.bundle.windows.webviewInstallMode.type, 'downloadBootstrapper');
});

test('PR checks also cover stacked PRs, retain the protected macOS status and build a Windows installer', () => {
  const source = read('.github/workflows/ci.yml');
  const triggers = source.slice(source.indexOf('\non:'), source.indexOf('\npermissions:'));
  assert.match(triggers, /pull_request:/);
  assert.match(triggers, /workflow_dispatch:/);
  assert.doesNotMatch(triggers, /\bpush:|branches:/);
  assert.match(job(source, 'check'), /name: 测试与构建检查\n/);
  assert.match(job(source, 'check'), /runs-on: macos-latest/);
  const windows = job(source, 'windows');
  assert.match(windows, /runs-on: windows-latest/);
  assert.match(windows, /npm test/);
  assert.match(windows, /cargo test -p mailbeacon-app --locked/);
  assert.match(windows, /tauri build --target x86_64-pc-windows-msvc --bundles nsis/);
  assert.match(windows, /actions\/upload-artifact@/);
  assert.match(windows, /if-no-files-found: error/);
  assert.doesNotMatch(source, /RELEASE_DEPLOY_KEY|contents: write/);
});

test('release writes its version once and both platforms build and publish the same prepared commit', () => {
  const source = read('.github/workflows/release.yml');
  const prepare = job(source, 'prepare');
  assert.match(prepare, /environment: release-main/);
  assert.match(prepare, /github.ref == 'refs\/heads\/main'/);
  assert.match(prepare, /release-writeback\.sh/);
  assert.match(prepare, /sha: \$\{\{ steps\.prepared\.outputs\.sha \}\}/);
  assert.match(prepare, /version: \$\{\{ steps\.prepared\.outputs\.version \}\}/);
  assert.match(prepare, /git rev-parse HEAD/);
  assert.equal((source.match(/RELEASE_DEPLOY_KEY:/g) || []).length, 1);
  for (const name of ['macos', 'windows']) {
    const build = job(source, name);
    assert.match(build, /needs: prepare/);
    assert.match(build, /ref: \$\{\{ needs\.prepare\.outputs\.sha \}\}/);
    assert.match(build, /node scripts\/release\.js check/);
    assert.match(build, /node scripts\/release-artifacts\.js record/);
    assert.doesNotMatch(build, /RELEASE_DEPLOY_KEY|environment: release-main|git push/);
  }
  const publish = job(source, 'publish');
  assert.match(publish, /needs: \[prepare, macos, windows\]/);
  assert.match(publish, /ref: \$\{\{ needs\.prepare\.outputs\.sha \}\}/);
  assert.match(publish, /node scripts\/release-artifacts\.js verify/);
  assert.match(publish, /--target "\$TARGET"/);
  assert.match(publish, /--draft/);
  assert.match(publish, /tagSha.*TARGET/s, 'never replace release assets from a different source commit');
  assert.ok(publish.indexOf('tagSha=$(git rev-list') < publish.indexOf('if gh release view'), 'check an existing tag even when its release does not exist yet');
});

test('release notes describe both platform installers and the Windows runtime requirement', () => {
  const { notes } = require('../scripts/release.js');
  const body = notes({ changelog: '# 更新记录\n\n## 1.0.0\n\n- 新版', version: '1.0.0', repo: 'owner/repo' });
  assert.match(body, /macOS/);
  assert.match(body, /Windows.*x64/);
  assert.match(body, /setup\.exe/);
  assert.match(body, /WebView2/);
  assert.match(body, /未签名|没有.*签名/);
});

test('Windows installer smoke is isolated to a disposable runner and requires a real window', () => {
  const source = read('scripts/windows-smoke.ps1');
  assert.match(source, /GITHUB_ACTIONS/);
  assert.match(source, /RUNNER_TEMP/);
  assert.match(source, /\[guid\]::NewGuid\(\)/);
  assert.match(source, /Start-Process.*Installer/s);
  assert.match(source, /MainWindowHandle/);
  assert.match(source, /msedgewebview2/);
  assert.match(source, /smoke-result\.json/);
  assert.match(source, /finally/);
  assert.match(source, /uninstall\.exe/);
  const workflow = job(read('.github/workflows/ci.yml'), 'windows');
  assert.match(workflow, /shell: pwsh/);
  assert.match(workflow, /scripts\/windows-smoke\.ps1/);
  assert.match(workflow, /if: always\(\)/);
});
