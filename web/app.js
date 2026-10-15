import * as THREE from 'three';
import { OrbitControls } from 'three/addons/controls/OrbitControls.js';
import { createArOverlay } from './ar.js';
import { createPhoneTrails, cameraShape } from './phone-trails.js';
import { createCompactPoseTrail } from './compact-pose-trail.js';
import { createSemanticVolumes, legacyVolumesForView } from './semantic-volumes.js';
import { createMap2D } from './map-2d.js';
import { createCrayon } from './crayon.js';
import { createGhostCamera, createCameraPan, isEditing } from './ghost-camera.js';
import { createNoteBubble } from './note-bubble.js';
import { createNoteLod } from './note-lod.js';
import { createPipWindow } from './pip-window.js';
import { createVideoPresentation } from './video-presentation.js';
import { readCameraResponse } from './camera-frame.js';
import { icons, icon, deviceIcon, sectionHeader, disclosure, statusKind, statusBadge, listRow, searchField, copyButton, emptyState, actionButton, formField, toast, reveal, conceal, animateDetails, markSelected, tactile } from './panel-ui.js';
animateDetails(document);
const plural=(count,word)=>`${count} ${word}${count===1?'':'s'}`;
import { moveMapCamera, applyChatActions, chatResultPresentation } from './view-camera.js';
import { createAttachedCamera } from './attached-camera.js';
import { createRobotModel } from './robot-model.js';
import { applyRobotHealth, applyPointingStatus } from './robot-health.js';
import { createAgentBar } from './agent-bar.js';
const $=id=>document.getElementById(id);
const chatForm=$('scene-chat-form'),chatInput=$('scene-chat-input'),chatStatus=$('scene-chat-status');
chatForm.querySelector('button').disabled=true;
const chatHistory=$('scene-chat-history'),chatConversations=new Map();let chatPending=false;
let chatCompact=false;try{chatCompact=localStorage.getItem('home.chat.compact')==='true';}catch{}
let agentBar=null,setPlanBar=()=>{};
function syncChatCompact(){document.querySelector('.scene-chat').classList.toggle('compact',chatCompact);const toggle=$('scene-chat-toggle');toggle.dataset.tip=chatCompact?'Show conversation':'Hide conversation';toggle.setAttribute('aria-label',toggle.dataset.tip);toggle.setAttribute('aria-expanded',String(!chatCompact));agentBar?.syncSheet();}
$('scene-chat-toggle').onclick=()=>{chatCompact=!chatCompact;try{localStorage.setItem('home.chat.compact',String(chatCompact));}catch{}syncChatCompact();};syncChatCompact();
// Messages already shown keep still; only new ones animate in. The thinking row stands in for a pending reply.
let chatShown={map:null,count:0};
function renderChat(){
 const messages=chatConversations.get(activeMap)||[],pending=chatPending&&chatPendingMap===activeMap;
 const fresh=chatShown.map===activeMap?chatShown.count:messages.length;chatShown={map:activeMap,count:messages.length};
 chatHistory.replaceChildren();chatHistory.hidden=!messages.length&&!pending;
 messages.forEach((m,i)=>{const row=document.createElement('div');row.className='scene-chat-message '+m.role+(i>=fresh?' enter':'');
  const who=document.createElement('strong');who.className='visually-hidden';who.textContent=m.role==='user'?'You':'Qwen';
  // The "nothing was saved" receipt stays with the reply, as a quiet note rather than body text.
  const shown=m.display_content||m.content,receipt=m.role==='user'?null:shown.match(/\n\nNo scene changes were saved\.$/);
  const text=document.createElement('div');text.className='scene-chat-text';text.textContent=receipt?shown.slice(0,receipt.index):shown;row.append(who,text);
  if(receipt){const note=document.createElement('div');note.className='scene-chat-receipt';note.textContent='No scene changes were saved.';row.append(note);}
  chatHistory.append(row);});
 if(pending){const row=document.createElement('div');row.className='scene-chat-message assistant thinking enter';const text=document.createElement('div');text.className='scene-chat-text';text.textContent='Thinking…';row.append(text);chatHistory.append(row);}
 document.querySelector('.scene-chat').classList.toggle('pending',pending);
 chatHistory.scrollTop=chatHistory.scrollHeight;
 document.querySelector('.agent-bar').classList.toggle('has-conversation',!!messages.length||pending);agentBar?.syncSheet();
}
let chatPendingMap=null;
$('agent-new').onclick=()=>{if(chatPending)return;chatConversations.delete(activeMap);renderChat();chatStatus.textContent='Local Qwen · inspect scene and edit volume descriptions';chatInput.focus();};
chatInput.addEventListener('input',()=>{chatForm.querySelector('button').disabled=chatPending||!chatInput.value.trim();});
chatForm.onsubmit=async e=>{
 e.preventDefault();if(chatPending||!chatInput.value.trim())return;
 // Sending while the conversation is minimised brings it back, so the reply isn't hidden.
 if(chatCompact){chatCompact=false;try{localStorage.setItem('home.chat.compact','false');}catch{}syncChatCompact();}
 const map=activeMap,message=chatInput.value.trim(),history=(chatConversations.get(map)||[]).slice(-18);
 history.push({role:'user',content:message});
 if(history.reduce((n,m)=>n+m.content.length,0)>32000){chatStatus.textContent='Conversation is too long. Start a shorter conversation.';toast(chatStatus.textContent);return;}
 chatConversations.set(map,history);chatPending=true;chatPendingMap=map;renderChat();chatInput.value='';chatForm.querySelector('button').disabled=true;chatStatus.textContent='Qwen is thinking…';
 try{
  const result=await api('/api/chat',{messages:history.map(({role,content})=>({role,content})),scene:map});
  const browser=result.ui_actions===undefined?null:await applyChatCamera(result.ui_actions,map);
  const presentation=chatResultPresentation(result,browser);
  history.push({role:'assistant',content:presentation.content,display_content:presentation.display_content});
  chatStatus.textContent=presentation.status;
  if(result.changes?.length){
   // A failed refresh cannot undo saved receipts or offer the whole edit again.
   lastScene=null;try{await refresh();}catch(error){chatStatus.textContent+=' · View refresh failed: '+error.message;}
  }
 }catch(error){history.pop();if(activeMap===map&&!chatInput.value)chatInput.value=message;chatStatus.textContent=error.message;toast(error.message);}
 finally{chatPending=false;chatPendingMap=null;renderChat();if(document.activeElement===document.body||document.activeElement?.closest?.('.agent-bar'))chatInput.focus({preventScroll:true});chatForm.querySelector('button').disabled=!chatInput.value.trim();}
};
$('map-select').addEventListener('change',()=>{renderChat();chatStatus.textContent=chatPending?'A local reply is pending for the previous scene…':'Local Qwen · inspect scene and edit volume descriptions';});
for(const event of ['pointerdown','pointerup','click','dblclick','contextmenu','wheel','keydown','keyup']){
 document.querySelector('.scene-chat').addEventListener(event,e=>e.stopPropagation());
}
// Organize existing live controls into a labelled icon rail and exclusive panels.
const sidebar=document.querySelector('aside'),shell=document.querySelector('.shell');
const rail=document.createElement('nav');rail.className='icon-rail';rail.setAttribute('aria-label','Workspace panels');shell.prepend(rail);
const panelNames={maps:'Maps',notes:'Notes',volumes:'Volumes',tags:'Tags',fleet:'Devices',settings:'Settings'};
const panelAccents={maps:'emerald',notes:'amber',volumes:'violet',tags:'pink',fleet:'cyan',settings:'zinc'};
const iconPaths={maps:'M2 4l5-2 4 2 5-2v12l-5 2-4-2-5 2ZM7 2v12M11 4v12',notes:'M4 2h10v14H4ZM6 6h6M6 9h6M6 12h4',tags:'M2 2h6.5l7.5 7.5-6.5 6.5L2 8.5ZM5.5 5.5h.01',fleet:'M4 6h10v9H4ZM6 2h6v4M7 9h.1M11 9h.1M7 12h4M1 10h3M14 10h3',settings:'M2 5h8M14 5h2M2 13h2M8 13h8M10 5a2 2 0 1 0 4 0 2 2 0 1 0-4 0M4 13a2 2 0 1 0 4 0 2 2 0 1 0-4 0'};
const panelBodies={};let activePanel=null,panelTarget=0;
{const sceneBar=document.createElement('div');sceneBar.className='scene-bar';sceneBar.append(document.querySelector('.viewport-heading'),document.querySelector('.view-controls'));document.querySelector('.viewport').append(sceneBar);
 const toggle=$('top');toggle.classList.add('mode-toggle');toggle.innerHTML='<span>3D</span><span>2D</span>';}
iconPaths.volumes='M2 5l7-3 7 3v8l-7 3-7-3ZM2 5l7 3 7-3M9 8v8';
const railIcon=(path,weight=1.5)=>`<svg aria-hidden="true" width="20" height="20" viewBox="0 0 18 18" fill="none" stroke="currentColor" stroke-width="${weight}" stroke-linecap="round" stroke-linejoin="round"><path d="${path}"/></svg>`;
// The McKenna mark heads the dock; the floating header keeps only status, scene tools and the account.
const railLogo=document.createElement('a');railLogo.className='rail-logo';railLogo.href='/';railLogo.setAttribute('aria-label','McKenna home');// The McKenna mark: a colour version over a grey one, revealed along a traced pen route (see style.css).
railLogo.innerHTML='<svg viewBox="0 0 231 118" aria-hidden="true"><defs><linearGradient id="mark-hue" gradientUnits="userSpaceOnUse" x1="0" y1="0" x2="231" y2="118"><stop offset="0.00" stop-color="#10b981"/><stop offset="0.20" stop-color="#3b82f6"/><stop offset="0.40" stop-color="#8b5cf6"/><stop offset="0.60" stop-color="#ec4899"/><stop offset="0.80" stop-color="#f59e0b"/><stop offset="1.00" stop-color="#06b6d4"/><animateTransform attributeName="gradientTransform" type="rotate" from="0 115 59" to="360 115 59" dur="10s" repeatCount="indefinite"/></linearGradient><mask id="mark-pen" maskUnits="userSpaceOnUse" x="-20" y="-20" width="271" height="158"><path class="mark-pen" d="M4 50C12 30 30 6 46 6C52 6 54 12 50 30C44 55 30 85 34 113C38 95 48 70 58 52C75 25 95 5 110 5C118 5 122 12 118 25C110 55 96 90 100 113C106 95 115 70 126 52C145 25 162 5 176 5C186 5 190 12 186 30C178 60 168 90 174 106C178 116 188 112 198 100C212 85 222 65 228 52" pathLength="1" fill="none" stroke="#fff" stroke-width="34" stroke-linecap="round" stroke-linejoin="round"/></mask></defs><path class="mark-base" d="M44.3657 0.0950353C46.975 -0.280215 50.568 0.523793 53.0372 1.31804C65.3195 5.26779 65.4902 15.351 63.0022 26.139C61.1842 34.0218 58.8415 41.4885 56.1087 49.0798C65.6655 32.6545 84.049 9.78554 101.899 2.31954C107.802 -0.149712 114.08 -0.764712 120.087 1.73329C123.845 3.29604 127.782 5.6543 129.348 9.6203C133.205 19.384 124.303 40.7483 120.279 49.8878C122.834 47.1308 126.484 41.4368 129.196 38.155C140.357 24.651 154.131 9.17354 170.484 2.23754C176.469 -0.137963 182.828 -0.913212 188.816 1.81954C198.433 6.20979 198.806 14.356 196.806 23.378C195.741 28.0323 194.341 32.6028 192.613 37.0533C187.681 49.9518 169.454 87.7148 174.308 99.5728C175.117 101.55 176.217 102.406 178.156 103.223C193.258 102.005 209.268 76.0905 215.678 63.8703C217.238 60.8693 218.706 57.8208 220.078 54.7295C220.913 52.8038 223.121 46.67 224.658 46.2288C230.816 44.4623 231.721 49.3138 230.348 53.4903C222.953 75.9843 192.981 127.158 163.411 115.368C158.162 113.221 153.986 109.07 151.806 103.835C145.191 88.239 154.445 65.3803 160.272 50.9063C164.921 39.422 171.766 25.9425 174.335 14.1935C166.294 16.7703 157.646 23.8343 151.66 29.8013C133.91 47.4945 117.948 70.3805 109.417 94.0003C107.829 98.3965 105.971 105.906 105.93 110.482C104.078 112.38 102.118 113.468 99.4557 112.966C83.3645 109.935 81.0105 98.16 83.483 83.9145C86.3002 67.681 93.3437 52.728 99.2995 37.576C101.989 30.7318 106.097 22.0493 106.863 15.0548C101.127 16.4648 94.7292 21.4115 90.3262 25.5003C70.9725 43.6363 56.6862 67.421 46.908 91.9313C43.937 99.3785 42.6125 107.748 40.2237 115.318C39.3235 116.509 36.5507 117.035 35.3475 117.004C7.63623 116.312 18.4755 84.888 23.417 68.8513C26.8595 57.6783 30.9715 48.1453 34.7202 37.3653C37.553 29.2193 42.073 19.097 41.9872 10.7438C29.6255 15.914 18.082 27.0875 12.037 39.195C10.7577 41.7575 10.1167 46.4215 8.47448 48.5588C7.96973 49.2158 5.27648 49.0088 4.26873 48.973C2.19073 47.6003 0.033724 45.8583 0.000973951 43.1483C-0.094526 35.2535 6.85372 26.1438 11.7555 20.5168C20.1292 10.9038 31.1815 1.21304 44.3657 0.0950353Z"/><path class="mark-ink" d="M44.3657 0.0950353C46.975 -0.280215 50.568 0.523793 53.0372 1.31804C65.3195 5.26779 65.4902 15.351 63.0022 26.139C61.1842 34.0218 58.8415 41.4885 56.1087 49.0798C65.6655 32.6545 84.049 9.78554 101.899 2.31954C107.802 -0.149712 114.08 -0.764712 120.087 1.73329C123.845 3.29604 127.782 5.6543 129.348 9.6203C133.205 19.384 124.303 40.7483 120.279 49.8878C122.834 47.1308 126.484 41.4368 129.196 38.155C140.357 24.651 154.131 9.17354 170.484 2.23754C176.469 -0.137963 182.828 -0.913212 188.816 1.81954C198.433 6.20979 198.806 14.356 196.806 23.378C195.741 28.0323 194.341 32.6028 192.613 37.0533C187.681 49.9518 169.454 87.7148 174.308 99.5728C175.117 101.55 176.217 102.406 178.156 103.223C193.258 102.005 209.268 76.0905 215.678 63.8703C217.238 60.8693 218.706 57.8208 220.078 54.7295C220.913 52.8038 223.121 46.67 224.658 46.2288C230.816 44.4623 231.721 49.3138 230.348 53.4903C222.953 75.9843 192.981 127.158 163.411 115.368C158.162 113.221 153.986 109.07 151.806 103.835C145.191 88.239 154.445 65.3803 160.272 50.9063C164.921 39.422 171.766 25.9425 174.335 14.1935C166.294 16.7703 157.646 23.8343 151.66 29.8013C133.91 47.4945 117.948 70.3805 109.417 94.0003C107.829 98.3965 105.971 105.906 105.93 110.482C104.078 112.38 102.118 113.468 99.4557 112.966C83.3645 109.935 81.0105 98.16 83.483 83.9145C86.3002 67.681 93.3437 52.728 99.2995 37.576C101.989 30.7318 106.097 22.0493 106.863 15.0548C101.127 16.4648 94.7292 21.4115 90.3262 25.5003C70.9725 43.6363 56.6862 67.421 46.908 91.9313C43.937 99.3785 42.6125 107.748 40.2237 115.318C39.3235 116.509 36.5507 117.035 35.3475 117.004C7.63623 116.312 18.4755 84.888 23.417 68.8513C26.8595 57.6783 30.9715 48.1453 34.7202 37.3653C37.553 29.2193 42.073 19.097 41.9872 10.7438C29.6255 15.914 18.082 27.0875 12.037 39.195C10.7577 41.7575 10.1167 46.4215 8.47448 48.5588C7.96973 49.2158 5.27648 49.0088 4.26873 48.973C2.19073 47.6003 0.033724 45.8583 0.000973951 43.1483C-0.094526 35.2535 6.85372 26.1438 11.7555 20.5168C20.1292 10.9038 31.1815 1.21304 44.3657 0.0950353Z" fill="url(#mark-hue)" mask="url(#mark-pen)"/></svg>';rail.append(railLogo);if(matchMedia('(prefers-reduced-motion: reduce)').matches)railLogo.querySelector('animateTransform')?.remove();
const railTabs=document.createElement('div');railTabs.className='rail-tabs';rail.append(railTabs);
for(const [id,name] of Object.entries(panelNames)){
 const button=document.createElement('button');button.type='button';button.className='rail-tab';button.dataset.panel=id;button.dataset.accent=panelAccents[id];button.setAttribute('aria-label',name);button.setAttribute('aria-controls','panel-'+id);
 button.innerHTML=`<span class="rail-face">${railIcon(iconPaths[id],1.35)}<span class="rail-glint" aria-hidden="true"></span></span>`;
 button.onclick=()=>{switchPanel(activePanel===id?null:id);pulseRailTab(button);};railTabs.append(button);
 const panel=document.createElement('section');panel.id='panel-'+id;panel.className='workspace-panel';panel.dataset.accent=panelAccents[id];panel.setAttribute('aria-labelledby','panel-title-'+id);
 panel.innerHTML=`<div class="panel-heading"><span class="panel-heading-icon" aria-hidden="true">${railIcon(iconPaths[id])}</span><h2 id="panel-title-${id}">${name}</h2><button type="button" class="panel-close" aria-label="Close ${name} panel">${railIcon('M4.5 4.5l9 9M13.5 4.5l-9 9')}</button></div><div class="panel-body"></div>`;
 panel.querySelector('.panel-close').onclick=()=>{openPanel(null);button.focus();};
 panel.addEventListener('keydown',e=>{if(e.key==='Escape'&&!e.target.closest('input,textarea,select,dialog')){e.stopPropagation();openPanel(null);button.focus();}});
 panelBodies[id]=panel.lastElementChild;sidebar.append(panel);
}
// Rail motion: the hovered plate tilts away from the cursor, and a click pulses it into the tab colour.
const railMotion=!matchMedia('(prefers-reduced-motion: reduce)').matches;
function pulseRailTab(button){if(!railMotion)return;button.classList.remove('pulse');void button.offsetWidth;button.classList.add('pulse');}
railTabs.addEventListener('animationend',e=>{if(e.animationName==='rail-wash')e.target.closest('.rail-tab')?.classList.remove('pulse');});
// The tab bar's tilt, spotlight and press feel, shared with every other control.
const ICON_BUTTONS='.panel-close,.inspector-close,.icon-toggle,.tree-toggle,.copy-button,.search-clear,.agent-icon-button';
tactile(document,[{selector:'.rail-tab,.agent-tool',tilt:7},{selector:ICON_BUTTONS,tilt:12,pulse:true},{selector:'.btn,.user-menu-button,.agent-ask',tilt:3,pulse:true},{selector:'.list-row,.fiducial-row,.disclosure',tilt:8}]);
// One shared tooltip shows each rail button's name on hover and keyboard focus (WCAG 1.4.13: dismissible, hoverable, persistent).
// It repeats the button's aria-label, so it is hidden from assistive technology.
const railTip=document.createElement('div');railTip.className='rail-tooltip';railTip.setAttribute('aria-hidden','true');railTip.hidden=true;document.body.append(railTip);
let railTipTimer=0;
function showRailTip(button,immediate){
 clearTimeout(railTipTimer);
 const show=()=>{
  railTip.textContent=button.getAttribute('aria-label');
  const box=button.getBoundingClientRect();railTip.style.top=(box.top+box.height/2)+'px';railTip.style.left=(box.right+10)+'px';railTip.hidden=false;
 };
 if(immediate||!railTip.hidden)show();else railTipTimer=setTimeout(show,300);
}
function hideRailTip(delay=0){clearTimeout(railTipTimer);railTipTimer=setTimeout(()=>{railTip.hidden=true;},delay);}
// The logo carries no tooltip; its own animation is the hover response.
rail.addEventListener('pointerover',e=>{const button=e.target.closest('button,a');if(button&&!button.matches('.rail-logo'))showRailTip(button);});
rail.addEventListener('pointerout',e=>{if(!rail.contains(e.relatedTarget)&&!railTip.contains(e.relatedTarget))hideRailTip(120);});
railTip.addEventListener('pointerenter',()=>clearTimeout(railTipTimer));
railTip.addEventListener('pointerleave',e=>{if(!rail.contains(e.relatedTarget))hideRailTip(120);});
rail.addEventListener('focusin',e=>{if(e.target.matches(':focus-visible')&&!e.target.matches('.rail-logo'))showRailTip(e.target,true);});
rail.addEventListener('focusout',()=>hideRailTip());
rail.addEventListener('click',()=>hideRailTip());
document.addEventListener('keydown',e=>{if(e.key==='Escape'&&!railTip.hidden){hideRailTip();e.stopPropagation();}},true);
// Arrow keys move between rail buttons; Tab enters and leaves the rail at the active one.
const railButtons=()=>[...rail.querySelectorAll('button')];
function syncRailTabStops(){const buttons=railButtons(),active=rail.querySelector('[aria-expanded="true"]')||buttons[0];for(const b of buttons)b.tabIndex=b===active?0:-1;}
rail.addEventListener('keydown',e=>{
 if(e.key==='Escape'&&activePanel){e.preventDefault();const tab=rail.querySelector(`[data-panel="${activePanel}"]`);openPanel(null);tab.focus();return;}
 const buttons=railButtons(),index=buttons.indexOf(document.activeElement);if(index<0)return;
 const next={ArrowDown:index+1,ArrowUp:index-1,Home:0,End:buttons.length-1}[e.key];if(next===undefined)return;
 e.preventDefault();const target=buttons[(next+buttons.length)%buttons.length];for(const b of buttons)b.tabIndex=b===target?0:-1;target.focus();
});
function moveTo(panel,selector){document.querySelectorAll(selector).forEach(el=>panelBodies[panel].append(el));}
// Home server status lives in the account menu; the old profile heading is retired.
document.querySelectorAll('aside > .network-status:not(#runner-card):not(#mapping-card)').forEach(el=>$('user-menu-server').append(el));
document.querySelectorAll('aside > .eyebrow, aside > h1, aside > .intro').forEach(el=>el.remove());
moveTo('maps','#map-select, #mapping-card, #domain-info, .sidebar-footer');
moveTo('fleet','#runner-card, #sensor-select, #sensor-catalog-status');
$('runner-card').hidden=true;
$('sensor-catalog-status').hidden=true;
moveTo('settings','#demo-controls, aside > .section-label, aside > .layer, #robot-volume-status');
sidebar.querySelectorAll(':scope > .map-label').forEach(el=>el.remove());
// Panel motion: opening fades in with a bounce from the clicked tab, switching tabs bounces,
// and closing fades out.
let panelMotion=null;
function panelOrigin(id){const tab=railTabs.querySelector(`[data-panel="${id}"]`);if(!tab)return '0px 40px';return `0px ${tab.getBoundingClientRect().top+tab.offsetHeight/2-sidebar.getBoundingClientRect().top}px`;}
// The bounce: a quick decaying ripple from the tab; on open it also fades in.
function bounceFrames(origin,fade){const frames=[];for(let i=0;i<=30;i++){const t=i/30,a=Math.exp(-5*t)*Math.sin(16*t);frames.push({transform:`perspective(1200px) skewY(${(2.6*a).toFixed(3)}deg) scale(${(1+.02*a).toFixed(4)},${(1-.045*a).toFixed(4)})`,transformOrigin:origin,...(fade?{opacity:Math.min(1,t*4)}:{})});}return frames;}
function switchPanel(id){
 panelMotion?.cancel();
 if(!railMotion){openPanel(id);return;}
 const from=activePanel;
 if(!id){ // Pour back into the tab, then collapse.
  panelMotion=sidebar.animate([{opacity:1},{opacity:0}],{duration:160,easing:'ease-in'});panelMotion.onfinish=()=>{panelMotion=null;openPanel(null);};return;
 }
 openPanel(id);enterPanel(id);
 const origin=panelOrigin(id);
 if(!from){panelMotion=sidebar.animate(bounceFrames(origin,true),{duration:520,easing:'linear'});return;}
 // Switching: a quick decaying ripple centred on the new tab.
 panelMotion=sidebar.animate(bounceFrames(origin,false),{duration:520,easing:'linear'});
}
// Panel contents settle in one after another.
function enterPanel(id){if(!railMotion)return;const body=$('panel-'+id);body.classList.remove('entering');void body.offsetWidth;body.classList.add('entering');setTimeout(()=>body.classList.remove('entering'),700);}
function openPanel(id){activePanel=id;shell.classList.toggle('panel-collapsed',!id);for(const key of Object.keys(panelNames)){$('panel-'+key).hidden=key!==id;rail.querySelector(`[data-panel="${key}"]`).setAttribute('aria-expanded',String(key===id));}syncRailTabStops();if(typeof measurePanelCover==='function')measurePanelCover();}
const userMenu=$('user-menu'),userMenuButton=$('user-menu-button');
userMenuButton.setAttribute('aria-expanded','false');
userMenu.addEventListener('beforetoggle',e=>{if(e.newState!=='open')return;const box=userMenuButton.getBoundingClientRect();userMenu.style.top=(box.bottom+6)+'px';userMenu.style.right=(innerWidth-box.right)+'px';});
userMenu.addEventListener('toggle',e=>userMenuButton.setAttribute('aria-expanded',String(e.newState==='open')));
openPanel(null);
const mapsList=document.createElement('div');mapsList.id='scenegraph-list';panelBodies.maps.prepend(mapsList);$('map-select').hidden=true;
const spatialBrowser=document.createElement('section');spatialBrowser.id='spatial-browser';spatialBrowser.setAttribute('aria-label','Anchors and landmarks');
const spatialCounts=document.createElement('p');spatialCounts.className='panel-meta';spatialCounts.setAttribute('role','status');
spatialCounts.innerHTML='<span id="anchor-count">0</span> anchors · <span id="landmark-count">0</span> landmarks';
const spatialStatus=document.createElement('p');spatialStatus.id='spatial-status';spatialStatus.className='panel-hint';
const spatialTree=document.createElement('div');spatialTree.id='anchor-list';spatialTree.className='spatial-tree';spatialTree.setAttribute('aria-label','Anchors');
const anchorsRegion=document.createElement('div');anchorsRegion.id='anchors-region';
anchorsRegion.append(searchField({id:'spatial-search',label:'Search anchors and landmarks',placeholder:'Search by ID or type',onInput:()=>renderSpatialLists()}),spatialCounts,spatialStatus,spatialTree);
let anchorsOpen=false;try{anchorsOpen=localStorage.getItem('maps-anchors-open')==='true';}catch{}
anchorsRegion.hidden=!anchorsOpen;
const anchorsToggle=disclosure('Anchors',{expanded:anchorsOpen,controls:'anchors-region',onToggle:()=>{}});anchorsToggle.classList.add('section-disclosure');
anchorsToggle.onclick=()=>{anchorsOpen=!anchorsOpen;anchorsToggle.setAttribute('aria-expanded',String(anchorsOpen));try{localStorage.setItem('maps-anchors-open',String(anchorsOpen));}catch{}
 if(anchorsOpen){anchorsRegion.hidden=false;reveal(anchorsRegion);}else conceal(anchorsRegion,()=>{anchorsRegion.hidden=true;});};
