// Slash input is parsed locally into typed arguments. It never submits chat.
export function tokens(text) {
 const result=[];let i=0;
 while(i<text.length){
  if(/\s/.test(text[i])){i++;continue;}
  const start=i;let prefix='';if(text[i]==='#'||text[i]==='@')prefix=text[i++];
  let value='',complete=true;
  if(text[i]==='"'){
   const quoted=i++;let escaped=false,closed=false;
   for(;i<text.length;i++){const c=text[i];if(c==='"'&&!escaped){i++;closed=true;break;}escaped=c==='\\'&&!escaped;}
   complete=closed;
   try{value=JSON.parse(text.slice(quoted,i));}catch{value=text.slice(quoted+1,closed?i-1:i);complete=false;}
   if(i<text.length&&!/\s/.test(text[i])){while(i<text.length&&!/\s/.test(text[i]))i++;complete=false;}
  }else{const from=i;while(i<text.length&&!/\s/.test(text[i]))i++;value=text.slice(from,i);}
  result.push({start,end:i,prefix,value,complete,raw:text.slice(start,i)});
 }
 return result;
}
export function rebaseBindings(before,after,bindings) {
 let start=0;while(start<before.length&&start<after.length&&before[start]===after[start])start++;
 let oldEnd=before.length,newEnd=after.length;
 while(oldEnd>start&&newEnd>start&&before[oldEnd-1]===after[newEnd-1]){oldEnd--;newEnd--;}
 const delta=newEnd-oldEnd;
 return bindings.flatMap(b=>{
  if(b.end<=start)return [b];
  if(b.start>=oldEnd)return [{...b,start:b.start+delta,end:b.end+delta}];
  return [];
 }).filter(b=>after.slice(b.start,b.end)===b.raw);
}
function parameter(def,index){return def?.args[index]||(def?.args.at(-1)?.variadic?def.args.at(-1):null);}
function usage(def){return `/${def.name} ${def.args.map(a=>a.variadic?`<${a.name}> <${a.name}> …`:`<${a.name}>`).join(' ')}`;}
export function parseCommand(input,bindings,inventory,scene) {
 if(!inventory||inventory.scene!==scene)throw new Error('Command suggestions are not ready for this scene.');
 const list=tokens(input),command=list.shift()?.value.slice(1);
 const def=inventory.commands.find(c=>c.name===command);
 if(!def)throw new Error(command==='point'?'Physical pointing is not exposed by Home’s tools. No robot command was sent.':'Unknown slash command. Choose an operation from / suggestions.');
 const tail=def.args.at(-1),variable=tail?.variadic;
 if(variable){if(list.length<def.args.length-1+tail.min||list.length>def.args.length-1+tail.max)throw new Error(`Usage: ${usage(def)} (${tail.min}–${tail.max} waypoints)`);}
 else if(list.length!==def.args.length)throw new Error(`Usage: ${usage(def)}`);
 const args=list.map((t,i)=>{
  const param=parameter(def,i),kind=param.kind;
  if(!t.complete)throw new Error('Close the quoted argument first.');
  if(kind==='text'){if(t.prefix||!t.value.trim())throw new Error('Use quoted text for the name.');return {type:'text',value:t.value};}
  if(kind==='number'){
   if(t.prefix||!/^[-+]?(?:\d+(?:\.\d*)?|\.\d+)(?:e[-+]?\d+)?$/i.test(t.value)||!Number.isFinite(Number(t.value)))throw new Error(`${param.name} must be a number in metres.`);
   return {type:'number',value:Number(t.value)};
  }
  const bound=bindings.find(b=>b.start===t.start&&b.end===t.end&&b.raw===t.raw);
  if(!bound)throw new Error(`Select ${param.name} from autocomplete to confirm its identity.`);
  const arg=bound.argument;
  const allowed=kind==='camera'?arg.type==='camera':kind==='peer'?arg.type==='peer':kind==='volume'?arg.type==='entity'&&arg.kind==='volume':kind==='editable_entity'?arg.type==='entity'&&['note','volume'].includes(arg.kind):kind==='editable_target'?arg.type==='peer'||arg.type==='entity'&&['note','volume'].includes(arg.kind):['peer','entity'].includes(arg.type);
  if(!allowed)throw new Error(`Invalid ${param.name}; select a matching suggestion.`);
  return arg;
 });
 return {command,args,scene,frame:inventory.frame};
}

