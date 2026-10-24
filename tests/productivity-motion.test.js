const test=require('node:test');
const assert=require('node:assert/strict');
const motion=require('../ui/reader-motion.js');
function snapshot(){assert.equal(typeof motion.createListSnapshot,'function','list updates must preserve the active gesture snapshot');return motion.createListSnapshot(x=>x.id);}
test('historical index completion while reading merges 40 to 3278 rows without a new-mail prompt',()=>{
 const state=snapshot(), all=Array.from({length:3278},(_,i)=>({id:String(3278-i),seen:true,date:3278-i}));
 state.update(all.slice(0,40),{scope:'a'});
 const page=state.update(all,{scope:'a',reading:true});
 assert.equal(page.items.length,3278);assert.equal(page.pending,0);
 assert.deepEqual(state.update(all,{scope:'a',reading:true}).items,all);
});
test('reading merges new mail immediately and retains its arrival identity for navigation',()=>{
 const state=snapshot();state.update([{id:'a',seen:false}],{scope:'a'});
 const newest={id:'new',date:1}, older={id:'older',date:0};
 state.markNew([newest]);
 const incoming=[newest,{id:'a',seen:true},older];
 const page=state.update(incoming,{scope:'a',reading:true});
 assert.deepEqual(page.items,incoming);assert.equal(page.pending,1);
 assert.deepEqual(page.arrivals,[newest]);
 assert.equal(state.update(incoming,{scope:'a',reading:true}).pending,1);
});
test('AN05/X09: drag freezes hit targets, then automatically merges history and new mail',()=>{
 const state=snapshot(), initial=Array.from({length:1000},(_,i)=>({id:String(i)}));
 assert.deepEqual(state.update(initial,{scope:'a'}).items,initial);
 const incoming=[{id:'new'},{id:'reminder'},...initial];
 state.markNew([{id:'new'}]);
 const frozen=state.update(incoming,{scope:'a',gesture:true});
 assert.equal(frozen.items[300].id,'300');assert.equal(frozen.items.length,1000);assert.equal(frozen.pending,1);
 const end=state.update(incoming,{scope:'a',anchor:'300',offset:17});
 assert.deepEqual(frozen.arrivals,[{id:'new'}]);
 assert.equal(end.anchorIndex,302);assert.equal(end.offset,17);assert.equal(end.pending,1);
 assert.deepEqual(end.items,incoming);
 const merged=state.update(incoming,{scope:'a',force:true,anchor:'300',offset:17});
 assert.equal(merged.anchorIndex,302);assert.equal(merged.pending,0);
});
test('drag does not label unmarked historical or search additions as new mail',()=>{
 const state=snapshot();state.update([{id:'a'}],{scope:'a'});
 const frozen=state.update([{id:'match'},{id:'a'},{id:'older'}],{scope:'a',gesture:true});
 assert.deepEqual(frozen.items,[{id:'a'}]);assert.equal(frozen.pending,0);
 const end=state.update([{id:'match'},{id:'a'},{id:'older'}],{scope:'a',reading:true});
 assert.equal(end.items.length,3);assert.equal(end.pending,0);
});
test('AN05: an explicit merge clears new-mail marks and displayed rows cannot be marked again',()=>{
 const s=snapshot();s.update([{id:'a'}],{scope:'a'});
 s.markNew([{id:'new'}]);
 assert.equal(s.update([{id:'new'},{id:'a'}],{scope:'a',reading:true}).pending,1);
 assert.deepEqual(s.update([{id:'new'},{id:'a'}],{scope:'a',reading:true,force:true}).items.map(x=>x.id),['new','a']);
 s.markNew([{id:'new'},{id:'a'}]);
 assert.equal(s.update([{id:'new'},{id:'a'}],{scope:'a',reading:true}).pending,0);
});
test('duplicate new-mail events count identities once and existing rows are not new',()=>{
 const s=snapshot();s.update([{id:'a'}],{scope:'a'});
 s.markNew([{id:'a'},{id:'new'},{id:'new'}]);s.markNew([{id:'new'}]);
 const incoming=[{id:'new'},{id:'a',seen:true}];
 const page=s.update(incoming,{scope:'a',reading:true});
 assert.deepEqual(page.items,incoming);assert.equal(page.pending,1);
 assert.deepEqual(page.arrivals,[{id:'new'}]);
 s.markNew([{id:'new'}]);assert.equal(s.update(incoming,{scope:'a',reading:true}).pending,1);
});
test('marks before initialization are ignored, and idle arrivals display immediately',()=>{
 const s=snapshot();s.markNew([{id:'first'}]);
 assert.deepEqual(s.update([{id:'first'}],{scope:'a',reading:true}),{items:[{id:'first'}],pending:0,arrivals:[],anchorIndex:-1,offset:0});
 s.markNew([{id:'new'}]);
 const page=s.update([{id:'new'},{id:'first'}],{scope:'a'});
 assert.equal(page.items.length,2);assert.equal(page.pending,1);
 assert.deepEqual(page.arrivals,[{id:'new'}]);
 assert.equal(s.update(page.items,{scope:'a',reading:true}).pending,1);
});
test('a new-mail candidate survives old search results and displays when its result arrives',()=>{
 const s=snapshot();s.update([{id:'matched'}],{scope:'search'});
 s.markNew([{id:'new-match'}]);
 for(let i=0;i<3;i++) {
  const unchanged=s.update([{id:'matched'}],{scope:'search',reading:true});
  assert.equal(unchanged.pending,0);assert.deepEqual(unchanged.items,[{id:'matched'}]);
 }
 const refreshed=s.update([{id:'new-match'},{id:'matched'}],{scope:'search',reading:true});
 assert.equal(refreshed.pending,1);assert.deepEqual(refreshed.items,[{id:'new-match'},{id:'matched'}]);
 assert.deepEqual(refreshed.arrivals,[{id:'new-match'}]);
});
test('an unmatched candidate neither counts as pending nor holds up an idle arrival',()=>{
 const s=snapshot();s.update([{id:'matched'}],{scope:'search'});
 s.markNew([{id:'not-yet-matched'}]);
 assert.equal(s.update([{id:'matched'}],{scope:'search',reading:true}).pending,0);
 s.markNew([{id:'idle-arrival'}]);
 const idle=s.update([{id:'idle-arrival'},{id:'matched'}],{scope:'search'});
 assert.equal(idle.pending,1);assert.deepEqual(idle.items,[{id:'idle-arrival'},{id:'matched'}]);
 // The unmatched candidate remains available for a later asynchronous result.
 const later=s.update([{id:'not-yet-matched'},...idle.items],{scope:'search',reading:true});
 assert.equal(later.pending,2);assert.deepEqual(later.items,[{id:'not-yet-matched'},...idle.items]);
 assert.deepEqual(later.arrivals,[{id:'not-yet-matched'},{id:'idle-arrival'}]);
});
test('acknowledging one arrival preserves the rest and repeated events cannot restore its prompt',()=>{
 const s=snapshot();s.update([{id:'a'}],{scope:'a'});
 s.markNew([{id:'second'},{id:'first'}]);
 const incoming=[{id:'first',seen:false},{id:'second'},{id:'a'}];
 assert.deepEqual(s.update(incoming,{scope:'a',reading:true}).arrivals,incoming.slice(0,2));
 s.acknowledge({id:'first'});
 s.markNew([{id:'first'},{id:'second'}]);
 const page=s.update(incoming,{scope:'a',reading:true});
 assert.deepEqual(page.items,incoming);assert.equal(page.pending,1);
 assert.deepEqual(page.arrivals,[{id:'second'}]);
 s.acknowledge({id:'second'});
 assert.deepEqual(s.update(incoming,{scope:'a'}).arrivals,[]);
});
test('acknowledging a candidate before its result arrives cannot be undone by duplicate events',()=>{
 const s=snapshot();s.update([{id:'a'}],{scope:'search'});
 s.markNew([{id:'candidate'}]);s.acknowledge({id:'candidate'});
 s.markNew([{id:'candidate'}]);
 const page=s.update([{id:'candidate'},{id:'a'}],{scope:'search',reading:true});
 assert.equal(page.pending,0);assert.deepEqual(page.arrivals,[]);assert.equal(page.items.length,2);
});
test('arrival navigation uses the current result order and current message data',()=>{
 const s=snapshot();s.update([{id:'a'}],{scope:'a'});
 s.markNew([{id:'first',seen:false},{id:'second',seen:false}]);
 const incoming=[{id:'second',seen:true},{id:'a'},{id:'first',seen:false}];
 const page=s.update(incoming,{scope:'a',reading:true});
 assert.deepEqual(page.arrivals,[incoming[0],incoming[2]]);assert.equal(page.pending,2);
 const changed=[{id:'first',seen:true},incoming[0],incoming[1]];
 assert.deepEqual(s.update(changed,{scope:'a'}).arrivals,changed.slice(0,2));
});
test('force, scope changes and reset also clear candidates that have not matched a result',()=>{
 for(const action of ['force','scope','reset']) {
  const s=snapshot();s.update([{id:'a'}],{scope:'a'});
  s.markNew([{id:'candidate'}]);s.update([{id:'a'}],{scope:'a',reading:true});
  if(action==='reset')s.reset();
  const scope=action==='scope'?'b':'a';
  s.update([{id:'a'}],{scope,reading:true,force:action==='force'});
  const later=s.update([{id:'candidate'},{id:'a'}],{scope,reading:true});
  assert.equal(later.pending,0,action);assert.equal(later.items.length,2,action);
 }
});
test('removed or filtered pending mail no longer contributes to the prompt',()=>{
 const s=snapshot();s.update([{id:'a'}],{scope:'a'});
 s.markNew([{id:'new'},{id:'filtered'}]);
 assert.equal(s.update([{id:'new'},{id:'filtered'},{id:'a'}],{scope:'a',reading:true}).pending,2);
 assert.equal(s.update([{id:'new'},{id:'a'}],{scope:'a',reading:true}).pending,1);
 const gone=s.update([{id:'a'}],{scope:'a',reading:true});
 assert.equal(gone.pending,0);assert.deepEqual(gone.items,[{id:'a'}]);
 // Re-entering through an ordinary list result must not revive a discarded mark.
 assert.equal(s.update([{id:'filtered'},{id:'a'}],{scope:'a',reading:true}).pending,0);
});
test('rows already displayed in this scope do not become new after disappearing and reappearing',()=>{
 const s=snapshot();s.update([{id:'a'},{id:'b'}],{scope:'a'});
 s.update([{id:'b'}],{scope:'a',reading:true});s.markNew([{id:'a'}]);
 const page=s.update([{id:'a'},{id:'b'}],{scope:'a',reading:true});
 assert.equal(page.pending,0);assert.equal(page.items.length,2);
});
test('X09: a scope change discards the old snapshot and pending items',()=>{
 const s=snapshot();s.update([{id:'a'}],{scope:'a'});
 s.markNew([{id:'new'}]);
 s.update([{id:'new'},{id:'a'}],{scope:'a',gesture:true});
 const page=s.update([{id:'new'},{id:'b'}],{scope:'b',gesture:true});
 assert.deepEqual(page.items,[{id:'new'},{id:'b'}]);assert.equal(page.pending,0);
});
test('reset clears pending mail and displayed identity history',()=>{
 const s=snapshot();s.update([{id:'a'}],{scope:'a'});
 s.markNew([{id:'new'}]);s.update([{id:'new'},{id:'a'}],{scope:'a',reading:true});
 s.reset();s.markNew([{id:'ignored'}]);
 assert.equal(s.update([{id:'ignored'}],{scope:'a',reading:true}).pending,0);
 s.markNew([{id:'a'}]);
 assert.equal(s.update([{id:'a'},{id:'ignored'}],{scope:'a',reading:true}).pending,1);
});
test('AN02: updates to an existing row do not reorder fixed hit targets',()=>{
 const s=snapshot();s.update([{id:'a',seen:false},{id:'b'}],{scope:'a'});
 const page=s.update([{id:'b'},{id:'a',seen:true}],{scope:'a',gesture:true});
 assert.deepEqual(page.items.map(x=>x.id),['a','b']);assert.equal(page.items[0].seen,true);
});
test('MO08: merging rows above the viewport preserves its first visible identity and offset',()=>{
 const {anchorScroll}=require('../ui/reader-motion');
 assert.equal(typeof anchorScroll,'function');
 assert.equal(anchorScroll(['a','b','c'],['new','a','b','c'],66+12,66),144);
 assert.equal(anchorScroll(['a','b','c'],['a','c'],66+12,66),78);
 assert.equal(anchorScroll(['a'],[],12,66),0);
});
test('MO08: a list at the top stays at the top when new rows arrive',()=>{
 assert.equal(motion.anchorScroll(['a','b'],['new','a','b'],0,66),0);
});
test('MO08: conversation member identities preserve the anchor when its representative changes',()=>{
 const {anchorScroll}=motion;
 const previous=[['a'],['reply-old','parent'],['c']];
 const next=[['new'],['a'],['reply-new','reply-old','parent'],['c']];
 assert.equal(anchorScroll(previous,next,66+12,66),144);
 assert.equal(anchorScroll(['a','reply-old','c'],next,66+12,66),144);
 assert.equal(anchorScroll(previous,['new','a','parent','c'],66+12,66),144);
 assert.equal(anchorScroll(previous,[['a'],['c']],66+12,66),78);
 assert.equal(anchorScroll(previous,next,0,66),0);
});
