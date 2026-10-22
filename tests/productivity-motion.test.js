const test=require('node:test');
const assert=require('node:assert/strict');
const motion=require('../ui/reader-motion.js');
function snapshot(){assert.equal(typeof motion.createListSnapshot,'function','list updates must preserve the active gesture snapshot');return motion.createListSnapshot(x=>x.id);}
test('AN05/X09: new mail, due reminders and search matches do not move a drag snapshot',()=>{
 const state=snapshot(), initial=Array.from({length:1000},(_,i)=>({id:String(i)}));
 assert.deepEqual(state.update(initial,{scope:'a'}).items,initial);
 const incoming=[{id:'new'},{id:'reminder'},...initial];
 const frozen=state.update(incoming,{scope:'a',gesture:true});
 assert.equal(frozen.items[300].id,'300');assert.equal(frozen.pending,2);
 const end=state.update(incoming,{scope:'a',anchor:'300',offset:17});
 assert.equal(end.anchorIndex,302);assert.equal(end.offset,17);assert.equal(end.pending,0);
});
test('AN05: reading in the middle defers insertions until explicitly integrated',()=>{
 const s=snapshot();s.update([{id:'a'}],{scope:'a'});
 assert.equal(s.update([{id:'new'},{id:'a'}],{scope:'a',reading:true}).pending,1);
 assert.deepEqual(s.update([{id:'new'},{id:'a'}],{scope:'a',reading:true,force:true}).items.map(x=>x.id),['new','a']);
});
test('X09: a scope change discards the old snapshot and pending items',()=>{
 const s=snapshot();s.update([{id:'a'}],{scope:'a'});
 s.update([{id:'new'},{id:'a'}],{scope:'a',gesture:true});
 assert.deepEqual(s.update([{id:'b'}],{scope:'b',gesture:true}).items,[{id:'b'}]);
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
