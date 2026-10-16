import test from 'node:test';
import assert from 'node:assert/strict';
import {tokens,rebaseBindings,parseCommand,mentionTokens,prepareChat} from '../web/slash-commands.js';
const inventory={scene:'local:test',frame:'home',commands:[{name:'focus',args:[{name:'target',kind:'target'}]},{name:'rename',args:[{name:'target',kind:'editable_target'},{name:'name',kind:'text'}]},{name:'note',args:[{name:'name',kind:'text'},...['x','y','z'].map(name=>({name,kind:'number'}))]}]};
const bind=(input,index,argument)=>({...tokens(input)[index],argument});
test('quoted peer names and apostrophes resolve to selected IDs',()=>{
 const input=`/focus @"Dan's Phone"`,b=bind(input,1,{type:'peer',id:'phone'});
 assert.equal(parseCommand(input,[b],inventory,'local:test').args[0].id,'phone');
 assert.throws(()=>parseCommand(input,[],inventory,'local:test'),/autocomplete/);
});
test('duplicate labels keep the explicit selection',()=>{
 const input='/focus #cheese';
 for(const id of ['note-a','note-b'])assert.equal(parseCommand(input,[bind(input,1,{type:'entity',kind:'note',id})],inventory,'local:test').args[0].id,id);
});
test('edits inside a reference invalidate it; edits before it shift it',()=>{
 const input='/focus #cheese',b=bind(input,1,{type:'entity',kind:'note',id:'a'});
 assert.deepEqual(rebaseBindings(input,'/focus #bread',[b]),[]);
 const shifted=rebaseBindings(input,'  '+input,[b]);assert.equal(shifted[0].start,b.start+2);
 assert.equal(parseCommand('  '+input,shifted,inventory,'local:test').args[0].id,'a');
});
test('scene changes, stale spans and invalid numbers fail locally',()=>{
 const input='/focus #cheese',b=bind(input,1,{type:'entity',kind:'note',id:'a'});
 assert.throws(()=>parseCommand(input,[b],inventory,'other'),/scene/);
 assert.throws(()=>parseCommand(input+'cake',[b],inventory,'local:test'),/autocomplete/);
 for(const number of ['NaN','Infinity','0x12','1abc','1e999'])assert.throws(()=>parseCommand(`/note "test" ${number} 1 2`,[],inventory,'local:test'),/number/);
 assert.deepEqual(parseCommand('/note "this is Dan" -1 .2 3e-2',[],inventory,'local:test').args.slice(1).map(a=>a.value),[-1,.2,.03]);
});
test('escaped names, malformed quotes and unknown commands',()=>{
 assert.equal(tokens('/rename #test "say \\"hi\\""')[2].value,'say "hi"');
 assert.equal(tokens('/focus #"unfinished')[1].complete,false);
 assert.throws(()=>parseCommand('/point @robot #target',[],inventory,'local:test'),/not exposed/);
 assert.throws(()=>parseCommand('/invent',[],inventory,'local:test'),/Unknown/);
});
test('path binds both multi-word peer and entity names independently',()=>{
 const inv={...inventory,commands:[{name:'path',args:[{name:'waypoint',kind:'target',variadic:true,min:2,max:64}]}]};
 for(const second of [{type:'peer',id:'robot'},{type:'entity',kind:'note',id:'note'}]){
  const input=`/path @Dan ${second.type==='peer'?'@':'#'}"Bracket 0125"`;
  const bindings=[bind(input,1,{type:'peer',id:'phone'}),bind(input,2,second)];
  assert.deepEqual(parseCommand(input,bindings,inv,'local:test').args,[{type:'peer',id:'phone'},second]);
  assert.throws(()=>parseCommand(input,bindings.slice(0,1),inv,'local:test'),/autocomplete/);
 }
});
test('variadic path preserves arbitrary ordered typed selections and validates the count',()=>{
 const inv={...inventory,commands:[{name:'path',args:[{name:'waypoint',kind:'target',variadic:true,min:2,max:64}]}]};
 const input='/path #1 #2 @"Bracket 0125" #3',args=[{type:'entity',kind:'note',id:'1'},{type:'entity',kind:'note',id:'2'},{type:'peer',id:'robot'},{type:'entity',kind:'note',id:'3'}];
 const bindings=args.map((arg,i)=>bind(input,i+1,arg));
 assert.deepEqual(parseCommand(input,bindings,inv,'local:test').args,args);
 assert.throws(()=>parseCommand(input,bindings.slice(0,3),inv,'local:test'),/autocomplete/);
 assert.throws(()=>parseCommand('/path #1',[bind('/path #1',1,args[0])],inv,'local:test'),/2–64/);
});
const chatInventory={...inventory,peers:[{id:'phone',name:"Dan's Phone"}],entities:[{id:'one',name:'Left shelf',kind:'volume'},{id:'two',name:'Left shelf',kind:'note'}]};
test('prose references preserve punctuation and exact IDs for duplicate names',()=>{
 const input=`How far is (@"Dan's Phone") from #"Left shelf"?`;
 const refs=mentionTokens(input),bindings=[{...refs[0],argument:{type:'peer',id:'phone'}},{...refs[1],argument:{type:'entity',kind:'note',id:'two'}}];
 assert.deepEqual(refs.map(t=>t.raw),[`@"Dan's Phone"`,'#"Left shelf"']);
 const content=prepareChat(input,bindings,chatInventory,'local:test');
 assert.ok(content.startsWith(input));
 const data=JSON.parse(content.split('\n').at(-1));
 assert.equal(data.frame,'home');assert.equal(data.references[1].id,'two');assert.equal(data.references[1].type,'entity');
 assert.equal(data.references[0].id,'phone');
});
test('prose references reject missing, edited, removed, wrong-kind and wrong-scene selections',()=>{
 const input='Where is #"Left shelf"?',t=mentionTokens(input)[0],b={...t,argument:{type:'entity',kind:'volume',id:'one'}};
 assert.throws(()=>prepareChat(input,[],chatInventory,'local:test'),/autocomplete/);
 assert.throws(()=>prepareChat(input,[b],chatInventory,'other'),/scene/);
 assert.throws(()=>prepareChat(input,[b],{...chatInventory,entities:[]},'local:test'),/no longer/);
 assert.throws(()=>prepareChat(input,[{...b,argument:{type:'peer',id:'phone'}}],chatInventory,'local:test'),/no longer/);
 const edited=input.replace('shelf','desk');
 assert.throws(()=>prepareChat(edited,rebaseBindings(input,edited,[b]),chatInventory,'local:test'),/autocomplete/);
 assert.throws(()=>prepareChat('Where is #"Left shelf',[],chatInventory,'local:test'),/autocomplete/);
});
test('ordinary prose and emails need no inventory; mention lexer handles escaped names',()=>{
 assert.equal(prepareChat('  Email dan@example.com please. ',[],null,'local:test'),'Email dan@example.com please.');
 assert.deepEqual(mentionTokens('foo@bar.com www.x/#anchor'),[]);
 assert.deepEqual(mentionTokens('Look at #cheese, then @Dan.').map(t=>t.value),['cheese','Dan']);
 const label='a "quoted" shelf';assert.equal(mentionTokens('See #'+JSON.stringify(label)+'!')[0].value,label);
});
