const test = require('node:test');
const assert = require('node:assert/strict');
const fs = require('node:fs');
const vm = require('node:vm');

function backend(search = '') {
  const context = vm.createContext({
    window: {}, location: { search }, document: {},
    setTimeout: fn => { fn(); }, structuredClone,
  });
  vm.runInContext(fs.readFileSync(require.resolve('../ui/mock.js'), 'utf8'), context);
  return context.window.__TAURI__.core.invoke;
}

const emptyFields = () => ({ subject: [], sender: [], recipient: [], body: [] });
const ruleDefaults = () => ({ keywordScope: 'any', fieldKeywords: emptyFields() });

test('browser preview creates a Chinese folder and moves a real message into it with a new UID', async () => {
  const invoke = backend();
  const before = await invoke('list_messages', { accountId: 'a1', folder: 'INBOX' });
  assert.ok(before.length > 0);
  const folder = await invoke('create_folder', { accountId: 'a1', name: '工作资料' });
  assert.ok((await invoke('list_folders', { accountId: 'a1' })).some(f => f.name === '工作资料'));
  await assert.rejects(invoke('create_folder', { accountId: 'a1', name: '工作资料' }));
  await invoke('move_messages', { accountId: 'a1', folder: 'INBOX', uids: [before[0].uid], role: 'other', target: folder.path });
  const source = await invoke('list_messages', { accountId: 'a1', folder: 'INBOX' });
  const destination = await invoke('list_messages', { accountId: 'a1', folder: folder.path });
  assert.equal(source.length, before.length - 1);
  assert.equal(destination.length, 1);
  assert.notEqual(destination[0].uid, before[0].uid);
  assert.equal(destination[0].subject, before[0].subject);
});

test('browser preview retains saved and cleared signatures and exports selected mail', async () => {
  const invoke = backend();
  const saved = await invoke('save_signature', { accountId: 'a1', signature: '李嘉文\n产品部' });
  assert.equal(saved.signature, '李嘉文\n产品部');
  assert.equal((await invoke('snapshot')).accounts[0].signature, saved.signature);
  assert.equal((await invoke('save_signature', { accountId: 'a1', signature: '' })).signature, '');
  const messages = await invoke('list_messages', { accountId: 'a1', folder: 'INBOX' });
  const view = await invoke('get_message', { accountId: 'a1', folder: 'INBOX', uid: messages[0].uid });
  assert.equal(view.uidValidity, 20261019);
  assert.match(await invoke('save_message', { accountId: 'a1', folder: 'INBOX', uid: messages[0].uid }), /\.eml$/);
});

test('browser preview names exported mail from its subject and numbers repeated saves without overwriting', async () => {
  const invoke = backend();
  const messages = await invoke('list_messages', { accountId: 'a1', folder: 'INBOX' });
  const source = messages.find(message => message.subject === 'Ness Labs: Outsourcing Discovery 🍎');
  assert.ok(source);
  const args = { accountId: 'a1', folder: 'INBOX', uid: source.uid };
  assert.equal(await invoke('save_message', args), '/Users/me/Downloads/Ness Labs_ Outsourcing Discovery 🍎.eml');
  assert.equal(await invoke('save_message', args), '/Users/me/Downloads/Ness Labs_ Outsourcing Discovery 🍎 (2).eml');
});

test('browser preview stores automatic downloads per account and reports their runtime status', async () => {
  const invoke = backend();
  const initial = await invoke('snapshot');
  assert.deepEqual(initial.accounts[0].autoDownload, { enabled: false, directory: '', filterMode: 'none', keywords: [], ...ruleDefaults() });
  await invoke('save_auto_download', { accountId: 'a1', enabled: true, directory: '/Users/me/邮件归档' });
  const saved = await invoke('snapshot');
  assert.equal(saved.accounts[0].autoDownload.directory, '/Users/me/邮件归档');
  assert.equal(saved.accounts[1].autoDownload.enabled, false);
  assert.equal(saved.autoDownloads.a1.state, 'waiting');
  await invoke('save_auto_download', { accountId: 'a1', enabled: false, directory: '' });
  assert.equal((await invoke('snapshot')).autoDownloads.a1.state, 'disabled');
});