spatialBrowser.append(anchorsToggle,anchorsRegion);
mapsList.after(spatialBrowser);
const fleetList=document.createElement('div');fleetList.id='fleet-sensors';panelBodies.fleet.append(fleetList);$('sensor-select').hidden=true;
const {createPoseInspector}=await import('/pose-inspector.js');
const inspectPose=createPoseInspector(panelBodies.fleet);
const disconnectRobots=document.createElement('button');disconnectRobots.id='disconnect-robots';disconnectRobots.className='panel-row';disconnectRobots.textContent='Disconnect simulated robots';disconnectRobots.hidden=true;panelBodies.fleet.append(disconnectRobots);
disconnectRobots.onclick=async()=>{disconnectRobots.disabled=true;try{await api('/api/simulation/disconnect',{});}catch(e){disconnectRobots.disabled=false;showAvailability(e.message);}};
const notesBody=panelBodies.notes;
const tagsBody=panelBodies.tags;
let fleetNetwork=null,fleetSignature='';
const componentControlResults=new Map();
let componentControlPending=false;
const sceneBackground=document.documentElement.dataset.theme==='light'?'#e8e8f0':'#0d0d0f';
const scene=new THREE.Scene();scene.background=new THREE.Color(sceneBackground);scene.fog=new THREE.Fog(sceneBackground,10,26);
const camera=new THREE.PerspectiveCamera(42,1,.01,100);
const renderer=new THREE.WebGLRenderer({antialias:true,preserveDrawingBuffer:true});renderer.setPixelRatio(Math.min(devicePixelRatio,2));renderer.shadowMap.enabled=true;renderer.shadowMap.type=THREE.PCFSoftShadowMap;renderer.outputColorSpace=THREE.SRGBColorSpace;renderer.toneMapping=THREE.ACESFilmicToneMapping;renderer.toneMappingExposure=1.1;
$('canvas').append(renderer.domElement);renderer.domElement.setAttribute('aria-label','3D voxel map');
const controls=new OrbitControls(camera,renderer.domElement);controls.enableDamping=true;
// Mouse: left-drag looks around (orbits; a click without a drag still selects), right- or middle-drag pans
// across the ground, and the wheel zooms toward the cursor. Touch: one finger pans, two pinch and rotate.
controls.mouseButtons={LEFT:THREE.MOUSE.ROTATE,MIDDLE:THREE.MOUSE.PAN,RIGHT:THREE.MOUSE.PAN};controls.touches={ONE:THREE.TOUCH.PAN,TWO:THREE.TOUCH.DOLLY_ROTATE};
controls.screenSpacePanning=false;controls.zoomToCursor=true;controls.minDistance=1.2;controls.maxDistance=60;controls.maxPolarAngle=Math.PI*.49;
let crayon=null;
const cameraPan=createCameraPan({camera,controls});
renderer.domElement.addEventListener('pointerdown',()=>cameraPan.cancel(),true);
renderer.domElement.addEventListener('wheel',()=>cameraPan.cancel(),{capture:true,passive:true});
for(const id of ['map-select','fit','top','demo-view'])$(id).addEventListener(id==='map-select'?'change':'click',()=>cameraPan.cancel(),true);
const ghostCamera=createGhostCamera({camera,controls,canvas:renderer.domElement,enabled:()=>!map2D.isActive()&&!crayon?.isActive(),onMove:()=>{cameraPan.cancel();if(attachedCamera.isActive())attachedCamera.detach();
 // The first time someone moves with the keyboard, show the whole scheme once.
 try{if(!localStorage.getItem('camera-keys-hint')){localStorage.setItem('camera-keys-hint','1');toast('WASD move · Space / Shift up and down · Q / E turn · drag to look around · right-drag to pan');}}catch{}}});
let activeMap='demo',current=null,busy=false,refreshing=false,lastScene=null,selectable=[],selectedNoteId=null,selectedPathId=null,deletingPathId=null,deletingNoteId=null,selectedSpatialNode=null,spatialSignature='';
let homeOffline=false,catalogInitialized=false;
function isLocalScene(map=activeMap){return map.startsWith('local:');}
function localSceneUrl(map=activeMap){return '/api/local-scenes/'+encodeURIComponent(map.slice(6));}
function noteUrl(id,map=activeMap){return '/api/notes/'+encodeURIComponent(id)+(isLocalScene(map)?'?scene='+encodeURIComponent(map.slice(6)):'');}
// Scene status notice: a glass pill at the top of the view. The text before the first " · " is the title and
// the rest the detail; the dot's tone comes from the wording (unavailable/offline/failed, read-only, live).
const availability=document.createElement('div');availability.id='scene-availability';availability.className='scene-notice';availability.setAttribute('role','status');availability.hidden=true;
availability.innerHTML='<span class="scene-notice-dot" aria-hidden="true"></span><span class="scene-notice-text"><strong></strong><span></span></span><button type="button" class="scene-notice-close" aria-label="Dismiss"><svg aria-hidden="true" width="14" height="14" viewBox="0 0 18 18" fill="none" stroke="currentColor" stroke-width="1.6" stroke-linecap="round"><path d="M4.5 4.5l9 9M13.5 4.5l-9 9"/></svg></button>';
let dismissedNotice=null;availability.querySelector('.scene-notice-close').onclick=()=>{dismissedNotice=availability.title;availability.hidden=true;};$('canvas').append(availability);
function availabilityText(text){
 if(availability.title===String(text))return;
 const [title,...rest]=String(text).split(' · '),detail=rest.join(' · ');
 availability.querySelector('strong').textContent=title;availability.querySelector('.scene-notice-text>span').textContent=detail;availability.title=text;
 availability.dataset.tone=/unavailable|offline|fail|error|invalid|no compact|multiple/i.test(text)?'error':/read-only|retained|unaligned/i.test(text)?'warn':'info';
 if(!availability.hidden&&!matchMedia('(prefers-reduced-motion: reduce)').matches)availability.animate([{opacity:.4,transform:'translate(-50%,-4px)'},{opacity:1,transform:'translate(-50%,0)'}],{duration:240,easing:'cubic-bezier(.2,.8,.2,1)'});
}
function showAvailability(text){if(String(text)===dismissedNotice)return;dismissedNotice=null;availability.hidden=false;availabilityText(text);}
function markOffline(){homeOffline=true;showAvailability('Home offline · showing a cached browser view, not a live or saved map');renderMapRows();}
function isRunner(map=activeMap){return map==='runner'||map.startsWith('runner:');}
function isSimulation(map=activeMap){return map==='sim-voxels'||map==='sim-occupancy';}
function runnerUrl(endpoint,map=activeMap){return endpoint+(map.startsWith('runner:')?'?peer='+encodeURIComponent(map.slice(7)):'');}
const runnerNames=new Map(),peerNames=new Map();
let primaryRunnerPeer=null;
function runnerDisplay(){return activeMap.startsWith('peer-scene:')?'Robot':isSimulation()?'Simulated robot':runnerNames.get(activeMap)||'Galbot';}
async function runnerCatalog(){
 try {
  const data=await api('/api/scene-sources');
  const local=document.createElement('optgroup');local.label='Local';
  homeOffline=false;
  for(const map of data.local||[]){if(map.available===false)continue;local.append(new Option(map.name+(map.status==='retained'?' · retained':''),map.id));}
  const network=document.createElement('optgroup');network.label='Network';
  for(const phone of data.phones||[]){
   // Prefer the phone-owned Catalog Product; the push cache is only a fallback.
   const product=(data.scenes||[]).find(m=>m.peer_id===phone.peer_id&&m.name==='phone-local-voxels');
   if(product){if(activeMap===phone.id){activeMap=product.id;lastScene=null;}continue;}
   network.append(new Option(phone.name,phone.id));
  }
  const visibleRunners=(data.runners||[]).filter(r=>r.available&&['connected','stale'].includes(r.status));
  for(const map of data.scenes||[]){
   // Configured runners subscribe to canonical-scene; keep other products on the same peer.
   const runner=map.name==='canonical-scene'&&visibleRunners.find(r=>r.peer_id===map.peer_id);
   const primary=!runner&&map.name==='canonical-scene'&&data.available&&['connected','stale'].includes(data.status)&&data.peer_id===map.peer_id;
   if(runner||primary){
    if(activeMap===map.id){activeMap=runner&&!runner.primary?'runner:'+runner.peer_id:'runner';lastScene=null;}
    continue;
   }
   const label=map.name==='phone-local-voxels'?'Phone · local map':map.name;
   const option=new Option(map.peer_alias||label+' · '+map.peer_id.slice(-8),map.id);
   option.title=label+' · '+map.peer_id;
   network.append(option);
  }
  runnerNames.clear();peerNames.clear();primaryRunnerPeer=null;
  for(const runner of data.runners||[]) {
   if(!runner.available||!['connected','stale'].includes(runner.status))continue;
   if(runner.primary)primaryRunnerPeer=runner.peer_id;
   const value=runner.primary?'runner':'runner:'+runner.peer_id;
   runnerNames.set(value,runner.name);peerNames.set(runner.peer_id,runner.name);
   network.append(new Option(runner.name||runner.peer_id,value));
  }
  // A primary scene may be present without a named runner configuration.
  if(data.available&&['connected','stale'].includes(data.status)&&!Array.from(network.children).some(o=>o.value==='runner')){
   network.append(new Option(data.peer_id||'Network scenegraph','runner'));
  }
  const select=$('map-select');select.replaceChildren(local,network);
  for(const peer of fleetNetwork?.peers||[])network.append(new Option((peer.name||peer.peer_id)+' · pose trail','peer-pose:'+peer.peer_id));
  // Open on the scene this browser last showed, when Home still has it; otherwise Home's preferred scene.
  if(!catalogInitialized){let last=null;try{last=localStorage.getItem('home.lastScene');}catch{}
   const start=[last,data.preferred].find(id=>id&&Array.from(select.options).some(o=>o.value===id));
   if(start){select.value=start;select.dispatchEvent(new Event('change'));}}
  else if(Array.from(select.options).some(o=>o.value===activeMap))select.value=activeMap;
  else if(select.options.length){select.value=select.options[0].value;select.dispatchEvent(new Event('change'));}
  catalogInitialized=true;
  renderMapRows();
 }catch{markOffline();}
 setTimeout(runnerCatalog,5000);
}

function domainCenter(){return new THREE.Box3().setFromPoints((current?.portals||[]).map(p=>new THREE.Vector3(...p.pose_in_map.translation))).getCenter(new THREE.Vector3());}
function fit(){if(current?.simulation){const d=Math.max(12,11/Math.max(camera.aspect,.2));camera.position.set(0,d,d*.4);controls.target.set(0,0,0);}else if(current?.portals.length){const c=domainCenter();const box=new THREE.Box3().setFromPoints(current.portals.map(p=>new THREE.Vector3(...p.pose_in_map.translation)));const size=box.getSize(new THREE.Vector3());const distance=Math.max(size.x/Math.max(camera.aspect,.2),size.z)*1.65+3;camera.position.copy(c).add(new THREE.Vector3(0,distance,distance*.25));controls.target.copy(c);}else{camera.position.set(3.6,3.5,4.6);controls.target.set(0,.5,0);}controls.update();}
fit();
// Entering 2D ends any 3D selection or creation, so volume handles and cards never linger over the plan.
const map2D=createMap2D($('canvas'),active=>{agentBar?.lock(active?'switch to 3D to use':null);setPlanBar(active);if(active){if(placingNote)stopNotePlacement();if($('volume-draw')?.getAttribute('aria-pressed')==='true')$('volume-draw').click();clearSelection();}controls.enabled=!active;$('top').setAttribute('aria-pressed',String(active));$('top').title=active?'Switch to 3D mode':'Switch to 2D mode';$('top').setAttribute('aria-label',$('top').title);$('canvas').dataset.viewMode=active?'2d':'3d';});
const attachedCamera=createAttachedCamera({host:$('canvas'),camera,controls,map2D,context:()=>({scene:activeMap,frame:current?.frame}),request:async(action,signal)=>{
 const q=new URLSearchParams({follow:'true',home_frame:action.frame,sensor:action.sensor_id,expected_manifest:action.pose_product.manifest_hash});
 const response=await fetch('/api/sensors?'+q,{signal});const value=await response.json();if(!response.ok)throw Error(value.error||'Sensor unavailable');return value;
}});
for(const id of ['map-select','fit','top','demo-view'])$(id).addEventListener(id==='map-select'?'change':'click',()=>{if(attachedCamera.isActive())attachedCamera.detach();},true);
$('demo-view').onclick=()=>{camera.position.set(-.5,4.5,5.5);controls.target.set(-3.7,.5,2.1);controls.update();};$('fit').onclick=()=>map2D.isActive()?map2D.fit():fit();$('top').onclick=()=>map2D.toggle();
scene.add(new THREE.HemisphereLight(0xffffff,0x1a2438,1.7));
const sun=new THREE.DirectionalLight(0xffffff,2.4);sun.position.set(-3,7,4);sun.castShadow=true;sun.shadow.mapSize.set(2048,2048);Object.assign(sun.shadow.camera,{left:-5,right:5,top:5,bottom:-5,near:.1,far:20});sun.shadow.bias=-.0005;scene.add(sun);
const ground=new THREE.Mesh(new THREE.PlaneGeometry(200,200),new THREE.MeshStandardMaterial({color:document.documentElement.dataset.theme==='light'?'#e0e3e8':'#101014',roughness:1}));ground.rotation.x=-Math.PI/2;ground.position.y=-.15;ground.receiveShadow=true;scene.add(ground);
// Ground grid: line grids (0.5 m minor, 2.5 m major) that follow the camera in whole major cells, so they
// never run out and never swim. A small shader patch fades each line toward the horizon; lines only cost
// their own pixels, which keeps software-rendered browsers (and the browser tests) fast.
// `grid.material.opacity` stays the opacity setting and drives both layers.
const gridFade={uCam:{value:new THREE.Vector3()},uCentre:{value:new THREE.Vector2()},uFadeFar:{value:40},uClip:{value:100}};
// Fades a line material toward the horizon. The fade is measured in 3D from the camera and scaled by the
// distance to what it looks at, so everything between the camera and its target stays crisp at any tilt
// (a flat distance from the camera faded the floor right in front of a downward look). `reach` scales how
// far past the target a layer carries; `limit` is the layer's half-extent, faded softly so its end never shows.
// Either way the fade finishes short of the camera's far plane (uClip): zoomed out and tilted, no hard cut-off.
function fadeWithDistance(material,reach,limit){
 material.onBeforeCompile=shader=>{Object.assign(shader.uniforms,gridFade,{uReach:{value:reach},uLimit:{value:limit}});
  shader.vertexShader='varying vec3 vGridWorld;\n'+shader.vertexShader.replace('#include <project_vertex>','#include <project_vertex>\nvGridWorld=(modelMatrix*vec4(position,1.)).xyz;');
  shader.fragmentShader='varying vec3 vGridWorld;uniform vec3 uCam;uniform vec2 uCentre;uniform float uFadeFar;uniform float uReach;uniform float uLimit;uniform float uClip;\n'+shader.fragmentShader.replace('#include <color_fragment>','#include <color_fragment>\nfloat fadeEnd=min(uFadeFar*(1.+1.8*uReach),uClip*.9);\ndiffuseColor.a*=(1.-smoothstep(min(uFadeFar*(1.+.4*uReach),fadeEnd*.55),fadeEnd,distance(uCam,vGridWorld)))*(1.-smoothstep(uLimit*.6,uLimit*.95,distance(uCentre,vGridWorld.xz)));');};
 return material;
}
// Each line is split into short pieces: the fade reads each fragment's world position, and one line hundreds of
// metres long interpolates that position badly once clipped, leaving full-length lines across the screen.
function lineGrid(spacing,extent,reach,piece){
 const points=[],half=extent/2;
 for(let v=-half;v<=half+1e-6;v+=spacing)for(let a=-half;a<half-1e-6;a+=piece){const b=Math.min(half,a+piece);points.push(a,0,v,b,0,v,v,0,a,v,0,b);}
 const geometry=new THREE.BufferGeometry();geometry.setAttribute('position',new THREE.Float32BufferAttribute(points,3));
 return new THREE.LineSegments(geometry,fadeWithDistance(new THREE.LineBasicMaterial({color:'#52525b',transparent:true,depthWrite:false}),reach,half));
}
const gridMinor=lineGrid(.5,90,1,1),gridMajor=lineGrid(2.5,400,3,5);
const axisPoints=[];for(let a=-200;a<200;a+=5)axisPoints.push(a,0,0,a+5,0,0,0,0,a,0,0,a+5);
const gridAxes=new THREE.LineSegments(new THREE.BufferGeometry().setAttribute('position',new THREE.Float32BufferAttribute(axisPoints,3)),fadeWithDistance(new THREE.LineBasicMaterial({color:'#4f8ef7',transparent:true,depthWrite:false}),3,200));
gridAxes.position.y=-.139;
const grid=new THREE.Group();grid.add(gridMinor,gridMajor);grid.position.y=-.14;
// One opacity for the whole grid (the Settings slider): minor lines at half strength, major and axes at full.
grid.material={get opacity(){return gridMajor.material.opacity;},set opacity(v){gridMajor.material.opacity=gridAxes.material.opacity=v;gridMinor.material.opacity=v*.55;}};
grid.material.opacity=.32;
// Follow the camera in whole major cells so the lines stay put in the world while the grid never ends.
gridMajor.onBeforeRender=()=>{const snap=2.5;grid.position.x=Math.round(controls.target.x/snap)*snap;grid.position.z=Math.round(controls.target.z/snap)*snap;grid.updateMatrixWorld();
 gridFade.uCam.value.copy(camera.position);gridFade.uCentre.value.set(controls.target.x,controls.target.z);gridFade.uFadeFar.value=Math.max(4,camera.position.distanceTo(controls.target));gridFade.uClip.value=camera.far;};
// The axes mark the world origin, so they stay put rather than following the camera.
scene.add(grid,gridAxes);
// The demo floor plane dissolves into the background the same way, instead of ending in a hard edge.
fadeWithDistance(ground.material,1.3);ground.material.transparent=true;ground.renderOrder=-2;
const groups=Object.fromEntries(['voxels','portals','objects','rays','notes'].map(k=>[k,new THREE.Group()]));Object.values(groups).forEach(g=>scene.add(g));
// Far notes become pins, and further out overlapping pins merge into a counted cluster; the open note keeps its text.
const noteLod=createNoteLod({scene,camera,renderer,notes:groups.notes,pinned:bubble=>bubble.userData.note?.id===selectedNoteId});
crayon=createCrayon({button:$('crayon'),canvas:renderer.domElement,camera,controls,scene,voxels:groups.voxels,
 state:()=>({map:activeMap,frame:current?.frame,paths:current?.paths,editable:isLocalScene()&&!!current&&!map2D.isActive()}),
 save:async(context,stroke,extend=false)=>{const result=await api('/api/paths'+(extend?'/'+encodeURIComponent(stroke.id):'')+'?scene='+encodeURIComponent(context.map),{frame:context.frame,stroke},extend?'PATCH':'POST');if(activeMap===context.map&&current?.frame===context.frame)current.paths=result;lastScene=null;return result;},
 remove:async(context,id)=>{const result=await api('/api/paths/'+encodeURIComponent(id)+'?scene='+encodeURIComponent(context.map),undefined,'DELETE');if(activeMap===context.map&&current?.frame===context.frame)current.paths=result;lastScene=null;return result;}});
for(const input of document.querySelectorAll('[data-layer]'))input.onchange=()=>{groups[input.dataset.layer].visible=input.checked;};

const boxGeometry=new THREE.BoxGeometry(1,1,1), cubeMaterial=new THREE.MeshStandardMaterial({roughness:.82,metalness:0});
function clear(group){for(const child of [...group.children]){group.remove(child);if(child.isInstancedMesh)child.dispose();child.traverse(o=>{if(o.geometry&&o.geometry!==boxGeometry)o.geometry.dispose();if(o.material&&o.material!==cubeMaterial){for(const m of Array.isArray(o.material)?o.material:[o.material]){m.map?.dispose();m.dispose();}}});}}
function label(text,color='#36594a'){
 const c=document.createElement('canvas');c.width=384;c.height=96;const ctx=c.getContext('2d');ctx.fillStyle='#fcfdf6';ctx.beginPath();ctx.roundRect(4,8,376,76,18);ctx.fill();ctx.font='500 30px system-ui';ctx.textAlign='center';ctx.fillStyle=color;ctx.fillText(text,192,56,356);const tex=new THREE.CanvasTexture(c);tex.colorSpace=THREE.SRGBColorSpace;const sprite=new THREE.Sprite(new THREE.SpriteMaterial({map:tex,depthTest:false}));sprite.scale.set(.65,.1625,1);return sprite;
}
// Volume labels: a light chip (violet cube tile, the title, optional muted detail) that keeps one on-screen size
// like the note bubbles, stands just above its anchor and truncates long titles rather than squashing them.
function volumeLabel(title,detail=''){
 const dpr=2,font='600 15px system-ui, -apple-system, sans-serif',small='500 12.5px system-ui, -apple-system, sans-serif',canvas=document.createElement('canvas'),ctx=canvas.getContext('2d');
 const fit=(text,f,max)=>{ctx.font=f;if(ctx.measureText(text).width<=max)return text;while(text&&ctx.measureText(text+'…').width>max)text=text.slice(0,-1);return text.trimEnd()+'…';};
 const name=fit(String(title||'Volume'),font,220),extra=detail?fit(String(detail),small,120):'';
 ctx.font=font;const nameWidth=ctx.measureText(name).width;ctx.font=small;const extraWidth=extra?ctx.measureText(extra).width+8:0;
 const pad=12,h=34,w=Math.ceil(5+24+9+nameWidth+extraWidth+13);canvas.width=(w+pad*2)*dpr;canvas.height=(h+pad*2)*dpr;ctx.scale(dpr,dpr);
 ctx.shadowColor='rgba(24,24,27,.2)';ctx.shadowBlur=10;ctx.shadowOffsetY=3;ctx.fillStyle='rgba(255,255,255,.97)';ctx.beginPath();ctx.roundRect(pad,pad,w,h,11);ctx.fill();
 ctx.shadowColor='transparent';ctx.strokeStyle='rgba(24,24,27,.09)';ctx.lineWidth=1;ctx.beginPath();ctx.roundRect(pad+.5,pad+.5,w-1,h-1,10.5);ctx.stroke();
 const tile=ctx.createLinearGradient(0,pad+5,0,pad+29);tile.addColorStop(0,'#8b5cf6');tile.addColorStop(1,'#6d28d9');ctx.fillStyle=tile;ctx.beginPath();ctx.roundRect(pad+5,pad+5,24,24,7);ctx.fill();
 ctx.save();ctx.translate(pad+8,pad+8);ctx.strokeStyle='#fff';ctx.lineWidth=1.6;ctx.lineJoin='round';ctx.lineCap='round';ctx.stroke(new Path2D(CUBE_ICON));ctx.restore();
 ctx.textBaseline='middle';ctx.font=font;ctx.fillStyle='#18181b';ctx.fillText(name,pad+38,pad+h/2+1);
 if(extra){ctx.font=small;ctx.fillStyle='#71717a';ctx.fillText(extra,pad+38+nameWidth+8,pad+h/2+1);}
 const texture=new THREE.CanvasTexture(canvas);texture.colorSpace=THREE.SRGBColorSpace;
 const sprite=new THREE.Sprite(new THREE.SpriteMaterial({map:texture,transparent:true,depthTest:false,depthWrite:false,sizeAttenuation:false,toneMapped:false}));
 const unit=.00072;sprite.scale.set((w+pad*2)*unit,(h+pad*2)*unit,1);sprite.center.set(.5,pad/(h+pad*2));sprite.renderOrder=70;return sprite;
}
const phoneTrails=createPhoneTrails(scene,text=>createNoteBubble(text,{peer:true}),peer=>cameraShape(fleetNetwork?.peers?.find(p=>p.peer_id===peer)?.catalog));
const compactTrail=createCompactPoseTrail(scene);
$('map-select').addEventListener('change',()=>compactTrail.reset());
async function refreshCompactTrail(){
 refreshPeer();
 grid.position.set(0,0,0);
 const selected=activeMap,peer=fleetNetwork?.peers?.find(p=>'peer-pose:'+p.peer_id===selected);
 const products=(peer?.catalog?.products||[]).filter(p=>p.metadata?.schema==='spatial-harness.compact-pose-contract/v1'&&(p.metadata.value?.lifetime??'sampled')==='sampled');
 $('scene-title').textContent=(peer?.name||peer?.peer_id||'Peer')+' · pose trail';
 $('frame-info').textContent='Native peer space · no Home alignment';
 $('portal-info').textContent='Last 300 received poses since opening · older ghosts are fainter';
 if(!products.length){compactTrail.reset();showAvailability('No compact pose Product advertised by this peer');return;}
 if(products.length!==1){compactTrail.reset();showAvailability('Multiple sampled pose Products; inspect the desired Product in Devices');return;}
 // Each Product has its own declared frame/session; do not mix unrelated outputs.
 const product=products[0];
 const query=new URLSearchParams({debug_pose:'true',sensor:peer.peer_id+'/'+product.manifest.product_id,expected_manifest:product.manifest_hash});
 try{
  const data=await api('/api/sensors?'+query);if(activeMap!==selected)return;
  const result=compactTrail.update(data);
  $('frame-info').textContent=result.frame+' · native coordinates (registry-oriented display)';
  $('observation-count').textContent=String(result.count);
  showAvailability(result.status+' · '+data.stream+' · '+product.manifest.product_id+(products.length>1?' · first pose Product':''));
 }catch(error){if(activeMap!==selected)return;compactTrail.reset();showAvailability('Pose trail unavailable · '+error.message);}
}
// The volume module renders into the shared inspector card through this small interface.
const volumeInspector={element:$('selection'),
 open(nodes){if(noteCardMode!=='volume'){selectedPathId=null;crayon.select(null);selectedNoteId=null;selectedSpatialNode=null;updateSpatialSelection();renderNotes();}
  const card=$('selection');for(const key of ['noteId','pathId','spatialId','spatialKind'])delete card.dataset[key];
  const close=document.createElement('button');close.type='button';close.className='inspector-close';close.setAttribute('aria-label','Close inspector');close.append(icon(icons.close));close.onclick=closeCard;
  card.replaceChildren(close,...nodes);card.hidden=false;noteCardMode='volume';},
 close(){if(noteCardMode==='volume'){noteCardMode='view';$('selection').hidden=true;}}};
