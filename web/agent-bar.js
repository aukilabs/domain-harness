// Bottom bar: scene tools plus the scene assistant. "Ask" morphs the toolbar into the chat input and
// raises the conversation sheet above it. The chat itself (sending, history) stays in app.js; this
// module owns the bar's modes, the tool buttons and the motion. Styles live in style.css under "Agent bar".
import { icon, icons } from './panel-ui.js';

const reduceMotion=()=>matchMedia('(prefers-reduced-motion: reduce)').matches;
const spring='cubic-bezier(.3,1.3,.5,1)';
const el=(tag,className,text)=>{const node=document.createElement(tag);if(className)node.className=className;if(text!==undefined)node.textContent=text;return node;};
export const orb=(size='')=>{const node=el('span','agent-orb'+(size?' '+size:''));node.setAttribute('aria-hidden','true');return node;};

// tools: [{label, iconPath, accent, source, onTrigger, shortcut}]. Each tool mirrors `source`'s disabled
// and aria-pressed state, so the panel buttons and the bar always agree.
export function createAgentBar({root,tools,onOpen,hasConversation=()=>true}){
 const shell=root.querySelector('.agent-shell'),toolbar=root.querySelector('.agent-tools'),toggle=root.querySelector('#agent-toggle'),
  form=root.querySelector('#scene-chat-form'),input=root.querySelector('#scene-chat-input'),close=root.querySelector('#agent-close'),sheet=root.querySelector('.agent-sheet');
 const tip=el('div','rail-tooltip tip-top');tip.setAttribute('aria-hidden','true');tip.hidden=true;document.body.append(tip);
 let tipTimer=0;
 const showTip=button=>{clearTimeout(tipTimer);const show=()=>{const label=button.dataset.tip||button.getAttribute('aria-label');tip.replaceChildren(label);
   if(button.dataset.shortcut){const key=el('kbd','',button.dataset.shortcut);tip.append(key);}
   const box=button.getBoundingClientRect();tip.style.left=(box.left+box.width/2)+'px';tip.style.top=(box.top-10)+'px';tip.hidden=false;};
  if(!tip.hidden)show();else tipTimer=setTimeout(show,300);};
 const hideTip=()=>{clearTimeout(tipTimer);tip.hidden=true;};
 root.addEventListener('pointerover',e=>{const button=e.target.closest('[data-tip],.agent-tool');if(button&&shell.contains(button))showTip(button);});
 root.addEventListener('pointerout',e=>{if(!e.relatedTarget||!e.relatedTarget.closest?.('[data-tip],.agent-tool'))hideTip();});
 root.addEventListener('focusin',e=>{const button=e.target.closest('[data-tip],.agent-tool');if(button&&e.target.matches(':focus-visible'))showTip(button);});
 root.addEventListener('focusout',hideTip);
 root.addEventListener('pointerdown',hideTip);

 for(const tool of tools){
  const button=el('button','agent-tool');button.type='button';button.dataset.accent=tool.accent;button.setAttribute('aria-label',tool.label);
  if(tool.shortcut)button.dataset.shortcut=tool.shortcut;
  const face=el('span','rail-face');face.append(icon(tool.iconPath,20));const glint=el('span','rail-glint');glint.setAttribute('aria-hidden','true');face.append(glint);button.append(face);
  button.onclick=()=>{tool.onTrigger();button.classList.remove('pulse');void button.offsetWidth;button.classList.add('pulse');};
  const sync=()=>{button.disabled=!!tool.source.disabled;button.setAttribute('aria-pressed',tool.source.getAttribute('aria-pressed')==='true'?'true':'false');
   const reason=tool.source.disabled?tool.disabledReason?.():'';button.dataset.tip=reason?`${tool.label} · ${reason}`:tool.label;};
  new MutationObserver(sync).observe(tool.source,{attributes:true,attributeFilter:['disabled','aria-pressed']});sync();
  toolbar.append(button);
 }
 toolbar.addEventListener('animationend',e=>{if(e.animationName==='rail-wash')e.target.closest('.agent-tool')?.classList.remove('pulse');});
 // Arrow keys move between tools (one tab stop for the toolbar).
 toolbar.addEventListener('keydown',e=>{if(!['ArrowLeft','ArrowRight','Home','End'].includes(e.key))return;
  const items=[...toolbar.querySelectorAll('.agent-tool:not(:disabled)')];const at=items.indexOf(document.activeElement);if(at<0)return;e.preventDefault();
  const next=e.key==='Home'?0:e.key==='End'?items.length-1:(at+(e.key==='ArrowRight'?1:-1)+items.length)%items.length;items[next].focus();});

 const isOpen=()=>root.dataset.mode==='agent';
 // FLIP: measure, switch mode, then animate the shell's width and fly the orb to its new place.
 function morph(mode,done){
  if(root.dataset.mode===mode){done?.();return;}
  const movingOrb=toggle.querySelector('.agent-orb'),from=shell.getBoundingClientRect(),orbFrom=movingOrb.getBoundingClientRect();
  const leaving=[...shell.querySelectorAll(mode==='agent'?'.agent-tools,.agent-divider,.agent-ask-label,.agent-ask-key':'#scene-chat-form,.agent-shell-actions')];
  const swap=()=>{
   root.dataset.mode=mode;toggle.setAttribute('aria-expanded',String(mode==='agent'));done?.();
   if(reduceMotion())return;
   const to=shell.getBoundingClientRect(),orbTo=movingOrb.getBoundingClientRect();
   shell.animate([{width:from.width+'px',height:from.height+'px'},{width:to.width+'px',height:to.height+'px'}],{duration:560,easing:spring});
   const dx=(orbFrom.left-from.left)-(orbTo.left-to.left);
   movingOrb.animate([{transform:`translateX(${dx}px) scale(.9)`},{transform:'none'}],{duration:620,easing:spring});
   const entering=[...shell.querySelectorAll(mode==='agent'?'#scene-chat-input,.agent-shell-actions>*':'.agent-tool,.agent-ask-label,.agent-ask-key')];
   entering.forEach((node,i)=>node.animate([{opacity:0,filter:'blur(6px)',transform:'translateY(6px) scale(.96)'},{opacity:1,filter:'none',transform:'none'}],{duration:380,delay:120+i*40,easing:'cubic-bezier(.2,.8,.2,1)',fill:'backwards'}));
  };
  if(reduceMotion()){swap();return;}
  Promise.all(leaving.map(node=>node.animate([{opacity:1},{opacity:0,filter:'blur(4px)',transform:'scale(.96)'}],{duration:110,easing:'ease-in'}).finished)).then(swap);
 }
 function open(){if(isOpen()){input.focus();return;}morph('agent',()=>{showSheet();input.focus({preventScroll:true});});onOpen?.();}
 function closeAgent(){if(!isOpen())return;hideSheet(()=>{morph('tools');toggle.focus({preventScroll:true});});}

 // The sheet rises out of the bar and settles with a little spring.
 function showSheet(){if(!sheetWanted())return;sheet.hidden=false;if(!reduceMotion())sheet.animate([{opacity:0,transform:'translateY(16px) scale(.97)',filter:'blur(8px)'},{opacity:1,transform:'none',filter:'none'}],{duration:520,delay:140,easing:spring,fill:'backwards'});}
 function hideSheet(done){if(sheet.hidden||reduceMotion()){sheet.hidden=true;done?.();return;}
  sheet.animate([{opacity:1,transform:'none'},{opacity:0,transform:'translateY(12px) scale(.97)',filter:'blur(6px)'}],{duration:180,easing:'cubic-bezier(.4,0,1,1)'}).finished.then(()=>{sheet.hidden=true;done?.();});}
 // The sheet only appears once there is a conversation to show; Ask alone opens just the input.
 const sheetWanted=()=>isOpen()&&!root.classList.contains('compact')&&hasConversation();
 function syncSheet(){if(sheetWanted()&&sheet.hidden)showSheet();else if(!sheetWanted()&&!sheet.hidden)hideSheet();}

 toggle.onclick=open;close.onclick=closeAgent;
 // Escape closes from the bar, or from nowhere in particular (focus can fall to the page after sending).
 // The bar stops key events from reaching the scene, so it needs its own listener as well as the page's.
 root.addEventListener('keydown',e=>{if(e.key==='Escape'&&isOpen()){e.preventDefault();closeAgent();}});
 document.addEventListener('keydown',e=>{if(e.key==='Escape'&&isOpen()&&e.target===document.body){e.preventDefault();closeAgent();}});
 // "/" opens the assistant from anywhere that isn't a text field.
 document.addEventListener('keydown',e=>{if(e.key!=='/'||e.metaKey||e.ctrlKey||e.altKey||e.target.closest?.('input,textarea,select,[contenteditable="true"]'))return;e.preventDefault();open();});
 root.dataset.mode='tools';sheet.hidden=true;
 return {open,close:closeAgent,isOpen,syncSheet};
}