test('browser preview validates active directory without preventing disabling and supports picker cancellation', async () => {
  const invoke = backend();
  await assert.rejects(invoke('save_auto_download', { accountId: 'a1', enabled: true, directory: 'relative/path' }));
  assert.equal((await invoke('snapshot')).accounts[0].autoDownload.enabled, false);
  await invoke('save_auto_download', { accountId: 'a1', enabled: false, directory: '' });
  assert.equal(await backend('?directory-cancel')('choose_download_directory'), null);
  await assert.rejects(backend('?directory-error')('choose_download_directory'));
});

test('browser preview save failure retains the original disabled configuration', async () => {
  const invoke = backend('?auto-save-error');
  await assert.rejects(invoke('save_auto_download', { accountId: 'a1', enabled: true, directory: '/Users/me/Mail' }));
  assert.equal((await invoke('snapshot')).accounts[0].autoDownload.enabled, false);
});


test('browser preview normalizes exclusive filter modes, keeps none drafts, and isolates accounts', async () => {
  const invoke = backend();
  const saved = await invoke('save_auto_download', { accountId: 'a1', enabled: true, directory: '/Users/me/Mail', filterMode: 'whitelist', keywords: [' GitHub ', '', 'github', '优惠'] });
  assert.deepEqual(saved.autoDownload, { enabled: true, directory: '/Users/me/Mail', filterMode: 'whitelist', keywords: ['GitHub', '优惠'], ...ruleDefaults() });
  const inactive = await invoke('save_auto_download', { accountId: 'a1', enabled: false, directory: '', filterMode: 'none', keywords: ['优惠'] });
  assert.deepEqual(inactive.autoDownload.keywords, ['优惠']);
  assert.equal(inactive.autoDownload.filterMode, 'none');
  assert.deepEqual((await invoke('snapshot')).accounts[1].autoDownload, { enabled: false, directory: '', filterMode: 'none', keywords: [], ...ruleDefaults() });
});

test('browser preview rejects invalid or empty active rules and length limits before changing settings', async () => {
  const invoke = backend();
  for (const [filterMode, keywords] of [['blacklist', []], ['whitelist', [' ']], ['both', ['x']], ['none', ['x'.repeat(201)]], ['blacklist', Array.from({ length: 101 }, (_, i) => String(i))]]) {
    await assert.rejects(invoke('save_auto_download', { accountId: 'a1', enabled: false, directory: '', filterMode, keywords }));
  }
  assert.deepEqual((await invoke('snapshot')).accounts[0].autoDownload, { enabled: false, directory: '', filterMode: 'none', keywords: [], ...ruleDefaults() });
});

test('browser preview preserves both keyword drafts and accepts only nonempty active field rules', async () => {
  const invoke = backend();
  const fields = { ...emptyFields(), subject: ['  项目  ', '项目'], sender: ['GitHub', 'github'] };
  const saved = await invoke('save_auto_download', { accountId: 'a1', enabled: false, directory: '', filterMode: 'blacklist', keywordScope: 'fields', keywords: ['unused'], fieldKeywords: fields });
  assert.equal(saved.autoDownload.keywordScope, 'fields');
  assert.deepEqual(saved.autoDownload.fieldKeywords, { ...emptyFields(), subject: ['项目'], sender: ['GitHub'] });
  const switched = await invoke('save_auto_download', { accountId: 'a1', enabled: false, directory: '', keywordScope: 'any' });
  assert.deepEqual(switched.autoDownload.keywords, ['unused']);
  assert.deepEqual(switched.autoDownload.fieldKeywords.sender, ['GitHub']);
  for (const args of [
    { keywordScope: 'fields', fieldKeywords: emptyFields() },
    { keywordScope: 'invalid' },
    { keywordScope: 'fields', fieldKeywords: { ...emptyFields(), body: ['😀'.repeat(201)] } },
    { keywordScope: 'fields', fieldKeywords: { ...emptyFields(), subject: Array.from({ length: 60 }, (_, i) => `s${i}`), body: Array.from({ length: 41 }, (_, i) => `b${i}`) } },
  ]) await assert.rejects(invoke('save_auto_download', { accountId: 'a1', enabled: false, directory: '', ...args }));
  const unchanged = (await invoke('snapshot')).accounts[0].autoDownload;
  assert.equal(unchanged.keywordScope, 'any');
  assert.deepEqual(unchanged.fieldKeywords.sender, ['GitHub']);
});