const semanticVolumes=createSemanticVolumes({scene,camera,renderer,controls,panel:panelBodies.volumes,api,label:volumeLabel,clear,openPanel,inspector:volumeInspector,toast,flyTo:(position,target)=>cameraPan.start(position,target),priorityPickObjects:()=>[...selectable.filter(o=>o.userData.kind==='note'),...noteLod.objects(),...crayon.objects()]});
// The volume tool draws on the 3D floor, so it can't start while the 2D plan is open.
$('volume-draw').addEventListener('click',e=>{if(map2D.isActive())e.stopImmediatePropagation();},true);
$('map-select').addEventListener('change',()=>{semanticVolumes.reset();try{localStorage.setItem('home.lastScene',$('map-select').value);}catch{}});
let phonePresenceBusy=false;
const homePoseStatus=document.createElement('p');homePoseStatus.className='panel-hint';panelBodies.maps.append(homePoseStatus);
$('map-select').addEventListener('change',()=>{homePoseStatus.textContent='';phoneTrails.reset(null);});
// Robots that declare a body on their pose's base frame, aligned to this scene.
let declaredBodies={frame:null,entries:[]};
const sameFrame=(a,b)=>Boolean(a&&b)&&a.peer_id===b.peer_id&&a.id===b.id&&a.hash===b.hash;
async function refreshPhonePresence(){
 if(phonePresenceBusy||!isLocalScene()||!current)return;
 const map=activeMap,frame=current.frame;phonePresenceBusy=true;
 try{const poses=[],errors=[],bodies=[];
  // Only peers with a sampled pose can be drawn as a body; skip the lookup without any.
  const posed=(fleetNetwork?.peers||[]).some(peer=>(peer.catalog?.products||[]).some(p=>p.metadata?.schema==='spatial-harness.compact-pose-contract/v1'));
  const declared=posed?(await api('/api/robot-states').catch(()=>({robots:[]}))).robots||[]:[];
  // Look every peer up at once: Home answers pose lookups one at a time, so a
  // peer-by-peer loop made each pass longer than the freshness window.
  await Promise.all((fleetNetwork?.peers||[]).map(async peer=>{
   const products=(peer.catalog?.products||[]).filter(p=>p.metadata?.schema==='spatial-harness.compact-pose-contract/v1'&&(p.metadata.value?.lifetime??'sampled')==='sampled');
   if(!products.length)return;
   const name=fleetNetwork?.aliases?.[peer.peer_id]||peer.name||peer.peer_id;
   if(products.length!==1){errors.push(name+': ambiguous pose Products');return;}
   const product=products[0],q=new URLSearchParams({debug_pose:'true',sensor:peer.peer_id+'/'+product.manifest.product_id,expected_manifest:product.manifest_hash,home_frame:frame});
   try{const value=await api('/api/sensors?'+q),home=value.home_pose;
    if(home?.frame!==frame||home.tracking?.status!=='valid'||value.stream!=='live')return;
    const p=home.tracking.pose,[x,y,z,w]=p.rotation_xyzw;
    const body=declared.find(r=>r.peer_id===peer.peer_id&&sameFrame(r.base_frame,product.metadata.value.from_frame));
    if(body){bodies.push({peer_id:peer.peer_id,name,received_age_ms:value.age_ms,receivedAt:performance.now(),body,
     base_frame:value.resolved?.contract?.from_frame,pose:{translation:p.translation,rotation_xyzw:p.rotation_xyzw}});return;}
    poses.push({receivedAt:performance.now(),peer_id:peer.peer_id,name,clock:{session_id:value.resolved.contract.session_id+':'+home.alignment_timestamp_ns},capture_timestamp_ns:value.timestamp_ns,received_age_ms:value.age_ms,
     pose:{to_frame_id:frame,translation:p.translation,rotation_wxyz:[w,x,y,z]}});
   }catch(e){errors.push(name+': '+e.message);}
  }));
  if(activeMap!==map||current?.frame!==frame)return;
  if(activeMap===map&&current?.frame===frame){declaredBodies={frame,entries:bodies};refreshRunner();}
  if(activeMap===map&&current?.frame===frame){const fresh=poses.map(p=>({...p,received_age_ms:p.received_age_ms+performance.now()-p.receivedAt})).filter(p=>p.received_age_ms<2000);phoneTrails.ingest(frame,fresh);homePoseStatus.textContent=errors.length?errors.join(' · '):fresh.length?plural(fresh.length,'phone')+' aligned to this Home scene':'';$('canvas').dataset.phonePoseStatus=errors.join('\n');}
 }catch{if(activeMap===map)phoneTrails.ingest(frame,[]);}finally{phonePresenceBusy=false;}
}
setInterval(refreshPhonePresence,200);
function build(data){flatSpritesAge=Infinity;
 map2D.update(data);
 semanticVolumes.update(activeMap,data);
 const first=current===null||current?.frame!==data.frame;data.receivedAt=performance.now();current=data;selectable=[];Object.values(groups).forEach(clear);
 crayon.update();
 const voxels=new THREE.InstancedMesh(boxGeometry,cubeMaterial,data.voxels.length);voxels.castShadow=true;voxels.receiveShadow=true;
 const matrix=new THREE.Matrix4(),color=new THREE.Color();
 data.voxels.forEach((v,i)=>{const scale=data.voxel_size;matrix.compose(new THREE.Vector3(...v.center),new THREE.Quaternion(...(data.voxel_rotation_xyzw||[0,0,0,1])),new THREE.Vector3(scale,scale,scale));voxels.setMatrixAt(i,matrix);const furnishing=data.synthetic_layout?.boxes.findLast(b=>v.center.every((n,j)=>n>=b.min[j]-.051&&n<=b.max[j]+.051));if(v.linear_rgb){color.setRGB(...v.linear_rgb,THREE.LinearSRGBColorSpace);}else color.set(furnishing?.color||(v.center[1]<0?'#c6cbbb':'#a7b4ac'));voxels.setColorAt(i,color);});
 voxels.userData.kind='voxels';groups.voxels.add(voxels);selectable.push(voxels);
 if(data.occupancy_tiles){
  const tiles=new THREE.InstancedMesh(boxGeometry,new THREE.MeshBasicMaterial(),data.occupancy_tiles.length);
  data.occupancy_tiles.forEach((tile,i)=>{
   matrix.compose(new THREE.Vector3(...tile.center),new THREE.Quaternion(),new THREE.Vector3(data.voxel_size*.96,.025,data.voxel_size*.96));
   tiles.setMatrixAt(i,matrix);color.set(tile.value===100?'#ee795e':tile.value===0?'#82b9a0':'#343c49');tiles.setColorAt(i,color);
  });
  tiles.userData.kind='occupancy';groups.voxels.add(tiles);selectable.push(tiles);
 }
 $('canvas').dataset.occupancyCount=String(data.occupancy_tiles?.length||0);
 for(const p of data.portals){
  const imported=p.anchor_id.startsWith('home:');const blue=data.local_map&&!imported;const code=imported?p.anchor_id.slice(5):p.anchor_id;
  const g=new THREE.Group();g.position.fromArray(p.pose_in_map.translation);const [w,x,y,z]=p.pose_in_map.rotation_wxyz;g.quaternion.set(x,y,z,w);
  const plate=new THREE.Mesh(new THREE.PlaneGeometry(p.side_length_m,p.side_length_m),new THREE.MeshBasicMaterial({color:blue?'#2477ec':'#e53935',wireframe:imported&&data.portals.some(o=>o.anchor_id===code),side:THREE.DoubleSide,depthTest:false,depthWrite:false}));plate.renderOrder=61;plate.userData={kind:'portal',portal:p};g.add(plate);selectable.push(plate);
  groups.portals.add(g);
 }
 if(first)phoneTrails.reset(data.frame);
 for(const volume of legacyVolumesForView(data)){
  const [w,h,d]=volume.dimensions;
  const group=new THREE.Group();group.position.fromArray(volume.position);group.rotation.y=volume.yaw;
  const geometry=new THREE.BoxGeometry(w,h,d);
  const body=new THREE.Mesh(geometry,new THREE.MeshBasicMaterial({color:0x22cddd,transparent:true,opacity:.25,depthWrite:false}));
  body.position.y=h/2;group.add(body);
  const edges=new THREE.LineSegments(new THREE.EdgesGeometry(geometry),new THREE.LineBasicMaterial({color:0x22cddd}));
  edges.position.y=h/2;group.add(edges);
  const tag=volumeLabel(volume.name,volume.semantic_type);tag.position.set(0,h+.06,0);group.add(tag);
  groups.objects.add(group);
 }
 $('canvas').dataset.volumeCount=String(data.semantic_volumes?.frame===data.frame?data.semantic_volumes.items.length:(data.volumes||[]).length);
 renderer.domElement.dataset.legacyVolumeCount=String(legacyVolumesForView(data).length);
 for(const note of data.notes||[]){
  if(note.phone_pose)continue;
  const peer=Boolean(note.robot_pose||note.anchor_robot||note.id?.startsWith('robot-name:')||note.id?.startsWith('phone-name:'));
  const tag=createNoteBubble(noteTitle(note),{peer});tag.position.fromArray(note.position);
  tag.userData={kind:'note',note,peer};groups.notes.add(tag);selectable.push(tag);
 }
 $('canvas').dataset.noteCount=String((data.notes||[]).length);
 for(const o of data.objects){
  const point=new THREE.Mesh(new THREE.SphereGeometry(.04,20,14),new THREE.MeshStandardMaterial({color:'#dd895d',emissive:'#65361b',emissiveIntensity:.15}));point.position.fromArray(o.position);point.userData={kind:'object',object:o};groups.objects.add(point);selectable.push(point);
  const ring=new THREE.Mesh(new THREE.TorusGeometry(.13,.009,8,48),new THREE.MeshBasicMaterial({color:'#d79367'}));ring.rotation.x=-Math.PI/2;ring.position.copy(point.position);ring.position.y+=.008;groups.objects.add(ring);
  const tag=label(o.label,'#a56845');tag.position.copy(point.position).add(new THREE.Vector3(0,.32,0));groups.objects.add(tag);
  const geo=new THREE.BufferGeometry().setFromPoints([new THREE.Vector3(...o.ray_origin),new THREE.Vector3(...o.position)]);const line=new THREE.Line(geo,new THREE.LineDashedMaterial({color:'#7898bd',dashSize:.05,gapSize:.035}));line.computeLineDistances();groups.rays.add(line);
 }
 if(data.integrated_observations){
  for(const [peer,p] of Object.entries(data.sensor_positions||{})){
   if(!data.sources[peer])continue;
   const marker=new THREE.Mesh(new THREE.OctahedronGeometry(.045),new THREE.MeshStandardMaterial({color:'#7f9aba'}));marker.position.fromArray(p);groups.rays.add(marker);
  }
 }
 $('empty').hidden=data.portals.length>0||data.voxels.length>0;if(activeMap==='home-live')$('empty').textContent='Waiting for Home to observe UCOF1J40N7O';else $('empty').textContent='No geometry yet';$('voxel-count').textContent=data.voxels.length.toLocaleString();$('observation-count').textContent=data.integrated_observations??'—';$('object-count').textContent=data.objects.length;
 for(const [peer,id] of [['iphone','phone-state'],['bracketbot','bot-state']]){const n=data.sources[peer];$(id).textContent=n?'Contributed':'Ready';$(id).classList.toggle('active',Boolean(n));}
 $('load').innerHTML=data.integrated_observations?'Reload test contributions <span>↗</span>':'Load two test peers <span>↗</span>';
 $('project').disabled=busy||!data.voxels.length;$('replay').disabled=busy||!data.sources.iphone;
 if(noteCardMode!=='view'){}else if(selectedSpatialNode){const {kind,id}=selectedSpatialNode;const node=data.landmarks?.[kind==='anchor'?'anchors':'landmarks']?.[id];if(node)showSpatialSelection(kind,node);else{selectedSpatialNode=null;$('selection').hidden=true;}}
 else if(selectedPathId){const path=data.paths?.items.find(p=>p.id===selectedPathId);if(path){if($('selection').dataset.pathId!==path.id)showPathSelection(path);}else{selectedPathId=null;crayon.select(null);$('selection').hidden=true;}}else if(selectedNoteId){const note=(data.notes||[]).find(n=>n.id===selectedNoteId);if(note){if($('selection').dataset.noteId!==note.id)showNoteSelection(note);}else{selectedNoteId=null;$('selection').hidden=true;}}else $('selection').hidden=true;
 updateMapUI();if(data.occupancy_tiles){$('empty').hidden=data.occupancy_tiles.length>0;$('voxel-count').textContent=data.occupancy_tiles.length.toLocaleString();}if(first)fit();revealScene();
}
function pathLength(path){let total=0;for(const segment of path.segments)for(let i=1;i<segment.length;i++)total+=Math.hypot(...segment[i].map((v,k)=>v-segment[i-1][k]));return total;}
function pathSwatch(path,size=12){const swatch=document.createElement('span');swatch.className='path-swatch';swatch.style.setProperty('--swatch',path.color||'#2563eb');swatch.style.width=swatch.style.height=size+'px';return swatch;}
function showPathSelection(path){
 noteCardMode='view';
 showSelection(path.title,`${pathLength(path).toFixed(1)} m · ${plural(path.segments.length,'stroke')}`,'',{kind:'Path'});
 selectedPathId=path.id;crayon.select(path.id);const card=$('selection');card.dataset.pathId=path.id;
 const line=card.querySelector('.inspector-position');line?.classList.add('is-text');line?.prepend(pathSwatch(path));
 if(isLocalScene()){
  const actions=cardActions(actionButton('Delete',{variant:'danger-ghost',onClick:()=>{
   noteCardMode='confirm';
   const box=document.createElement('div');box.className='card-confirm';box.setAttribute('role','group');box.setAttribute('aria-label','Confirm delete');
   const question=document.createElement('p');question.textContent=`Delete “${path.title}”? This can’t be undone.`;
   const cancel=actionButton('Cancel',{onClick:()=>showPathSelection(path)});
   const remove=actionButton('Delete path',{variant:'danger',onClick:async()=>{remove.disabled=true;if(await deletePath(path,activeMap,current?.frame,remove)!==false){noteCardMode='view';toast(`Deleted “${path.title}”`);}}});
   remove.dataset.deletePath=path.id;
   box.append(question,cardActions(cancel,remove));actions.replaceWith(box);cancel.focus();
   box.addEventListener('keydown',e=>{if(e.key==='Escape'){e.stopPropagation();showPathSelection(path);}});
  }}));
  card.append(actions);
 }else{const lock=document.createElement('p');lock.className='card-readonly';lock.append(icon(icons.lock,14),document.createTextNode('Read-only · Paths can be changed in a local Home scene'));card.append(lock);}
 renderNotes();
}
function focusPath(path){
 const box=new THREE.Box3().setFromPoints(path.segments.flat().map(p=>new THREE.Vector3(...p))),center=box.getCenter(new THREE.Vector3());
 const distance=Math.max(2,box.getSize(new THREE.Vector3()).length());
 if(attachedCamera.isActive())attachedCamera.detach();
 cameraPan.start(center.clone().add(new THREE.Vector3(distance*.4,distance,distance)),center);showPathSelection(path);
}
async function deletePath(path,map=activeMap,frame=current?.frame,button=null){
  if(deletingPathId||activeMap!==map||current?.frame!==frame)return false;
  deletingPathId=path.id;if(button)button.disabled=true;
  const index=(current.paths?.items||[]).findIndex(p=>p.id===path.id);
  const status=document.createElement('div');status.setAttribute('role','status');if(button)button.after(status);else $('selection').append(status);status.textContent='Deleting…';
  try{
   const result=await api('/api/paths/'+encodeURIComponent(path.id)+'?scene='+encodeURIComponent(map),undefined,'DELETE');
   if(activeMap!==map||current?.frame!==frame)return false;
   current.paths=result;crayon.update();
   const stillSelected=selectedPathId===path.id;
   if(stillSelected){
    const next=result.items[Math.min(Math.max(index,0),result.items.length-1)];
    if(next){showPathSelection(next);}
    else{selectedPathId=null;crayon.select(null);$('selection').hidden=true;}
   }
   renderNotes();lastScene=null;hideAr();return stillSelected;
  }catch(e){status.textContent=e.message;return false;}finally{
   deletingPathId=null;document.querySelectorAll('[data-delete-path]').forEach(b=>b.disabled=false);
  }
}
let backspaceQueue=0,backspaceDeleting=false;
window.addEventListener('keydown',async e=>{
 if(e.key!=='Backspace'||e.defaultPrevented||e.isComposing||e.ctrlKey||e.metaKey||e.altKey||isEditing(e.target)||isEditing(document.activeElement)||document.querySelector('dialog[open]')||!selectedPathId||!isLocalScene()||crayon.isActive())return;
 e.preventDefault();if(e.repeat)return;
 if(deletingPathId&&!backspaceDeleting)return;
 backspaceQueue++;if(backspaceDeleting)return;
 backspaceDeleting=true;const map=activeMap,frame=current?.frame;
 try{while(backspaceQueue>0){
  backspaceQueue--;if(map!==activeMap||frame!==current?.frame)break;
  const path=current?.paths?.items.find(p=>p.id===selectedPathId);
  if(!path||!await deletePath(path,map,frame))break;
 }}finally{backspaceQueue=0;backspaceDeleting=false;}
});
function closeUp(point){
 if(attachedCamera.isActive())attachedCamera.detach();
 const target=new THREE.Vector3(...point),direction=camera.position.clone().sub(target);
 if(direction.lengthSq()<1e-8)direction.set(0,.3,1);
 const position=target.clone().addScaledVector(direction.normalize(),2);
 cameraPan.start(position,target);
}
function focusNote(note){closeUp(note.position);showNoteSelection(note);}
// The inspector card is the one home for a note: view, edit, create and delete-confirm modes.
let noteCardMode='view';
function noteReadOnlyReason(note){
 if(note.anchor_robot||note.robot_pose)return 'Attached to a robot';
 if(note.phone_pose)return 'Placed by a phone';
 if(isRunner()||activeMap==='domain')return 'This map is read-only';
 return null;
}
function noteAnchorText(note){
 if(note.robot_pose)return 'Live robot position';
 if(note.phone_pose)return 'Live phone position';
 if(note.anchor_surface)return 'On a surface';
 if(note.anchor_robot)return `${note.height_m} m above ${note.text} base`;
 return note.anchor_portal?`${note.height_m} m above portal ${note.anchor_portal}`:'Anchored in Home';
}
function cardActions(...buttons){const row=document.createElement('div');row.className='card-actions';row.append(...buttons);return row;}
function showNoteSelection(note){
 noteCardMode='view';
 showSelection(noteTitle(note),'',noteAnchorText(note),{kind:'Note'});
 selectedNoteId=note.id;const card=$('selection');card.dataset.noteId=note.id;
 if(note.details?.description){const description=document.createElement('p');description.className='card-description';description.textContent=note.details.description;card.append(description);}
 if(note.details?.photo)card.append(notePhoto(note));
 const technical=document.createElement('details');technical.className='technical-details';technical.innerHTML='<summary>Technical details</summary>';
 const fields=document.createElement('dl');fields.className='spatial-fields';technical.append(fields);
 const field=(name,value,copy)=>{const term=document.createElement('dt'),description=document.createElement('dd'),text=document.createElement('span');term.textContent=name;text.className='mono-id';text.textContent=value;description.append(text);if(copy)description.append(copyButton(value,'Copy '+name.toLowerCase()));fields.append(term,description);};
 field('Position',`[${note.position.map(v=>v.toFixed(2)).join(', ')}] m`,true);
 if(note.anchor_portal)field('Anchor portal',note.anchor_portal,true);
 field('Note ID',note.id,true);
 card.append(technical);
 const reason=noteReadOnlyReason(note);
 if(reason){const lock=document.createElement('p');lock.className='card-readonly';lock.append(icon(icons.lock,14),document.createTextNode('Read-only · '+reason));card.append(lock);}
 else card.append(cardActions(actionButton('Edit',{onClick:()=>editNoteCard(note)}),actionButton('Delete',{variant:'danger-ghost',onClick:()=>confirmNoteDelete(note)})));
 renderNotes();
}
function confirmNoteDelete(note){
 noteCardMode='confirm';
 const card=$('selection'),old=card.querySelector('.card-actions');
 const box=document.createElement('div');box.className='card-confirm';box.setAttribute('role','group');box.setAttribute('aria-label','Confirm delete');
 const question=document.createElement('p');question.textContent=`Delete “${noteTitle(note)}”? This can’t be undone.`;
 const status=document.createElement('p');status.className='form-status';status.setAttribute('role','status');
 const cancel=actionButton('Cancel',{onClick:()=>showNoteSelection(note)});
 const remove=actionButton('Delete note',{variant:'danger',onClick:async()=>{
  if(deletingNoteId)return;deletingNoteId=note.id;remove.disabled=cancel.disabled=true;status.textContent='Deleting…';
  try{await api(noteUrl(note.id),undefined,'DELETE');noteCardMode='view';clearSelection();hideAr();lastScene=null;await refresh();toast(`Deleted “${noteTitle(note)}”`);notesList.querySelector('.list-row')?.focus();}
  catch(e){status.textContent=e.message;remove.disabled=cancel.disabled=false;}
  finally{deletingNoteId=null;}
 }});
 box.append(question,cardActions(cancel,remove),status);old.replaceWith(box);cancel.focus();
 box.addEventListener('keydown',e=>{if(e.key==='Escape'){e.stopPropagation();showNoteSelection(note);}});
}
function noteForm({heading,note,id}){
 const card=$('selection');card.replaceChildren();card.hidden=false;
 const form=document.createElement('form');form.className='note-card-form';if(id)form.id=id;form.noValidate=true;
 const chip=document.createElement('span');chip.className='inspector-kind';chip.textContent=heading;
 const status=document.createElement('p');status.className='form-status';status.setAttribute('role','status');
 const title=formField({label:'Title',id:id?'label-dialog-text':undefined,max:120,value:noteTitle(note),required:true,placeholder:'What should people know here?'});
 form.append(chip,status,title.field);
 const fields=noteFields(form,note);
 status.hidden=!!id;
 form.addEventListener('keydown',e=>{if(e.key==='Escape'){e.preventDefault();e.stopPropagation();form.querySelector('[data-cancel]')?.click();}});
 card.append(form);
 return {form,status,title:title.input,titleField:title.field,fields};
}
function editNoteCard(note){
 noteCardMode='edit';const editingMap=activeMap;
 const {form,status,title,fields}=noteForm({heading:'Edit note',note});
 const cancel=actionButton('Cancel',{onClick:()=>showNoteSelection(note)});cancel.dataset.cancel='';
 const save=actionButton('Save changes',{variant:'primary',type:'submit'});
 form.append(cardActions(cancel,save));title.focus();
 form.onsubmit=async e=>{e.preventDefault();if(!title.value.trim()){status.textContent='Add a title.';title.setAttribute('aria-invalid','true');title.focus();return;}
  save.disabled=true;status.textContent='Saving…';
  try{const details=await fields.read(title.value);const result=await api(noteUrl(note.id,editingMap),{text:title.value,details},'PATCH');
   lastScene=null;await refresh();const updated=(current?.notes||[]).find(n=>n.id===(result.id||note.id))||note;showNoteSelection(updated);toast('Note saved');}
  catch(err){status.textContent=err.message;save.disabled=false;}};
}
function focusPortal(p){const center=new THREE.Vector3(...p.pose_in_map.translation);cameraPan.start(center.clone().add(new THREE.Vector3(.35,.65,.65)),center);showSelection(p.anchor_id,`[${p.pose_in_map.translation.map(v=>v.toFixed(4)).join(', ')}] m`,`${Math.round(p.side_length_m*100)} cm · ${p.anchor_id.startsWith('home:')?'Home reference aligned via measured portal':current?.local_map?'measured camera observation':'stored height, not mounting height'}`);}
function spatialPosition(kind,node){
 if(kind==='anchor')return node.pose?.translation;
 if(!node.parent_anchor)return node.position;
 const pose=current?.landmarks?.anchors?.[node.parent_anchor]?.pose;
 if(!pose||pose.from_frame_id!==node.frame)return null;
 const [w,x,y,z]=pose.rotation_wxyz;
 return new THREE.Vector3(...node.position).applyQuaternion(new THREE.Quaternion(x,y,z,w)).add(new THREE.Vector3(...pose.translation)).toArray();
}
function spatialCoordinates(position){return `[${position.map(v=>(Math.abs(v)<5e-5?0:v).toFixed(4)).join(', ')}] m`;}
function spatialComponent(component){return component?`${component.peer_id} / ${component.component_id} (${component.manifest_hash})`:'Not recorded';}
function showSpatialSelection(kind,node,focus=false){
 const position=spatialPosition(kind,node),registry=current.landmarks;
 if(focus&&position){const center=new THREE.Vector3(...position);cameraPan.start(center.clone().add(new THREE.Vector3(.5,.65,1)),center);}
 const parent=kind==='landmark'&&node.parent_anchor?registry.anchors?.[node.parent_anchor]:null;
 showSelection(parent?landmarkTitle(node):node.id,position?spatialCoordinates(position):'Map position unavailable',kind==='anchor'?'Full pose':'Position only',
  {kind:kind==='anchor'?'Anchor':'Landmark',monoTitle:!parent,subtitle:parent?`of anchor ${parent.id}`:undefined,onSubtitle:()=>showSpatialSelection('anchor',parent,true)});
 selectedSpatialNode={kind,id:node.id};const selection=$('selection');selection.dataset.spatialKind=kind;selection.dataset.spatialId=node.id;
 // Everyday facts stay visible; frames, rotation and provenance sit behind "Technical details".
 const fields=document.createElement('dl');fields.className='spatial-fields';
 const technical=document.createElement('details');technical.className='technical-details';technical.innerHTML='<summary>Technical details</summary>';
 const technicalFields=document.createElement('dl');technicalFields.className='spatial-fields';technical.append(technicalFields);
 function field(name,value,list=fields,{mono=false,copy=false}={}){const term=document.createElement('dt'),description=document.createElement('dd');term.textContent=name;const text=document.createElement('span');text.textContent=value;if(mono)text.className='mono-id';description.append(text);if(copy)description.append(copyButton(value,'Copy '+name.toLowerCase()));list.append(term,description);}
 field('Type',node.recognition.landmark_type);field('Identity',node.recognition.identity,fields,{mono:true,copy:true});
 if(kind==='anchor'){const portal=current.portals?.find(p=>p.anchor_id===node.id);if(portal)field('Portal size',`${(portal.side_length_m*1000).toLocaleString()} mm encoded square`);}
 else field('Stored position',spatialCoordinates(node.position));
 const tech={mono:true,copy:true};
 field('Map frame',registry.frame,technicalFields,tech);
 if(kind==='anchor'){field('Local frame',node.pose.from_frame_id,technicalFields,tech);field('Rotation (w, x, y, z)',node.pose.rotation_wxyz.map(v=>v.toFixed(4)).join(', '),technicalFields,tech);}
 else field('Position frame',node.frame,technicalFields,tech);
 const provenance=node.provenance||{};
 field('Detected by',spatialComponent(provenance.detected_by),technicalFields,{mono:true});field('Mapped by',spatialComponent(provenance.mapped_by),technicalFields,{mono:true});
 if(provenance.source_observation){const source=provenance.source_observation;field('Source observation',`${source.output.output_id} · sequence ${source.sequence} · ${source.timestamp_ns} ns`,technicalFields,tech);}
 selection.append(fields,technical);if(kind==='landmark'&&node.parent_anchor&&!expandedAnchors.has(node.parent_anchor)){expandedAnchors.add(node.parent_anchor);renderSpatialLists();}updateSpatialSelection();
 if(focus&&window.matchMedia('(max-width:720px)').matches)openPanel(null);
}
function updateSpatialSelection(){
 for(const row of spatialBrowser.querySelectorAll('[data-spatial-kind]')){const selected=row.dataset.spatialKind===selectedSpatialNode?.kind&&row.dataset.spatialId===selectedSpatialNode?.id;if(selected)markSelected(row,'spatial',row.dataset.spatialKind+':'+row.dataset.spatialId);else row.removeAttribute('aria-current');}
}
const expandedAnchors=new Set();
const landmarkTitle=node=>{const name=node.id.includes('/')?node.id.slice(node.id.lastIndexOf('/')+1):node.id;return name.replace(/[_-]+/g,' ').replace(/^./,c=>c.toUpperCase());};
function spatialRow(kind,node,parent){
 const row=kind==='anchor'
  ?listRow({title:node.id,meta:node.recognition.landmark_type,tile:icons.anchor,accent:'emerald',mono:true,label:`Anchor ${node.id}`,onSelect:e=>{showSpatialSelection(kind,node,true);focusCard(e,()=>spatialTree.querySelector(`.list-row[data-spatial-id="${CSS.escape(node.id)}"]`));}})
  :parent?listRow({title:landmarkTitle(node),label:`Landmark ${landmarkTitle(node)} of ${parent}`,onSelect:e=>{showSpatialSelection(kind,node,true);focusCard(e,()=>spatialTree.querySelector(`.list-row[data-spatial-id="${CSS.escape(node.id)}"]`));}})
  :listRow({title:node.id,meta:node.recognition.landmark_type,tile:icons.landmark,mono:true,label:`Landmark ${node.id}`,onSelect:e=>{showSpatialSelection(kind,node,true);focusCard(e,()=>spatialTree.querySelector(`.list-row[data-spatial-id="${CSS.escape(node.id)}"]`));}});
 row.dataset.spatialKind=kind;row.dataset.spatialId=node.id;
 return row;
}
function renderSpatialLists(){
 const registry=current?.landmarks,query=$('spatial-search').value.trim().toLowerCase();
 const signature=JSON.stringify([activeMap,current?.frame,registry,query,[...expandedAnchors]]);
 if(signature===spatialSignature){updateSpatialSelection();return;}spatialSignature=signature;
 spatialStatus.textContent=!current?'Waiting for the selected scenegraph…':!registry?'This scenegraph does not publish anchors and landmarks.':'';spatialStatus.hidden=!spatialStatus.textContent;
 const matches=node=>[node.id,node.recognition.landmark_type,node.recognition.identity,node.parent_anchor].filter(Boolean).join(' ').toLowerCase().includes(query);
 const anchors=Object.values(registry?.anchors||{}).sort((a,b)=>a.id.localeCompare(b.id));
 const landmarks=Object.values(registry?.landmarks||{}).sort((a,b)=>a.id.localeCompare(b.id));
 $('anchor-count').textContent=query?`${anchors.filter(matches).length} / ${anchors.length}`:String(anchors.length);
 $('landmark-count').textContent=query?`${landmarks.filter(matches).length} / ${landmarks.length}`:String(landmarks.length);
 const focusedId=spatialTree.contains(document.activeElement)?[document.activeElement.dataset.spatialId||document.activeElement.dataset.anchorToggle,document.activeElement.dataset.anchorToggle?'toggle':'row']:null;
 const nothing=!anchors.length&&!landmarks.length;spatialBrowser.querySelector('.search-field').hidden=nothing&&!query;spatialCounts.hidden=nothing;
 spatialTree.replaceChildren();
 const anchorIds=new Set(anchors.map(a=>a.id));
 for(const anchor of anchors){
  const children=landmarks.filter(l=>l.parent_anchor===anchor.id),shownChildren=query?children.filter(matches):children;
  if(query&&!matches(anchor)&&!shownChildren.length)continue;
  const open=query?shownChildren.length>0:expandedAnchors.has(anchor.id);
  const item=document.createElement('div');item.className='tree-item';
  const line=document.createElement('div');line.className='tree-line';
  if(children.length){
   const toggle=document.createElement('button');toggle.type='button';toggle.className='tree-toggle';toggle.dataset.anchorToggle=anchor.id;
   toggle.setAttribute('aria-expanded',String(open));toggle.setAttribute('aria-controls','landmarks-'+anchor.id);toggle.setAttribute('aria-label',`Landmarks of ${anchor.id}`);toggle.append(icon(icons.chevron));
   toggle.onclick=()=>{const group=()=>document.getElementById('landmarks-'+anchor.id);if(expandedAnchors.has(anchor.id))conceal(group(),()=>{expandedAnchors.delete(anchor.id);renderSpatialLists();});else{expandedAnchors.add(anchor.id);renderSpatialLists();reveal(group());}};
   line.append(toggle);
  }else line.append(Object.assign(document.createElement('span'),{className:'tree-toggle-space'}));
  const row=spatialRow('anchor',anchor);const kindName=anchor.recognition.landmark_type.split('.').pop().replace(/^./,c=>c.toUpperCase());row.querySelector('.list-row-meta').textContent=children.length?`${kindName} · ${plural(children.length,'landmark')} · position only`:kindName;
  line.append(row);item.append(line);
  if(children.length){const group=document.createElement('div');group.className='tree-children';group.id='landmarks-'+anchor.id;group.hidden=!open;group.append(...shownChildren.map(child=>spatialRow('landmark',child,anchor.id)));item.append(group);}
  spatialTree.append(item);
 }
 const orphans=landmarks.filter(l=>!anchorIds.has(l.parent_anchor)&&(!query||matches(l)));
 if(orphans.length){const group=document.createElement('div');group.className='tree-orphans';group.append(sectionHeader('Other landmarks',orphans.length),...orphans.map(node=>spatialRow('landmark',node)));spatialTree.append(group);}
 if(!spatialTree.children.length)spatialTree.append(emptyState(!registry?'Not available':query?'No matches':'No anchors in this scene',query?{label:'Clear search',onClick:()=>{$('spatial-search').value='';$('spatial-search').dispatchEvent(new Event('input'));$('spatial-search').focus();}}:null));
 if(focusedId)spatialTree.querySelector(focusedId[1]==='toggle'?`[data-anchor-toggle="${CSS.escape(focusedId[0])}"]`:`.list-row[data-spatial-id="${CSS.escape(focusedId[0])}"]`)?.focus();
 updateSpatialSelection();
}
function updateMapUI(){
 const local=Boolean(current?.local_map)||((isRunner()||activeMap==='home-live')&&!current);
 $('frame-info').textContent=activeMap==='home-live'?'Live origin: UCOF1J40N7O · shared Home frame':local?(current?.display_alignment?`Display frame: ${current.frame} · robot-native frame unchanged`:'Local origin: robot boot pose · fixed frame'): activeMap==='domain'?'Stored domain frame · OpenGL · Y up · meters':'Home origin: UCOF1J40N7O · OpenGL · Y up · meters';
 $('portal-info').textContent=activeMap==='home-live'?'Blue: measured portals. Each robot calibrates through its own observed canonical portal.':local?'Blue: camera observations. Red: Home references aligned through a shared portal. The robot boot origin stays fixed.':'Stored Y = 0 for every portal. Actual mounting heights are unverified.';
 $('portal-swatch').style.background=local?'#2477ec':'#e53935';
 const domain=activeMap==='domain';$('domain-info').hidden=false;$('demo-controls').hidden=domain||!current?.demo;
 $('scene-title').textContent=activeMap==='home-live'?'Home live map':isRunner()?runnerDisplay()+' scenegraph':domain?'13 stored domain portals':(current?.demo?'Domain + synthetic scene':current?.environment_name||'Canonical portals');
 $('map-authority').textContent=isRunner()?runnerDisplay()+' mapper':'Home Server';
 $('scene-badge').textContent=activeMap==='home-live'?'HOME LIVE MAP':isRunner()?'GALBOT SCENEGRAPH':domain?'STORED DOMAIN POSES':(current?.demo?'DOMAIN + SYNTHETIC SCENE':'CANONICAL PORTALS');
 $('scene-note').textContent=activeMap==='home-live'?'Mapped on Home from both robots’ raw observations, each calibrated through known portal poses. UCOF1J40N7O defines the fixed origin.':local?(current?.display_alignment?`Display aligned via ${current.display_alignment.portal}. Robot data remains in its native frame.`:current?.frame_transform?.validity==='stale'?'Robot-native map. Portal transform is stale; waiting for a fresh tracked observation.':'Robot-native map. Waiting for a shared portal transform from its Catalog.'):domain?'Supplied poses and sizes. Grid is a coordinate reference, not a measured floor.':(current?.demo?'Illustrative L-shaped workspace fitted to the marker footprint. Furniture and walls are synthetic; stored portal poses are unchanged.':'Canonical portal record with supplied poses and sizes. Live geometry is in Home live map.');
 document.querySelector('.flow').hidden=domain||!current?.demo;
 $('demo-view').hidden=!current?.demo;
 setSnapshotLinks(activeMap==='home-live'?'/api/home-live-snapshot':isRunner()?runnerUrl('/api/runner-snapshot'):domain?'/api/domain-snapshot':'/api/snapshots');
 ground.visible=Boolean(current?.demo);scene.fog=null;grid.position.x=local?0:-5;grid.position.z=local?0:3;
 const list=$('portal-list');list.replaceChildren();{for(const p of (current?.portals||[]).filter(p=>p.anchor_id.length===11||p.anchor_id.startsWith('home:'))){const button=document.createElement('button');button.className='portal-row';button.textContent=p.anchor_id;const size=document.createElement('span');size.textContent=`${Math.round(p.side_length_m*100)} cm`;button.append(size);button.onclick=()=>focusPortal(p);list.append(button);}}
 list.hidden=Boolean(current?.landmarks);renderSpatialLists();
 if(isLocalScene()) {
  $('scene-title').textContent=current?.environment_name||'Local scenegraph';
  $('scene-badge').textContent='SAVED LOCAL SCENEGRAPH';
  $('frame-info').textContent=(current?.frame||'')+' · OpenGL · meters';
  $('portal-info').textContent=current?.frame_description||'';
  $('scene-note').textContent='Independent Home scenegraph. Pair your phone with this scene and scan its Portal to share depth and annotations. Grid assumes a flat floor at Y = 0.';
  $('mapping-status').textContent=current?.mobile_enabled?'Phone mapping · '+(current.integrated_observations||0)+' scans':'Saved reference · phone mapping unavailable';
  $('mapping-detail').textContent=(current?.portals.length||0)+' Portal(s) · observations and notes remain separate from other scenes';
  setSnapshotLinks(localSceneUrl()+'/snapshot');
  grid.position.x=0;grid.position.z=0;
 }
 if(activeMap.startsWith('peer-scene:') && current) {
  $('scene-title').textContent=current.product_id||'Peer scenegraph';
  $('map-authority').textContent=current.authority_peer_id;
  $('scene-badge').textContent=current.local_map?'ROBOT-LOCAL MAP':'PEER SCENEGRAPH';
  $('frame-info').textContent=current.frame+' · '+current.convention;
  $('portal-info').textContent=current.local_map?'Independent robot frame · not merged into Home':'Publisher-owned scenegraph';
  $('scene-note').textContent=current.local_mapping?.detail||'Discovered SDK scenegraph product · read-only';
  $('mapping-status').textContent=(current.integrated_observations||0)+' scans integrated';
  $('mapping-detail').textContent='Latest retained map from the publishing peer';
  if(current.product_id==='phone-local-voxels'){
   $('scene-title').textContent='Phone · local voxel map';
   $('scene-badge').textContent='PHONE-LOCAL MAP';
   $('portal-info').textContent='Independent phone AR session frame · not merged into Home';
   $('scene-note').textContent='Phone-owned 10 cm voxels · read directly from its SDK Catalog Product';
   $('mapping-status').textContent='Phone-local depth mapping';
  }
  setSnapshotLinks(peerSceneUrl(activeMap));
  if(current.layer_type==='occupancy_2d'){
   $('scene-badge').textContent='OCCUPANCY 2D';
   $('scene-note').textContent='Robot-owned 10 cm occupancy · first meter above the flat floor · 2 m cells · unaligned';
   $('mapping-status').textContent='Occupancy · '+current.occupancy_status;
   const counts=current.occupancy_counts;
   $('mapping-detail').textContent=counts.occupied+' occupied · '+counts.free+' free · '+counts.unknown+' unknown';
   $('portal-info').textContent='Coral: occupied · green: free · slate: unknown. Not merged into Home.';
  }
 }
 if(activeMap.startsWith('phone:')) {
  $('scene-title').textContent='Phone · local voxel map';
  $('map-authority').textContent='Phone peer · AR depth';
  $('scene-badge').textContent='UNALIGNED PHONE MAP';
  $('frame-info').textContent='Phone AR session · OpenGL · Y up · meters';
  $('portal-info').textContent='No Portal calibration. Not merged into Home.';
  $('scene-note').textContent='Real depth hits accumulated into 10 cm voxels on the phone. Tracking loss starts a new map. Unknown space is not free space.';
  $('mapping-status').textContent='Phone-local depth mapping';
  $('mapping-detail').textContent='Unaligned · read-only · removed after 10 seconds without updates';
  setSnapshotLinks('/api/phone-local-scene?peer='+encodeURIComponent(activeMap.slice(6)));
 }
 if(isSimulation()){
  const occupancy=activeMap==='sim-occupancy';
  $('frame-info').textContent='Synthetic room · flat floor Y = 0 · meters';
  $('portal-info').textContent=occupancy?'Coral: occupied (100) · green: free (0) · slate: unknown (-1). Blank areas are also unknown.':'10 cm voxels · synthetic walls, floor and central obstacle';
  $('scene-title').textContent=occupancy?'Simulated robot · occupancy 2D':'Simulated robot · voxels';
  $('map-authority').textContent=occupancy?'Home occupancy mapper':'Simulated robot peer';
  $('scene-badge').textContent='SIMULATION · '+(occupancy?'OCCUPANCY 2D':'VOXELS');
  const robotCount=Object.keys(current?.local_mapping?.runners||{}).length;
  $('scene-note').textContent=occupancy?'Accumulated occupancy from robot observations. Outside view retains its last evidence; obstacles clear only when observed free. No time decay; mapper: 1 Hz.':`${robotCount||1} robot(s) producing live voxel observations with a 1.5 m footprint. A low block shuttles every 12 seconds.`;
  $('mapping-status').textContent='Simulation · '+(current?.simulation_status||'starting')+(occupancy?' · '+(current?.occupancy_status||'waiting'):'');
  $('mapping-detail').textContent=current?.simulation_detail||'Waiting for the synthetic peer';
  setSnapshotLinks('/api/simulation/'+(occupancy?'occupancy':'voxels')+'?snapshot=true');
  grid.position.x=0;grid.position.z=0;
 }
}
$('map-select').onchange=async()=>{activeMap=$('map-select').value;selectedPathId=null;crayon.select(null);selectedNoteId=null;selectedSpatialNode=null;lastScene=null;current=null;crayon.update();hideRobots();selectable=[];Object.values(groups).forEach(clear);$('selection').hidden=true;updateMapUI();$('scene-title').textContent='Loading selected scene…';$('voxel-count').textContent='—';$('observation-count').textContent='—';$('object-count').textContent='—';try{await refresh();}catch{$('scene-title').textContent=isRunner()?'Waiting for Galbot scenegraph':'Scene unavailable';}};
// Closing the card returns focus to whatever opened it (a row), so keyboard users aren't dropped on the page.
function closeCard(){const back=cardReturn;clearSelection();back?.()?.focus?.();}
// Snapshot links: the legacy pair and the account menu's pair always point at the selected scene.
function setSnapshotLinks(url){for(const link of document.querySelectorAll('#inspect-snapshot,.export,[data-snapshot-link]'))link.href=url;}
function setConnection(text){if($('connection').textContent!==text)$('connection').textContent=text;}
function clearSelection(){const volumeOpen=noteCardMode==='volume';noteCardMode='view';if(volumeOpen)semanticVolumes.closed();placementGeneration++;placement=null;selectedPathId=null;crayon.select(null);selectedNoteId=null;selectedSpatialNode=null;updateSpatialSelection();$('selection').hidden=true;if(scene.userData.placementPin)scene.userData.placementPin.visible=false;renderNotes();}
let cardReturn=null;
// Keyboard activation moves focus into the card; Escape brings it back to the row.
function focusCard(event,back){cardReturn=back;if(event?.detail!==0)return;const heading=$('selection').querySelector('.card-title,:scope>strong');if(heading){heading.tabIndex=-1;heading.focus();}}
function showSelection(title,body,note,{kind,subtitle,onSubtitle,monoTitle=false,facts}={}){if(noteCardMode==='volume'){noteCardMode='view';semanticVolumes.closed();}selectedPathId=null;crayon?.select(null);delete $('selection').dataset.pathId;selectedNoteId=null;selectedSpatialNode=null;updateSpatialSelection();const el=$('selection');delete el.dataset.noteId;delete el.dataset.spatialId;delete el.dataset.spatialKind;el.replaceChildren();
 const close=document.createElement('button');close.type='button';close.className='inspector-close';close.setAttribute('aria-label','Close inspector');close.append(icon(icons.close));close.onclick=closeCard;el.append(close);
 if(kind){const chip=document.createElement('span');chip.className='inspector-kind';chip.textContent=kind;el.append(chip);}
 const strong=document.createElement('strong');strong.textContent=title;if(monoTitle)strong.className='mono-id';el.append(strong);
 if(subtitle){const parent=document.createElement('button');parent.type='button';parent.className='text-action inspector-parent';parent.textContent=subtitle;parent.onclick=onSubtitle;el.append(parent);}
 const line=document.createElement('div');line.className='inspector-position';line.textContent=body;const small=document.createElement('small');small.textContent=note;if(body)el.append(line);if(note)el.append(small);
 if(facts?.length)el.append(factsList(facts));el.hidden=false;}
