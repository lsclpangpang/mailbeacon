(function(root) {
  'use strict';
  function active(s) {
    if (s.activeSavedView || String(s.query || '').trim()) return true;
    const f=s.searchFilters || {};
    return !!s.searching && !!(f.unreadOnly || f.flaggedOnly || f.attachmentOnly || String(f.sender || '').trim() || f.after || f.before || f.relativeDays);
  }
  function dateBoundary(value, next=false) {
    if (!value) return null;
    const d=new Date(value+'T00:00:00');
    if (!Number.isFinite(d.getTime())) throw new Error('日期格式无效');
    if (next) d.setDate(d.getDate()+1);
    return d.getTime();
  }
  function request(s,cursor=null,now=new Date()) {
    const scope=s.searchScope||'folder';
    const currentView=scope==='folder';
    const f=s.searchFilters||{};
    let after=dateBoundary(f.after), before=dateBoundary(f.before,true);
    if (f.relativeDays) {
      const days=Number(f.relativeDays);
      if (!Number.isInteger(days) || days<1 || days>36500) throw new Error('最近天数必须为1至36500');
      const start=new Date(now.getFullYear(),now.getMonth(),now.getDate());
      const end=new Date(start); end.setDate(end.getDate()+1);
      start.setDate(start.getDate()-days+1);
      after=start.getTime(); before=end.getTime();
    }
    return {scope,accountId:s.acct==='all'?null:s.acct,folder:s.folder,includeExcluded:!!s.searchIncludeExcluded,query:String(s.query||'').trim(),unreadOnly:!!f.unreadOnly || (currentView && s.folder==='__unread'),flaggedOnly:!!f.flaggedOnly || (currentView && s.folder==='__starred'),attachmentOnly:!!f.attachmentOnly,sender:f.sender||'',after,before,cursor};
  }
  function editSpec(saved,s) {
    const spec=JSON.parse(JSON.stringify(saved)), f=s.searchFilters||{};
    Object.assign(spec,{query:String(s.query||'').trim(),unreadOnly:!!f.unreadOnly,flaggedOnly:!!f.flaggedOnly,attachmentOnly:!!f.attachmentOnly,sender:String(f.sender||'').trim(),after:dateBoundary(f.after),before:dateBoundary(f.before,true),relativeDays:f.relativeDays?Number(f.relativeDays):null,includeExcluded:!!s.searchIncludeExcluded});
    if (spec.relativeDays) { spec.after=null; spec.before=null; }
    return spec;
  }
  function captureSpec(s,accounts,folders) {
    const scope=s.searchScope||'folder', targets=[];
    if (scope!=='all') {
      const selected=s.acct==='all'?accounts:accounts.filter(a=>a.id===s.acct);
      if (!selected.length) throw new Error('请先选择有效账户');
      for (const a of selected) {
        if (a.receiveProtocol==='pop3') throw new Error('智能文件夹暂不支持POP3账户');
        const catalog=folders[a.id];
        if (!catalog) throw new Error('请先加载账户文件夹，再保存范围');
        let paths;
        if (scope==='account') paths=catalog.filter(f=>!String(f.path).startsWith('__')&&(s.searchIncludeExcluded||!['trash','junk'].includes(f.role))).map(f=>f.path);
        else if (s.folder==='__unread'||s.folder==='__starred') paths=['INBOX'];
        else if (s.folder==='__sent'||s.folder==='__drafts') paths=catalog.filter(f=>f.role===(s.folder==='__sent'?'sent':'drafts')).map(f=>f.path);
        else if (String(s.folder).startsWith('__')) throw new Error('这个视图不能保存为邮件搜索范围');
        else paths=[s.folder];
        if (!paths.length || paths.some(path=>!catalog.some(f=>f.path===path))) throw new Error('文件夹范围不可用，请重新选择');
        for (const folder of paths) targets.push({accountId:a.id,receiveEpoch:a.receiveEpoch||1,folder});
      }
    }
    const spec=editSpec({specVersion:1,scope:scope==='all'?'all':'fixed',targets,sort:'dateDesc'},s);
    if (scope==='folder'&&s.folder==='__unread') spec.unreadOnly=true;
    if (scope==='folder'&&s.folder==='__starred') spec.flaggedOnly=true;
    return spec;
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
  const api={active,request,captureSpec,editSpec,source,coverageText};
  if(typeof module!=='undefined')module.exports=api;
  root.MailBeaconSearch=api;
})(typeof window!=='undefined'?window:globalThis);