export function createSlashCommands({input,container,getScene,fetchInventory}) {
 const menu=document.createElement('div');menu.id='slash-suggestions';menu.className='slash-suggestions';menu.hidden=true;menu.setAttribute('role','listbox');menu.setAttribute('aria-label','Command suggestions');
 const hint=document.createElement('div');hint.className='slash-hint';hint.id='slash-hint';hint.hidden=true;hint.setAttribute('role','status');
 // The current agent bar nests its form inside the shell. Keep the popup on
 // the outer bar so it is not clipped or squeezed into the inline input row.
 container.append(menu,hint);
 input.setAttribute('role','combobox');input.setAttribute('aria-autocomplete','list');input.setAttribute('aria-controls',menu.id);input.setAttribute('aria-expanded','false');input.setAttribute('aria-describedby',hint.id);
 let inventory=null,bindings=[],previous=input.value,rows=[],selected=0,generation=0,loading=false,error='',dismissed=false,loadedAt=0;
 const close=()=>{menu.hidden=true;input.setAttribute('aria-expanded','false');input.removeAttribute('aria-activedescendant');};
 function reset(){generation++;inventory=null;bindings=[];loading=false;error='';previous=input.value;close();hint.hidden=true;}
 async function load(){
  const scene=getScene(),ticket=++generation;loading=true;error='';render();
  try{const next=await fetchInventory(scene);if(ticket!==generation||scene!==getScene())return;
   if(inventory?.frame!==next.frame)bindings=[];inventory=next;loadedAt=Date.now();
  }catch(e){if(ticket===generation){error=e.message;inventory=null;}}
  finally{if(ticket===generation){loading=false;render();}}
 }
 function context(){
  const all=tokens(input.value),cursor=input.selectionStart??input.value.length;
  let index=all.findIndex(t=>cursor>=t.start&&cursor<=t.end);
  if(index<0)index=all.filter(t=>t.end<cursor).length;
  return {all,index,token:all[index]||{start:cursor,end:cursor,value:'',prefix:''}};
 }
 function candidates(){
  if(!inventory||inventory.scene!==getScene())return [];
  const {all,index,token}=context(),query=token.value.toLowerCase();
  if(index===0)return inventory.commands.filter(c=>c.name.startsWith(query.replace(/^\//,''))).map(c=>({label:`/${c.name}`,detail:c.description,insert:`/${c.name}`,command:true}));
  const def=inventory.commands.find(c=>'/'+c.name===all[0]?.value),param=parameter(def,index-1);
  if(!param)return [];
  // After enough waypoints, don't consume Enter by inventing another selection.
  // Typing # or @ explicitly opens autocomplete for every additional waypoint.
  if(param.variadic&&!token.prefix&&!token.value&&index-1>=param.min)return [];
  const match=(name,id)=>`${name} ${id}`.toLowerCase().includes(query);
  let choices=[];
  if(['peer','target','editable_target'].includes(param.kind)&&token.prefix!=='@')choices.push(...inventory.peers.filter(p=>match(p.name,p.id)).map(p=>({label:'#'+p.name,detail:`${p.status||'discovered'} · ${p.id}`,insert:'#'+JSON.stringify(p.name),argument:{type:'peer',id:p.id}})));
  if(['target','editable_target','editable_entity','volume'].includes(param.kind)&&token.prefix!=='#')choices.push(...inventory.entities.filter(e=>(param.kind!=='volume'||e.kind==='volume')&&(!['editable_entity','editable_target'].includes(param.kind)||['note','volume'].includes(e.kind))&&match(e.name,e.id)).map(e=>({label:'@'+(e.name||e.id),detail:`${e.kind} · ${e.id}`,insert:'@'+JSON.stringify(e.name||e.id),argument:{type:'entity',kind:e.kind,id:e.id}})));
  if(param.kind==='camera'){
   const ref=bindings.find(b=>b.start===all[1]?.start&&b.end===all[1]?.end)?.argument;
   const peer=inventory.peers.find(p=>p.id===ref?.id);
   choices=(peer?.cameras||[]).filter(c=>match(c.product_id,'')&&(def.name!=='attach'||c.frame_id)).map(c=>({label:c.product_id,detail:c.frame_id||'Camera product',insert:JSON.stringify(c.product_id),argument:{type:'camera',reference:c.reference}}));
  }
  return choices.slice(0,30);
 }
 function choose(row){
  const {token}=context(),before=input.value,tail=before.slice(token.end),spacer=/^\s/.test(tail)?'':' ';
  input.value=before.slice(0,token.start)+row.insert+spacer+tail;
  bindings=rebaseBindings(before,input.value,bindings).filter(b=>b.start!==token.start);
  if(row.argument)bindings.push({start:token.start,end:token.start+row.insert.length,raw:row.insert,argument:row.argument});
  previous=input.value;input.setSelectionRange(token.start+row.insert.length+1,token.start+row.insert.length+1);selected=0;dismissed=false;
  input.dispatchEvent(new Event('input',{bubbles:true}));input.focus();render();
 }
 function render(){
  const slash=input.value.trimStart().startsWith('/');hint.hidden=!slash;
  if(!slash){close();return;}
  const command=tokens(input.value)[0]?.value.slice(1),def=inventory?.commands.find(c=>c.name===command);
  hint.textContent=error|| (loading?'Loading current scene and Catalog…':def?`Direct · ${usage(def)} · ${def.args.at(-1)?.variadic?'#/@ adds another waypoint; Send draws the path':'names with spaces use quotes'}`:'Direct commands · #peer · @entity · no Qwen');
  rows=candidates();selected=Math.min(selected,Math.max(0,rows.length-1));menu.replaceChildren();
  if(dismissed||!rows.length){close();return;}
  rows.forEach((row,i)=>{
   const item=document.createElement('div');item.id=`slash-option-${i}`;item.className='slash-option';item.setAttribute('role','option');item.setAttribute('aria-selected',String(i===selected));
   const title=document.createElement('strong'),detail=document.createElement('small');title.textContent=row.label;detail.textContent=row.detail;item.append(title,detail);
   item.addEventListener('pointerdown',e=>{e.preventDefault();choose(row);});menu.append(item);
  });
  menu.hidden=false;input.setAttribute('aria-expanded','true');input.setAttribute('aria-activedescendant',`slash-option-${selected}`);menu.children[selected]?.scrollIntoView({block:'nearest'});
 }
 input.addEventListener('input',()=>{
  bindings=rebaseBindings(previous,input.value,bindings);previous=input.value;selected=0;dismissed=false;
  if(input.value.trimStart().startsWith('/')&&!loading&&(!inventory||inventory.scene!==getScene()))load();else render();
 });
 input.addEventListener('focus',()=>{dismissed=false;if(input.value.trimStart().startsWith('/')&&!loading&&Date.now()-loadedAt>5000)load();else render();});
 input.addEventListener('click',()=>{dismissed=false;render();});
 input.addEventListener('blur',close);
 input.addEventListener('keydown',e=>{
  if(e.key==='Escape'&&!menu.hidden){e.preventDefault();e.stopPropagation();dismissed=true;close();return;}
  if(menu.hidden)return;
  if(e.key==='ArrowDown'||e.key==='ArrowUp'){e.preventDefault();selected=(selected+(e.key==='ArrowDown'?1:-1)+rows.length)%rows.length;render();}
  else if(e.key==='Tab'||e.key==='Enter'){e.preventDefault();choose(rows[selected]);}
 });
 return {
  prepare:()=>parseCommand(input.value,bindings,inventory,getScene()),
  submitted:()=>{bindings=[];previous=input.value;close();hint.hidden=true;},
  reset,
 };
}