// Label/value rows for a card body; [label, value, copyable].
function factsList(facts){const list=document.createElement('dl');list.className='spatial-fields card-facts';
 for(const [name,value,copy] of facts){const term=document.createElement('dt'),description=document.createElement('dd'),text=document.createElement('span');term.textContent=name;text.textContent=value;if(copy)text.className='mono-id';description.append(text);if(copy)description.append(copyButton(String(value),'Copy '+name.toLowerCase()));list.append(term,description);}
 return list;}
// Pretty-printed JSON as text nodes, with keys, strings, numbers and literals wrapped for colour.
function jsonTokens(json){const out=[];let at=0;
 for(const m of json.matchAll(/("(?:\\.|[^"\\])*")(\s*:)?|\b(?:true|false|null)\b|-?\d+(?:\.\d+)?(?:[eE][+-]?\d+)?/g)){if(m.index>at)out.push(json.slice(at,m.index));
  const span=document.createElement('span');span.className='json-'+(m[2]?'key':m[1]?'string':/\d/.test(m[0])?'number':'literal');span.textContent=m[1]||m[0];out.push(span);if(m[2])out.push(m[2]);at=m.index+m[0].length;}
 out.push(json.slice(at));return out;}
const xyz=(values,digits=2)=>['x','y','z'].map((axis,i)=>`${axis} ${values[i].toFixed(digits)}`).join('  ')+' m';
// Every card variant shares one anatomy: a header (a tile in the section colour, the kind and the title), the
// close button in the corner, then the body. Builders keep emitting a kind chip and title; this turns them
// into the header, so notes, paths, anchors, volumes and scene picks all match. It also runs the card motion.
const CUBE_ICON='M9 1.5 15.5 5v8L9 16.5 2.5 13V5ZM2.5 5 9 8.5l6.5-3.5M9 8.5v8';
const CARD_KINDS=[[/note/i,'amber',icons.note],[/path/i,'amber',icons.pen],[/volume/i,'violet',CUBE_ICON],[/anchor/i,'emerald',icons.anchor],[/landmark/i,'emerald',icons.landmark],[/portal/i,'emerald',icons.pin],[/robot/i,'cyan',icons.device],[/.*/,'zinc',CUBE_ICON]];
function decorateCard(card){
 if(!card.querySelector(':scope>.inspector-close')){const close=document.createElement('button');close.type='button';close.className='inspector-close';close.setAttribute('aria-label','Close inspector');close.append(icon(icons.close));close.onclick=closeCard;card.prepend(close);}
 const chip=card.querySelector('.inspector-kind');if(!chip||chip.closest('.card-head'))return;
 const [,accent,path]=CARD_KINDS.find(([pattern])=>pattern.test(chip.textContent));
 const head=document.createElement('div');head.className='card-head';const tile=document.createElement('span');tile.className='card-tile';tile.dataset.accent=accent;tile.append(icon(path,16));
 const text=document.createElement('div');text.className='card-head-text';const title=chip.nextElementSibling?.matches('strong')?chip.nextElementSibling:null;
 chip.replaceWith(head);chip.className='card-kind';text.append(chip);
 if(title){title.classList.add('card-title');title.id='card-title';text.append(title);}
 head.append(tile,text);card.setAttribute('aria-labelledby',title?'card-title':'');
}
{
 const card=$('selection');card.setAttribute('role','region');let shownAs=null;
 const signature=()=>card.querySelector('.card-kind,.inspector-kind')?.textContent+'|'+(card.querySelector('.card-title,:scope>strong')?.textContent||'');
 const observer=new MutationObserver(()=>{
  if(card.hidden){shownAs=null;return;}
  decorateCard(card);observer.takeRecords();
  const now=signature();if(now===shownAs)return;const entering=shownAs===null;shownAs=now;
  if(matchMedia('(prefers-reduced-motion: reduce)').matches)return;
  if(entering)card.animate([{opacity:0,transform:'translateX(14px) scale(.97)'},{opacity:1,transform:'none'}],{duration:440,easing:'cubic-bezier(.3,1.25,.5,1)'});
  else for(const [i,node] of [...card.children].filter(n=>!n.matches('.inspector-close')).slice(0,4).entries())node.animate([{opacity:0,transform:'translateY(6px)'},{opacity:1,transform:'none'}],{duration:280,delay:i*40,easing:'cubic-bezier(.2,.8,.2,1)',fill:'backwards'});
 });
 observer.observe(card,{childList:true,attributes:true,attributeFilter:['hidden']});
}
document.addEventListener('keydown',e=>{if(e.key==='Escape'&&!$('selection').hidden&&!e.target.closest('input,textarea,select,dialog')){const back=cardReturn;clearSelection();back?.()?.focus();}});
let pointerStart;
renderer.domElement.addEventListener('pointerdown',e=>{pointerStart=[e.clientX,e.clientY];});
renderer.domElement.addEventListener('pointerup',e=>{
 if(e.semanticVolume)return;
 if(e.button!==0)return;
 if(!current||!pointerStart||Math.hypot(e.clientX-pointerStart[0],e.clientY-pointerStart[1])>4)return;
 if(placingNote){const target=placementTarget;stopNotePlacement();const box=renderer.domElement.getBoundingClientRect(),pick=new THREE.Raycaster();pick.setFromCamera(new THREE.Vector2((e.clientX-box.left)/box.width*2-1,1-(e.clientY-box.top)/box.height*2),camera);const request={source:'scene',frame:current.frame,origin:pick.ray.origin.toArray(),direction:pick.ray.direction.toArray()};if(target)target(request);else beginLabel(request);return;}
 const r=renderer.domElement.getBoundingClientRect(), ray=new THREE.Raycaster();ray.setFromCamera(new THREE.Vector2((e.clientX-r.left)/r.width*2-1,-(e.clientY-r.top)/r.height*2+1),camera);
 const hits=ray.intersectObjects([...selectable,...noteLod.objects(),...crayon.objects()].filter(o=>{for(let p=o;p;p=p.parent){if(!p.visible)return false;}return true;}));
 // Nothing hit: the click moves the spot cursor (the floor ring) there. Drags orbit and never get this far.
 if(!hits.length){clearSelection();const floor=ray.ray.intersectPlane(new THREE.Plane(new THREE.Vector3(0,1,0),0),new THREE.Vector3());if(floor&&!crayon.isActive()&&!map2D.isActive())placeSpotMarker(floor);return;}const h=hits.find(h=>h.object.userData.kind==='note')||hits.find(h=>h.object.userData.kind==='note-cluster')||hits.find(h=>h.object.userData.kind==='path')||hits.find(h=>h.object.userData.kind==='portal')||hits[0],d=h.object.userData;
 // A cluster flies in until its notes stand apart again.
 if(d.kind==='note-cluster'){const offset=camera.position.clone().sub(controls.target).setLength(noteLod.clusterAt*.5);cameraPan.start(d.centre.clone().add(offset),d.centre.clone());return;}
 if(d.kind==='voxels'){const v=current.voxels[h.instanceId];showSelection('Occupied voxel','','',{kind:'Voxel',facts:[['Position',xyz(v.center),true],['Evidence',String(v.evidence)],['Cell size','10 cm']]});}
 if(d.kind==='occupancy'){const t=current.occupancy_tiles[h.instanceId],state=t.value===100?'Occupied':t.value===0?'Free':'Unknown';showSelection(state+' cell','','',{kind:'Occupancy',facts:[['State',state],['Position',xyz(t.center),true],['Value',String(t.value)],['Sample','10 cm · retrieved in 2 m cells']]});}
 if(d.kind==='path'){showPathSelection(d.path);return;}
 if(d.kind==='note')showNoteSelection(d.note);
 if(d.kind==='object'){const o=d.object;showSelection(o.label,'','',{kind:'Object',facts:[['Position',xyz(o.position),true],['Source',o.source_peer,true]]});}
 if(d.kind==='portal'&&(d.portal.anchor_id.length===11||d.portal.anchor_id.startsWith('home:'))){showSelection(d.portal.anchor_id,'','Double-click to approach',{kind:'Portal',monoTitle:true});return;}
 if(d.kind==='portal')showSelection(`Portal ${d.portal.payload.split(':').at(-1)}`,'','',{kind:'Portal',facts:[['Encoded size',`${d.portal.side_length_m.toFixed(2)} m`],['Source','Manually supplied · no backend lookup']]});
});
renderer.domElement.addEventListener('dblclick',e=>{
 if(e.button!==0||!current||crayon.isActive()||map2D.isActive())return;
 const r=renderer.domElement.getBoundingClientRect(),ray=new THREE.Raycaster();
 ray.setFromCamera(new THREE.Vector2((e.clientX-r.left)/r.width*2-1,1-(e.clientY-r.top)/r.height*2),camera);
 const hits=ray.intersectObjects(selectable.filter(o=>{for(let p=o;p;p=p.parent)if(!p.visible)return false;return true;}));
 const note=hits.find(h=>h.object.userData.kind==='note');
 if(note){focusNote(note.object.userData.note);return;}
 const volume=semanticVolumes.pick(ray);if(volume){closeUp(volume.point.toArray());return;}
 const hit=hits[0];if(hit)closeUp(hit.point.toArray());
});
async function api(path,body,method){if(path==='/api/chat'&&body){camera.updateMatrixWorld();const right=map2D.isActive()?[1,0,0]:new THREE.Vector3().setFromMatrixColumn(camera.matrixWorld,0).toArray();body={...body,viewer:{frame:current?.frame||'',mode:map2D.isActive()?'2d':'3d',screen_right:right,camera_position:camera.position.toArray(),camera_target:controls.target.toArray(),...(chatFocus?{focus_point:chatFocus}:{})}};chatFocus=null;spotMarker.visible=false;}const r=await fetch(path,body?{method:method||'POST',headers:{'Content-Type':'application/json'},body:JSON.stringify(body)}:method?{method}:undefined);const text=await r.text();let result;try{result=JSON.parse(text);}catch{throw new Error(text);}if(!r.ok){const error=new Error(result.error||text);error.status=r.status;throw error;}return result;}
function peerSceneUrl(map){return '/api/sensors?scene=true&sensor='+encodeURIComponent(map.slice(11));}
// The 3D view stays hidden until the first scene is built and framed, then fades in, so a refresh never
// shows the default grid, the camera jump or a flash of the empty state. A fallback reveals it regardless.
function revealScene(){if(!$('canvas').classList.contains('scene-loading'))return;resizeCanvas();requestAnimationFrame(()=>requestAnimationFrame(()=>$('canvas').classList.remove('scene-loading')));}
setTimeout(revealScene,5000);
async function refresh(){if(activeMap.startsWith('peer-pose:')){if(refreshing)return;refreshing=true;try{await refreshCompactTrail();}finally{refreshing=false;}return;}if(refreshing)return;refreshing=true;refreshPeer();if(!isSimulation())refreshMapping();try{const selected=activeMap;const data=await api(isLocalScene(selected)?localSceneUrl(selected):selected.startsWith('peer-scene:')?peerSceneUrl(selected):selected.startsWith('phone:')?'/api/phone-local-scene?peer='+encodeURIComponent(selected.slice(6)):isSimulation(selected)?'/api/simulation/'+selected.slice(4):selected==='home-live'?'/api/home-live-scene':isRunner(selected)?runnerUrl('/api/runner-scene',selected):selected==='domain'?'/api/domain-scene':'/api/scene');if(selected!==activeMap)return;const content=JSON.stringify({...data,remote_status:undefined});if(content!==lastScene){lastScene=content;build(data);}else {data.receivedAt=performance.now();current=data;}homeOffline=false;renderMapRows();availability.hidden=!(selected==='sim-occupancy'&&data.simulation_status!=='running');if(!availability.hidden)availabilityText('Retained on Home · robot publishers offline · no new observations');setConnection('Home Server online');}catch(e){revealScene();if(!e.status){markOffline();}else{showAvailability('Scene unavailable · '+e.message);if(activeMap==='sim-voxels'||activeMap.startsWith('peer-scene:')){Object.values(groups).forEach(clear);hideRobots();current=null;lastScene=null;}}setConnection(e.status?'Home online · scene unavailable':'Home offline');throw e;}finally{refreshing=false;}}
async function action(fn){if(busy)return;busy=true;document.querySelectorAll('#load,#project,#replay').forEach(b=>b.disabled=true);$('notice').classList.remove('error');$('notice').textContent='Integrating…';try{await fn();await refresh();}catch(e){$('notice').textContent=e.message;$('notice').classList.add('error');}finally{busy=false;$('load').disabled=false;$('project').disabled=!current?.voxels.length;$('replay').disabled=!current?.sources.iphone;}}
$('load').onclick=()=>action(async()=>{const f=await api('/api/fixtures');for(const c of f.contributions)await api('/api/contributions',c);$('notice').textContent='Two scans aligned through Portal A. Ready to project a detection.';});
$('project').onclick=()=>action(async()=>{const f=await api('/api/fixtures');const result=await api('/api/detections',f.detection);$('notice').textContent=result.applied?'Book placed at the first occupied voxel hit.':'This detection is already placed. No duplicate object created.';});
$('replay').onclick=()=>action(async()=>{const f=await api('/api/fixtures');const r=await api('/api/contributions',f.contributions[0]);$('notice').textContent=r.applied?'Contribution accepted.':'Replay ignored. Voxel evidence and object count are unchanged.';});
// Resizing reallocates the full-resolution drawing buffer, so wait until the panel slide settles
// instead of resizing on every frame of it.
let canvasResizeTimer=0;
function resizeCanvas(){const {width,height}=$('canvas').getBoundingClientRect();if(!width||!height)return;renderer.setSize(width,height);applyViewOffset();}
// The floating panel covers the left of the canvas. Instead of resizing, shift the projection so the
// scene stays centred in the visible area; the shift eases with the panel slide.
let viewOffset=0;
// Layout offsets ignore the slide transform, so this is the panel's resting edge; it is read on change, not per frame.
function measurePanelCover(){const main=document.querySelector('.shell>main');panelTarget=Math.max(0,rail.offsetLeft+rail.offsetWidth-main.offsetLeft);}
addEventListener('resize',measurePanelCover);
function applyViewOffset(){const width=renderer.domElement.clientWidth,height=renderer.domElement.clientHeight;if(!width||!height)return;
 if(viewOffset>.5){camera.aspect=(width+viewOffset)/height;camera.setViewOffset(width+viewOffset,height,0,0,width,height);}else{camera.aspect=width/height;camera.clearViewOffset();}
 camera.updateProjectionMatrix();}
