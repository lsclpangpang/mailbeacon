(function(root){
  'use strict';
  const esc=s=>String(s??'').replace(/[&<>"']/g,c=>({'&':'&amp;','<':'&lt;','>':'&gt;','"':'&quot;',"'":'&#39;'}[c]));
  const attachmentIdentity=item=>item?JSON.stringify([item.source?.accountId,item.source?.receiveEpoch,item.source?.folder,item.source?.uidValidity,item.source?.uid,item.partId,item.rawHash,item.parserVersion]):null;
  const centerFocusTargets=new WeakMap();
  /** Every preview owns a cancellable identity, including requests still downloading. */
  function createPreview({invoke,onChange,randomId=()=>root.crypto.randomUUID(),platform=root.MailBeaconPlatform?.current||'macos'}) {
    let generation=0,requestId=null;
    const cancel=id=>id?invoke('cancel_attachment_preview',{requestId:id}).catch(()=>{}):Promise.resolve();
    return {
      async open(item,localOnly=false) {
        const version=++generation, previous=requestId, id=randomId();requestId=id;
        onChange({status:'loading',item,requestId:id});
        void cancel(previous);
        try {
          const externalPdf=platform!=='macos' && (item.mime==='application/pdf'||/\.pdf$/i.test(item.filename||''));
          const result=externalPdf ? {kind:'externalPdf'} : await invoke('preview_attachment',{requestId:id,item,localOnly});
          if(version!==generation){await cancel(id);return null;}
          onChange({status:'ready',item,requestId:id,result});return result;
        } catch(error) {
          if(version===generation)onChange({status:'error',item,requestId:id,error:String(error)});
          return null;
        }
      },
      async close(){++generation;const id=requestId;requestId=null;onChange({status:'closed'});await cancel(id);},
      nativeClosed(event){if(event.requestId===requestId){++generation;requestId=null;onChange(event.error?{status:'error',error:event.error}:{status:'closed'});}},
    };
  }
  let preview=null,layer=null,restoreFocus=null,resolveRestoreFocus=null,previewOptions=null,previewItems=[],previewIndex=0;
  const requestItem=item=>({source:item.source,partId:item.partId,rawHash:item.rawHash,parserVersion:item.parserVersion});
  function closePreview(){return preview?.close();}
  function previewActive(){return !!layer?.isConnected;}
  function showPreview(items,index,options,trigger) {
    if(!items[index])return;
    previewOptions=options;previewItems=items;previewIndex=index;
    if(!layer?.isConnected){
      restoreFocus=trigger||document.activeElement;
      const owner=centerFocusTargets.get(restoreFocus?.closest?.('.attachment-items'));
      const identity=attachmentIdentity(items[index]),action=restoreFocus?.dataset?.centerAction==='preview'?'preview':'select';
      resolveRestoreFocus=owner?()=>owner(identity,action):null;
      layer=document.createElement('div');layer.className='attachment-preview-layer';
      layer.innerHTML='<section class="attachment-preview-sheet" role="dialog" aria-modal="true" aria-labelledby="attachment-preview-title"><header><strong id="attachment-preview-title"></strong><button type="button" data-preview-action="close" class="icon-btn" aria-label="关闭预览">×</button></header><div class="attachment-preview-content" role="status"></div><footer><button type="button" class="secondary" data-preview-action="previous">上一份</button><button type="button" class="secondary" data-preview-action="next">下一份</button><span class="spacer"></span><button type="button" class="secondary" data-preview-action="source">返回原邮件</button><button type="button" class="secondary" data-preview-action="save">下载</button><button type="button" class="secondary" data-preview-action="open">默认应用打开</button></footer></section>';
      document.body.append(layer);
      layer.addEventListener('click',async event=>{
        const action=event.target.closest('[data-preview-action]')?.dataset.previewAction;if(!action)return;
        event.preventDefault();event.stopPropagation();
        const item=previewItems[previewIndex];
        if(action==='close')return closePreview();
        if(action==='previous'||action==='next')return showPreview(previewItems,previewIndex+(action==='next'?1:-1),previewOptions,restoreFocus);
        if(action==='source'){await closePreview();return previewOptions.onSource(item.source);}
        try {await previewOptions.onFileAction(action,item);}catch(error){previewOptions.onError?.(error);}
      });
      layer.querySelector('[data-preview-action="close"]').focus();
    }
    if(!preview)preview=createPreview({invoke:options.invoke,onChange:state=>{
      if(state.status==='closed'){
        layer?.remove();layer=null;
        const target=resolveRestoreFocus?resolveRestoreFocus():restoreFocus;
        resolveRestoreFocus=null;restoreFocus=null;
        if(target?.isConnected)target.focus({preventScroll:true});
        return;
      }
      if(!layer?.isConnected)return;
      const title=layer.querySelector('#attachment-preview-title'),content=layer.querySelector('.attachment-preview-content');
      title.textContent=state.item?.filename||previewItems[previewIndex]?.filename||'附件预览';
      if(state.status==='loading')content.textContent='正在读取附件…';
      else if(state.status==='error')content.textContent=state.error;
      else if(state.result.kind==='externalPdf')content.innerHTML='<div class="attachment-preview-fallback"><span class="attachment-type" aria-hidden="true">PDF</span><h4>用默认应用查看 PDF</h4><p>此版本暂不支持内嵌 PDF 预览。点击下方按钮，使用电脑上的 PDF 阅读器打开。</p><button type="button" class="primary" data-preview-action="open">默认应用打开</button></div>';
      else if(state.result.kind==='nativePdf')content.textContent='已在系统快速预览中打开完整 PDF。关闭系统预览可返回这里。';
      else if(/^data:image\/(png|jpeg|gif|webp);base64,[A-Za-z0-9+/=]+$/.test(state.result.dataUrl||'')){
        const img=document.createElement('img');img.alt=state.item.filename;img.src=state.result.dataUrl;content.replaceChildren(img);
      }else content.textContent='此类型暂不支持快速预览，请下载或用默认应用打开。';
    }});
    layer.querySelector('[data-preview-action="previous"]').disabled=index===0;
    layer.querySelector('[data-preview-action="next"]').disabled=index===items.length-1;
    void preview.open({...requestItem(items[index]),filename:items[index].filename,mime:items[index].mime},!!options.localOnly);
  }
  function nativeClosed(event){preview?.nativeClosed(event);}
  if(typeof document!=='undefined')document.addEventListener('keydown',event=>{
    if(!previewActive())return;
    if(event.isComposing||event.keyCode===229)return;
    if(event.key==='Escape'){event.preventDefault();event.stopImmediatePropagation();void closePreview();return;}
    if(event.key==='Tab'){
      const buttons=[...layer.querySelectorAll('button:not(:disabled)')],at=buttons.indexOf(document.activeElement);
      event.preventDefault();event.stopImmediatePropagation();buttons[(at+(event.shiftKey?buttons.length-1:1))%buttons.length]?.focus();return;
    }
    // Prevent mail/hidden-compose shortcuts beneath this focus scope.
    if([' ','j','k','r','e','s','Delete','Backspace'].includes(event.key)||((event.metaKey||event.ctrlKey)&&event.key==='Enter')){
      if(event.key===' '&&event.target.closest?.('button'))return;
      event.preventDefault();event.stopImmediatePropagation();
    }
  },true);
  function mount(options){
    const {container,invoke}=options;
    const events=new AbortController();
    const on=(type,callback)=>container.addEventListener(type,callback,{signal:events.signal});
    let seq=0,rows=[],cursor=null,disposed=false,timer=null,selected=-1,loading=false,collection=null,loadedPages=1,hasResult=false;
    const rootTargets=options.targets.slice();
    const accounts=(options.accounts||[]).filter(a=>a.receiveProtocol!=='pop3');
    const popOnly=(options.accounts||[]).length>0 && accounts.length===0;
    const scopeNote='范围为当前 IMAP 账户，不含草稿、垃圾邮件和已删除。';
    const identity=attachmentIdentity;
    container.innerHTML=`<div class="list-head"><strong>附件</strong><button type="button" class="icon-btn" data-center-action="refresh" aria-label="刷新附件">↻</button><button type="button" class="link" data-center-action="collect">补充本地附件</button></div><div class="attachment-filters"><input type="search" data-center-filter="query" aria-label="附件名称" placeholder="搜索附件名称"><select data-center-filter="kind" aria-label="附件类型"><option value="all">所有类型</option><option value="pdf">PDF</option><option value="image">图片</option><option value="spreadsheet">表格</option><option value="other">其他</option></select><select data-center-filter="account" aria-label="附件账户"><option value="">全部当前账户</option>${accounts.map(a=>`<option value="${esc(a.id)}">${esc(a.email)}</option>`).join('')}</select><details><summary>更多条件</summary><input data-center-filter="sender" placeholder="发件人" aria-label="附件发件人"><label>从 <input type="date" data-center-filter="after"></label><label>至 <input type="date" data-center-filter="before"></label></details></div><p class="attachment-coverage" role="status">正在读取已发现的附件…</p><p class="attachment-center-error" role="alert" hidden></p><div class="attachment-items" tabindex="-1"></div><button class="list-more" data-center-action="more" hidden>加载更多附件</button>`;
    const query=()=>{const value=name=>container.querySelector(`[data-center-filter="${name}"]`).value;const before=value('before')?new Date(value('before')+'T00:00:00'):null;if(before)before.setDate(before.getDate()+1);return {targets:rootTargets.filter(t=>!value('account')||t.accountId===value('account')),query:value('query').trim(),kind:value('kind'),sender:value('sender').trim(),after:value('after')?new Date(value('after')+'T00:00:00').getTime():null,before:before?.getTime()??null,pageSize:100,cursor:null};};
    const error=value=>{const el=container.querySelector('.attachment-center-error');el.hidden=!value;el.textContent=String(value||'');};
    centerFocusTargets.set(container.querySelector('.attachment-items'),(key,action)=>{
      if(disposed || container.isConnected===false)return null;
      const index=rows.findIndex(item=>identity(item)===key);
      return index<0?container.querySelector('.attachment-items'):container.querySelector(`[data-center-index="${index}"] [data-center-action="${action}"]`);
    });
    function paint(empty='当前条件下还没有发现附件。这里只显示已检查邮件中的附件，不代表邮箱没有附件。'){container.querySelector('.attachment-items').innerHTML=rows.length?rows.map((item,index)=>`<article class="attachment-item${selected===index?' selected':''}" data-center-index="${index}"><button type="button" class="attachment-file" data-center-action="select" title="Space 预览，双击默认应用打开"><span class="attachment-type">${esc((item.filename.split('.').pop()||'文件').slice(0,5).toUpperCase())}</span><span><strong>${esc(item.filename)}</strong><small>${esc(item.sender)} · ${esc(options.formatDate(item.messageDate))}</small><small>${esc(options.accountName(item.source.accountId))} · ${esc(options.formatSize(item.size))}</small></span></button><button class="icon-btn" type="button" data-center-action="preview" aria-label="预览 ${esc(item.filename)}">⌕</button></article>`).join(''):`<div class="empty">${esc(empty)}</div>`;}
    async function load(more=false){
      if(disposed || (more && (!cursor || loading || timer!==null)))return;
      clearTimeout(timer);timer=null;
      const generation=++seq;loading=true;error('');
      try{
        const input=query();if(more)input.cursor=cursor;
        const found=[];let page,pages=0;
        // Re-read every loaded page into a temporary result. A failed or stale
        // later page must not replace a usable, longer list with its first page.
        do {
          page=input.targets.length ? await invoke('list_attachments',{input:{...input}}) : {items:[],coverage:[],hasMore:false,nextCursor:null,catalogIncomplete:[]};
          if(disposed||generation!==seq)return;
          found.push(...page.items);pages++;input.cursor=page.nextCursor;
        } while(!more && pages<loadedPages && page.hasMore && page.nextCursor);
        const chosen=identity(rows[selected]),active=container.ownerDocument?.activeElement;
        const focusedRow=active?.closest?.('[data-center-index]');
        const focused=container.contains?.(active)&&focusedRow?identity(rows[Number(focusedRow.dataset.centerIndex)]):null;
        const focusAction=active?.dataset?.centerAction==='preview'?'preview':'select';
        const list=container.querySelector('.attachment-items'),scroll=list.scrollTop;
        rows=more?rows.concat(found):found;cursor=page.nextCursor;hasResult=true;
        if(more)loadedPages++;
        selected=chosen===null?-1:rows.findIndex(item=>identity(item)===chosen);
        const empty=popOnly?'附件中心暂不支持 POP3 账户。':!input.targets.length?'当前账户尚无可用文件夹，请先刷新邮件目录。':undefined;
        paint(empty);list.scrollTop=scroll;
        const focusIndex=focused===null?-1:rows.findIndex(item=>identity(item)===focused);
        if(focusIndex>=0)container.querySelector(`[data-center-index="${focusIndex}"] [data-center-action="${focusAction}"]`)?.focus({preventScroll:true});
        else if(focused)list.focus?.({preventScroll:true});
        const coverage=page.coverage||[],parsed=coverage.reduce((n,c)=>n+(c.parsedMessages||0),0),indexed=coverage.reduce((n,c)=>n+(c.indexedMessages||0),0);
        container.querySelector('.attachment-coverage').textContent=empty||`当前显示 ${rows.length} 个附件 · 已检查 ${parsed} / ${indexed} 封已收录邮件，其他邮件可能还有附件。${scopeNote}${(page.catalogIncomplete||[]).join('；')}`;
        container.querySelector('[data-center-action="more"]').hidden=!page.hasMore;
      }catch(e){if(!disposed&&generation===seq){
        error(`附件读取失败：${e}`);
        container.querySelector('.attachment-coverage').textContent=hasResult?`刷新未完成，仍显示上次读取的 ${rows.length} 个附件。`:'附件尚未读取成功，请重试。';
        if(!hasResult)paint('附件暂时无法读取，请重试。');
      }}finally{if(generation===seq)loading=false;}
    }
    function scheduleRefresh(){if(disposed)return;++seq;clearTimeout(timer);timer=setTimeout(()=>load(),200);}
    function resetFilters(){
      collection?.abort();++seq;loadedPages=1;selected=-1;cursor=null;rows=[];hasResult=false;
      container.querySelector('[data-center-action="more"]').hidden=true;
      error('');paint('正在筛选已发现的附件…');
      container.querySelector('.attachment-coverage').textContent='正在读取当前筛选条件下的附件…';
    }
    on('input',event=>{if(!event.target.matches('[data-center-filter]'))return;resetFilters();scheduleRefresh();});
    on('change',event=>{if(!event.target.matches('[data-center-filter]'))return;resetFilters();void load();});
    on('click',event=>{
      const button=event.target.closest('[data-center-action]');if(!button)return;
      const action=button.dataset.centerAction,index=Number(button.closest('[data-center-index]')?.dataset.centerIndex);
      if(action==='collect'){
        if(collection){collection.abort();collection=null;button.textContent='补充本地附件';return;}
        const controller=new AbortController();collection=controller;button.textContent='取消补充';
        Promise.resolve(options.onCollect?.(query().targets,controller.signal,(done,total)=>{if(!disposed&&!controller.signal.aborted)button.textContent=`取消补充 ${done}/${total}`;})).catch(e=>{if(!disposed&&collection===controller)error(e);}).finally(()=>{if(collection===controller){collection=null;if(button.isConnected)button.textContent='补充本地附件';}});return;
      }
      if(action==='refresh'){options.resetCollection?.();void load();return;}
      if(action==='more'){if(!loading)void load(true);return;}
      if(!rows[index])return;
      if(action==='select'){selected=index;for(const el of container.querySelectorAll('[data-center-index]'))el.classList.toggle('selected',Number(el.dataset.centerIndex)===index);}
      if(action==='preview')showPreview(rows,index,options,button);
    });
    on('dblclick',event=>{const index=Number(event.target.closest('[data-center-index]')?.dataset.centerIndex);if(rows[index])void options.onFileAction('open',rows[index]).catch(error);});
    on('contextmenu',event=>{const index=Number(event.target.closest('[data-center-index]')?.dataset.centerIndex);if(rows[index]){event.preventDefault();options.onMenu?.(event,rows[index]);}});
    on('keydown',event=>{
      const row=event.target.closest('[data-center-index]');if(!row||event.isComposing||event.keyCode===229||event.metaKey||event.ctrlKey||event.altKey)return;
      const index=Number(row.dataset.centerIndex);
      if(event.key===' '&&!event.repeat){event.preventDefault();event.stopPropagation();showPreview(rows,index,options,event.target);}
      if(event.key==='ArrowDown'||event.key==='ArrowUp'){event.preventDefault();event.stopPropagation();container.querySelector(`[data-center-index="${index+(event.key==='ArrowDown'?1:-1)}"] button`)?.focus();}
    });
    void load();return {refresh:()=>load(),scheduleRefresh,dispose(){collection?.abort();events.abort();disposed=true;++seq;clearTimeout(timer);},get items(){return rows;}};
  }
  const api={createPreview,mount,showPreview,closePreview,previewActive,nativeClosed};
  if(typeof module==='object'&&module.exports)module.exports=api;else root.MailBeaconAttachments=api;
})(typeof window!=='undefined'?window:globalThis);
