(function (root) {
  'use strict';
  function input(account, fields) {
    const out = { expectedRevision: account.connectionRevision || 1 };
    for (const name of ['name', 'password', 'smtpHost', 'smtpPort', ...(account.receiveProtocol === 'pop3' ? ['pop3Host', 'pop3Port', 'pop3Tls'] : ['imapHost', 'imapPort'])]) {
      if (!(name in fields)) continue;
      out[name] = name.endsWith('Port') ? Number(fields[name]) : fields[name];
    }
    return out;
  }
  function capture(form) {
    return [...form.elements].filter(field => field.name).map(field => ({ name: field.name, value: field.value, checked: field.type === 'checkbox' ? field.checked : undefined }));
  }
  function restore(form, saved) {
    for (const field of form.elements) {
      const old = saved.find(item => item.name === field.name);
      if (!old) continue;
      field.value = old.value;
      if (old.checked !== undefined) field.checked = old.checked;
    }
  }
  function acceptEvent(account, event) {
    if (!account) return true;
    return (!event.connectionRevision || event.connectionRevision === (account.connectionRevision || 1))
      && (!event.receiveEpoch || event.receiveEpoch === (account.receiveEpoch || 1));
  }
  function testSummary(result) {
    const label = part => part?.ok ? '正常' : part?.detail || '未完成';
    return `收信：${label(result.receive)}；发信：${label(result.send)}`;
  }
  const api = { input, capture, restore, acceptEvent, testSummary };
  if (typeof module === 'object' && module.exports) module.exports = api;
  else root.MailBeaconAccountRepair = api;
})(typeof window !== 'undefined' ? window : globalThis);