function easeViewOffset(){const target=panelTarget,gap=target-viewOffset;if(Math.abs(gap)<.5){if(viewOffset!==target){viewOffset=target;applyViewOffset();}return;}viewOffset=railMotion?viewOffset+gap*.22:target;applyViewOffset();}
new ResizeObserver(()=>{clearTimeout(canvasResizeTimer);canvasResizeTimer=setTimeout(resizeCanvas,renderer.domElement.width?140:0);}).observe($('canvas'));
// Top-down flattening: as the camera looks straight down, its projection blends from perspective into a
// flat plan view of the same scene (fully flat within 2°); tilting back blends it out again.
const FLAT_START=12*Math.PI/180,FLAT_END=2*Math.PI/180,flatPerspective=new THREE.Matrix4(),flatOrtho=new THREE.Matrix4();
let flatten=0;
// Picking must follow the blended projection: rays come from unprojecting the near and far planes.
const baseSetFromCamera=THREE.Raycaster.prototype.setFromCamera;
THREE.Raycaster.prototype.setFromCamera=function(coords,cam){if(!cam.userData?.flatten)return baseSetFromCamera.call(this,coords,cam);
 this.ray.origin.set(coords.x,coords.y,-1).unproject(cam);this.ray.direction.set(coords.x,coords.y,1).unproject(cam).sub(this.ray.origin).normalize();this.camera=cam;};
function updateFlatten(){
 const polar=controls.getPolarAngle(),t=Math.min(1,Math.max(0,(FLAT_START-polar)/(FLAT_START-FLAT_END))),amount=railMotion?t*t*(3-2*t):Math.round(t);
 if(amount===0&&flatten===0)return;
 flatten=amount;camera.userData.flatten=amount;camera.updateProjectionMatrix();
 if(!amount){keepSpriteSizes(0);return;}
 flatPerspective.copy(camera.projectionMatrix);
 // An orthographic frustum matching the perspective view at the orbit target, including the dock's view offset.
 const halfHeight=camera.position.distanceTo(controls.target)*Math.tan(THREE.MathUtils.degToRad(camera.fov/2))/camera.zoom,halfWidth=halfHeight*camera.aspect;
 let left=-halfWidth,right=halfWidth,top=halfHeight,bottom=-halfHeight;
 if(camera.view?.enabled){const v=camera.view,w=right-left,h=top-bottom;left+=v.offsetX*w/v.fullWidth;right=left+v.width*w/v.fullWidth;top-=v.offsetY*h/v.fullHeight;bottom=top-v.height*h/v.fullHeight;}
 flatOrtho.makeOrthographic(left,right,top,bottom,camera.near,camera.far);
 const e=camera.projectionMatrix.elements;for(let i=0;i<16;i++)e[i]=flatPerspective.elements[i]*(1-amount)+flatOrtho.elements[i]*amount;
 camera.projectionMatrixInverse.copy(camera.projectionMatrix).invert();keepSpriteSizes(amount);
}
// Screen-sized sprites (note bubbles) rely on a perspective matrix to stay constant on screen; while the
// projection is blended, scale them so their on-screen size doesn't change. Restores them when flat ends.
const spriteView=new THREE.Vector3();
// The screen-sized sprites are gathered every 30 frames rather than walking the whole scene each frame
// (the default top-down view keeps the flatten on constantly).
let flatSprites=[],flatSpritesAge=Infinity;
function keepSpriteSizes(amount){
 camera.updateMatrixWorld();const p00=camera.projectionMatrix.elements[0],perspective00=flatPerspective.elements[0];
 if(!amount||++flatSpritesAge>30){flatSprites=[];scene.traverse(obj=>{if(obj.isSprite&&obj.material.sizeAttenuation===false)flatSprites.push(obj);});flatSpritesAge=0;}
 for(const obj of flatSprites){
  if(!amount){if(obj.userData.baseScale){obj.scale.copy(obj.userData.baseScale);delete obj.userData.baseScale;}continue;}
  const base=obj.userData.baseScale??=obj.scale.clone();
  const depth=-spriteView.setFromMatrixPosition(obj.matrixWorld).applyMatrix4(camera.matrixWorldInverse).z;
  obj.scale.copy(base).multiplyScalar(perspective00*((1-amount)*depth+amount)/p00);
 }
}
// The camera never goes beneath the grid. Orbiting is limited to above the horizon, but views the assistant sets
// (and detaching an attached camera) lift that limit, so the floor is also held here, every frame.
function keepAboveGrid(){if(attachedCamera.isActive()||map2D.isActive())return;const floor=grid.position.y+.05;if(camera.position.y<floor)camera.position.y=floor;}
renderer.setAnimationLoop(now=>{easeViewOffset();if(!cameraPan.tick(now)&&!attachedCamera.isActive())controls.update();keepAboveGrid();ghostCamera.tick(now);if(!current)phoneTrails.reset(null);const phones=phoneTrails.tick();phoneTrails.group.visible=groups.notes.visible;$('canvas').dataset.phoneGhostCount=String(phones.ghosts);$('canvas').dataset.phoneLiveCount=String(phones.live);updateFlatten();
 // Volume handles are HTML placed from this frame's camera: settle its matrices and projection first so they never trail a glide.
 camera.updateMatrixWorld();semanticVolumes.tick();noteLod.tick();renderer.render(scene,camera);});
refresh().catch(e=>{$('notice').textContent=e.message;$('notice').classList.add('error');});setInterval(()=>{if(!busy)refresh().catch(()=>{});},500);

function updatePairingDownload(){const link=$('pairing-download');link.href=isLocalScene()?localSceneUrl()+'/connection':'/api/home-connection';link.download=isLocalScene()?activeMap.slice(6)+'-connection.json':'home-connection.json';link.textContent=isLocalScene()?'Download phone connection for selected scene ↗':'Download Xmart connection ↗';}
async function refreshPeer(){try{const peer=await api('/api/peer');$('p2p-status').textContent=peer.enabled?'P2P · '+peer.status:'P2P disabled';$('p2p-detail').textContent=peer.connection?peer.connection.peer_id:peer.detail||'Configure a peer to join the network.';$('pairing-download').hidden=peer.status!=='Ready';updatePairingDownload();fleetNetwork=peer;renderFleetRows();}catch{$('p2p-status').textContent='P2P unavailable';$('pairing-download').hidden=true;fleetNetwork=null;renderFleetRows();}}

// Dynamic telemetry is independent of scene updates and synthetic peer scans.
const robots=new Map(),poseUpdates=new Map(),robotVolumes=new Map(),bodyVolumePeers=new Set();
function usesBodyVolume(key){
 const peer=key==='selected'?(current?.authority_peer_id||(activeMap.startsWith('runner:')?activeMap.slice(7):primaryRunnerPeer)):key;
 return bodyVolumePeers.has(peer)||(key==='selected'&&(current?.source_frame||current?.frame||'').startsWith('bracketbot-local:'));
}
const poseColors=['#e49b32','#985cdb','#219b96'];
let runner=null,runnerBusy=false,lastRunnerReply=0;
function hideRobots(){for(const glyph of robots.values())glyph.group.visible=false;}
// Robot models drawn instead of the pose box, by the robot's native map family.
function robotModel(frame){return frame?.startsWith('bracketbot-local:')?'bracketbot':null;}
// Swap the box glyph for the robot's URDF model. The model's root is in ROS body
// axes like the glyph; the box stays as the fallback while loading, on failure,
// and for robots without a model.
function setGlyphModel(glyph,model){
 if(glyph.model===model)return;
 glyph.model=model;
 if(glyph.robot){glyph.group.remove(glyph.robot.root);glyph.robot=null;}
 glyph.body.visible=true;
 if(model!=='bracketbot')return;
 createRobotModel().then(robot=>{
  if(glyph.model!=='bracketbot'||glyph.robot)return;
  glyph.robot=robot;glyph.group.add(robot.root);glyph.body.visible=false;applyJoints(glyph);applyRobotHealth(glyph,glyph.health);applyPointingStatus(glyph,glyph.pointing);
 }).catch(e=>console.warn('Robot model unavailable; keeping pose glyph',e));
}
// URDF joint positions published next to the pose; joints left out keep their last value.
function applyJoints(glyph){
 if(!glyph.robot||!glyph.joints)return;
 for(const [name,value] of Object.entries(glyph.joints))glyph.robot.setJoint(name,value);
}
// The glyph and URDF root use ROS body axes (+X forward, +Y left, +Z up). The base
// frame's Registry entry declares its own axes, so the turn between them is read
// from that declaration, never assumed from the robot kind.
const OPPOSITE={forward:'backward',backward:'forward',left:'right',right:'left',up:'down',down:'up'};
function rosBodyInFrame(axes){
 const along=direction=>{const v=['x','y','z'].map(a=>axes?.[a]===direction?1:axes?.[a]===OPPOSITE[direction]?-1:0);
  if(Math.abs(v[0])+Math.abs(v[1])+Math.abs(v[2])!==1)throw Error('Base frame axes undeclared');return new THREE.Vector3(...v);};
 const m=new THREE.Matrix4().makeBasis(along('forward'),along('left'),along('up'));
 if(m.determinant()<0)throw Error('Base frame is left-handed');
 return new THREE.Quaternion().setFromRotationMatrix(m);
}
function declaredGlyphPose(entry){
 const [x,y,z,w]=entry.pose.rotation_xyzw,q=new THREE.Quaternion(x,y,z,w).multiply(rosBodyInFrame(entry.base_frame?.axes));
 return {translation:entry.pose.translation,rotation_xyzw:[q.x,q.y,q.z,q.w]};
}
// Body state is shown only while fresh; Home reports its own receipt age.
const freshState=(value,ms)=>value&&value.age_ms<ms?value.state:null;
function glyphFor(key,name){
 let glyph=robots.get(key);
 if(!glyph){
  const group=new THREE.Group();scene.add(group);
  const color=poseColors[robots.size%poseColors.length];
  const material=new THREE.MeshStandardMaterial({color,transparent:true,opacity:.82,roughness:.6,depthTest:false});
  // Robot-local ROS body axes: +X forward, +Y left, +Z up; illustrative pose glyph.
  const body=new THREE.Mesh(new THREE.BoxGeometry(.42,.32,.18),material);body.position.z=.11;body.renderOrder=5;group.add(body);
  const arrow=new THREE.ArrowHelper(new THREE.Vector3(1,0,0),new THREE.Vector3(0,0,.25),.8,color,.2,.12);arrow.traverse(o=>{if(o.material){o.material.depthTest=false;o.renderOrder=6;}});group.add(arrow);
  glyph={group,material,arrow,color,body};robots.set(key,glyph);
 }
 if(glyph.name!==name){
  if(glyph.tag){glyph.group.remove(glyph.tag);glyph.tag.material.map.dispose();glyph.tag.material.dispose();}
  // The same peer bubble phones carry, so every peer's name reads alike.
  glyph.tag=createNoteBubble(name,{peer:true});glyph.tag.position.set(0,0,.6);glyph.group.add(glyph.tag);glyph.name=name;
 }
 return glyph;
}
function focusPose(name,state){
 if(!state.pose)return;
 const target=new THREE.Vector3(...state.pose.translation).add(new THREE.Vector3(0,.3,0));cameraPan.start(target.clone().add(new THREE.Vector3(2,2.4,2.7)),target);
 showSelection(name,'',state.status,{kind:'Robot',facts:[['Position',xyz(state.pose.translation,3),true],['Pose','Calibrated robot base · arrow points forward · illustrative glyph']]});
}
function renderRunners(entries){
 const keys=new Set(entries.map(e=>e.key));
 for(const [key,glyph] of robots)if(!keys.has(key)){scene.remove(glyph.group);clear(glyph.group);robots.delete(key);poseUpdates.delete(key);}
 const list=$('runner-list');
 const signature=JSON.stringify(entries.map(e=>[e.key,e.name]));
 if(list.dataset.signature!==signature){
  list.replaceChildren();list.dataset.signature=signature;
  for(const entry of entries){const row=document.createElement('div');row.className='runner-pose-row';row.dataset.peer=entry.key;
   const title=document.createElement('strong'),position=document.createElement('small'),button=document.createElement('button');button.className='text-button';button.textContent='Focus '+entry.name+' ↗';row.append(title,position,button);list.append(row);}
 }
 entries.forEach((entry,i)=>{
  const glyph=glyphFor(entry.key,entry.name),state=entry.state;
  // Robots drawn from registry poses have no presence note bubble, so name them here;
  // above the URDF's battery and pointing labels once the model has loaded.
  glyph.tag.visible=activeMap!=='home-live';glyph.tag.position.z=glyph.robot?2.02:.6;
  glyph.state=state;glyph.group.visible=Boolean(state.pose)&&$('runner-layer').checked&&!usesBodyVolume(entry.key);
  if(state.pose){glyph.group.position.fromArray(state.pose.translation);glyph.group.quaternion.fromArray(state.pose.rotation_xyzw);}
  setGlyphModel(glyph,state.model||null);
  if(state.joints&&state.joints!==glyph.joints){glyph.joints=state.joints;applyJoints(glyph);}
  glyph.health=state.health||null;applyRobotHealth(glyph,glyph.health);
  glyph.pointing=state.pointing||null;applyPointingStatus(glyph,glyph.pointing);
  glyph.material.color.set(state.status==='live'?glyph.color:'#84908f');glyph.material.opacity=state.status==='live'?.82:.35;glyph.arrow.setColor(state.status==='live'?glyph.color:'#84908f');
  const row=list.children[i];row.dataset.status=state.status;row.dataset.visible=String(glyph.group.visible);
  row.dataset.position=state.pose?JSON.stringify(glyph.group.position.toArray()):'';row.dataset.rotation=state.pose?JSON.stringify(glyph.group.quaternion.toArray()):'';
  row.children[0].textContent=entry.name+' · '+state.status;row.children[0].style.color=state.status==='live'?glyph.color:'#84908f';
  row.children[1].textContent=state.pose?`[${state.pose.translation.map(n=>n.toFixed(3)).join(', ')}] m${state.status==='stale'?' · last known':''}`:'Waiting for calibrated pose';
  row.children[2].disabled=!state.pose;row.children[2].onclick=()=>focusPose(entry.name,state);
 });
 list.hidden=entries.length<2;
 $('runner-card').dataset.poseCount=String(entries.filter(e=>e.state.pose).length);
}
function showRunner(state){
 runner=state;renderRunners([{key:'selected',name:runnerDisplay(),state}]);
 $('runner-status').textContent=runnerDisplay()+' · '+state.status;
 $('runner-card').dataset.status=state.status;
 $('runner-position').textContent=state.pose?`[${state.pose.translation.map(n=>n.toFixed(3)).join(', ')}] m${state.status==='stale'?' · last known':''}`:'No calibrated pose available';
 $('runner-detail').textContent=state.status==='disabled'?'Configure a runner peer to show its pose.':state.detail||'';
 $('focus-runner').textContent='Focus '+runnerDisplay()+' ↗';$('focus-runner').disabled=!state.pose;
 $('focus-runner').onclick=()=>focusPose(runnerDisplay(),state);
 const glyph=robots.get('selected');
 $('runner-card').dataset.position=state.pose?JSON.stringify(glyph.group.position.toArray()):'';
 $('runner-card').dataset.rotation=state.pose?JSON.stringify(glyph.group.quaternion.toArray()):'';
}
// Scene responses retain the physical body pose and provide a separate SDK-
// converted pose for this +X-forward glyph. Legacy ROS telemetry/simulations
// already supply glyph-compatible poses.
function robotGlyphPose(sample){return sample?.glyph_pose??sample?.pose??null;}
function showHomeRunners(local,fresh){
 const now=performance.now();
 const entries=Object.entries(local.runners).map(([key,value])=>{
  const sample=value.robot_pose,stamp=sample?JSON.stringify([sample.clock,sample.capture_timestamp_ns]):null;
  let update=poseUpdates.get(key);if(!update||update.stamp!==stamp){update={stamp,at:now};poseUpdates.set(key,update);}
  const pose=robotGlyphPose(sample);
  return {key,name:peerNames.get(key)||'Galbot '+key.slice(-6),state:{pose,status:pose?(fresh&&value.status==='aligned'&&now-update.at<10000?'live':'stale'):'waiting'}};
 });
 renderRunners(entries);
 $('runner-list').hidden=false;
 const live=entries.filter(e=>e.state.status==='live').length;
 $('runner-status').textContent=`${entries.length} Galbots · ${live} live`;
 $('runner-card').dataset.status=live===entries.length?'live':'stale';
 $('runner-card').dataset.position='';$('runner-card').dataset.rotation='';
 $('runner-position').textContent='Poses in Home’s shared portal frame';$('runner-detail').textContent='Each arrow points forward.';
 $('focus-runner').textContent='Focus all Galbots ↗';$('focus-runner').disabled=!entries.some(e=>e.state.pose);
 $('focus-runner').onclick=()=>{
  const points=entries.filter(e=>e.state.pose).map(e=>new THREE.Vector3(...e.state.pose.translation));if(!points.length)return;
  const box=new THREE.Box3().setFromPoints(points),center=box.getCenter(new THREE.Vector3()),size=box.getSize(new THREE.Vector3());
  const distance=Math.max(4,size.length()*1.5/Math.min(camera.aspect,1));controls.target.copy(center);camera.position.copy(center).add(new THREE.Vector3(0,distance,distance*.6));controls.update();
 };
}
$('runner-layer').onchange=()=>{refreshRunner();};
async function refreshRunner(){
 if(isLocalScene()){
  // Robots aligned to this scene: registry poses of peers that declare a body.
  const present=declaredBodies.frame===current?.frame?declaredBodies.entries:[];
  const entries=[];
  for(const r of present){
   let pose;try{pose=declaredGlyphPose(r);}catch(e){console.warn('Robot body not drawn:',e.message);continue;}
   const age=r.received_age_ms+performance.now()-r.receivedAt;
   entries.push({key:'body:'+r.peer_id,name:r.name,state:{status:age<2000?'live':'stale',pose,model:r.body.model,
    joints:freshState(r.body.joints,3000)?.positions,health:freshState(r.body.health,5000),
    pointing:freshState(r.body.pointing,20000)}});
  }
  if(!entries.length){hideRobots();$('runner-status').textContent='No robot aligned to this scene';return;}
  renderRunners(entries);
  $('runner-status').textContent=entries.length+' robot(s) aligned to this scene';
  return;
 }
 if(activeMap.startsWith('peer-scene:')){
  const pose=robotGlyphPose(current?.local_mapping?.robot_pose);
  showRunner({status:pose&&performance.now()-(current?.receivedAt||0)<5000?'live':'stale',pose:pose||null,model:robotModel(current?.frame),joints:current?.local_mapping?.robot_joints?.positions,health:current?.local_mapping?.robot_health,detail:'Pose in the publishing robot’s map frame'});return;
 }
 if(isSimulation()){
  const local=current?.local_mapping;
  const fresh=current?.simulation_status==='running'&&performance.now()-(current?.receivedAt||0)<5000;
  if(local?.runners){
   renderRunners(Object.entries(local.runners).map(([key,r])=>({key,name:r.name,state:{status:fresh?'live':'stale',pose:robotGlyphPose(r.robot_pose)}})));
   $('runner-status').textContent=Object.keys(local.runners).length+' simulated robot(s) · '+(fresh?'live':'stale');
   $('runner-card').dataset.status=fresh?'live':'stale';return;
  }
  showRunner({status:fresh?'live':'stale',pose:robotGlyphPose(local?.robot_pose),detail:'Synthetic patrol pose · not robot telemetry'});return;
 }
 if((isRunner()||activeMap==='home-live')) {
  const local=current?.local_mapping, remote=current?.remote_status;
  const fresh=remote?.status==='connected' && remote.age_ms<10000 && performance.now()-(current?.receivedAt||0)<5000;
  if(activeMap==='home-live'&&local?.runners){showHomeRunners(local,fresh);return;}
  showRunner({status:fresh&&local?.robot_pose?'live':'stale',pose:robotGlyphPose(local?.robot_pose),model:robotModel(current?.frame),joints:local?.robot_joints?.positions,health:local?.robot_health,detail:local?(activeMap==='home-live'?'Pose in Home’s observed portal frame':current?.display_alignment?'Display pose uses the Catalog transform; robot pose remains native':'Pose in the robot boot frame'):'Waiting for independent robot pose'});return;
 }
 if(runnerBusy)return;runnerBusy=true;try{const state=await api('/api/runner-pose');lastRunnerReply=performance.now();if(!(isLocalScene()||isRunner()||isSimulation()||activeMap==='home-live'||activeMap.startsWith('peer-scene:')))showRunner(state);}catch{if(!(isLocalScene()||isRunner()||isSimulation()||activeMap==='home-live'||activeMap.startsWith('peer-scene:')))showRunner({...runner,status:'unavailable',pose:null,detail:'Home telemetry connection lost'});}finally{runnerBusy=false;}}
runnerCatalog();refreshRunner();setInterval(refreshRunner,250);
setInterval(()=>{if(!(isLocalScene()||isRunner()||isSimulation()||activeMap==='home-live'||activeMap.startsWith('peer-scene:'))&&lastRunnerReply&&performance.now()-lastRunnerReply>3000&&runner?.status==='live')showRunner({...runner,status:'stale',detail:'Home telemetry has stopped updating'});},500);

async function refreshMapping(){if(isLocalScene()||activeMap.startsWith('phone:')||activeMap.startsWith('peer-scene:'))return;if(activeMap==='home-live'){try{const state=await api('/api/home-live-status');$('mapping-status').textContent='Home map · '+state.status;$('mapping-detail').textContent=state.detail||'';}catch{$('mapping-status').textContent='Home map · unavailable';}return;}if(activeMap==='demo'&&!current?.demo){$('mapping-status').textContent='Canonical reference';$('mapping-detail').textContent='13 supplied portal poses and physical sizes';return;}if(isRunner()){try{const sources=await api('/api/scene-sources');const remote=activeMap.startsWith('runner:')?(sources.runners||[]).find(r=>r.peer_id===activeMap.slice(7))||{status:'unavailable'}:sources;$('mapping-status').textContent='Galbot map · '+remote.status;$('mapping-detail').textContent=remote.detail||'';}catch{$('mapping-status').textContent='Galbot map · unavailable';}return;}try{const mapping=await api('/api/mapping');$('mapping-status').textContent='LiDAR · '+mapping.status+(mapping.rgbd?' / RGB-D · '+mapping.rgbd.status:'');$('mapping-detail').textContent=(mapping.detail||'')+(mapping.rgbd?' · '+(mapping.rgbd.detail||`${mapping.rgbd.colored_voxels} colored voxels`):'');$('mapping-card').dataset.status=mapping.status;}catch{$('mapping-status').textContent='LiDAR · unavailable';}}

// Prefer live topography; canonical scenes can also use the reference floor.
let placement=null,placementGeneration=0,displayedCamera=null,placingNote=false,placementTarget=null;
// A static pin marks the picked point while the create card is open.
{const blue=new THREE.MeshBasicMaterial({color:'#2563eb'}),pin=new THREE.Group();pin.visible=false;pin.renderOrder=10;
 const head=new THREE.Mesh(new THREE.SphereGeometry(.07,24,16),blue);head.position.y=.45;
 const stem=new THREE.Mesh(new THREE.CylinderGeometry(.012,.012,.45,8),blue);stem.position.y=.225;
 const base=new THREE.Mesh(new THREE.RingGeometry(.06,.11,32),new THREE.MeshBasicMaterial({color:'#2563eb',side:THREE.DoubleSide,transparent:true,opacity:.55}));base.rotation.x=-Math.PI/2;base.position.y=.005;
 pin.add(head,stem,base);scene.add(pin);scene.userData.placementPin=pin;}
function showPlacementPin(position){const pin=scene.userData.placementPin;pin.visible=!!position;if(position)pin.position.set(...position);}
// "New note" enters placement mode: click a surface, then finish the note in the inspector card.
const placementBanner=document.createElement('div');placementBanner.className='placement-banner';placementBanner.setAttribute('role','status');placementBanner.hidden=true;
placementBanner.innerHTML='<span>Click a surface to place your note</span><kbd>Esc</kbd>';
placementBanner.append(actionButton('Use a portal instead',{variant:'on-dark',onClick:()=>{stopNotePlacement();createNoteCard({portal:true});}}),actionButton('Cancel',{variant:'on-dark',onClick:()=>{stopNotePlacement();newNoteButton.focus();}}));
document.querySelector('.viewport').append(placementBanner);
function startNotePlacement(target=null){if(!current||map2D.isActive())return;if(!target)clearSelection();placingNote=true;placementTarget=target;placementBanner.hidden=false;renderer.domElement.style.cursor='crosshair';newNoteButton.setAttribute('aria-pressed','true');}
function stopNotePlacement(){placingNote=false;placementTarget=null;placementBanner.hidden=true;renderer.domElement.style.cursor='';newNoteButton.setAttribute('aria-pressed','false');}
document.addEventListener('keydown',e=>{if(e.key==='Escape'&&placingNote){e.stopPropagation();stopNotePlacement();newNoteButton.focus();}},true);
async function beginLabel(request,error){
 if(request?.source!=='camera'&&activeMap.startsWith('peer-scene:')) {showAvailability('Peer maps are read-only.');return;}
 if(request?.source!=='camera'&&activeMap.startsWith('phone:')) {showAvailability('Phone-local maps are unaligned and read-only.');return;}
 createNoteCard({request,error});
}
const shortId=id=>id.length>14?`${id.slice(0,6)}…${id.slice(-4)}`:id;
function createNoteCard({request,error,portal=false}={}){
 clearSelection();noteCardMode='create';
 let generation=++placementGeneration;const editingMap=activeMap;placement=null;let usePortal=portal;
 const {form,title,titleField,fields}=noteForm({heading:'New note',note:null,id:'label-dialog'});
 // Location row: where the note goes, in words, with a way to change it.
 const location=document.createElement('div');location.className='location-row';
 const tile=document.createElement('span');tile.className='list-row-tile';tile.dataset.accent='blue';tile.append(icon(icons.pin,16));
 const text=document.createElement('div');text.className='location-text';
 const caption=document.createElement('span');caption.className='form-label';caption.textContent='Location';
 const where=document.createElement('strong');where.className='location-value';
 const status=document.createElement('small');status.id='label-dialog-status';status.setAttribute('role','status');status.className='location-status';
 text.append(caption,where,status);
 const change=document.createElement('button');change.type='button';change.className='text-action';change.textContent='Change';change.setAttribute('aria-expanded','false');change.setAttribute('aria-controls','location-options');
 location.append(tile,text,change);
 const warning=document.createElement('p');warning.className='location-warning';warning.textContent='Not a measured surface — position is approximate.';warning.hidden=true;
 const options=document.createElement('div');options.id='location-options';options.className='location-options';options.hidden=true;
 const repick=actionButton('Pick another surface',{iconPath:icons.pin,onClick:()=>{options.hidden=true;change.setAttribute('aria-expanded','false');usePortal=false;startNotePlacement(pickSurface);}});
 const portalRegion=document.createElement('div');portalRegion.id='portal-options';
 const portalToggle=disclosure('Use a portal',{expanded:usePortal,controls:'portal-options',onToggle:()=>{}});
 portalToggle.onclick=()=>{usePortal=!usePortal;portalToggle.setAttribute('aria-expanded',String(usePortal));if(usePortal){portalRegion.hidden=false;reveal(portalRegion);}else conceal(portalRegion,()=>{portalRegion.hidden=true;});if(usePortal)loadPortals();describe();};
 const portalField=document.createElement('div');portalField.className='form-field';
 const portalLabel=document.createElement('label');portalLabel.className='form-label';portalLabel.textContent='Portal';portalLabel.htmlFor='note-portal';
 const portalSelect=document.createElement('select');portalSelect.id='note-portal';portalSelect.className='form-control mono-id';portalField.append(portalLabel,portalSelect);
 const heightField=document.createElement('div');heightField.className='form-field';
 const heightLabel=document.createElement('label');heightLabel.className='form-label';heightLabel.textContent='Height above portal';heightLabel.htmlFor='note-height';
 const height=document.createElement('input');height.id='note-height';height.type='number';height.step='0.1';height.min='-100';height.max='100';height.value='1.0';height.className='form-control';
 const step=(delta,label)=>{const button=actionButton(label);button.setAttribute('aria-label',(delta>0?'Raise':'Lower')+' by 10 cm');button.onclick=()=>{height.value=(Math.round((Number(height.value)+delta)*10)/10).toFixed(1);describe();};return button;};
 const stepper=document.createElement('div');stepper.className='stepper';const unit=document.createElement('span');unit.textContent='m';unit.className='stepper-unit';
 stepper.append(step(-.1,'−'),height,unit,step(.1,'+'));heightField.append(heightLabel,stepper);
 portalRegion.append(portalField,heightField);portalRegion.hidden=!usePortal;
 options.append(repick,portalToggle,portalRegion);
 change.onclick=()=>{const open=options.hidden;change.setAttribute('aria-expanded',String(open));if(open){options.hidden=false;reveal(options);}else conceal(options,()=>{options.hidden=true;});};
 titleField.after(location,warning,options);
 const cancel=actionButton('Cancel',{id:'label-dialog-cancel',onClick:()=>{stopNotePlacement();clearSelection();newNoteButton.focus();}});cancel.dataset.cancel='';
 const save=actionButton('Save note',{variant:'primary',type:'submit',id:'label-dialog-save'});save.disabled=true;
 form.append(cardActions(cancel,save));
 function describe(){
  warning.hidden=usePortal||!placement?.floor;
  if(usePortal){where.textContent=portalSelect.value?`${Number(height.value).toFixed(1)} m above portal ${shortId(portalSelect.value)}`:'Choose a portal';showPlacementPin(null);}
  else if(placement){where.textContent=placement.floor?'On the reference floor':'On a measured surface';showPlacementPin(placement.position);}
  else{where.textContent=error||request?'No location yet':'Choose a location';showPlacementPin(null);}
  save.disabled=usePortal?!portalSelect.value:!placement;
 }
 let portalsLoaded=false;
 async function loadPortals(){if(portalsLoaded)return;portalsLoaded=true;try{const canonical=await api(isLocalScene(editingMap)?localSceneUrl(editingMap):'/api/scene');for(const p of canonical.portals||[]){const option=new Option(shortId(p.anchor_id),p.anchor_id);option.title=p.anchor_id;portalSelect.add(option);}if(!portalSelect.options.length)status.textContent='No portals in this scene. Pick a surface instead.';}catch(e){status.textContent=e.message;}describe();}
 portalSelect.onchange=describe;height.oninput=describe;
 function pickSurface(next){
  generation=++placementGeneration;placement=null;status.textContent='Finding a surface · 50 cm beam…';describe();where.textContent='Finding a surface…';
  api('/api/label-pick',next).then(result=>{if(generation!==placementGeneration||noteCardMode!=='create')return;
   if(!result.hit){status.textContent='No surface within 40 metres. Try nearer the floor.';describe();return;}
   placement=result.hit;status.textContent='';describe();
  }).catch(e=>{if(generation===placementGeneration){status.textContent=e.message;describe();}});
 }
 if(usePortal){options.hidden=false;change.setAttribute('aria-expanded','true');loadPortals();}
 if(error){status.textContent=error;describe();}
 else if(request)pickSurface(request);
 else describe();
 title.focus();
 form.onsubmit=async e=>{e.preventDefault();if(save.disabled)return;
  if(!title.value.trim()){status.textContent='Add a title.';title.setAttribute('aria-invalid','true');title.focus();return;}
  save.disabled=true;
  try{const details=await fields.read(title.value);
   const note=usePortal
    ?await api('/api/notes'+(isLocalScene(editingMap)?'?scene='+encodeURIComponent(editingMap.slice(6)):''),{text:title.value,details,portal:portalSelect.value,height_m:Number(height.value)})
    :await api('/api/notes/from-pick',{token:placement.token,text:title.value,details});
   noteCardMode='view';showPlacementPin(null);lastScene=null;await refresh();openPanel('notes');
   const created=(current?.notes||[]).find(n=>n.id===note.id);if(created)showNoteSelection(created);else clearSelection();toast('Note saved');
  }catch(err){status.textContent=err.message;save.disabled=false;}};
}
// Right-drag pans the 3D view, so right-click no longer places notes there; New note does that.
renderer.domElement.addEventListener('contextmenu',e=>e.preventDefault());
$('sensor-stage').addEventListener('contextmenu',e=>{e.preventDefault();e.stopPropagation();
 const f=displayedCamera;
 if(!f?.token||Date.now()-f.receivedAt>10000){beginLabel(null,f?.error||'Waiting for a fresh capture-matched camera pose. Try again when calibrated video arrives.');return;}
 beginLabel({source:'camera',camera_token:f.token,pixel:videoPresentation.pixel(e.clientX,e.clientY)});
});

