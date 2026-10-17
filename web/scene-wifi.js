// Browser → Home management only. Peers retrieve credentials through the SDK.
export function createSceneWifi({container,getScene,getName}) {
 const button=document.createElement('button');button.type='button';button.className='secondary';button.textContent='Scene Wi-Fi';container.append(button);
 let dialog=null,request=null,lastScene=null;
 function close(){request?.abort();request=null;if(dialog){dialog.querySelectorAll('input').forEach(i=>i.value='');dialog.close();dialog.remove();dialog=null;}}
 function sync(){const selected=getScene();button.disabled=!selected.startsWith('local:');button.title=button.disabled?'Select a local Home scene':'Configure Wi-Fi for onboarding';if(selected!==lastScene)close();lastScene=selected;}
 async function open(){
  close();const selected=getScene();if(!selected.startsWith('local:'))return;
  const url='/api/local-scenes/'+encodeURIComponent(selected.slice(6))+'/wifi';
  const modal=document.createElement('dialog');dialog=modal;modal.className='scene-wifi-dialog';
  modal.innerHTML=`<form><h2>Scene Wi-Fi</h2><p class="wifi-scene"></p>
   <p>Included in the first onboarding QR. Kept when this scene is reset.</p>
   <fieldset disabled><label>Network name (SSID)<input name="ssid" required maxlength="32" autocomplete="off"></label>
   <label>Security<select name="security"><option value="personal">WPA personal</option><option value="open">Open network</option></select></label>
   <label class="wifi-password-label">Password<input name="password" type="password" maxlength="63" autocomplete="new-password"></label>
   <label class="wifi-check"><input name="hidden" type="checkbox">Hidden network</label>
   <h3>Allowed onboarding hosts</h3><p>Only selected peers can retrieve the password and put it in a QR. Anyone viewing that QR can read it.</p>
   <div class="wifi-hosts"></div></fieldset>
   <p role="status" class="wifi-status">Loading…</p><div class="wifi-actions"><button type="button" class="wifi-remove secondary" disabled>Remove settings</button><button type="button" class="wifi-close secondary">Cancel</button><button type="submit" class="primary" disabled>Save</button></div></form>`;
  const form=modal.querySelector('form'),fields=form.querySelector('fieldset'),save=form.querySelector('[type=submit]'),remove=form.querySelector('.wifi-remove'),status=form.querySelector('.wifi-status');
  const input=name=>form.elements.namedItem(name);
  modal.querySelector('.wifi-scene').textContent=getName();
  form.querySelector('.wifi-close').onclick=close;modal.addEventListener('cancel',e=>{e.preventDefault();close();});
  document.body.append(modal);modal.showModal();
  let saved=null;
  async function json(path,body){
   const controller=new AbortController();request=controller;const timer=setTimeout(()=>controller.abort(),15000);
   try {const r=await fetch(path,{method:body?'POST':'GET',headers:body?{'Content-Type':'application/json'}:{},body:body?JSON.stringify(body):undefined,cache:'no-store',signal:controller.signal});
    if(!r.ok){const failure=await r.json().catch(()=>null);throw new Error(failure?.error||'Home could not '+(body?'save':'load')+' scene Wi-Fi settings. Check the network name, password and Home connection.');}return await r.json();
   }finally{clearTimeout(timer);if(request===controller)request=null;}
  }
  const security=()=>{const protectedNetwork=input('security').value==='personal';input('password').disabled=!protectedNetwork;form.querySelector('.wifi-password-label').hidden=!protectedNetwork;};
  input('security').onchange=security;
  try {
   saved=await json(url);if(dialog!==modal)return;
   input('ssid').value=saved.ssid||'';input('hidden').checked=saved.hidden;input('security').value=saved.ssid&&!saved.has_password?'open':'personal';
   input('password').placeholder=saved.has_password?'Saved · leave blank to keep':'8–63 bytes';
   let network={};try{network=await json('/api/peer');}catch{ /* Existing grants still remain editable. */ }
   if(dialog!==modal)return;
   const rows=new Map((network.peers||[]).map(p=>[p.peer_id,p]));for(const id of saved.allowed_peer_ids)if(!rows.has(id))rows.set(id,{peer_id:id});
   const hosts=form.querySelector('.wifi-hosts');
   for(const [id,peer] of rows){
    const label=document.createElement('label');label.className='wifi-check';const check=document.createElement('input');check.type='checkbox';check.dataset.peerId=id;check.checked=saved.allowed_peer_ids.includes(id);
    const text=document.createElement('span'),name=document.createElement('span'),detail=document.createElement('small');name.textContent=peer.alias||network.aliases?.[id]||peer.name||'Peer';detail.textContent=id;text.append(name,detail);label.append(check,text);hosts.append(label);
   }
   if(!rows.size)hosts.textContent='Connect Annotator to Home, then reopen this dialog to authorize it.';
   fields.disabled=false;security();save.disabled=false;remove.disabled=!saved.ssid;status.textContent='Passwords are never returned to this editor.';
  }catch(e){if(dialog===modal)status.textContent=e.name==='AbortError'?'Request timed out. Close and retry.':e.message;}
  async function write(body){
   fields.disabled=true;save.disabled=true;remove.disabled=true;status.textContent='Saving…';
   try{await json(url,body);if(dialog!==modal)return;input('password').value='';status.textContent=body.action==='remove'?'Settings removed.':'Saved. Annotator will use these settings for its next invitation.';setTimeout(()=>{if(dialog===modal)close();},900);}
   catch(e){if(dialog!==modal)return;fields.disabled=false;security();save.disabled=false;remove.disabled=!saved?.ssid;status.textContent=e.name==='AbortError'?'Request timed out. Reopen to check whether it saved.':e.message;}
  }
  form.onsubmit=e=>{
   e.preventDefault();if(save.disabled)return;
   const ssid=input('ssid').value,password=input('password').value,protectedNetwork=input('security').value==='personal';
   if(protectedNetwork&&!password&&(!saved.has_password||saved.ssid!==ssid)){status.textContent='Enter a password for this network.';return;}
   write({action:'save',ssid,password:!protectedNetwork?{action:'clear'}:password?{action:'set',value:password}:{action:'keep'},hidden:input('hidden').checked,allowed_peer_ids:[...form.querySelectorAll('[data-peer-id]:checked')].map(i=>i.dataset.peerId)});
  };
  remove.onclick=()=>{if(confirm('Remove this scene’s saved Wi-Fi settings and permissions?'))write({action:'remove'});};
 }
 button.onclick=open;sync();return {sync,close};
}
