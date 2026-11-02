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
  const highlightCss='mark[data-mailbeacon-search-hit]{background:#ffe47a!important;color:#211900!important;border-radius:2px;padding:0;font:inherit;box-decoration-break:clone;-webkit-box-decoration-break:clone}';
  const highlights=new WeakMap();
  const highlightMarkLimit=5000;
  const highlightSkippedTags=new Set(['SCRIPT','STYLE','HEAD','NOSCRIPT','TEXTAREA','INPUT','SELECT','OPTION','SVG','MATH','TEMPLATE']);
  const highlightBlockTags=new Set(['ADDRESS','ARTICLE','ASIDE','BLOCKQUOTE','BR','CAPTION','DD','DETAILS','DIALOG','DIV','DL','DT','FIELDSET','FIGCAPTION','FIGURE','FOOTER','FORM','H1','H2','H3','H4','H5','H6','HEADER','HGROUP','HR','LI','MAIN','NAV','OL','P','PRE','SECTION','SUMMARY','TABLE','TBODY','TD','TFOOT','TH','THEAD','TR','UL','IMG','IFRAME','CANVAS','VIDEO','AUDIO','OBJECT','EMBED']);
  const highlightBlockDisplays=new Set(['block','flow-root','list-item','flex','grid','table','table-row','table-cell','table-caption','inline-block','inline-flex','inline-grid','inline-table']);
  const escapeHtml=value=>String(value??'').replace(/[&<>"']/g,c=>({'&':'&amp;','<':'&lt;','>':'&gt;','"':'&quot;',"'":'&#39;'}[c]));
  function highlightPattern(query) {
    return new RegExp(query.replace(/[.*+?^${}()|[\]\\]/g, '\\$&'), 'giu');
  }
  function highlightText(value,query) {
    const text=String(value??''), keyword=String(query??'').trim();
    if(!keyword)return escapeHtml(text);
    let html='',offset=0;
    for(const match of text.matchAll(highlightPattern(keyword))) {
      html+=escapeHtml(text.slice(offset,match.index))+`<mark class="mail-search-hit" data-mailbeacon-search-hit>${escapeHtml(match[0])}</mark>`;
      offset=match.index+match[0].length;
    }
    return html+escapeHtml(text.slice(offset));
  }
  // Change only display text. Original email HTML, links and sender marks stay
  // intact; retaining the same query leaves selection and DOM nodes untouched.
  function highlight(root,query) {
    const doc=root?.ownerDocument;
    if(!doc?.createTextNode||!doc?.createElement||!doc?.createDocumentFragment||!root.childNodes)return 0;
    const keyword=String(query??'').trim(), previous=highlights.get(root);
    if(previous?.query===keyword)return previous.marks.length;
    const parents=new Set();
    for(const mark of previous?.marks||[]) {
      const parent=mark.parentNode;
      if(!parent)continue;
      parent.replaceChild(doc.createTextNode(mark.textContent),mark);
      parents.add(parent);
    }
    for(const parent of parents)parent.normalize();
    const marks=[];
    highlights.set(root,{query:keyword,marks});
    if(!keyword)return 0;
    const skipped=node=>highlightSkippedTags.has(node.tagName?.toUpperCase())
      ||node.hasAttribute?.('hidden')||node.getAttribute?.('aria-hidden')?.toLowerCase()==='true'
      ||node.style?.display?.toLowerCase()==='none'||['hidden','collapse'].includes(node.style?.visibility?.toLowerCase());
    for(let ancestor=root.parentElement;ancestor;ancestor=ancestor.parentElement) {
      if(skipped(ancestor))return 0;
    }
    const pattern=highlightPattern(keyword);
    let run=[],length=0;
    const flush=()=>{
      const remaining=highlightMarkLimit-marks.length;
      if(!run.length||!remaining)return;
      const parts=run, text=parts.map(part=>part.text).join('');
      run=[];length=0;
      const matches=[];
      for(const match of text.matchAll(pattern)) {
        matches.push({start:match.index,end:match.index+match[0].length});
        if(matches.length===remaining)break;
      }
      let matchIndex=0;
      for(const part of parts) {
        if(marks.length===highlightMarkLimit)break;
        while(matchIndex<matches.length&&matches[matchIndex].end<=part.start)matchIndex++;
        if(matchIndex===matches.length||matches[matchIndex].start>=part.end)continue;
        const fragment=doc.createDocumentFragment();
        let offset=0;
        for(let i=matchIndex;i<matches.length&&matches[i].start<part.end&&marks.length<highlightMarkLimit;i++) {
          const start=Math.max(part.start,matches[i].start)-part.start;
          const end=Math.min(part.end,matches[i].end)-part.start;
          if(start>offset)fragment.appendChild(doc.createTextNode(part.text.slice(offset,start)));
          const mark=doc.createElement('mark');
          mark.className='mail-search-hit';
          mark.setAttribute('data-mailbeacon-search-hit','');
          mark.textContent=part.text.slice(start,end);
          marks.push(mark);fragment.appendChild(mark);
          offset=end;
        }
        if(offset<part.text.length)fragment.appendChild(doc.createTextNode(part.text.slice(offset)));
        part.node.parentNode.replaceChild(fragment,part.node);
      }
    };
    // Word mail often splits one word across several spans. Match each visual
    // text run together, then mark only intersecting pieces in the original
    // text nodes. Blocks, line breaks and omitted content end the run.
    const stack=[{node:root}];
    while(stack.length&&marks.length<highlightMarkLimit) {
      const entry=stack.pop(),node=entry.node;
      if(entry.exit){flush();continue;}
      if(node.nodeType===3) {
        const text=node.textContent;
        if(text){run.push({node,text,start:length,end:length+text.length});length+=text.length;}
        continue;
      }
      if(node.nodeType!==1&&node.nodeType!==11)continue;
      if(node.nodeType===1&&skipped(node)){flush();continue;}
      const boundary=node.nodeType===1&&(highlightBlockTags.has(node.tagName?.toUpperCase())||highlightBlockDisplays.has(node.style?.display?.toLowerCase()));
      if(boundary){flush();if(marks.length===highlightMarkLimit)break;stack.push({exit:true});}
      for(let i=node.childNodes.length-1;i>=0;i--)stack.push({node:node.childNodes[i]});
    }
    flush();
    return marks.length;
  }
  const api={active,request,captureSpec,editSpec,source,coverageText,highlightText,highlight,highlightCss};
  if(typeof module!=='undefined')module.exports=api;
  root.MailBeaconSearch=api;
})(typeof window!=='undefined'?window:globalThis);