// Camera previews are independent of the selected map and never control the robot.
const sensorSelect=document.querySelector('#sensor-select'),pip=document.querySelector('#sensor-pip'),sensorCanvas=document.querySelector('#sensor-image');
const pipWindow=createPipWindow(pip);
const videoPresentation=createVideoPresentation($('sensor-stage'),pip.querySelector('.sensor-pip-heading'),()=>{pipWindow.fit();arOverlay?.setRotation(Number($('sensor-stage').dataset.rotation)||0);});
let sensorGeneration=0,lastSensorSequence=null,lastSensorChange=0;
const arToggle=$('sensor-ar'),arStatus=$('sensor-ar-status');
let arOverlay=null,arLastSuccess=0;
const arAvailable=new Map();
function hideAr(){arOverlay?.hide();arLastSuccess=0;}
function updateArAvailability(){
 const supported=arAvailable.get(sensorSelect.value)===true;
 arToggle.disabled=!supported;
 arStatus.hidden=!sensorSelect.value||(!arToggle.checked&&supported);
 if(!supported)arStatus.textContent='AR needs capture-matched camera calibration and transforms';
}
function restartSensor(){
 updateArAvailability();
 const generation=++sensorGeneration;displayedCamera=null;lastSensorSequence=null;lastSensorChange=0;hideAr();
 if(arToggle.checked){arStatus.hidden=false;arStatus.textContent='AR · waiting for a matched image and pose…';}
 watchSensor(generation);
}
async function applyChatCamera(actions,map){
 return applyChatActions(actions,{scene:map,context:()=>({scene:activeMap,frame:current?.frame}),apply:async(action,guard)=>{
  if(['attach_camera','detach_camera','move_camera'].includes(action.type))cameraPan.cancel();
  if(action.type==='attach_camera')return attachedCamera.attach(action);
  if(action.type==='detach_camera'){
   if(action.scene!==activeMap||action.frame!==current?.frame)throw Error('Selected scene changed; ask again');
   return attachedCamera.detach();
  }
  if(action.type==='move_camera'){const result=moveMapCamera(action,{scene:activeMap,frame:current?.frame,camera,controls,map2D});attachedCamera.detach();return result;}
  const catalog=await api('/api/sensors');
  guard();
  const sensor=catalog.sensors.find(s=>s.id===action.sensor_id&&s.peer_id===action.peer_id);
  if(!sensor)throw Error('Camera is no longer available');
  let option=Array.from(sensorSelect.options).find(o=>o.value===sensor.id);
  if(!option){option=new Option(sensor.label,sensor.id);option.dataset.peer=sensor.peer_id;sensorSelect.append(option);}
  arAvailable.set(sensor.id,sensor.ar_available===true);
  if(sensorSelect.value!==sensor.id||pip.hidden){sensorSelect.value=sensor.id;sensorSelect.dispatchEvent(new Event('change'));}
  return 'Camera PiP opened · waiting for live frames.';
 }});
}
arToggle.addEventListener('change',restartSensor);
setInterval(()=>{if(noteCardMode!=='create'&&arToggle.checked&&arLastSuccess&&Date.now()-arLastSuccess>3000){hideAr();arStatus.hidden=false;arStatus.textContent='AR paused · camera frames are stale';}},500);
async function sensorCatalog(){
 try{
  const response=await fetch('/api/sensors');const data=await response.json();if(!response.ok)throw Error(data.error);
  const selected=sensorSelect.value;const options=[new Option('None','')];
  arAvailable.clear();
  for(const s of data.sensors){arAvailable.set(s.id,s.ar_available===true);const option=new Option(s.label+(s.ar_available?' · AR':''),s.id);option.dataset.peer=s.peer_id;options.push(option);}
  const stillAvailable=data.sensors.some(s=>s.id===selected);
  sensorSelect.replaceChildren(...options);sensorSelect.value=stillAvailable?selected:'';updateArAvailability();
  if(selected&&!stillAvailable)sensorSelect.dispatchEvent(new Event('change'));
  document.querySelector('#sensor-catalog-status').textContent=data.sensors.length?`${data.sensors.length} published camera feeds`:'No camera feeds available · retrying';
 }catch(e){document.querySelector('#sensor-catalog-status').textContent='Camera peers unavailable · retrying';}
 setTimeout(sensorCatalog,10000);
}
async function watchSensor(generation){
 if(generation!==sensorGeneration||!sensorSelect.value)return;
 const status=document.querySelector('#sensor-feed-status');
 try{
  const response=await fetch('/api/sensors?sensor='+encodeURIComponent(sensorSelect.value)+(arToggle.checked?'&ar=true':'')+(arAvailable.get(sensorSelect.value)?'&pick=true':''));const f=await readCameraResponse(response);
  if(generation!==sensorGeneration)return;
  if(f.sequence!==lastSensorSequence&&noteCardMode!=='create'){
   const bytes=f.bytes;
   if(!Number.isInteger(f.width)||!Number.isInteger(f.height)||f.width<1||f.height<1||f.width>4096||f.height>4096)throw Error('Invalid image dimensions');
   const bitmap=(f.encoding==='jpeg'||f.encoding==='jpg')?await createImageBitmap(new Blob([bytes],{type:'image/jpeg'})):null;
   if(generation!==sensorGeneration){bitmap?.close();return;}
   if(noteCardMode==='create'){bitmap?.close();setTimeout(()=>watchSensor(generation),100);return;}
   sensorCanvas.style.aspectRatio=`${f.width}/${f.height}`;sensorCanvas.width=f.width;sensorCanvas.height=f.height;const ctx=sensorCanvas.getContext('2d');
   if(bitmap){ctx.drawImage(bitmap,0,0,f.width,f.height);bitmap.close();}else{
    const channels=f.encoding==='rgb8'?3:f.encoding==='luma8'?1:0;
    if(!channels||bytes.length!==f.width*f.height*channels)throw Error('Unsupported camera frame');
    const image=ctx.createImageData(f.width,f.height);
    for(let i=0;i<f.width*f.height;i++){image.data[i*4]=bytes[i*channels];image.data[i*4+1]=bytes[i*channels+(channels===3?1:0)];image.data[i*4+2]=bytes[i*channels+(channels===3?2:0)];image.data[i*4+3]=255;}ctx.putImageData(image,0,0);
   }
   displayedCamera={token:f.camera_token,error:f.pick_error,width:f.width,height:f.height,receivedAt:Date.now()};
   videoPresentation.update(f);
   if(arToggle.checked){arOverlay??=createArOverlay($('sensor-ar-overlay'));arOverlay.show(f,Number($('sensor-stage').dataset.rotation)||0);arLastSuccess=Date.now();arStatus.textContent='AR · Home notes + drawings · capture-matched camera';}
   lastSensorSequence=f.sequence;lastSensorChange=Date.now();
  }
  const stale=Date.now()-lastSensorChange>3000;status.textContent=stale?'Stale · no new frames':'Receiving · frame '+f.sequence;status.dataset.state=stale?'stale':'live';
 }catch(e){if(generation===sensorGeneration){displayedCamera=null;status.textContent=(lastSensorSequence!==null?'Stale image · ':'Waiting · ')+e.message;if(noteCardMode!=='create')hideAr();if(arToggle.checked){arStatus.hidden=false;arStatus.textContent='AR paused · '+e.message;}}}
 if(generation===sensorGeneration)setTimeout(()=>watchSensor(generation),100);
}
sensorSelect.addEventListener('change',()=>{
 videoPresentation.select(sensorSelect.value);
 pip.hidden=!sensorSelect.value;arToggle.checked=false;updateArAvailability();
 sensorCanvas.getContext('2d').clearRect(0,0,sensorCanvas.width,sensorCanvas.height);
 document.querySelector('#sensor-title').textContent=sensorSelect.selectedOptions[0].textContent;
 document.querySelector('#sensor-feed-status').textContent='Waiting for a frame…';restartSensor();
});
document.querySelector('#sensor-close').addEventListener('click',()=>{sensorSelect.value='';sensorSelect.dispatchEvent(new Event('change'));});
sensorCatalog();

// Transient body occupancy: keep cube axes in robot.base, transform the entire
// group with the SDK-composed capture-time pose. Never add cells to topography.
let volumeBusy=false,volumeScope=null;
function volumeContext(){return JSON.stringify([activeMap,current?.frame,current?.source_frame,current?.local_mapping?.map_correction,current?.display_alignment]);}
function staleVolume(v){
 v.stale=true;v.material.color.set('#84908f');v.material.emissive.set('#000000');v.material.transparent=true;v.material.opacity=.35;
 if(!v.tag){v.tag=label('Last known position · tracking lost','#84908f');v.tag.position.set(0,0,1);v.group.add(v.tag);}
 v.group.visible=$('robot-volume-layer').checked;
}
function volumeStatus(){
 const values=[...robotVolumes.values()],stale=values.filter(v=>v.stale).length;
 const status=$('robot-volume-status');status.dataset.robots=String(values.length);status.dataset.stale=String(stale);status.dataset.cells=String(values.reduce((n,v)=>n+v.cells,0));
 if(stale)status.textContent=`${stale} robot${stale===1?'':'s'} · last known position · tracking lost · not live`;
}
function removeVolume(key){const v=robotVolumes.get(key);if(v){scene.remove(v.group);clear(v.group);robotVolumes.delete(key);}}
function resetVolumes(){for(const key of [...robotVolumes.keys()])removeVolume(key);volumeScope=null;}
$('map-select').addEventListener('change',resetVolumes);
$('robot-volume-layer').onchange=()=>{for(const v of robotVolumes.values())v.group.visible=$('robot-volume-layer').checked;};
async function refreshRobotVolumes(){
 if(volumeBusy)return;
 const scope=volumeContext();if(volumeScope!==scope){resetVolumes();volumeScope=scope;}
 if(!(activeMap==='home-live'||isRunner())){resetVolumes();$('robot-volume-status').textContent='Robot volume · choose a live map';return;}
 const selected=activeMap,frame=current?.frame;
 const peer=selected==='runner'?primaryRunnerPeer:selected.startsWith('runner:')?selected.slice(7):null;
 if(isRunner()&&!peer)return;
 volumeBusy=true;
 try{
  const data=await api('/api/robot-occupancy'+(peer?'?peer='+encodeURIComponent(peer):''));
  if(selected!==activeMap||frame!==current?.frame||scope!==volumeContext())return;
  const entries=(data.robots||[]).filter(r=>r.frame===frame);
  const keys=new Set(entries.map(r=>r.peer_id));for(const [key,v] of robotVolumes)if(!keys.has(key))staleVolume(v);
  for(const r of entries){
   bodyVolumePeers.add(r.peer_id);
   const signature=JSON.stringify([r.model_sha256,r.timestamp_ns,r.sequence,r.frame,r.pose]);let v=robotVolumes.get(r.peer_id);
   if(!v||v.signature!==signature){
    removeVolume(r.peer_id);
    const group=new THREE.Group(),material=new THREE.MeshStandardMaterial({color:'#ff2020',emissive:'#a00000',emissiveIntensity:.3,roughness:.7});
    const mesh=new THREE.InstancedMesh(boxGeometry,material,r.cells.length),matrix=new THREE.Matrix4();
    r.cells.forEach((c,i)=>{const s=r.voxel_size;matrix.makeScale(s*.96,s*.96,s*.96);matrix.setPosition(...c.map(n=>(n+.5)*s));mesh.setMatrixAt(i,matrix);});
    mesh.instanceMatrix.needsUpdate=true;mesh.computeBoundingSphere();group.add(mesh);
    group.position.fromArray(r.pose.translation);group.quaternion.fromArray(r.pose.rotation_xyzw);group.visible=$('robot-volume-layer').checked;
    scene.add(group);v={group,material,signature,cells:r.cells.length,at:performance.now(),stale:false};robotVolumes.set(r.peer_id,v);
   }
  }
  const count=entries.reduce((n,r)=>n+r.cells.length,0),missing=entries.reduce((n,r)=>n+r.missing_links.length,0);
  const status=$('robot-volume-status');status.dataset.robots=String(entries.length);status.dataset.cells=String(count);
  status.dataset.poses=JSON.stringify(entries.map(r=>({peer:r.peer_id,pose:r.pose})));
  status.textContent=entries.length?`${entries.length} robot${entries.length===1?'':'s'} · ${count.toLocaleString()} red cells · 2 Hz${missing?' · '+missing+' links missing':''}${entries.some(r=>r.approximate_links.length)?' · approximate envelopes':''}`:
   'Robot volume · '+(data.unavailable?.[0]?.detail||data.status||'waiting for runner');
  volumeStatus();
  refreshRunner();
 }catch(e){if(selected!==activeMap||scope!==volumeContext())return;for(const v of robotVolumes.values())staleVolume(v);$('robot-volume-status').textContent='Robot volume · '+e.message;volumeStatus();}
 finally{volumeBusy=false;}
}
setInterval(()=>{for(const v of robotVolumes.values())if(performance.now()-v.at>3000)staleVolume(v);volumeStatus();refreshRobotVolumes();},500);

function uiButton(text,action,className='panel-row'){
 const button=document.createElement('button');button.type='button';button.className=className;button.textContent=text;button.onclick=action;return button;
}
let mapRowsSignature='';
const collapsedMapGroups=new Set((()=>{try{return JSON.parse(localStorage.getItem('maps-collapsed-groups'))||[];}catch{return [];}})());
function renderMapRows(){
 // Polling calls this every refresh; rebuild only on change so focus and clicks survive.
 const peers=fleetNetwork?.peers||[];
 const signature=JSON.stringify([homeOffline,$('map-select').value,[...collapsedMapGroups],peers.map(p=>[p.peer_id,p.status]),Array.from($('map-select').options,o=>[o.value,o.textContent,o.title,o.disabled,o.parentElement.label||''])]);
 if(signature===mapRowsSignature)return;mapRowsSignature=signature;
 const focused=mapsList.contains(document.activeElement)?document.activeElement.dataset.map||document.activeElement.dataset.group:null;
 mapsList.replaceChildren(sectionHeader('Scenes'));
 for(const [name,title] of [['Local','On this server'],['Network','On the network']]){
  const options=Array.from($('map-select').options).filter(o=>(o.parentElement.label||'Local')===name),expanded=!collapsedMapGroups.has(name),id='scene-group-'+name.toLowerCase();
  const section=document.createElement('section');section.setAttribute('aria-label',name+' scenegraphs');
  const toggle=disclosure(title,{count:options.length,expanded,controls:id,onToggle:open=>{const apply=()=>{if(open)collapsedMapGroups.delete(name);else collapsedMapGroups.add(name);try{localStorage.setItem('maps-collapsed-groups',JSON.stringify([...collapsedMapGroups]));}catch{}renderMapRows();};if(open){apply();reveal($(id));}else conceal($(id),apply);}});toggle.dataset.group=name;
  const list=document.createElement('div');list.id=id;list.className='list-group';list.hidden=!expanded;
  for(const option of options){
   const trail=option.value.startsWith('peer-pose:'),peer=trail?peers.find(p=>'peer-pose:'+p.peer_id===option.value):null;
   const status=homeOffline?'offline':statusKind(peer?.status);
   const row=listRow({title:trail?option.textContent.replace(/ · pose trail$/,''):option.textContent,meta:trail?(status?[statusBadge(status),' · Pose trail']:'Pose trail'):(status?statusBadge(status):undefined),tile:name==='Local'?icons.map:deviceIcon(option.textContent),accent:name==='Local'?'emerald':undefined,
    onSelect:()=>{$('map-select').value=option.value;$('map-select').dispatchEvent(new Event('change'));renderMapRows();}});
   row.title=option.title;row.disabled=homeOffline||option.disabled;row.dataset.map=option.value;
   if(status==='offline')row.dataset.offline='true';
   if(option.value===$('map-select').value)markSelected(row,'maps',option.value);
   list.append(row);
  }
  if(!options.length)list.append(emptyState(name==='Local'?'No saved scenes on this server':'No scenegraphs available'));
  section.append(toggle,list);mapsList.append(section);
 }
 if(focused)mapsList.querySelector(`[data-map="${CSS.escape(focused)}"],[data-group="${CSS.escape(focused)}"]`)?.focus();
}
new MutationObserver(renderMapRows).observe($('map-select'),{childList:true,subtree:true,characterData:true});
$('map-select').addEventListener('change',()=>{renderMapRows();renderNotes();});renderMapRows();
function catalogType(entry,kind,catalog){
 const producer=entry.manifest.producer;
 const component=kind==='component'?entry:(catalog.components||[]).find(c=>c.manifest.component_id===producer?.component_id&&(!producer.component_manifest_hash||c.manifest_hash===producer.component_manifest_hash));
 const outputs=Object.entries(component?.current_outputs||{}).filter(([slot])=>kind==='component'||slot===producer?.slot).map(([,o])=>o.manifest.payload);
 const contracts=(component?.manifest.observables||[]).filter(o=>kind==='component'||o.name===producer?.slot);
 const types=[...outputs,...contracts].filter(Boolean);
 const signature=types.map(t=>[t.kind,t.modality,t.datatype,t.schema].filter(Boolean).join(' ')).join(' ')+' '+(entry.metadata?.schema||'');
 if(types.some(t=>t.kind==='camera')||/video[-_.]?frame|camera[-_.]?frame/i.test(signature))return 'Cameras';
 if(/scenegraph|canonical-scene|voxel|occupancy/i.test(signature))return 'Maps';
 if(/odometry|pose[-_.]?update|frame[-_]?transform|\bpose\b/i.test(signature))return 'Poses & odometry';
 if(/detection|landmark-observations/i.test(signature))return 'Detections';
 if(/lidar|depth|sensor|mapping.observation|mapping-input/i.test(signature))return 'Sensors & observations';
 return 'Other';
}
// Devices: a status-sorted list; choosing a device opens its detail view inside the panel.
let deviceView=null,deviceOfflineOpen=false,deviceRenaming=false;
const humanize=id=>String(id).replace(/[-_.]+/g,' ').replace(/^./,c=>c.toUpperCase());
function deviceStatusSentence(peer){
 if(peer.self)return fleetNetwork?.enabled?'This server · '+fleetNetwork.status:fleetNetwork?'This server · P2P disabled':'Network status unavailable';
 const kind=statusKind(peer.status);
 if(kind==='live')return peer.received_observations!==undefined?`Sending data · ${peer.received_observations.toLocaleString()} observations received over P2P`:'Connected';
 if(kind==='stale')return 'Last data is out of date';
 if(kind==='connecting')return 'Trying to reach this device · retrying';
 if(kind==='offline')return 'Offline';
 return peer.status||'Connected peer';
}
function deviceSummary(peer,feeds){return feeds?plural(feeds,'camera'):'';}
let openedTechnical=new Set();
function technicalDetails(rows,key){
 const box=document.createElement('details');box.className='technical-details';if(key){box.dataset.technical=key;box.open=openedTechnical.has(key);}box.innerHTML='<summary>Technical details</summary>';
 const list=document.createElement('dl');list.className='spatial-fields technical-list';box.append(list);
 for(const [name,value,copy] of rows){if(value===undefined||value===null||value==='')continue;const dt=document.createElement('dt'),dd=document.createElement('dd'),text=document.createElement('span');dt.textContent=name;text.className='mono-id';text.textContent=value;dd.append(text);if(copy)dd.append(copyButton(String(value),'Copy '+name.toLowerCase()));list.append(dt,dd);}
 return box;
}
function renderFleetRows(){
 if(componentControlPending||(fleetList.contains(document.activeElement)&&document.activeElement.matches('input,textarea,select')))return;
 disconnectRobots.hidden=!fleetNetwork?.simulation;disconnectRobots.disabled=homeOffline||!(fleetNetwork?.peers||[]).some(p=>p.status==='connected');
 const selfId=fleetNetwork?.connection?.peer_id;
 const peers=[{peer_id:selfId||'self',name:'Home Server',self:true,catalog:fleetNetwork?.catalog},...Array.from(new Map((fleetNetwork?.peers||[]).filter(p=>p.peer_id&&p.peer_id!==selfId).map(p=>[p.peer_id,p])).values())];
 const sensors=Array.from($('sensor-select').options).filter(o=>o.value);
 for(const peer of peers){const line=fleetList.querySelector(`[data-observations-peer="${CSS.escape(peer.peer_id)}"]`);if(line&&peer.received_observations!==undefined)line.textContent=deviceStatusSentence(peer);}
 const signature=JSON.stringify([peers.map(({received_observations,...rest})=>rest),fleetNetwork?.enabled,fleetNetwork?.status,fleetNetwork?.aliases,sensors.map(o=>[o.value,o.textContent,o.dataset.peer]),$('sensor-select').value,deviceView,deviceOfflineOpen]);
 if(signature===fleetSignature)return;fleetSignature=signature;
 const focus=fleetList.contains(document.activeElement)?document.activeElement.dataset.focusKey:null;
 openedTechnical=new Set(Array.from(fleetList.querySelectorAll('details[open][data-technical],div[data-technical]:not([hidden])'),d=>d.dataset.technical));
 fleetList.replaceChildren();
 const peer=deviceView&&peers.find(p=>p.peer_id===deviceView);
 if(deviceView&&!peer)deviceView=null;
 if(peer)renderDeviceDetail(peer,sensors.filter(o=>o.dataset.peer===peer.peer_id),openedTechnical);
 else renderDeviceList(peers,sensors);
 if(focus)fleetList.querySelector(`[data-focus-key="${CSS.escape(focus)}"]`)?.focus();
}
// Navigating into a device slides its detail in from the right; going back slides the list in from the left.
function slideDevices(direction){if(!railMotion)return;fleetList.animate([{opacity:0,transform:`translateX(${direction*18}px)`},{opacity:1,transform:'none'}],{duration:280,easing:'cubic-bezier(.2,.8,.2,1)'});
 if(direction>0)fleetList.querySelectorAll('#device-detail>*').forEach((el,i)=>el.animate([{opacity:0,transform:'translateY(6px)'},{opacity:1,transform:'none'}],{duration:320,delay:40+Math.min(i,8)*28,easing:'cubic-bezier(.2,.8,.2,1)',fill:'backwards'}));}