test('browser preview matches only each configured field and ORs independent field rules', async () => {
  for (const [fields, expected] of [
    [{ subject: ['annelaure@nesslabs.com'] }, 0],
    [{ sender: ['annelaure@nesslabs.com'] }, 1],
    [{ subject: ['never-matches'], sender: ['annelaure@nesslabs.com'] }, 1],
    [{ recipient: ['jarvis@gmail.com'] }, 8],
    [{ recipient: ['我'] }, 0],
    [{ body: ['annelaure@nesslabs.com'] }, 0],
    [{ body: ['what do we lose'] }, 1],
    [{ body: ['详细内容见附件'] }, 8],
    [{ body: ['neuroscientist'] }, 1],
  ]) {
    const { invoke, flush } = scheduledBackend();
    await invoke('save_auto_download', { accountId: 'a1', enabled: false, directory: '', filterMode: 'whitelist', keywordScope: 'fields', keywords: [], fieldKeywords: { ...emptyFields(), ...fields } });
    await invoke('start_bulk_download', { accountId: 'a1', directory: '/tmp/fields', inheritFilters: true });
    flush();
    const status = (await invoke('snapshot')).bulkDownloads.a1;
    assert.equal(status.savedCount, expected, JSON.stringify(fields));
    assert.equal(status.keywordScope, 'fields');
  }
});

test('browser preview snapshots field rules and unfiltered download bypasses both scopes', async () => {
  const { invoke, flush } = scheduledBackend();
  await invoke('save_auto_download', { accountId: 'a1', enabled: false, directory: '', filterMode: 'blacklist', keywordScope: 'fields', fieldKeywords: { ...emptyFields(), sender: ['GitHub'] } });
  const started = await invoke('start_bulk_download', { accountId: 'a1', directory: '/tmp/fields', inheritFilters: true });
  assert.equal(started.keywordScope, 'fields');
  await invoke('save_auto_download', { accountId: 'a1', enabled: false, directory: '', keywordScope: 'any', keywords: ['never-matches'] });
  flush();
  const filtered = (await invoke('snapshot')).bulkDownloads.a1;
  assert.equal(filtered.keywordScope, 'fields');
  assert.equal(filtered.skippedCount, 1);
  await invoke('start_bulk_download', { accountId: 'a1', directory: '/tmp/fields', inheritFilters: false });
  flush();
  const unfiltered = (await invoke('snapshot')).bulkDownloads.a1;
  assert.equal(unfiltered.keywordScope, 'any');
  assert.equal(unfiltered.skippedCount, 0);
  assert.equal(unfiltered.savedCount, 1);
});

function scheduledBackend(search = '') {
  const timers = [];
  const context = vm.createContext({ window: {}, location: { search }, document: {}, structuredClone,
    setTimeout: (fn, delay) => { if (delay <= 30) fn(); else timers.push(fn); },
  });
  vm.runInContext(fs.readFileSync(require.resolve('../ui/mock.js'), 'utf8'), context);
  return { invoke: context.window.__TAURI__.core.invoke, listen: context.window.__TAURI__.event.listen,
    tick: () => timers.shift()?.(), flush: () => { let count = 0; while (timers.length && count++ < 100) timers.shift()(); assert.ok(count < 100); } };
}

