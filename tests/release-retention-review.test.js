const test = require('node:test');
const assert = require('node:assert/strict');
const path = require('node:path');
const { spawnSync } = require('node:child_process');
const R = require('../scripts/release-retention.js');

function service({ latestId = 8, before = () => {} } = {}) {
  const rows = Array.from({ length: 8 }, (_, i) => {
    const id = i + 1, version = `1.${id}.0`;
    return { id, tag_name: `v${version}`, draft: false, prerelease: false,
      published_at: '2026-10-23T00:00:00Z', updated_at: '2026-10-23T00:00:00Z',
      assets: ['universal.dmg', 'x64-setup.exe'].map(suffix => ({
        name: `MailBeacon_${version}_${suffix}`, state: 'uploaded', size: 20,
      })),
    };
  });
  const state = { rows, latestId, latestReads: 0, calls: [] };
  const api = async (method, endpoint) => {
    state.calls.push([method, endpoint]);
    if (endpoint.endsWith('/latest')) state.latestReads++;
    before({ method, endpoint, ...state });
    if (method === 'DELETE') {
      state.rows.splice(state.rows.findIndex(r => endpoint.endsWith('/' + r.id)), 1);
      return null;
    }
    if (endpoint.includes('?per_page=')) return structuredClone(state.rows);
    if (endpoint.endsWith('/latest')) return structuredClone(state.rows.find(r => r.id === state.latestId));
    return structuredClone(state.rows.find(r => endpoint.endsWith('/' + r.id)));
  };
  return { state,
    run: options => R.runCleanup({ repository: 'owner/mailbeacon', currentTag: 'v1.8.0', keep: 5, apply: true, api, ...options }),
    deletes: () => state.calls.filter(([method]) => method === 'DELETE'),
  };
}

test('review: latest changing to a deletion candidate at the final check prevents all deletion', async () => {
  const f = service({ before({ endpoint, latestReads }) {
    if (endpoint.endsWith('/latest') && latestReads === 3) f.state.latestId = 1;
  } });
  await assert.rejects(f.run(), /latest/);
  assert.deepEqual(f.deletes(), []);
});

test('review: a changed candidate timestamp or tag prevents deletion', async () => {
  for (const change of [r => r.updated_at = '2026-10-23T00:00:01Z', r => r.tag_name = 'v1.1.1']) {
    const f = service({ before({ method, endpoint, rows }) {
      if (method === 'GET' && endpoint.endsWith('/releases/1')) change(rows[0]);
    } });
    await assert.rejects(f.run(), /变化/);
    assert.deepEqual(f.deletes(), []);
  }
});

test('review: current installer becoming unavailable before the final check prevents deletion', async () => {
  for (const latestId of [8, 7]) {
    const f = service({ latestId, before({ method, endpoint, rows }) {
      if (method === 'GET' && endpoint.endsWith('/releases/1')) rows.find(r => r.id === 8).assets.pop();
    } });
    await assert.rejects(f.run(), /安装包/);
    assert.deepEqual(f.deletes(), []);
  }
});

test('review: CLI rejects unexpected commands and extra arguments', () => {
  for (const args of [['delete'], ['apply', '--force'], ['plan', 'apply']]) {
    const result = spawnSync(process.execPath, [path.join(__dirname, '../scripts/release-retention.js'), ...args], {
      encoding: 'utf8', env: { ...process.env, GITHUB_REPOSITORY: 'invalid', RELEASE_VERSION: '1.8.0' },
    });
    assert.equal(result.status, 1);
    assert.match(result.stderr, /用法/);
  }
});

test('review: successful deletions are reported before a later deletion fails', async () => {
  const reported = [];
  const f = service({ before({ method, endpoint }) {
    if (method === 'DELETE' && endpoint.endsWith('/releases/2')) {
      assert.deepEqual(reported, ['v1.1.0']);
      throw Error('second deletion failed');
    }
  } });
  await assert.rejects(f.run({ onDelete: async release => {
    await Promise.resolve();
    reported.push(release.tag_name);
  } }), /second deletion failed/);
  assert.deepEqual(reported, ['v1.1.0']);
});