function deviceName(peer){return peer.self?'Home Server':fleetNetwork?.aliases?.[peer.peer_id]||peer.name||peer.peer_id;}
function deviceRow(peer,feeds){
 const item=document.createElement('div');item.className='device-item';item.dataset.peer=peer.peer_id;
 const kind=peer.self?(fleetNetwork?.enabled?'live':null):statusKind(peer.status);
 const summary=deviceSummary(peer,feeds.length);
 const row=listRow({title:deviceName(peer),meta:kind?[statusBadge(kind),summary?' · '+summary:'']:(summary||deviceStatusSentence(peer)),tile:peer.self?icons.server:deviceIcon(deviceName(peer)),onSelect:e=>{deviceView=peer.peer_id;fleetSignature='';renderFleetRows();slideDevices(1);fleetList.querySelector('#device-detail h3')?.focus({preventScroll:true});}});
 row.dataset.focusKey='row:'+peer.peer_id;if(kind==='offline')row.dataset.offline='true';
 item.append(row);
 if(feeds.length){const first=feeds[0];const view=document.createElement('button');view.type='button';view.className='icon-toggle device-camera';view.setAttribute('aria-label','View '+first.textContent);view.title='View '+first.textContent;view.append(icon(icons.camera,16));view.dataset.focusKey='cam:'+peer.peer_id;
  view.onclick=()=>{$('sensor-select').value=first.value;$('sensor-select').dispatchEvent(new Event('change'));};item.append(view);}
 return item;
}
function renderDeviceList(peers,sensors){
 const self=peers.find(p=>p.self),others=peers.filter(p=>!p.self);
 const rank={live:0,stale:1,connecting:2};const order=p=>rank[statusKind(p.status)]??3;
 fleetList.append(sectionHeader('This server'),deviceRow(self,sensors.filter(o=>o.dataset.peer===self.peer_id)));
 const online=others.filter(p=>statusKind(p.status)!=='offline').sort((a,b)=>order(a)-order(b)||deviceName(a).localeCompare(deviceName(b)));
 const offline=others.filter(p=>statusKind(p.status)==='offline');
 fleetList.append(sectionHeader('Devices',others.length));
 for(const p of online)fleetList.append(deviceRow(p,sensors.filter(o=>o.dataset.peer===p.peer_id)));
 if(!others.length)fleetList.append(emptyState(fleetNetwork?.enabled?'No devices connected yet.':'Turn on P2P to discover devices.'));
 if(offline.length){
  const toggle=disclosure('Offline',{count:offline.length,expanded:deviceOfflineOpen,controls:'devices-offline',onToggle:()=>{}});toggle.dataset.focusKey='offline';
  toggle.onclick=()=>{const apply=()=>{deviceOfflineOpen=!deviceOfflineOpen;fleetSignature='';renderFleetRows();};if(deviceOfflineOpen)conceal($('devices-offline'),apply);else{apply();reveal($('devices-offline'));}};
  const group=document.createElement('div');group.id='devices-offline';group.hidden=!deviceOfflineOpen;for(const p of offline)group.append(deviceRow(p,sensors.filter(o=>o.dataset.peer===p.peer_id)));
  fleetList.append(toggle,group);
 }
 fleetList.append(disconnectRobots);
}
function renderDeviceDetail(peer,feeds,openedTechnical){
 const view=document.createElement('section');view.id='device-detail';view.dataset.peer=peer.peer_id;view.setAttribute('aria-label',deviceName(peer));
 const back=actionButton('All devices',{variant:'ghost',iconPath:'M11 4.5 6.5 9 11 13.5'});back.dataset.focusKey='back';back.onclick=()=>{const id=peer.peer_id;deviceView=null;deviceRenaming=false;fleetSignature='';renderFleetRows();slideDevices(-1);fleetList.querySelector(`[data-focus-key="row:${CSS.escape(id)}"]`)?.focus({preventScroll:true});};
 view.addEventListener('keydown',e=>{if(e.key==='Escape'&&!e.target.closest('input,textarea,select')){e.stopPropagation();back.click();}});
 // Identity and rename.
 const identity=document.createElement('div');identity.className='device-identity';
 const tile=document.createElement('span');tile.className='list-row-tile device-tile';tile.append(icon(peer.self?icons.server:deviceIcon(deviceName(peer)),20));
 const name=document.createElement('h3');name.tabIndex=-1;name.textContent=deviceName(peer);
 identity.append(tile,name);
 if(!peer.self){const rename=document.createElement('button');rename.type='button';rename.className='icon-toggle';rename.setAttribute('aria-label','Rename '+deviceName(peer));rename.append(icon(icons.pen,16));rename.dataset.focusKey='rename';rename.onclick=()=>{deviceRenaming=true;showRename();};identity.append(rename);}
 const idLine=document.createElement('div');idLine.className='device-id';const idText=document.createElement('span');idText.className='mono-id';idText.textContent=peer.peer_id;idLine.append(idText,copyButton(peer.peer_id,'Copy peer ID'));
 view.append(back,identity,idLine);
 function showRename(){
  const form=document.createElement('form');form.className='device-rename';
  const field=formField({label:'Display name',max:64,value:fleetNetwork?.aliases?.[peer.peer_id]||'',placeholder:peer.name||peer.peer_id});
  const status=document.createElement('p');status.className='form-status';status.setAttribute('role','status');
  const cancel=actionButton('Cancel',{onClick:()=>{deviceRenaming=false;fleetSignature='';renderFleetRows();}}),save=actionButton('Save',{variant:'primary',type:'submit'});
  const row=document.createElement('div');row.className='card-actions';row.append(cancel,save);form.append(field.field,row,status);
  form.addEventListener('keydown',e=>{if(e.key==='Escape'){e.stopPropagation();cancel.click();}});
  form.onsubmit=async e=>{e.preventDefault();save.disabled=true;try{await api('/api/peer-alias',{peer_id:peer.peer_id,alias:field.input.value});deviceRenaming=false;await refreshPeer();fleetSignature='';renderFleetRows();toast('Name saved');}catch(err){status.textContent=err.message;save.disabled=false;}};
  identity.after(form);idLine.hidden=true;field.input.focus();
 }
 // Status.
 const statusBox=document.createElement('div');statusBox.className='device-status';
 const kind=peer.self?(fleetNetwork?.enabled?'live':null):statusKind(peer.status);
 if(kind)statusBox.append(statusBadge(kind));
 const sentence=document.createElement('p');sentence.textContent=deviceStatusSentence(peer);if(peer.received_observations!==undefined){sentence.dataset.observations=String(peer.received_observations);sentence.dataset.observationsPeer=peer.peer_id;}statusBox.append(sentence);
 if(!peer.self){
  const catalogNote=document.createElement('p');catalogNote.className='device-note';catalogNote.dataset.catalogStatus=peer.catalog_status||'unknown';
  if(peer.catalog_status==='unavailable'){catalogNote.classList.add('warning');catalogNote.textContent='Component catalog unavailable · capabilities can’t be shown';}
  else if(peer.catalog_status&&peer.catalog_status!=='ready'){catalogNote.textContent='Loading component catalog…';}
  else if(peer.catalog_cached){catalogNote.classList.add('warning');catalogNote.textContent='Catalog refresh delayed · using last known entries; stream freshness checked separately';}
  if(catalogNote.textContent)statusBox.append(catalogNote);
 }
 view.append(statusBox);
 // Cameras.
 const cameras=document.createElement('section');cameras.setAttribute('aria-label','Camera feeds');cameras.append(sectionHeader('Cameras',feeds.length));
 for(const option of feeds.sort((a,b)=>a.textContent.localeCompare(b.textContent))){
  const active=$('sensor-select').value===option.value;
  const row=listRow({title:option.textContent,meta:active?'Showing in picture-in-picture':'Open live view',tile:icons.camera,onSelect:()=>{$('sensor-select').value=active?'':option.value;$('sensor-select').dispatchEvent(new Event('change'));}});
  row.setAttribute('aria-label',option.textContent);row.setAttribute('aria-pressed',String(active));row.dataset.focusKey='feed:'+option.value;cameras.append(row);
 }
 if(feeds.length)view.append(cameras);
 // Location: pose logs only when the device publishes them.
 const catalog=peer.catalog||{};
 const poses=(catalog.products||[]).filter(p=>p.metadata?.schema==='spatial-harness.compact-pose-contract/v1');
 if(poses.length){const location=document.createElement('section');location.setAttribute('aria-label','Pose logs');location.append(sectionHeader('Location'));
  // Product IDs are long ("frame transform:phone ar:<uuid>"): the kind is the title and the rest a truncated meta line.
  for(const product of poses){const id=product.manifest.product_id,[kind,...rest]=id.split(':');
   const row=listRow({title:humanize(kind),meta:rest.join(':')||id,tile:icons.pin,label:'Pose log · '+humanize(id),onSelect:()=>inspectPose(peer.peer_id,product,deviceName(peer)+' / '+id)});
   row.classList.add('pose-log-row');row.title=id;row.dataset.focusKey='pose:'+id;location.append(row);}
  view.append(location);}
 // Capabilities: components with the products they make; raw manifests behind Technical details.
 const components=catalog.components||[],products=catalog.products||[];
 if(components.length||products.length){
  const capabilities=document.createElement('section');capabilities.className='device-capabilities';capabilities.setAttribute('aria-label','Capabilities');capabilities.append(sectionHeader('Capabilities',components.length));
  const entries=[...components.map(entry=>({kind:'component',entry})),...products.filter(p=>!components.some(c=>c.manifest.component_id===p.manifest.producer?.component_id)).map(entry=>({kind:'product',entry}))];
  for(const type of ['Cameras','Maps','Poses & odometry','Detections','Sensors & observations','Other']){
   const matching=entries.filter(item=>catalogType(item.entry,item.kind,catalog)===type).sort((a,b)=>a.entry.manifest[a.kind+'_id'].localeCompare(b.entry.manifest[b.kind+'_id']));
   if(!matching.length)continue;
   // A caption (glyph, type, count) over a glass card of rows, so groups never read as one more row.
   const group=document.createElement('section');group.dataset.catalogType=type;group.setAttribute('aria-label',type);const heading=document.createElement('h4');heading.className='capability-group';
   const label=document.createElement('span'),count=document.createElement('span');label.textContent=type;count.className='capability-group-count';count.textContent=matching.length;heading.append(icon(CATALOG_ICONS[type],14),label,count);
   const card=document.createElement('div');card.className='capability-card';group.append(heading,card);
   for(const {kind,entry} of matching)card.append(capabilityRow(peer,kind,entry,catalog,openedTechnical));
   capabilities.append(group);
  }
  view.append(capabilities);
 }
 view.append(technicalDetails([['Peer ID',peer.peer_id,true],['Status',peer.self?(fleetNetwork?.status||''):peer.status],['Catalog',peer.catalog_status],['Catalog detail',peer.catalog_detail],['Catalog cache',peer.catalog_cached?'Using last known entries':''],['Registry',peer.registries?`${peer.registries.status} · ${Object.keys(peer.registries.definitions||{}).length} resolved definitions`:'']],'device:'+peer.peer_id));
 fleetList.append(view);
 if(deviceRenaming&&!peer.self)showRename();
}
const CATALOG_ICONS={'Cameras':icons.camera,'Maps':icons.map,'Poses & odometry':icons.pin,'Detections':'M2 6V2h4M12 2h4v4M16 12v4h-4M6 16H2v-4M9 9h.01','Sensors & observations':'M9 9h.01M6 6a4.2 4.2 0 0 0 0 6M12 6a4.2 4.2 0 0 1 0 6M3.5 3.5a7.8 7.8 0 0 0 0 11M14.5 3.5a7.8 7.8 0 0 1 0 11','Other':CUBE_ICON};
function capabilityRow(peer,kind,entry,catalog,openedTechnical){
 const id=entry.manifest[kind+'_id'],row=document.createElement('div');row.className='capability';row.dataset[kind]=id;
 const made=kind==='component'?(catalog.products||[]).filter(p=>p.manifest.producer?.component_id===id):[];
 const outputs=made.filter(p=>p.metadata?.schema!=='spatial-harness.component-enabled-state/v1');
 const text=document.createElement('div');text.className='list-row-text';const title=document.createElement('span');title.className='list-row-title';title.textContent=humanize(id);
 const meta=document.createElement('span');meta.className='list-row-meta';meta.textContent={'Cameras':'Image frames','Maps':'Map data','Poses & odometry':'Poses','Detections':'Detections','Sensors & observations':'Sensor readings'}[catalogType(entry,kind,catalog)]||'';meta.hidden=!meta.textContent;text.append(title,meta);
 const head=document.createElement('div');head.className='capability-head';head.append(text);row.append(head);
 // Publication switch, only for components that declare the set_enabled operation.
 if(kind==='component'&&!peer.self&&entry.manifest.operables?.some(o=>o.name==='set_enabled'&&o.instruction==='spatial-harness.set-enabled/v1'&&o.result==='spatial-harness.component-enabled-state/v1')){
  const stateProduct=(catalog.products||[]).find(p=>p.manifest.producer?.component_id===id&&p.manifest.producer?.component_manifest_hash===entry.manifest_hash&&p.metadata?.schema==='spatial-harness.component-enabled-state/v1');
  let enabled=stateProduct?.metadata?.value?.enabled;
  const key=JSON.stringify([peer.peer_id,id,entry.manifest_hash,stateProduct?.manifest_hash]),ack=componentControlResults.get(key);
  if(ack){if(enabled===ack.enabled||stateProduct?.metadata?.source_sequence>ack.sequence)componentControlResults.delete(key);else enabled=ack.enabled;}
  const toggle=document.createElement('button');toggle.type='button';toggle.className='switch';toggle.setAttribute('role','switch');toggle.setAttribute('aria-label',humanize(id));toggle.dataset.focusKey='switch:'+peer.peer_id+':'+id;
  const status=document.createElement('span');status.className='capability-status';status.setAttribute('role','status');
  // The switch shows on/off; the status text is only shown for waiting and error states.
  const paint=()=>{toggle.setAttribute('aria-checked',String(enabled===true));status.textContent=typeof enabled==='boolean'?(enabled?'Enabled':'Disabled'):'Waiting for state';status.classList.toggle('visually-hidden',typeof enabled==='boolean');};paint();
  toggle.disabled=typeof enabled!=='boolean'||peer.catalog_status!=='ready'||!!peer.catalog_cached;
  toggle.onclick=async()=>{
   componentControlPending=true;toggle.disabled=true;status.textContent='Applying…';
   try{const confirmed=await api('/api/component-enabled',{peer_id:peer.peer_id,component_id:id,manifest_hash:entry.manifest_hash,enabled:!enabled});enabled=confirmed.enabled;componentControlResults.set(key,{enabled,sequence:stateProduct?.metadata?.source_sequence});paint();toggle.disabled=false;}
   // The device may or may not have applied it: keep the switch locked until a refresh confirms.
   catch(e){status.textContent=e.message;status.classList.remove('visually-hidden');toggle.title='Refresh Devices to confirm the current state';}
   finally{componentControlPending=false;}
  };
  head.append(status,toggle);
 }
 const raw=document.createElement('details');raw.className='technical-details capability-raw';raw.dataset.technical=peer.peer_id+':'+kind+':'+id;raw.open=openedTechnical.has(raw.dataset.technical);
 const summary=document.createElement('summary');summary.className='visually-hidden';summary.textContent=(kind==='component'?'Component':'Product')+' · '+id;raw.append(summary);
 const info=document.createElement('button');info.type='button';info.className='icon-toggle';info.setAttribute('aria-label','Technical details for '+humanize(id));info.setAttribute('aria-expanded',String(raw.open));info.append(icon(icons.info,16));info.dataset.focusKey='info:'+raw.dataset.technical;
 info.onclick=()=>{const open=!raw.open;info.setAttribute('aria-expanded',String(open));if(open){raw.open=true;reveal(raw);}else conceal(raw,()=>{raw.open=false;});};head.append(info);
 // Opened, it is an inset well: the product and key manifest fields as label/value rows, then the manifest
 // itself behind "Raw manifest" (its open state survives re-renders like the well's, via data-technical).
 const well=document.createElement('div');well.className='capability-well';raw.append(well);
 const manifest=entry.manifest,names=list=>(list||[]).map(o=>o.name).filter(Boolean).join(', ');
 const facts=factsList([['ID',id,true],...made.map(p=>['Product',p.manifest.product_id,true]),['Outputs',names(manifest.observables)],['Operations',names(manifest.operables)],['Producer',manifest.producer&&[manifest.producer.component_id,manifest.producer.slot].filter(Boolean).join(' · ')],['Schema',entry.metadata?.schema],['Manifest hash',entry.manifest_hash,true]].filter(([,value])=>value));
 facts.classList.add('capability-facts');for(const value of facts.querySelectorAll('dd>span'))value.title=value.textContent;well.append(facts);
 for(const product of made)if(product.metadata?.schema==='spatial-harness.compact-pose-contract/v1')well.append(actionButton('Inspect live pose',{iconPath:icons.pin,onClick:()=>inspectPose(peer.peer_id,product,deviceName(peer)+' / '+product.manifest.product_id)}));
 const json=JSON.stringify(manifest,null,2),source=document.createElement('div');source.className='capability-manifest';source.id='manifest-'+raw.dataset.technical.replace(/[^\w-]/g,'_');source.dataset.technical=raw.dataset.technical+':json';source.hidden=!openedTechnical.has(source.dataset.technical);
 const pre=document.createElement('pre');pre.className='capability-json';pre.tabIndex=0;pre.setAttribute('aria-label','Manifest JSON');pre.append(...jsonTokens(json));source.append(pre);
 const header=document.createElement('div');header.className='capability-manifest-header';
 const toggle=disclosure('Raw manifest',{expanded:!source.hidden,controls:source.id,onToggle:()=>{const open=source.hidden;toggle.setAttribute('aria-expanded',String(open));if(open){source.hidden=false;reveal(source);}else conceal(source,()=>{source.hidden=true;});}});
 toggle.dataset.focusKey='manifest:'+raw.dataset.technical;header.append(toggle,copyButton(json,'Copy manifest'));well.append(header,source);
 row.append(raw);
 return row;
}
new MutationObserver(renderFleetRows).observe($('sensor-select'),{childList:true});$('sensor-select').addEventListener('change',renderFleetRows);renderFleetRows();
let noteSignature='';
function noteTitle(note){return note?.details?.title||note?.text||'';}
function notePhoto(note){
 const button=uiButton('',()=>{
  let dialog=$('photo-dialog');
  if(!dialog){dialog=document.createElement('dialog');dialog.id='photo-dialog';document.body.append(dialog);dialog.addEventListener('click',e=>{if(e.target===dialog)dialog.close();});}
  dialog.replaceChildren();const title=document.createElement('h3');title.textContent=noteTitle(note);dialog.append(title);
  const image=document.createElement('img');image.src=note.details.photo;image.alt='Photo for '+noteTitle(note);dialog.append(image,uiButton('Close photo',()=>dialog.close()));dialog.showModal();
 },'note-photo-button');button.setAttribute('aria-label','Open photo for '+noteTitle(note));
 const image=document.createElement('img');image.className='note-photo';image.src=note.details.thumbnail||note.details.photo;image.alt='Photo for '+noteTitle(note);button.append(image);return button;
}
function noteFields(container,note){
 const details=note?.details||{};
 const description=formField({label:'Description',multiline:true,max:10000,value:details.description,placeholder:'Optional details'});container.append(description.field);
 const picker=document.createElement('div');picker.className='photo-picker';
 const caption=document.createElement('span');caption.className='form-label';caption.textContent='Photo';
 const upload=document.createElement('input');upload.type='file';upload.accept='image/*';upload.hidden=true;upload.setAttribute('aria-label','Upload photo');
 const drop=document.createElement('button');drop.type='button';drop.className='photo-drop';drop.append(icon(icons.image,20));
 const dropText=document.createElement('span');dropText.innerHTML='<strong>Add photo</strong><small>Drop, paste or browse · JPEG or PNG</small>';drop.append(dropText);drop.onclick=()=>upload.click();
 const preview=document.createElement('figure');preview.className='photo-preview';
 const image=document.createElement('img');image.alt='Attached note photo';
 const replace=actionButton('Replace',{onClick:()=>upload.click()}),remove=actionButton('Remove photo',{variant:'danger-ghost'});
 const tools=document.createElement('div');tools.className='photo-preview-actions';tools.append(replace,remove);preview.append(image,tools);
 const status=document.createElement('p');status.className='form-status';status.setAttribute('role','status');
 picker.append(caption,drop,preview,upload,status);container.append(picker);
 let photo=details.photo||null,pending=Promise.resolve(),photoError=null,generation=0;
 function render(){preview.hidden=!photo;drop.hidden=!!photo;if(photo)image.src=photo;else image.removeAttribute('src');}render();
 remove.onclick=()=>{generation++;photo=null;photoError=null;upload.value='';status.textContent='';render();drop.focus();};
 function accept(file){if(!file)return;const version=++generation;photoError=null;status.textContent='Preparing photo…';pending=(async()=>{let bitmap;try{
  if(file.size>20*1024*1024)throw Error('Choose an image smaller than 20 MB.');
  bitmap=await createImageBitmap(file);const scale=Math.min(1,1280/Math.max(bitmap.width,bitmap.height));const canvas=document.createElement('canvas');canvas.width=Math.max(1,Math.round(bitmap.width*scale));canvas.height=Math.max(1,Math.round(bitmap.height*scale));const ctx=canvas.getContext('2d');ctx.fillStyle='#fff';ctx.fillRect(0,0,canvas.width,canvas.height);ctx.drawImage(bitmap,0,0,canvas.width,canvas.height);const encoded=canvas.toDataURL('image/jpeg',.8);if(encoded.length>1400000)throw Error('Photo is too large. Choose a smaller image.');if(version!==generation)return;photo=encoded;status.textContent='';render();
 }catch(e){if(version===generation){photoError=e;status.textContent=e.message;}}finally{bitmap?.close();}})();}
 upload.onchange=()=>accept(upload.files[0]);
 drop.addEventListener('dragover',e=>{e.preventDefault();drop.classList.add('dragging');});
 drop.addEventListener('dragleave',()=>drop.classList.remove('dragging'));
 drop.addEventListener('drop',e=>{e.preventDefault();drop.classList.remove('dragging');accept([...e.dataTransfer.files].find(f=>f.type.startsWith('image/')));});
 container.addEventListener('paste',e=>{const file=[...(e.clipboardData?.files||[])].find(f=>f.type.startsWith('image/'));if(file){e.preventDefault();accept(file);}});
 return {read:async title=>{await pending;if(photoError)throw photoError;return {title,description:description.input.value,photo};}};
}
const notesActions=document.createElement('div');notesActions.className='panel-actions';
const newNoteButton=actionButton('New note',{variant:'primary',iconPath:icons.plus,onClick:()=>startNotePlacement()});
const drawPathButton=actionButton('Draw path',{iconPath:icons.pen,onClick:()=>$('crayon').click()});
const notesReadOnly=document.createElement('p');notesReadOnly.className='panel-hint';
notesActions.append(newNoteButton,drawPathButton);
let notesQuery='';
const notesSearch=searchField({id:'notes-search',label:'Search notes and paths',placeholder:'Search notes and paths',onInput:value=>{notesQuery=value.trim().toLowerCase();renderNotes();}});
const notesList=document.createElement('div');notesList.className='notes-list';
notesBody.append(notesActions,notesReadOnly,notesSearch,notesList);
const drawPathHint=document.createElement('p');drawPathHint.className='panel-hint';notesActions.after(drawPathHint);
const syncDrawPath=()=>{drawPathButton.setAttribute('aria-pressed',$('crayon').getAttribute('aria-pressed')||'false');drawPathButton.disabled=$('crayon').disabled;drawPathHint.textContent=$('crayon').disabled?'Drawing isn’t available here · '+($('crayon').title||'Choose a local Home scene'):'';drawPathHint.hidden=!drawPathHint.textContent;};
new MutationObserver(syncDrawPath).observe($('crayon'),{attributes:true,attributeFilter:['aria-pressed','disabled','title']});syncDrawPath();
// Bottom bar: the creation tools mirror their panel buttons; Ask morphs the bar into the assistant.
agentBar=createAgentBar({root:document.querySelector('.agent-bar'),hasConversation:()=>!!(chatConversations.get(activeMap)?.length)||chatPending&&chatPendingMap===activeMap,tools:[
 {label:'New note',iconPath:icons.note,accent:'amber',source:newNoteButton,onTrigger:()=>placingNote?stopNotePlacement():startNotePlacement(),disabledReason:()=>'switch to a Home scene'},
 {label:'Draw path',iconPath:icons.pen,accent:'amber',source:$('crayon'),onTrigger:()=>$('crayon').click(),disabledReason:()=>'switch to a Home scene'},
 {label:'New volume',iconPath:'M9 1.5 15.5 5v8L9 16.5 2.5 13V5ZM2.5 5 9 8.5l6.5-3.5M9 8.5v8',accent:'violet',source:$('volume-draw'),onTrigger:()=>$('volume-draw').click(),disabledReason:()=>'select a local Home scene'},
]});syncChatCompact();
// Only one creation tool at a time: starting one ends the others, whichever button started it.
// In 2D the plan's floor, legend and scale take the tools' place in the bottom bar.
{
 const bar=document.querySelector('.agent-bar'),shell=bar.querySelector('.agent-shell');
 shell.insertBefore(map2D.controlStrip,bar.querySelector('.agent-divider'));
 setPlanBar=active=>{
  const from=shell.getBoundingClientRect().width;bar.toggleAttribute('data-plan',active);
  if(bar.dataset.mode==='agent'||matchMedia('(prefers-reduced-motion: reduce)').matches)return;
  const to=shell.getBoundingClientRect().width;shell.animate([{width:from+'px'},{width:to+'px'}],{duration:480,easing:'cubic-bezier(.3,1.3,.5,1)'});
  (active?map2D.controlStrip:bar.querySelector('.agent-tools')).animate([{opacity:0,filter:'blur(4px)',transform:'translateY(4px)'},{opacity:1,filter:'none',transform:'none'}],{duration:320,delay:80,easing:'cubic-bezier(.2,.8,.2,1)',fill:'backwards'});
 };
}
const creationTools=[{source:newNoteButton,stop:()=>stopNotePlacement()},{source:$('crayon'),stop:()=>$('crayon').click()},{source:$('volume-draw'),stop:()=>$('volume-draw').click()}];
for(const tool of creationTools)new MutationObserver(()=>{
 if(tool.source.getAttribute('aria-pressed')!=='true')return;
 for(const other of creationTools)if(other!==tool&&other.source.getAttribute('aria-pressed')==='true')other.stop();
}).observe(tool.source,{attributes:true,attributeFilter:['aria-pressed']});
// Right-click (without dragging) on the 3D scene opens the bottom bar's tools at that spot. Each tool
// starts from the point under the cursor, and Ask passes it to the assistant as the viewer's focus_point.
let chatFocus=null;
const spotMarker=new THREE.Mesh(new THREE.RingGeometry(.7,1,40),new THREE.MeshBasicMaterial({color:'#8b5cf6',side:THREE.DoubleSide,depthWrite:false,transparent:true,opacity:.9,polygonOffset:true,polygonOffsetFactor:-2}));
// Drawn under notes and everything else in the scene; it only marks the floor.
spotMarker.rotation.x=-Math.PI/2;spotMarker.renderOrder=-1;spotMarker.visible=false;scene.add(spotMarker);
// A donut centred on the spot: four tools on the diagonals of a glass ring split into wedges, the spot itself
// showing through the hollow centre. Disabled tools stay focusable (aria-disabled) so their reason is
// reachable from the keyboard; arrow keys step around the ring (roving tabindex).
const spotMenu=document.createElement('div');spotMenu.className='spot-menu';spotMenu.hidden=true;spotMenu.setAttribute('role','menu');spotMenu.setAttribute('aria-label','Tools at this spot');spotMenu.tabIndex=-1;document.body.append(spotMenu);
// Clockwise from the top right: [place, x, y, the side its hint opens on].
const SPOT_PLACES=[['top-right',1,-1,'right'],['bottom-right',1,1,'right'],['bottom-left',-1,1,'left'],['top-left',-1,-1,'left']],SPOT_RADIUS=50,SPOT_SIZE=152,SPOT_HOLE=18;
// Each tool is one quarter of the ring: a wedge for its hit area and, inset from it, a rounded wedge plate that
// lifts and tilts on hover. Drawn in a quadrant's own frame (x, y >= 0), mirrored into place and given in the
// coordinates of the tool's box, which is that quadrant of the menu; `gap` is the distance from the cross.
function spotWedge(qx,qy,{outer,inner,gap,round=0}){const c=SPOT_SIZE/2,k=round,sweep=qx*qy<0?1:0,
  p=([x,y])=>`${(qx>0?qx*x:c+qx*x).toFixed(2)} ${(qy>0?qy*y:c+qy*y).toFixed(2)}`,side=r=>Math.sqrt(r*r-gap*gap),e1=r=>[gap,side(r)],e2=r=>[side(r),gap],
  arc=(r,[x,y],turn)=>{const a=Math.atan2(y,x)+turn/r;return [r*Math.cos(a),r*Math.sin(a)];};
 return `path('M${p(e1(outer-k))} Q${p(e1(outer))} ${p(arc(outer,e1(outer),-k))} A${outer} ${outer} 0 0 ${sweep} ${p(arc(outer,e2(outer),k))} Q${p(e2(outer))} ${p(e2(outer-k))} L${p(e2(inner+k))} Q${p(e2(inner))} ${p(arc(inner,e2(inner),k))} A${inner} ${inner} 0 0 ${1-sweep} ${p(arc(inner,e1(inner),-k))} Q${p(e1(inner))} ${p(e1(inner+k))} Z')`;}
let spot=null,rightStart=null;
// About 10 px across whatever the distance.
function placeSpotMarker(point){const r=renderer.domElement.getBoundingClientRect();spotMarker.position.copy(point).add(new THREE.Vector3(0,.01,0));spotMarker.scale.setScalar(camera.position.distanceTo(point)*Math.tan(camera.fov*Math.PI/360)*10/r.height);spotMarker.visible=true;}
function spotTool(label,face,accent,run,disabledReason=''){
 const button=document.createElement('button');button.type='button';button.className='agent-tool';button.dataset.accent=accent;button.setAttribute('role','menuitem');button.setAttribute('aria-label',label);button.tabIndex=-1;
 button.dataset.tip=disabledReason?`${label} · ${disabledReason}`:label;if(disabledReason)button.setAttribute('aria-disabled','true');
 const hit=document.createElement('span'),lift=document.createElement('span'),plate=document.createElement('span'),glyph=document.createElement('span');hit.className='spot-hit';lift.className='spot-lift';plate.className='spot-plate';lift.append(plate);glyph.className='rail-face';glyph.append(typeof face==='string'?icon(face,20):face);button.append(hit,lift,glyph);
 // The ring has done its job once a tool starts there; only Ask keeps it, until the question is sent.
 button.onclick=()=>{if(disabledReason)return;const at=spot;closeSpotMenu();run(at);};return button;}
function closeSpotMenu(clearMarker=true){if(spotMenu.hidden||spotMenu.dataset.closing)return;clearTimeout(spotTipTimer);spotTip.hidden=true;spot=null;if(clearMarker&&!chatFocus)spotMarker.visible=false;
 if(matchMedia('(prefers-reduced-motion: reduce)').matches){spotMenu.hidden=true;return;}
 // A quick scale-down into the spot; the menu ignores the pointer while it goes.
 spotMenu.dataset.closing='true';spotMenu.getAnimations({subtree:true}).forEach(a=>a.cancel());
 spotMenu.animate([{opacity:1,transform:'none'},{opacity:0,transform:'scale(.6)'}],{duration:140,easing:'cubic-bezier(.4,0,1,1)'}).finished.then(()=>{if(spotMenu.dataset.closing){delete spotMenu.dataset.closing;spotMenu.hidden=true;}},()=>{});}