test('browser preview full-account download ignores filters, publishes progress and resumes without duplicate files', async () => {
  const { invoke, listen, tick, flush } = scheduledBackend();
  await invoke('save_auto_download', { accountId: 'a1', enabled: true, directory: '/tmp/auto', filterMode: 'whitelist', keywords: ['not-in-any-message'] });
  const before = await invoke('snapshot'), events = [];
  await listen('mail', ev => events.push(ev.payload));
  const started = await invoke('start_bulk_download', { accountId: 'a1', directory: '/tmp/full' });
  assert.equal(started.state, 'preparing');
  await assert.rejects(invoke('start_bulk_download', { accountId: 'a1', directory: '/tmp/full' }));
  tick();
  assert.equal((await invoke('snapshot')).bulkDownloads.a1.state, 'scanning');
  flush();
  const after = await invoke('snapshot');
  assert.equal(after.bulkDownloads.a1.state, 'complete');
  assert.equal(after.bulkDownloads.a1.savedCount, before.live.a1.inbox.length);
  assert.equal(after.bulkDownloads.a1.folderCount, (await invoke('list_folders', { accountId: 'a1' })).length);
  assert.equal(after.bulkDownloads.a1.foldersDone, after.bulkDownloads.a1.folderCount);
  assert.equal(after.bulkDownloads.a2.state, 'idle');
  assert.equal(after.live.a1.unread, before.live.a1.unread);
  assert.deepEqual(after.accounts, before.accounts);
  assert.ok(events.some(e => e.type === 'bulkDownload' && e.state === 'complete'));
  await invoke('start_bulk_download', { accountId: 'a1', directory: '/tmp/full' });
  flush();
  const repeated = (await invoke('snapshot')).bulkDownloads.a1;
  assert.equal(repeated.savedCount, 0);
  assert.equal(repeated.existingCount, before.live.a1.inbox.length);
});

test('browser preview cancellation retains progress and isolates concurrently running accounts', async () => {
  const { invoke, tick, flush } = scheduledBackend('?bulk-slow');
  await invoke('start_bulk_download', { accountId: 'a1', directory: '/tmp/full' });
  await invoke('start_bulk_download', { accountId: 'a2', directory: '/tmp/full' });
  for (let step = 0; step < 20 && !(await invoke('snapshot')).bulkDownloads.a1.savedCount; step++) tick();
  assert.equal((await invoke('cancel_bulk_download', { accountId: 'a1' })).state, 'cancelling');
  flush();
  const state = (await invoke('snapshot')).bulkDownloads;
  assert.equal(state.a1.state, 'cancelled');
  assert.ok(state.a1.savedCount > 0);
  assert.equal(state.a2.state, 'complete');
  assert.equal((await invoke('cancel_bulk_download', { accountId: 'a2' })).state, 'complete');
});

test('browser preview full downloads expose startup and runtime errors', async () => {
  const failing = scheduledBackend('?bulk-error');
  await assert.rejects(failing.invoke('start_bulk_download', { accountId: 'a1', directory: '/tmp/full' }));
  assert.equal((await failing.invoke('snapshot')).bulkDownloads.a1.state, 'idle');
  const running = scheduledBackend('?bulk-runtime-error');
  await running.invoke('start_bulk_download', { accountId: 'a1', directory: '/tmp/full' });
  running.flush();
  const failed = (await running.invoke('snapshot')).bulkDownloads.a1;
  assert.equal(failed.state, 'error');
  assert.ok(failed.failedCount > 0);
});

