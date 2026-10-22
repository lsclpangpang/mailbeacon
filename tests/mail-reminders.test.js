const test=require('node:test');const assert=require('node:assert/strict');const fs=require('node:fs');const path=require('node:path');
const p=path.join(__dirname,'../ui/productivity-panels.js'),api=fs.existsSync(p)?require(p):{};
const source={accountId:'a',receiveEpoch:2,folder:'INBOX',uidValidity:4,uid:7};
test('RM01: snooze hides only its exact inbox identity, never search or unread aggregate',()=>{
 assert.equal(typeof api.hiddenBySnooze,'function');
 const reminders=[{kind:'snooze',state:'scheduled',dueAt:200,source}];
 assert.equal(api.hiddenBySnooze(source,reminders,{folder:'INBOX',search:false},100),true);
 assert.equal(api.hiddenBySnooze({...source,uidValidity:5},reminders,{folder:'INBOX'},100),false);
 assert.equal(api.hiddenBySnooze(source,reminders,{folder:'__unread'},100),false);
 assert.equal(api.hiddenBySnooze(source,reminders,{folder:'INBOX',search:true},100),false);
 assert.equal(api.hiddenBySnooze(source,reminders,{folder:'INBOX'},200),false);
});
test('RM04: tomorrow and next Monday presets preserve local calendar time',()=>{
 assert.equal(typeof api.remindAt,'function');
 const friday=new Date(2026,9,23,21,30);
 const tomorrow=new Date(api.remindAt('tomorrow',friday));assert.equal(tomorrow.getDate(),24);assert.equal(tomorrow.getHours(),9);
 const monday=new Date(api.remindAt('monday',friday));assert.equal(monday.getDay(),1);assert.equal(monday.getDate(),26);
 assert.throws(()=>api.remindAt('custom',friday,'2020-01-01T09:00'),/未来/);
});
test('RM11: checking never claims the recipient did not reply',()=>{
 assert.equal(typeof api.reminderLabel,'function');
 assert.match(api.reminderLabel({kind:'followup',state:'checking'}),/待核对/);
 assert.match(api.reminderLabel({kind:'followup',state:'due',lastVerifiedAt:123}),/尚未检测到/);
});
test('X07: late list snapshots cannot roll back newer send or reminder events',()=>{
 const {mergeVersions}=require('../ui/productivity-panels');
 assert.equal(typeof mergeVersions,'function');
 const current=[{id:'a',version:3,state:'completed'},{id:'b',version:2,status:'accepted'}];
 const merged=mergeVersions(current,[{id:'a',version:1,state:'scheduled'}]);
 assert.equal(merged.find(x=>x.id==='a').state,'completed');
 assert.equal(merged.find(x=>x.id==='b').status,'accepted');
 assert.equal(mergeVersions(merged,[{id:'a',version:4,state:'cancelled'}]).find(x=>x.id==='a').state,'cancelled');
});
test('X08: the new modal keeps Tab within visible enabled controls',()=>{
 const {trapFocus}=require('../ui/productivity-panels'); assert.equal(typeof trapFocus,'function');
 const a={focus(){this.focused=true}},b={focus(){this.focused=true}};
 let prevented=false;const sheet={querySelectorAll:()=>[a,b]};
 trapFocus({key:'Tab',target:b,preventDefault(){prevented=true;}},sheet);
 assert.equal(a.focused,true);assert.equal(prevented,true);
 a.focused=b.focused=false;trapFocus({key:'Tab',target:a,shiftKey:true,preventDefault(){}},sheet);assert.equal(b.focused,true);
});
