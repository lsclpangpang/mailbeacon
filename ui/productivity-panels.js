(function(root){
  'use strict';
  const esc=s=>String(s??'').replace(/[&<>"']/g,c=>({'&':'&amp;','<':'&lt;','>':'&gt;','"':'&quot;',"'":'&#39;'}[c]));
  const sourceKey=m=>JSON.stringify([m?.accountId,m?.receiveEpoch,m?.folder,m?.uidValidity,m?.uid]);
  function trapFocus(event,sheet) {
    if(event.key!=='Tab'||event.defaultPrevented)return false;
    const controls=[...sheet.querySelectorAll('button,input,select,textarea,[tabindex]')].filter(el=>!el.disabled&&!el.hidden&&el.tabIndex!==-1&&(!el.getClientRects||el.getClientRects().length>0));
    if(!controls.length)return false;
    const index=controls.indexOf(event.target);
    const next=(index+(event.shiftKey?-1:1)+controls.length)%controls.length;
    event.preventDefault();controls[next].focus();return true;
  }
  function mergeVersions(current,incoming) {
    const map=new Map(current.map(row=>[row.id,row]));
    for(const row of incoming){const old=map.get(row.id);if(!old||Number(row.version||0)>=Number(old.version||0))map.set(row.id,row);}
    return [...map.values()];
  }
  function snoozeFor(message,reminders,now=Date.now()) {
    if(!message?.receiveEpoch||!message.uidValidity)return null;
    return reminders.find(r=>r.kind==='snooze'&&r.state==='scheduled'&&r.dueAt>now&&sourceKey(r.source)===sourceKey(message))||null;
  }
  function hiddenBySnooze(message,reminders,view,now=Date.now()){
    if(view.search||String(view.folder).toUpperCase()!=='INBOX')return false;
    if(!message?.receiveEpoch||!message.uidValidity)return false;
    return !!snoozeFor(message,reminders,now);
  }
  function remindAt(preset,now=new Date(),custom=''){
    const date=new Date(now);
    if(preset==='tomorrow'){date.setDate(date.getDate()+1);date.setHours(9,0,0,0);}
    else if(preset==='tonight'){date.setHours(20,0,0,0);if(date<=now)date.setDate(date.getDate()+1);}
    else if(preset==='monday'){date.setDate(date.getDate()+((8-date.getDay())%7||7));date.setHours(9,0,0,0);}
    else if(preset==='hour')date.setTime(date.getTime()+3600000);
    else if(preset==='custom')date.setTime(new Date(custom).getTime());
    else throw new Error('请选择提醒时间');
    if(!Number.isFinite(date.getTime())||date<=now)throw new Error('请选择未来的提醒时间');
    return date.getTime();
  }
  function reminderLabel(r){
    if(r.state==='checking')return '待核对回复 · 需要完整同步';
    if(r.state==='due')return r.kind==='followup'?'此范围内尚未检测到回复':'已到提醒时间';
    return {scheduled:r.kind==='followup'?'等待回复':'稍后处理',completed:'已完成',cancelled:'已取消',orphaned:'原邮件位置待确认'}[r.state]||'状态待确认';
  }
  const dateInput=ms=>{if(!ms)return '';const d=new Date(ms);return `${d.getFullYear()}-${String(d.getMonth()+1).padStart(2,'0')}-${String(d.getDate()).padStart(2,'0')}`;};
  function savedSidebar(views,active){
    if(!views.length)return '';
    return '<div class="side-label">智能文件夹</div>'+views.map(v=>`<div class="saved-view-nav"><button class="nav-item ${v.id===active?'active':''}" data-saved-action="open" data-saved-id="${esc(v.id)}" title="${esc(v.repairReason||v.name)}"><span class="saved-view-icon" aria-hidden="true">⌕</span><span class="label">${esc(v.name)}</span>${v.repairReason?'<span aria-label="需要修复">!</span>':''}</button><button type="button" class="icon-btn" data-saved-action="edit" data-saved-id="${esc(v.id)}" aria-label="编辑 ${esc(v.name)}">⋯</button></div>`).join('');
  }
  function editSavedView(options){
    const {overlay,view,initialSpec,invoke}=options;
    const spec=JSON.parse(JSON.stringify(initialSpec||view?.spec||{}));
    const dates={after:dateInput(spec.after),before:dateInput(spec.before?spec.before-1:null)};
    overlay.innerHTML=`<div class="scrim" role="dialog" aria-modal="true" aria-labelledby="saved-view-title"><form class="sheet productivity-sheet"><div class="sheet-head"><h3 id="saved-view-title">${view?'编辑智能文件夹':'保存为智能文件夹'}</h3><button type="button" class="icon-btn" data-panel-close aria-label="关闭">×</button></div><div class="productivity-fields"><label>名称<input name="name" required maxlength="120" value="${esc(view?.name||'')}" placeholder="例如：最近的合同附件"></label><label>范围<select name="scope"><option value="keep">${spec.scope==='all'?'全部账户（包含以后添加）':'保持指定账户与文件夹'}</option><option value="current">改为当前账户/文件夹范围</option><option value="all">全部账户（包含以后添加）</option></select></label><p class="hint">固定范围不会随侧栏账户切换；删除智能文件夹不会删除邮件。</p><label>关键词<input name="query" value="${esc(spec.query||'')}" placeholder="可留空，仅按条件查询"></label><label>发件人<input name="sender" value="${esc(spec.sender||'')}"></label><div class="productivity-checks">${[['unreadOnly','未读'],['flaggedOnly','星标'],['attachmentOnly','有附件'],['includeExcluded','包含垃圾邮件与已删除']].map(([key,label])=>`<label><input type="checkbox" name="${key}" ${spec[key]?'checked':''}>${label}</label>`).join('')}</div><div class="productivity-dates"><label>最近天数<input name="relativeDays" type="number" min="1" max="36500" value="${esc(spec.relativeDays||'')}" placeholder="不限"></label><label>从<input type="date" name="after" value="${dates.after}"></label><label>至<input type="date" name="before" value="${dates.before}"></label></div><label>排序<select name="sort"><option value="dateDesc" ${spec.sort!=='dateAsc'?'selected':''}>最新在前</option><option value="dateAsc" ${spec.sort==='dateAsc'?'selected':''}>最早在前</option></select></label><label>侧栏顺序<input name="position" type="number" min="0" value="${view?.position??options.position??0}"></label><p class="productivity-error" role="alert" hidden></p></div><div class="sheet-foot">${view?'<button type="button" class="danger-link" data-saved-delete>删除此视图</button>':''}<span class="spacer"></span><button type="button" class="secondary" data-panel-close>取消</button><button class="primary" type="submit">保存</button></div></form></div>`;
    const form=overlay.querySelector('form'),field=name=>form.elements.namedItem(name),error=message=>{const el=form.querySelector('.productivity-error');el.textContent=String(message);el.hidden=!message;};
    form.querySelectorAll('[data-panel-close]').forEach(b=>b.addEventListener('click',options.onClose));
    form.querySelector('[data-saved-delete]')?.addEventListener('click',async event=>{
      const button=event.currentTarget;
      if(button.dataset.confirmed!=='true'){button.dataset.confirmed='true';button.textContent='确认删除视图（保留邮件）';return;}
      button.disabled=true;try{await invoke('delete_saved_view',{id:view.id,expectedRevision:view.revision});if(form.isConnected)await options.onSaved(null);}catch(e){error(e);button.disabled=false;}
    });
    form.addEventListener('submit',async event=>{
      event.preventDefault();event.stopPropagation();const save=form.querySelector('[type="submit"]');if(save.disabled)return;save.disabled=true;error('');
      try{
        let next=JSON.parse(JSON.stringify(spec));
        if(field('scope').value==='all')Object.assign(next,{specVersion:1,scope:'all',targets:[]});
        if(field('scope').value==='current')next=await options.captureCurrent();
        const relativeDays=Number(field('relativeDays').value)||null;
        const before=field('before').value?new Date(field('before').value+'T00:00:00'):null;if(before)before.setDate(before.getDate()+1);
        Object.assign(next,{specVersion:1,query:field('query').value.trim(),sender:field('sender').value.trim(),relativeDays,after:relativeDays?null:field('after').value?new Date(field('after').value+'T00:00:00').getTime():null,before:relativeDays?null:before?.getTime()??null,sort:field('sort').value});
        for(const key of ['unreadOnly','flaggedOnly','attachmentOnly','includeExcluded'])next[key]=field(key).checked;
        const saved=await invoke('save_saved_view',{id:view?.id||null,name:field('name').value.trim(),position:Number(field('position').value),spec:next,expectedRevision:view?.revision??null});
        if(form.isConnected)await options.onSaved(saved);
      }catch(e){error(e);}finally{save.disabled=false;}
    });
    field('name').focus();
  }
  function snoozeDialog({overlay,title,onSave,onClose,initialDue}){
    overlay.innerHTML=`<div class="scrim" role="dialog" aria-modal="true" aria-labelledby="snooze-title"><form class="sheet productivity-sheet snooze-sheet"><div class="sheet-head"><h3 id="snooze-title">稍后处理</h3><button type="button" class="icon-btn" data-panel-close aria-label="关闭">×</button></div><div class="productivity-fields"><p>${esc(title||'这封邮件')}</p><p class="hint">暂时从本机收件箱隐藏，到期恢复。网页邮箱中的位置和已读状态保持。</p><div class="snooze-presets">${[['hour','一小时后'],['tonight','今晚'],['tomorrow','明早 9 点'],['monday','下周一']].map(([value,label])=>`<button type="button" class="secondary" data-preset="${value}">${label}</button>`).join('')}</div><label>自定义时间<input name="due" type="datetime-local" required></label><p class="productivity-error" role="alert" hidden></p></div><div class="sheet-foot"><button type="button" class="secondary" data-panel-close>取消</button><button type="submit" class="primary">确定</button></div></form></div>`;
    const form=overlay.querySelector('form'),input=form.elements.namedItem('due');
    const setDate=ms=>{const d=new Date(ms);input.value=dateInput(ms)+'T'+String(d.getHours()).padStart(2,'0')+':'+String(d.getMinutes()).padStart(2,'0');};setDate(initialDue||remindAt('tomorrow'));
    form.querySelectorAll('[data-preset]').forEach(b=>b.addEventListener('click',()=>setDate(remindAt(b.dataset.preset))));
    form.querySelectorAll('[data-panel-close]').forEach(b=>b.addEventListener('click',onClose));
    form.addEventListener('submit',async event=>{event.preventDefault();event.stopPropagation();const button=form.querySelector('[type="submit"]');if(button.disabled)return;button.disabled=true;try{await onSave(remindAt('custom',new Date(),input.value));if(form.isConnected)onClose();}catch(e){const error=form.querySelector('.productivity-error');error.hidden=false;error.textContent=String(e);}finally{button.disabled=false;}});input.focus();
  }
  function reminderList(reminders,{kind,accountId,formatDate,lookup=()=>null,accountName=id=>id}){
    const rows=reminders.filter(r=>(!accountId||r.accountId===accountId)&&!['completed','cancelled'].includes(r.state)&&(kind==='snoozed'?r.kind==='snooze'&&r.state==='scheduled':r.kind==='followup'||r.state==='due'||r.state==='orphaned'));
    return rows.length?rows.map(r=>`<article class="reminder-card" data-reminder-id="${esc(r.id)}"><button type="button" class="reminder-open" data-reminder-action="open"><strong>${esc(lookup(r)?.subject|| (r.kind==='followup'?'等待回复的邮件':'稍后处理的邮件'))}</strong><span>${esc(reminderLabel(r))}</span></button><small>${esc(accountName(r.accountId))} · ${esc(formatDate(r.dueAt))}</small>${r.lastVerifiedAt?`<small>核对截至 ${esc(formatDate(r.lastVerifiedAt))}</small>`:''}${r.error?`<p class="productivity-error">${esc(r.error)}</p>`:''}<details><summary>查看检测范围</summary><p>${r.kind==='snooze'?'仅在本机收件箱暂时隐藏':esc(r.scope?.join('、')||'待核对：收件箱、归档与自定义目录；排除已发送、草稿、垃圾邮件及已删除')}</p></details><div class="reminder-actions">${r.kind==='snooze'&&r.state==='scheduled'?'<button class="secondary" data-reminder-action="restore">提前恢复</button>':''}<button class="secondary" data-reminder-action="reschedule">改时间</button><button class="secondary" data-reminder-action="complete">完成</button><button class="link" data-reminder-action="cancel">取消提醒</button></div></article>`).join(''):'<div class="empty">这里还没有提醒</div>';
  }
  function bufferMarkup(records,now=Date.now()){
    return records.filter(r=>['buffered','paused','submitting'].includes(r.status)).map(r=>`<div class="send-buffer-item" data-buffer-id="${esc(r.id)}"><span class="buffer-state">${r.status==='buffered'?`${Math.max(0,Math.ceil((r.deadline-now)/1000))} 秒后发送`:r.status==='paused'?'已暂停 · 请确认发送':'正在发送'}</span><span class="buffer-subject">${esc(r.subject||'（无主题）')}</span>${r.status!=='submitting'?'<button type="button" data-buffer-action="cancel">撤销</button><button type="button" data-buffer-action="sendNow">立即发送</button>':''}</div>`).join('');
  }
  const api={snoozeFor,trapFocus,mergeVersions,sourceKey,hiddenBySnooze,remindAt,reminderLabel,savedSidebar,editSavedView,snoozeDialog,reminderList,bufferMarkup,dateInput};
  if(typeof module==='object'&&module.exports)module.exports=api;else root.MailBeaconProductivity=api;
})(typeof window!=='undefined'?window:globalThis);