test('browser preview optionally inherits a saved blacklist even when automatic download is off and snapshots it', async () => {
  const { invoke, flush } = scheduledBackend();
  await invoke('save_auto_download', { accountId: 'a1', enabled: false, directory: '', filterMode: 'blacklist', keywords: ['gItHuB'] });
  const before = await invoke('snapshot');
  const started = await invoke('start_bulk_download', { accountId: 'a1', directory: '/tmp/filtered', inheritFilters: true });
  assert.equal(started.inheritFilters, true);
  assert.equal(started.filterMode, 'blacklist');
  await invoke('save_auto_download', { accountId: 'a1', enabled: false, directory: '', filterMode: 'whitelist', keywords: ['never-matches'] });
  flush();
  const after = await invoke('snapshot'), downloaded = after.bulkDownloads.a1;
  assert.equal(downloaded.filterMode, 'blacklist');
  assert.equal(downloaded.skippedCount, 1);
  assert.equal(downloaded.savedCount, before.live.a1.inbox.length - 1);
  assert.equal(after.live.a1.unread, before.live.a1.unread);
  await invoke('start_bulk_download', { accountId: 'a1', directory: '/tmp/filtered', inheritFilters: false });
  flush();
  const resumed = (await invoke('snapshot')).bulkDownloads.a1;
  assert.equal(resumed.filterMode, 'none');
  assert.equal(resumed.savedCount, 1);
  assert.equal(resumed.existingCount, before.live.a1.inbox.length - 1);
  assert.equal(resumed.skippedCount, 0);
});

test('browser preview whitelist counts skips before existing files and no-rule inheritance exports everything', async () => {
  const { invoke, flush } = scheduledBackend();
  const before = await invoke('snapshot');
  await invoke('start_bulk_download', { accountId: 'a1', directory: '/tmp/full', inheritFilters: true });
  flush();
  assert.equal((await invoke('snapshot')).bulkDownloads.a1.savedCount, before.live.a1.inbox.length);
  await invoke('save_auto_download', { accountId: 'a1', enabled: false, directory: '', filterMode: 'whitelist', keywords: ['gItHuB'] });
  await invoke('start_bulk_download', { accountId: 'a1', directory: '/tmp/full', inheritFilters: true });
  flush();
  const after = await invoke('snapshot'), state = after.bulkDownloads.a1;
  assert.equal(state.inheritFilters, true);
  assert.equal(state.filterMode, 'whitelist');
  assert.equal(state.savedCount, 0);
  assert.equal(state.existingCount, 1);
  assert.equal(state.skippedCount, before.live.a1.inbox.length - 1);
  assert.match(state.detail, /符合规则/);
  assert.equal(after.bulkDownloads.a2.skippedCount, 0);
});

test('browser preview scans a stable total before saving and processed totals include filtered and vanished mail', async () => {
  const { invoke, listen, tick, flush } = scheduledBackend(), events = [];
  await listen('mail', ev => events.push(ev.payload));
  await invoke('save_auto_download', { accountId: 'a1', enabled: false, directory: '', filterMode: 'whitelist', keywords: ['GitHub'] });
  const before = await invoke('list_messages', { accountId: 'a1', folder: 'INBOX' });
  await invoke('start_bulk_download', { accountId: 'a1', directory: '/tmp/totals', inheritFilters: true });
  tick();
  const scanned = (await invoke('snapshot')).bulkDownloads.a1;
  assert.equal(scanned.state, 'scanning');
  assert.equal(scanned.totalCount, null);
  assert.equal(scanned.foldersScanned, 1);
  await invoke('move_messages', { accountId: 'a1', folder: 'INBOX', uids: [before[0].uid], role: 'trash' });
  flush();
  const final = (await invoke('snapshot')).bulkDownloads.a1;
  // The move created a new message in a folder not scanned yet; both identities are in the scan.
  assert.equal(final.totalCount, before.length + 1);
  assert.equal(final.processedCount, final.totalCount);
  assert.equal(final.vanishedCount, 1);
  assert.equal(final.savedCount, 1);
  assert.equal(final.skippedCount, before.length - 1);
  assert.ok(events.filter(e => e.state === 'scanning').every(e => e.savedCount === 0 && e.processedCount === 0));
  assert.ok(events.filter(e => e.state === 'downloading').every(e => e.totalCount === final.totalCount));
});
