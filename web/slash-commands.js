// Shared completion: @peer and #scene node. Only slash input executes directly.
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
// Unlike command arguments, prose mentions can touch punctuation. A prefix
// inside a word/email is ordinary text, not a reference.
export function mentionTokens(text){
 const result=[],pattern=/(^|[\s([{])([@#])/g;let match;
 while((match=pattern.exec(text))){
  const start=match.index+match[1].length,prefix=match[2];let end=pattern.lastIndex,value='',complete=true;
  if(text[end]==='"'){
   const from=end++;let escaped=false,closed=false;
   while(end<text.length){const c=text[end++];if(c==='"'&&!escaped){closed=true;break;}escaped=c==='\\'&&!escaped;}
   complete=closed;
   try{value=JSON.parse(text.slice(from,end));}catch{value=text.slice(from+1,closed?end-1:end);complete=false;}
  }else{const from=end;while(end<text.length&&!/[\s.,!?;:()[\]{}"]/.test(text[end]))end++;value=text.slice(from,end);}
  result.push({start,end,prefix,value,complete,raw:text.slice(start,end)});pattern.lastIndex=end;
 }
 return result;
}
export function prepareChat(input,bindings,inventory,scene){
 const mentions=mentionTokens(input);
 if(!mentions.length)return input.trim();
 if(!inventory||inventory.scene!==scene)throw Error('Reference suggestions are not ready for this scene.');
 const references=mentions.map(t=>{
  const b=bindings.find(b=>b.start===t.start&&b.end===t.end&&b.raw===t.raw);
  if(!t.complete||!b)throw Error(`Select ${t.raw} from autocomplete to confirm its identity.`);
  const a=b.argument;
  const valid=t.prefix==='@'?a.type==='peer'&&inventory.peers.some(p=>p.id===a.id):a.type==='entity'&&inventory.entities.some(e=>e.id===a.id&&e.kind===a.kind);
  if(!valid)throw Error('Reference is no longer available. Select it again from autocomplete.');
  return {text:t.raw,...a};
 });
 // Keep readable text in the UI, but retain exact selected identities in the
 // user message sent to Qwen (and subsequent conversation history). These are
 // references, not trusted state or instructions; tools still validate targets.
 return input.trim()+'\n\nSelected references (identity only; use scene tools for current state):\n'+JSON.stringify({scene,frame:inventory.frame,references});
}
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
  if((arg.type==='peer'&&t.prefix!=='@')||(arg.type==='entity'&&t.prefix!=='#'))throw new Error('Use @ for peers and # for scene nodes.');
  const allowed=kind==='camera'?arg.type==='camera':kind==='peer'?arg.type==='peer':kind==='volume'?arg.type==='entity'&&arg.kind==='volume':kind==='editable_entity'?arg.type==='entity'&&['note','volume'].includes(arg.kind):kind==='editable_target'?arg.type==='peer'||arg.type==='entity'&&['note','volume'].includes(arg.kind):['peer','entity'].includes(arg.type);
  if(!allowed)throw new Error(`Invalid ${param.name}; select a matching suggestion.`);
  return arg;
 });
 return {command,args,scene,frame:inventory.frame};
}

export function createSlashCommands({input,container,getScene,fetchInventory}) {
 const menu=document.createElement('div');menu.id='slash-suggestions';menu.className='slash-suggestions';menu.hidden=true;menu.setAttribute('role','listbox');menu.setAttribute('aria-label','Command and reference suggestions');
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
  const slash=input.value.trimStart().startsWith('/'),cursor=input.selectionStart??input.value.length;
  const all=slash?tokens(input.value):mentionTokens(input.value);
  if(!slash)return {all,index:-1,token:all.find(t=>cursor>=t.start&&cursor<=t.end)||{start:cursor,end:cursor,value:'',prefix:''}};
  let index=all.findIndex(t=>cursor>=t.start&&cursor<=t.end);
  if(index<0)index=all.filter(t=>t.end<cursor).length;
  return {all,index,token:all[index]||{start:cursor,end:cursor,value:'',prefix:''}};
 }
 function candidates(){
  if(!inventory||inventory.scene!==getScene())return [];
  const {all,index,token}=context(),query=token.value.toLowerCase();
  if(index===0)return inventory.commands.filter(c=>c.name.startsWith(query.replace(/^\//,''))).map(c=>({label:`/${c.name}`,detail:c.description,insert:`/${c.name}`,command:true}));
  if(index===-1&&!token.prefix)return [];
  const def=inventory.commands.find(c=>'/'+c.name===all[0]?.value),param=parameter(def,index-1);
  const kind=index===-1?'target':param?.kind;
  if(!kind)return [];
  // After enough waypoints, don't consume Enter by inventing another selection.
  // Typing # or @ explicitly opens autocomplete for every additional waypoint.
  if(param?.variadic&&!token.prefix&&!token.value&&index-1>=param.min)return [];
  const match=(name,id)=>`${name} ${id}`.toLowerCase().includes(query);
  let choices=[];
  if(['peer','target','editable_target'].includes(kind)&&token.prefix!=='#')choices.push(...inventory.peers.filter(p=>match(p.name,p.id)).map(p=>({label:'@'+p.name,detail:`${p.status||'discovered'} · ${p.id}`,insert:'@'+JSON.stringify(p.name),argument:{type:'peer',id:p.id}})));
  if(['target','editable_target','editable_entity','volume'].includes(kind)&&token.prefix!=='@')choices.push(...inventory.entities.filter(e=>(kind!=='volume'||e.kind==='volume')&&(!['editable_entity','editable_target'].includes(kind)||['note','volume'].includes(e.kind))&&match(e.name,e.id)).map(e=>({label:'#'+(e.name||e.id),detail:`${e.kind} · ${e.id}`,insert:'#'+JSON.stringify(e.name||e.id),argument:{type:'entity',kind:e.kind,id:e.id}})));
  if(kind==='camera'){
   const ref=bindings.find(b=>b.start===all[1]?.start&&b.end===all[1]?.end)?.argument;
   const peer=inventory.peers.find(p=>p.id===ref?.id);
   choices=(peer?.cameras||[]).filter(c=>match(c.product_id,'')&&(def.name!=='attach'||c.frame_id)).map(c=>({label:c.product_id,detail:c.frame_id||'Camera product',insert:JSON.stringify(c.product_id),argument:{type:'camera',reference:c.reference}}));
  }
  return choices.slice(0,30);
 }
 function choose(row){
  const {token}=context(),before=input.value,tail=before.slice(token.end),spacer=/^[\s.,!?;:)\]}]/.test(tail)?'':' ';
  input.value=before.slice(0,token.start)+row.insert+spacer+tail;
  bindings=rebaseBindings(before,input.value,bindings).filter(b=>b.start!==token.start);
  if(row.argument)bindings.push({start:token.start,end:token.start+row.insert.length,raw:row.insert,argument:row.argument});
  const caret=token.start+row.insert.length+(spacer||/^\s/.test(tail)?1:0);
  previous=input.value;input.setSelectionRange(caret,caret);selected=0;dismissed=false;
  input.dispatchEvent(new Event('input',{bubbles:true}));input.focus();render();
 }
 function render(){
  const slash=input.value.trimStart().startsWith('/'),mention=!!context().token.prefix;hint.hidden=!slash&&!mention;
  if(!slash&&!mention){close();return;}
  const command=tokens(input.value)[0]?.value.slice(1),def=inventory?.commands.find(c=>c.name===command);
  hint.textContent=error|| (loading?'Loading current scene and Catalog…':!slash?'Qwen · @peer · #scene node · select to bind its identity':def?`Direct · ${usage(def)} · ${def.args.at(-1)?.variadic?'@/# adds another waypoint; Send draws the path':'@peer · #scene node · names with spaces use quotes'}`:'Direct commands · @peer · #scene node · no Qwen');
  rows=candidates();selected=Math.min(selected,Math.max(0,rows.length-1));menu.replaceChildren();
  if(dismissed||!rows.length||document.activeElement!==input){close();return;}
  rows.forEach((row,i)=>{
   const item=document.createElement('div');item.id=`slash-option-${i}`;item.className='slash-option';item.setAttribute('role','option');item.setAttribute('aria-selected',String(i===selected));
   const title=document.createElement('strong'),detail=document.createElement('small');title.textContent=row.label;detail.textContent=row.detail;item.append(title,detail);
   item.addEventListener('pointerdown',e=>{e.preventDefault();choose(row);});menu.append(item);
  });
  menu.hidden=false;input.setAttribute('aria-expanded','true');input.setAttribute('aria-activedescendant',`slash-option-${selected}`);menu.children[selected]?.scrollIntoView({block:'nearest'});
 }
 input.addEventListener('input',()=>{
  bindings=rebaseBindings(previous,input.value,bindings);previous=input.value;selected=0;dismissed=false;
  if((input.value.trimStart().startsWith('/')||context().token.prefix)&&!loading&&(!inventory||inventory.scene!==getScene()||Date.now()-loadedAt>5000))load();else render();
 });
 const refresh=()=>{dismissed=false;if((input.value.trimStart().startsWith('/')||context().token.prefix)&&!loading&&(!inventory||inventory.scene!==getScene()||Date.now()-loadedAt>5000))load();else render();};
 input.addEventListener('focus',refresh);
 input.addEventListener('click',refresh);
 input.addEventListener('keyup',e=>{if(['ArrowLeft','ArrowRight','Home','End'].includes(e.key))refresh();});
 input.addEventListener('blur',close);
 input.addEventListener('keydown',e=>{
  if(e.isComposing)return;
  if(e.key==='Escape'&&!menu.hidden){e.preventDefault();e.stopPropagation();dismissed=true;close();return;}
  if(menu.hidden)return;
  if(e.key==='ArrowDown'||e.key==='ArrowUp'){e.preventDefault();selected=(selected+(e.key==='ArrowDown'?1:-1)+rows.length)%rows.length;render();}
  else if(e.key==='Tab'||e.key==='Enter'){e.preventDefault();choose(rows[selected]);}
 });
 return {
  prepare:()=>parseCommand(input.value,bindings,inventory,getScene()),
  prepareChat:()=>prepareChat(input.value,bindings,inventory,getScene()),
  draft:()=>({text:input.value,bindings:bindings.slice(),scene:getScene(),frame:inventory?.frame}),
  restore:draft=>{if(draft.scene!==getScene())return;input.value=draft.text;previous=input.value;bindings=draft.frame===inventory?.frame?draft.bindings:[];dismissed=true;render();},
  submitted:()=>{bindings=[];previous=input.value;close();hint.hidden=true;},
  reset,
 };
}
