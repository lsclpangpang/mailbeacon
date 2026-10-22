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

test('Windows installer preserves the standard directory picker and verifies custom paths through updates', () => {
  const base = JSON.parse(read('src-tauri/tauri.conf.json'));
  const win = JSON.parse(read('src-tauri/tauri.windows.conf.json'));
  for (const config of [base, win]) {
    const nsis = config.bundle?.windows?.nsis || {};
    assert.equal(nsis.template, undefined, 'keep the standard NSIS directory page');
    assert.equal(nsis.installerHooks, undefined, 'do not skip or force the directory page');
  }
  const source = read('scripts/windows-smoke.ps1');
  assert.match(source, /自选目录 MailBeacon /, 'exercise spaces and non-ASCII characters in the chosen directory');
  assert.match(source, /Assert-InstallLocation/, 'verify registry, protocol and shortcut point to the chosen executable');
  assert.match(source, /-ArgumentList '\/S \/UPDATE'/, 'update without a directory override must retain the chosen location');
  assert.match(source, /upgradeRetainedLocation = \$true/);
  assert.match(source, /GetPathRoot/);
  assert.match(source, /ReadShortcutTarget/);
  assert.match(source, /shell\\open\\command/);
});

test('shortcut smoke verifies Windows file identity and records differing path diagnostics', () => {
  const source = read('scripts/windows-smoke.ps1');
  assert.match(source, /shortcutChecks/);
  assert.match(source, /actualTarget = \$actualTarget/);
  assert.match(source, /expectedTarget = \$ExpectedBinary/);
  assert.match(source, /\[MailBeaconFileIdentity\]::Compare\(/);
  assert.match(source, /if \(-not \$comparison\.SameFile\)/);
  assert.match(source, /Test-WindowsFileIdentity/);
  assert.match(source, /ConvertTo-Json -Depth 6/);
  const identity = read('scripts/windows-file-identity.ps1');
  assert.match(identity, /GetFileInformationByHandle/);
  assert.match(identity, /GetFinalPathNameByHandleW/);
  assert.match(identity, /VolumeSerialNumber ==/);
  assert.match(identity, /FileIndexHigh ==/);
  assert.match(identity, /FileIndexLow ==/);
  assert.match(identity, /-ItemType HardLink/);
  assert.match(identity, /Different file was accepted/);
  assert.match(identity, /Missing file was accepted/);
});

test('Windows shortcuts use the Unicode COM interface and exercise a real Unicode shortcut', () => {
  const source = read('scripts/windows-smoke.ps1');
  const identity = read('scripts/windows-file-identity.ps1');
  assert.doesNotMatch(source, /WScript\.Shell|CreateShortcut/);
  assert.match(source, /reader = 'IShellLinkW'/);
  assert.match(source, /\[MailBeaconFileIdentity\]::ReadShortcutTarget/);
  assert.match(identity, /Guid\("000214F9-0000-0000-C000-000000000046"\)/);
  assert.match(identity, /interface IShellLinkW/);
  assert.match(identity, /IPersistFile\)instance\)\.Load\(shortcutPath, 0\)/);
  assert.match(identity, /MarshalAs\(UnmanagedType\.LPWStr\)/);
  assert.match(identity, /CreateShortcut\(\$shortcut, \$expected\)/);
  assert.match(identity, /ReadShortcutTarget\(\$shortcut\)/);
  assert.match(identity, /Unicode shortcut target was corrupted/);
});

test('installer smoke verifies native UI without changing WebView security or debugging settings', () => {
  const source = read('scripts/windows-smoke.ps1');
  const ui = read('scripts/windows-ui-smoke.ps1');
  assert.match(source, /windows-ui-smoke\.ps1/);
  assert.match(source, /powershell\.exe/);
  assert.match(source, /MainWindowHandle/);
  assert.match(source, /WaitForExit\(120000\)/);
  assert.match(source, /\$uiProbe = Start-Process[^\n]*-WindowStyle Hidden/, 'probe console must not cover app screenshots');
  assert.match(source, /ui-smoke-result\.json/);
  assert.match(source, /uiReady = \$false/);
  assert.match(source, /\$result\.uiReady = \$true/);
  assert.doesNotMatch(source + ui, /WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS|remote-debugging|ExecutionPolicy|HKLM:|disable-web-security|no-sandbox/);
  assert.match(ui, /AutomationElement\]::FromHandle/);
  assert.match(ui, /InvokePattern/);
  assert.match(ui, /IsOffscreen/);
  assert.match(ui, /BoundingRectangle/);
  assert.match(ui, /IsEnabled/);
  assert.match(ui, /ControlType/);
  assert.match(ui, /Test-UiRuleBridge/);
  assert.match(ui, /WindowsPrincipal/);
  assert.ok([...ui].every(character => character.charCodeAt(0) < 128), 'Windows PowerShell 5.1 helper source must remain ASCII');
  assert.match(ui, /ReadAllText\(\$assessmentPath, \$utf8\)/);
  assert.match(ui, /WriteAllText\(\$Path, [\s\S]*\$utf8\)/);
  assert.doesNotMatch(ui, /RootElement/, 'scope accessibility reads to the given MailBeacon HWND');
});


test('UIA probe records native visibility and minimized state on its verified window handle', () => {
  const source = read('scripts/windows-ui-smoke.ps1');
  assert.match(source, /DllImport\("user32\.dll"\)/);
  assert.match(source, /MarshalAs\(UnmanagedType\.Bool\)/);
  assert.match(source, /extern bool IsWindowVisible\(IntPtr hWnd\)/);
  assert.match(source, /extern bool IsIconic\(IntPtr hWnd\)/);
  assert.match(source, /\$window\['nativeVisible'\] = \[MailBeaconWindowVisibility\]::IsWindowVisible\(\[IntPtr\]\$WindowHandle\)/);
  assert.match(source, /\$window\['minimized'\] = \[MailBeaconWindowVisibility\]::IsIconic\(\[IntPtr\]\$WindowHandle\)/);
});
