(function (root) {
  "use strict";
  function summary(result) {
    const items = Array.isArray(result?.items) ? result.items : [];
    const count = status => items.filter(m => m.status === status).length;
    const succeeded = count("succeeded"), failed = count("failed");
    const unknown = items.length - succeeded - failed;
    const ok = items.length > 0 && succeeded === items.length;
    const text = !items.length ? "操作结果未确认，请刷新核对" : ok ? `已完成 ${succeeded} 封` :
      [`成功 ${succeeded} 封`, failed ? `失败 ${failed} 封` : "", unknown ? `待确认 ${unknown} 封，请刷新核对，勿重复操作` : ""].filter(Boolean).join(" · ");
    return { ok, succeeded, failed, unknown, text };
  }
  function partitionTrash(items, role) {
    return { permanent: items.filter(m => role(m.accountId, m.folder) === "trash"), normal: items.filter(m => role(m.accountId, m.folder) !== "trash") };
  }
  const retryItems = result => (result?.items || []).filter(m => m.status === "failed").map(m => m.source);
  function completeResult(result, sources) {
    const id = m => JSON.stringify([m?.accountId,m?.receiveEpoch,m?.folder,m?.uidValidity,m?.uid]);
    return { ...result, items:sources.map(source => {
      const matches=(result.items || []).filter(item=>id(item.source)===id(source));
      return matches.length===1 ? matches[0] : {source,status:"unknown",message:"未收到完整操作结果，请刷新核对"};
    }) };
  }
  const api = { summary, partitionTrash, retryItems, completeResult };
  if (typeof module === "object" && module.exports) module.exports = api;
  else root.MailBeaconOperations = api;
})(typeof window !== "undefined" ? window : globalThis);