function openSpotMenu(e){
 const r=renderer.domElement.getBoundingClientRect(),ray=new THREE.Raycaster();ray.setFromCamera(new THREE.Vector2((e.clientX-r.left)/r.width*2-1,1-(e.clientY-r.top)/r.height*2),camera);
 const hit=ray.intersectObjects(selectable.filter(o=>{for(let p=o;p;p=p.parent)if(!p.visible)return false;return true;}))[0];
 const point=hit?hit.point.clone():ray.ray.intersectPlane(new THREE.Plane(new THREE.Vector3(0,1,0),0),new THREE.Vector3());
 if(!point)return;
 spot={point,request:{source:'scene',frame:current.frame,origin:ray.ray.origin.toArray(),direction:ray.ray.direction.toArray()}};
 placeSpotMarker(point);
 const writable=notesWritable(),orb=document.createElement('span');orb.className='agent-orb';orb.setAttribute('aria-hidden','true');
 const tools=[
  spotTool('New note here',icons.note,'amber',at=>beginLabel(at.request),writable&&!newNoteButton.disabled?'':'switch to a Home scene'),
  spotTool('Draw a path',icons.pen,'amber',()=>{if($('crayon').getAttribute('aria-pressed')!=='true')$('crayon').click();},$('crayon').disabled?'switch to a Home scene':''),
  spotTool('New volume here','M9 1.5 15.5 5v8L9 16.5 2.5 13V5ZM2.5 5 9 8.5l6.5-3.5M9 8.5v8','violet',at=>semanticVolumes.beginAt(at.point.toArray()),$('volume-draw').disabled?'select a local Home scene':''),
  spotTool('Ask about this spot',orb,'zinc')];
 const ask=tools[3];ask.classList.add('spot-ask');ask.onclick=()=>{chatFocus=spot.point.toArray().map(n=>Math.round(n*1000)/1000);closeSpotMenu(false);agentBar.open();};
 const offset=v=>Math.round(v*SPOT_RADIUS*Math.SQRT1_2);
 // Each tool's box is its quadrant (so the tilt pivots on the wedge); the icon sits on the diagonal at SPOT_RADIUS.
 tools.forEach((tool,i)=>{const [place,dx,dy,side]=SPOT_PLACES[i],half=SPOT_SIZE/2;tool.dataset.place=place;tool.dataset.side=side;
  tool.style.left=(dx>0?half:0)+'px';tool.style.top=(dy>0?half:0)+'px';tool.style.transformOrigin=`${dx>0?0:100}% ${dy>0?0:100}%`;
  tool.style.setProperty('--x',(dx>0?offset(1):half-offset(1))+'px');tool.style.setProperty('--y',(dy>0?offset(1):half-offset(1))+'px');
  tool.querySelector('.spot-hit').style.clipPath=spotWedge(dx,dy,{outer:half-.5,inner:SPOT_HOLE+1,gap:1});tool.querySelector('.spot-plate').style.clipPath=spotWedge(dx,dy,{outer:half-8,inner:SPOT_HOLE+7,gap:6,round:8});});
 spotMenu.replaceChildren(...tools);spotMenu.getAnimations({subtree:true}).forEach(a=>a.cancel());delete spotMenu.dataset.closing;spotMenu.hidden=false;
 // Centred on the cursor, shifted inward near an edge so the whole ring stays on screen.
 const half=SPOT_SIZE/2,x=Math.min(Math.max(half+8,e.clientX),innerWidth-half-8),y=Math.min(Math.max(half+8,e.clientY),innerHeight-half-8);
 spotMenu.style.left=x-half+'px';spotMenu.style.top=y-half+'px';
 // Tools fly out under a resting cursor: hints wait until they have landed.
 clearTimeout(spotMenu.settle);delete spotMenu.dataset.opening;
 if(!matchMedia('(prefers-reduced-motion: reduce)').matches){
  const spring='cubic-bezier(.34,1.56,.64,1)';spotMenu.dataset.opening='true';spotMenu.settle=setTimeout(()=>delete spotMenu.dataset.opening,620);
  spotMenu.animate([{opacity:0,transform:'scale(.4)'},{opacity:1,transform:'none'}],{duration:380,easing:spring});
  tools.forEach((tool,i)=>tool.animate([{opacity:0,transform:'scale(.5)'},{opacity:1,transform:'none'}],{duration:420,delay:50+i*45,easing:spring,fill:'backwards'}));
 }
 // The menu takes focus, not a tool: nothing looks selected until the pointer or an arrow key picks one.
 spotMenu.focus({preventScroll:true});
}
// Hints come quickly here: the menu is opened to be used at once. Each sits outside its tool, away from the ring.
const spotTip=document.createElement('div');spotTip.className='rail-tooltip spot-tip';spotTip.setAttribute('aria-hidden','true');spotTip.hidden=true;document.body.append(spotTip);
let spotTipTimer=0;
function showSpotTip(button){
 spotTip.textContent=button.dataset.tip;spotTip.dataset.side=button.dataset.side;spotTip.hidden=false;
 // Level with the tool's icon, just outside the ring on its side.
 const ring=spotMenu.getBoundingClientRect(),box=button.querySelector('.rail-face').getBoundingClientRect(),tip=spotTip.getBoundingClientRect(),gap=10;
 const x=button.dataset.side==='left'?ring.left-gap-tip.width:ring.right+gap,y=box.top+(box.height-tip.height)/2;
 spotTip.style.left=Math.min(Math.max(8,x),innerWidth-tip.width-8)+'px';spotTip.style.top=Math.min(Math.max(8,y),innerHeight-tip.height-8)+'px';}
spotMenu.addEventListener('pointerover',e=>{const button=e.target.closest('[data-tip]');if(!button||spotMenu.dataset.opening)return;clearTimeout(spotTipTimer);spotTipTimer=setTimeout(()=>showSpotTip(button),spotTip.hidden?120:0);});
spotMenu.addEventListener('pointerout',e=>{if(!e.relatedTarget?.closest?.('.spot-menu [data-tip]')){clearTimeout(spotTipTimer);spotTip.hidden=true;}});
spotMenu.addEventListener('focusin',e=>{if(e.target===spotMenu)return;for(const item of spotMenu.querySelectorAll('[role="menuitem"]'))item.tabIndex=item===e.target?0:-1;if(e.target.matches(':focus-visible')&&!spotMenu.dataset.opening){clearTimeout(spotTipTimer);showSpotTip(e.target);}});
renderer.domElement.addEventListener('pointerdown',e=>{if(e.button===2)rightStart=[e.clientX,e.clientY];else closeSpotMenu();});
renderer.domElement.addEventListener('pointerup',e=>{
 if(e.button!==2||!rightStart)return;const moved=Math.hypot(e.clientX-rightStart[0],e.clientY-rightStart[1]);rightStart=null;
 // A right-drag orbits the view; only a still right-click opens the menu.
 if(moved>4||!current||map2D.isActive()||crayon.isActive()||placingNote)return;openSpotMenu(e);});
document.addEventListener('pointerdown',e=>{if(!spotMenu.hidden&&!spotMenu.contains(e.target)&&e.target!==renderer.domElement)closeSpotMenu();},true);
document.addEventListener('keydown',e=>{if(e.key==='Escape'&&!spotMenu.hidden){e.preventDefault();closeSpotMenu();}});
// Right and Down step clockwise, Left and Up back; Home and End go to the first and last tool.
spotMenu.addEventListener('keydown',e=>{const items=[...spotMenu.querySelectorAll('[role="menuitem"]')],at=items.indexOf(document.activeElement),step={ArrowRight:1,ArrowDown:1,ArrowLeft:-1,ArrowUp:-1}[e.key];
 const next=step?items[at<0?(step>0?0:items.length-1):(at+step+items.length)%items.length]:e.key==='Home'?items[0]:e.key==='End'?items.at(-1):null;if(!next)return;e.preventDefault();next.focus();});
function notesWritable(){return !activeMap.startsWith('peer-scene:')&&!activeMap.startsWith('phone:')&&!isRunner()&&activeMap!=='domain';}
function noteRowTile(note){
 if(!note.details?.thumbnail)return icons.note;
 const image=document.createElement('img');image.src=note.details.thumbnail;image.alt='';image.className='list-row-photo';return image;
}
let notesSignature='';
function renderNotes(){
 const notes=current?.notes||[],paths=current?.paths?.items||[];
 const writable=notesWritable();
 const signature=JSON.stringify([activeMap,writable,notesQuery,selectedNoteId,selectedPathId,notes.map(n=>[n.id,noteTitle(n),n.details?.description,Boolean(n.details?.thumbnail),noteReadOnlyReason(n)]),paths.map(p=>[p.id,p.title,p.color,p.segments.length])]);
 newNoteButton.disabled=!writable||!current;notesReadOnly.hidden=writable;notesReadOnly.textContent=writable?'':'This map is read-only. Switch to a Home scene to add notes.';
 if(signature===notesSignature)return;notesSignature=signature;
 const focused=notesList.contains(document.activeElement)?document.activeElement.dataset.noteRow||document.activeElement.dataset.pathRow:null;
 const matches=(...text)=>!notesQuery||text.filter(Boolean).join(' ').toLowerCase().includes(notesQuery);
 const shownNotes=notes.filter(n=>matches(noteTitle(n),n.details?.description)),shownPaths=paths.filter(p=>matches(p.title));
 notesSearch.hidden=!notes.length&&!paths.length&&!notesQuery;
 notesList.replaceChildren();
 notesList.append(sectionHeader('Notes',notesQuery?`${shownNotes.length} / ${notes.length}`:notes.length));
 for(const note of shownNotes){
  const reason=noteReadOnlyReason(note),lock=reason?document.createElement('span'):null;
  if(lock){lock.className='row-flag';lock.append(icon(icons.lock,14));lock.setAttribute('aria-label','Read-only');lock.title=reason;}
  const row=listRow({title:noteTitle(note),meta:(note.details?.description||'').split('\n')[0]||noteAnchorText(note),tile:noteRowTile(note),accent:'amber',trailing:lock,onSelect:e=>{focusNote(note);focusCard(e,()=>notesList.querySelector(`[data-note-row="${CSS.escape(note.id)}"]`));}});
  row.dataset.noteRow=note.id;if(note.id===selectedNoteId)markSelected(row,'notes',note.id);notesList.append(row);
 }
 if(!shownNotes.length)notesList.append(notesQuery?emptyState('No notes match',{label:'Clear search',onClick:()=>{notesSearch.querySelector('.search-clear').click();}}):emptyState(writable?'No notes here yet. Right-click any surface for a quick note.':'No notes in this map.'));
 if(paths.length||notesQuery){
  notesList.append(sectionHeader('Paths',notesQuery?`${shownPaths.length} / ${paths.length}`:paths.length));
  for(const path of shownPaths){const row=listRow({title:path.title,meta:pathLength(path).toFixed(1)+' m',tile:pathSwatch(path,14),onSelect:e=>{focusPath(path);focusCard(e,()=>notesList.querySelector(`[data-path-row="${CSS.escape(path.id)}"]`));}});row.dataset.pathRow=path.id;row.dataset.pathId=path.id;if(path.id===selectedPathId)row.setAttribute('aria-current','true');notesList.append(row);}
  if(!shownPaths.length)notesList.append(emptyState('No paths match'));
 }
 if(focused)notesList.querySelector(`[data-note-row="${CSS.escape(focused)}"],[data-path-row="${CSS.escape(focused)}"]`)?.focus();
}
setInterval(()=>{const signature=JSON.stringify([activeMap,current?.notes||[],current?.paths]);if(signature!==noteSignature){noteSignature=signature;renderNotes();}},500);renderNotes();
function applyTheme(light){document.documentElement.dataset.theme=light?'light':'dark';scene.background.set(light?'#e8e8f0':'#0d0d0f');scene.fog?.color.copy(scene.background);ground.material.color.set(light?'#e0e3e8':'#101014');for(const layer of [gridMinor,gridMajor])layer.material.color.set(light?'#52525b':'#a1a1aa');for(const choice of document.querySelectorAll('[data-theme-choice]'))choice.setAttribute(choice.getAttribute('role')==='radio'?'aria-checked':'aria-pressed',String((choice.dataset.themeChoice==='light')===light));}
for(const choice of document.querySelectorAll('[data-theme-choice]'))choice.onclick=()=>{const light=choice.dataset.themeChoice==='light';applyTheme(light);try{localStorage.setItem('spatial-theme',light?'light':'dark');}catch{}};
try{applyTheme(localStorage.getItem('spatial-theme')==='light');}catch{}
const opacityLabel=document.createElement('label');opacityLabel.className='layer';opacityLabel.textContent='Grid opacity';const opacityInput=document.createElement('input');opacityInput.type='range';opacityInput.min='0';opacityInput.max='1';opacityInput.step='.01';opacityInput.value=String(grid.material.opacity);opacityInput.oninput=()=>{grid.material.opacity=Number(opacityInput.value);};opacityLabel.append(opacityInput);panelBodies.settings.append(opacityLabel);
// Settings: grouped layer switches, display options, and demo tools tucked into a disclosure.
{
 const body=panelBodies.settings,load=key=>{try{return JSON.parse(localStorage.getItem(key));}catch{return null;}},save=(key,value)=>{try{localStorage.setItem(key,JSON.stringify(value));}catch{}};
 const layers=document.createElement('section');layers.setAttribute('aria-label','Layers');layers.append(sectionHeader('Layers'));
 const renames={'Text notes':'Notes','Live Galbot poses':'Robot poses'},saved=load('settings-layers')||{};
 for(const [name,selectors] of [['Annotations',['[data-layer="notes"]']],['Scene',['[data-layer="voxels"]','[data-layer="portals"]','[data-layer="objects"]']],['Live',['#runner-layer','#robot-volume-layer','[data-layer="rays"]']]]){
  const heading=document.createElement('h4');heading.className='settings-group';heading.textContent=name;layers.append(heading);
  for(const selector of selectors){
   const input=body.querySelector(selector);if(!input)continue;const label=input.closest('label');label.className='setting-row';
   input.setAttribute('role','switch');input.classList.add('switch-input');
   for(const node of label.querySelector('span').childNodes)if(node.nodeType===3&&renames[node.textContent.trim()])node.textContent=renames[node.textContent.trim()];
   const key=input.dataset.layer||input.id;
   if(typeof saved[key]==='boolean'&&saved[key]!==input.checked){input.checked=saved[key];input.dispatchEvent(new Event('change'));}
   input.addEventListener('change',()=>{saved[key]=input.checked;save('settings-layers',saved);});
   layers.append(label);
   if(input.id==='robot-volume-layer'){const status=$('robot-volume-status');status.classList.add('setting-meta');layers.append(status);}
  }
 }
 // Display: grid opacity with a readable value, and the theme.
 const display=document.createElement('section');display.setAttribute('aria-label','Display');display.append(sectionHeader('Display'));
 opacityLabel.className='setting-row range-row';opacityLabel.firstChild.textContent='Grid opacity';
 const value=document.createElement('output');value.className='range-value';
 const syncOpacity=()=>{const percent=Math.round(Number(opacityInput.value)*100);value.textContent=percent+'%';opacityInput.setAttribute('aria-valuetext',percent+' percent');opacityInput.style.setProperty('--fill',percent+'%');};
 const savedOpacity=load('settings-grid-opacity');if(typeof savedOpacity==='number'){opacityInput.value=String(savedOpacity);grid.material.opacity=savedOpacity;}
 opacityInput.addEventListener('input',()=>{syncOpacity();save('settings-grid-opacity',Number(opacityInput.value));});syncOpacity();
 opacityLabel.insertBefore(value,opacityInput);
 const theme=document.createElement('div');theme.className='setting-row';
 const themeLabel=document.createElement('span');themeLabel.id='settings-theme-label';themeLabel.textContent='Theme';
 const choices=document.createElement('div');choices.className='theme-switch';choices.setAttribute('role','radiogroup');choices.setAttribute('aria-labelledby',themeLabel.id);
 for(const [choice,text] of [['light','Light'],['dark','Dark']]){const button=document.createElement('button');button.type='button';button.setAttribute('role','radio');button.dataset.themeChoice=choice;button.textContent=text;button.onclick=()=>{const light=choice==='light';applyTheme(light);try{localStorage.setItem('spatial-theme',light?'light':'dark');}catch{}};choices.append(button);}
 theme.append(themeLabel,choices);display.append(opacityLabel,theme);
 // Demo tools: shown only in demo scenes (the app toggles #demo-controls), collapsed by default.
 const demo=$('demo-controls');
 for(const label of demo.querySelectorAll('.section-label'))label.remove();
 const region=document.createElement('div');region.id='demo-tools';region.hidden=true;region.append(...demo.childNodes);
 const peersHeading=document.createElement('h4');peersHeading.className='settings-group';peersHeading.textContent='Contributing peers';region.prepend(peersHeading);
 for(const [id,hint] of [['load','Adds iPhone and Bracketbot scans to the map.'],['project','Places a book detection onto the nearest surface.'],['replay','Re-sends the iPhone scans to check nothing duplicates.']]){const button=region.querySelector('#'+id);button.className='btn btn-secondary demo-button';button.querySelector('span')?.remove();const small=document.createElement('small');small.className='form-helper';small.textContent=hint;button.after(small);}
 const toggle=disclosure('Demo tools',{expanded:false,controls:'demo-tools',onToggle:()=>{}});
 toggle.onclick=()=>{const open=region.hidden;toggle.setAttribute('aria-expanded',String(open));if(open){region.hidden=false;reveal(region);}else conceal(region,()=>{region.hidden=true;});};
 demo.append(toggle,region);
 body.replaceChildren(layers,display,demo);
 applyTheme(document.documentElement.dataset.theme==='light');
}

let createdFiducialKey=null;
// Tags: list first; "New tag" reveals the generate form; each row expands to print, download and measure.
let fiducialCreateOpen=null;const expandedFiducials=new Set();
const relativeTime=new Intl.RelativeTimeFormat(undefined,{numeric:'auto'});
function ago(ms){const minutes=Math.round((Date.now()-ms)/60000);if(minutes<1)return 'just now';if(minutes<60)return minutes+' min ago';const hours=Math.round(minutes/60);if(hours<24)return hours+' h ago';return relativeTime.format(-Math.round(hours/24),'day');}
let explainerCount=0;
function measureExplainer(){
 const figure=document.createElement('figure');figure.className='measure-explainer';const hatch='quiet-zone-'+(++explainerCount);
 const caption='Edge to edge across the black square. Leave out the white border.';
 figure.innerHTML=`<svg role="img" aria-label="${caption}" width="168" height="84" viewBox="0 0 168 84"><defs><pattern id="${hatch}" width="4" height="4" patternUnits="userSpaceOnUse" patternTransform="rotate(45)"><line x1="0" y1="0" x2="0" y2="4" stroke="#d4d4d8" stroke-width="1.5"/></pattern></defs><rect x="4" y="4" width="56" height="56" fill="url(#${hatch})" stroke="#d4d4d8"/><rect x="12" y="12" width="40" height="40" fill="#18181b"/><rect x="20" y="20" width="8" height="8" fill="#fff"/><rect x="36" y="28" width="8" height="8" fill="#fff"/><rect x="24" y="36" width="8" height="8" fill="#fff"/><path d="M12 66h40M12 63v6M52 63v6M15 64l-3 2 3 2M49 64l3 2-3 2" stroke="#2563eb" stroke-width="1.2" fill="none"/><text x="32" y="80" font-size="9" fill="#1d4ed8" font-weight="600" text-anchor="middle">Measure this</text><text x="70" y="10" font-size="9" fill="#71717a">Not this</text><path d="M68 7h-6" stroke="#a1a1aa"/></svg><figcaption>${caption}</figcaption>`;
 return figure;
}
function mmField(label,id,value,{min=10,max=2000,placeholder=''}={}){
 const field=document.createElement('div');field.className='form-field';
 const name=document.createElement('label');name.className='form-label';name.textContent=label;name.htmlFor=id;
 const box=document.createElement('div');box.className='unit-field';
 const input=document.createElement('input');input.id=id;input.className='form-control';input.type='number';input.min=String(min);input.max=String(max);input.step='.1';input.value=value;input.placeholder=placeholder;
 const unit=document.createElement('span');unit.className='unit-suffix';unit.textContent='mm';unit.setAttribute('aria-hidden','true');
 box.append(input,unit);field.append(name,box);return {field,input};
}
async function renderFiducials(focusKey=null){
 let catalog;
 try{catalog=await api('/api/fiducials');}catch(e){tagsBody.replaceChildren();const status=document.createElement('p');status.className='panel-hint';status.setAttribute('role','status');status.textContent=e.message;tagsBody.append(status);return;}
 tagsBody.replaceChildren();
 const createOpen=fiducialCreateOpen??!catalog.fiducials.length;
 const newTag=actionButton('New tag',{variant:'primary',iconPath:icons.plus});newTag.setAttribute('aria-expanded',String(createOpen));newTag.setAttribute('aria-controls','fiducial-create');
 newTag.onclick=()=>{const go=()=>{fiducialCreateOpen=!createOpen;return renderFiducials();};if(createOpen)conceal($('fiducial-create'),go);else go().then(()=>{reveal($('fiducial-create'));$('fiducial-family')?.focus();});};
 const actions=document.createElement('div');actions.className='panel-actions single';actions.append(newTag);
 const intro=document.createElement('p');intro.className='panel-hint';intro.textContent='Print markers robots can see, then measure the printed black square so poses come out to scale.';
 tagsBody.append(actions,intro);
 // Generate form.
 const form=document.createElement('form');form.id='fiducial-create';form.className='fiducial-create';form.hidden=!createOpen;
 const familyField=document.createElement('div');familyField.className='form-field';const familyLabel=document.createElement('label');familyLabel.className='form-label';familyLabel.textContent='Tag family';
 const family=document.createElement('select');family.id='fiducial-family';family.className='form-control';familyLabel.htmlFor=family.id;for(const item of catalog.families)family.add(new Option(item.label,item.id));familyField.append(familyLabel,family);
 const idField=document.createElement('div');idField.className='form-field';const idLabel=document.createElement('label');idLabel.className='form-label';idLabel.textContent='Marker ID (optional)';
 const id=document.createElement('input');id.id='fiducial-id';id.className='form-control';id.type='number';id.min='0';id.step='1';id.placeholder='Next available';idLabel.htmlFor=id.id;idField.append(idLabel,id);
 const size=mmField('Printed marker size (mm)','fiducial-size','150');size.input.required=true;
 const helper=document.createElement('small');helper.className='form-helper';helper.textContent='Size of the black square on paper';size.field.append(helper);
 const message=document.createElement('p');message.className='form-status';message.setAttribute('role','status');
 const generate=actionButton('Generate tag',{variant:'primary',type:'submit'}),cancel=actionButton('Cancel',{onClick:()=>{fiducialCreateOpen=false;renderFiducials().then(()=>tagsBody.querySelector('.panel-actions .btn')?.focus());}});
 const formActions=document.createElement('div');formActions.className='card-actions';if(catalog.fiducials.length)formActions.append(cancel);formActions.append(generate);
 form.append(familyField,idField,size.field,measureExplainer(),formActions,message);
 form.addEventListener('keydown',e=>{if(e.key==='Escape'&&catalog.fiducials.length){e.stopPropagation();cancel.click();}});
 form.onsubmit=async event=>{event.preventDefault();generate.disabled=true;message.textContent='Generating…';
  try{const marker=await api('/api/fiducials',{family:family.value,id:id.value===''?null:Number(id.value),side_length_m:Number(size.input.value)/1000});
   createdFiducialKey=marker.key;fiducialCreateOpen=false;expandedFiducials.add(marker.key);await renderFiducials(marker.key);toast(`${marker.family_label} · ${marker.id} created`);
  }catch(e){message.textContent=e.message;generate.disabled=false;}};
 tagsBody.append(form);
 if(!catalog.fiducials.length){tagsBody.append(emptyState('No tags yet. Generate a marker, print it at 100%, then measure the black square.'));return;}
 // Tags grouped by family, newest first.
 for(const item of catalog.families){
  const markers=catalog.fiducials.filter(m=>m.family===item.id).sort((a,b)=>(b.created_unix_ms||0)-(a.created_unix_ms||0));if(!markers.length)continue;
  tagsBody.append(sectionHeader(item.label,markers.length));
  for(const marker of markers){
   const open=expandedFiducials.has(marker.key),bodyId='fiducial-'+marker.key.replace(/[^a-z0-9]/gi,'-');
   const card=document.createElement('article');card.className='fiducial-card';if(marker.key===createdFiducialKey)card.dataset.created='true';
   const row=document.createElement('button');row.type='button';row.className='fiducial-row';row.setAttribute('aria-expanded',String(open));row.setAttribute('aria-controls',bodyId);
   const tile=document.createElement('span');tile.className='fiducial-tile';const image=document.createElement('img');image.className='fiducial-preview';image.src=marker.download_url;image.alt=`${marker.family_label} marker ${marker.id}`;tile.append(image);
   const text=document.createElement('span');text.className='list-row-text';const title=document.createElement('span');title.className='list-row-title';title.textContent=`${marker.family_label} · ${marker.id}`;
   const meta=document.createElement('span');meta.className='list-row-meta';meta.textContent=`${marker.side_length_mm.toLocaleString(undefined,{maximumFractionDigits:1})} mm black square`+(marker.created_unix_ms?` · ${ago(marker.created_unix_ms)}`:'');meta.title=meta.textContent;text.append(title,meta);
   const chevron=icon(icons.chevron);chevron.classList.add('fiducial-chevron');
   row.append(tile,text,chevron);
   row.onclick=()=>{const refocus=()=>tagsBody.querySelector(`[aria-controls="${bodyId}"]`)?.focus();if(expandedFiducials.has(marker.key))conceal($(bodyId),()=>{expandedFiducials.delete(marker.key);renderFiducials().then(refocus);});else{expandedFiducials.add(marker.key);renderFiducials().then(()=>{reveal($(bodyId));refocus();});}};
   const body=document.createElement('div');body.id=bodyId;body.className='fiducial-body';body.hidden=!open;
   const sizeText=`${marker.side_length_mm.toLocaleString(undefined,{maximumFractionDigits:1})} mm`;
   const print=document.createElement('a');print.className='btn btn-secondary btn-sm';print.href=marker.print_url;print.target='_blank';print.rel='noopener';print.textContent=`Print at ${sizeText} ↗`;print.setAttribute('aria-label',`Print at ${sizeText} (opens in a new tab)`);
   const download=document.createElement('a');download.className='btn btn-secondary btn-sm';download.href=marker.download_url;download.download=`${marker.key}.svg`;download.textContent='Download SVG';
   const linkRow=document.createElement('div');linkRow.className='fiducial-actions';linkRow.append(print,download);
   const measured=document.createElement('div');measured.className='fiducial-measured';const measuredText=document.createElement('span');measuredText.textContent=`Measured black square: ${sizeText}`;
   const edit=actionButton('Set measured size',{variant:'secondary'});edit.classList.add('btn-sm');measured.append(measuredText,edit);
   body.append(linkRow,measured);
   edit.onclick=()=>{
    const measure=document.createElement('form');measure.className='fiducial-measurement';
    const field=mmField('Measured black square (mm)',bodyId+'-measured',String(marker.side_length_mm));field.input.required=true;
    const status=document.createElement('p');status.className='form-status';status.setAttribute('role','status');
    const save=actionButton('Save size',{variant:'primary',type:'submit'}),back=actionButton('Cancel',{onClick:()=>{measure.replaceWith(measured);edit.focus();}});
    const buttons=document.createElement('div');buttons.className='card-actions';buttons.append(back,save);
    measure.append(field.field,measureExplainer(),buttons,status);measured.replaceWith(measure);field.input.focus();field.input.select();
    measure.addEventListener('keydown',e=>{if(e.key==='Escape'){e.stopPropagation();back.click();}});
    measure.onsubmit=async event=>{event.preventDefault();save.disabled=true;try{await api(`/api/fiducials/${marker.family}/${marker.id}`,{side_length_m:Number(field.input.value)/1000},'PATCH');await renderFiducials();toast('Measured size saved');}catch(e){status.textContent=e.message;save.disabled=false;}};
   };
   card.append(row,body);tagsBody.append(card);
   if(marker.key===focusKey)requestAnimationFrame(()=>print.focus());
  }
 }
 createdFiducialKey=null;
}
renderFiducials();
// The interface is built: show it (see html.booting in style.css).
requestAnimationFrame(()=>document.documentElement.classList.replace('booting','booted'));
