(function(root) {
  'use strict';
  function request(s,cursor=null) {
    const scope=s.searchScope||'folder';
    const currentView=scope==='folder';
    const f=s.searchFilters||{};
    return {scope,accountId:s.acct==='all'?null:s.acct,folder:s.folder,includeExcluded:!!s.searchIncludeExcluded,query:s.query.trim(),unreadOnly:!!f.unreadOnly || (currentView && s.folder==='__unread'),flaggedOnly:!!f.flaggedOnly || (currentView && s.folder==='__starred'),attachmentOnly:!!f.attachmentOnly,sender:f.sender||'',after:f.after?new Date(f.after+'T00:00:00').getTime():null,before:f.before?new Date(f.before+'T00:00:00').getTime()+86400000:null,cursor};
  }
  function source(m,v={}) {
    if(!m) return null;
    const receiveEpoch=Number(m.receiveEpoch??v.receiveEpoch??1), uidValidity=Number(m.uidValidity||v.uidValidity);
    if(!m.accountId||!m.folder||!Number.isInteger(m.uid)||m.uid<=0||!Number.isInteger(receiveEpoch)||receiveEpoch<=0||!Number.isInteger(uidValidity)||uidValidity<=0) return null;
    return {accountId:m.accountId,receiveEpoch,folder:m.folder,uidValidity,uid:m.uid};
  }
  function coverageText(coverage=[],catalogIncomplete=[]) {
    const sum=k=>coverage.reduce((n,c)=>n+(Number(c[k])||0),0);
    const known=coverage.length>0&&coverage.every(c=>c.total!==null&&c.total!==undefined);
    const indexed=sum('indexed'),total=known?sum('total'):null,full=sum('full'),prefix=sum('prefix'),skipped=sum('skipped');
    const counts=`摘要 ${indexed}/${total??'未知'} · 完整正文 ${full} · 正文开头 ${prefix}${skipped?` · 暂时跳过 ${skipped}`:''}`;
    const gap=total===null||indexed<total||full<indexed||catalogIncomplete.length>0;
    const errors=coverage.filter(c=>c.error).map(c=>c.error);
    return [counts,gap?'仍有未覆盖内容，未匹配不代表邮箱中不存在':'已覆盖当前已同步邮件正文','不搜索附件内容',...catalogIncomplete,...errors].join('；');
  }
  const api={request,source,coverageText};
  if(typeof module!=='undefined')module.exports=api;
  root.MailBeaconSearch=api;
})(typeof window!=='undefined'?window:globalThis);
