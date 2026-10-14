// Shared building blocks for the sidebar panels. Styles live in style.css under "Panel components".

export const icons={
 map:'M2 4l5-2 4 2 5-2v12l-5 2-4-2-5 2ZM7 2v12M11 4v12',
 device:'M4 6h10v9H4ZM6 2h6v4M7 9h.1M11 9h.1M7 12h4M1 10h3M14 10h3',
 anchor:'M9 5a2 2 0 1 0 0-4 2 2 0 0 0 0 4M9 5v12M4 9H2a7 7 0 0 0 14 0h-2',
 landmark:'M9 16s-5-4.5-5-8a5 5 0 0 1 10 0c0 3.5-5 8-5 8ZM9 9.5a1.5 1.5 0 1 0 0-3 1.5 1.5 0 0 0 0 3',
 chevron:'M7 4.5 11.5 9 7 13.5',
 search:'M8 14A6 6 0 1 0 8 2a6 6 0 0 0 0 12M16 16l-3.5-3.5',
 close:'M4.5 4.5l9 9M13.5 4.5l-9 9',
 copy:'M6 6h9v9H6ZM3 12V3h9',
 check:'M3.5 9.5 7 13l7.5-8',
 phone:'M5 1.5h8v15H5ZM8 13.5h2',
 camera:'M1.5 5.5 12 3l1.5 5L3 10.5ZM13 6l3.5-1v4.5L13.5 9M7 9.5v4.5H3',
 forklift:'M2 13V6.5h5.5L10 10v3M10 2.5V13h6M2 13a1.5 1.5 0 1 0 3 0 1.5 1.5 0 0 0-3 0M7 13a1.5 1.5 0 1 0 3 0 1.5 1.5 0 0 0-3 0',
 plus:'M9 3.5v11M3.5 9h11',
 pen:'M3 15l1-4 7.5-7.5 3 3L7 14ZM10 5l3 3',
 note:'M4 2h10v14H4ZM6 6h6M6 9h6M6 12h4',
 lock:'M5.5 8V6a3.5 3.5 0 0 1 7 0v2M4 8h10v8H4Z',
 image:'M2 3.5h14v11H2ZM2 12l4-4 4 4 2-2 4 4M12 7h.01',
 pin:'M9 16.5s-5-4.6-5-8.5a5 5 0 0 1 10 0c0 3.9-5 8.5-5 8.5ZM9 9.5a1.5 1.5 0 1 0 0-3 1.5 1.5 0 0 0 0 3',
 server:'M3 3h12v5H3ZM3 10h12v5H3ZM6 5.5h.01M6 12.5h.01',
 info:'M9 16.5a7.5 7.5 0 1 0 0-15 7.5 7.5 0 0 0 0 15ZM9 8.2v4.8M9 5.6h.01',
};

// Decorative only: picks a device glyph from its display name, falling back to the generic device.
export function deviceIcon(name=''){
 if(/phone|pixel|android|ipad/i.test(name))return icons.phone;
 if(/cctv|camera/i.test(name))return icons.camera;
 if(/forklift/i.test(name))return icons.forklift;
 return icons.device;
}

export function icon(path,size=16){
 const svg=document.createElementNS('http://www.w3.org/2000/svg','svg');
 svg.setAttribute('aria-hidden','true');svg.setAttribute('width',size);svg.setAttribute('height',size);svg.setAttribute('viewBox','0 0 18 18');
 svg.setAttribute('fill','none');svg.setAttribute('stroke','currentColor');svg.setAttribute('stroke-width','1.5');svg.setAttribute('stroke-linecap','round');svg.setAttribute('stroke-linejoin','round');
 const path_=document.createElementNS('http://www.w3.org/2000/svg','path');path_.setAttribute('d',path);svg.append(path_);
 return svg;
}

const element=(tag,className,text)=>{const el=document.createElement(tag);if(className)el.className=className;if(text!==undefined)el.textContent=text;return el;};

export function sectionHeader(title,count){
 const header=element('div','section-header');const heading=element('h3','section-header-title',title);header.append(heading);
 if(count!==undefined)header.append(element('span','count-pill',String(count)));
 return header;
}

// A button that shows or hides `region`. The caller owns the open state.
export function disclosure(label,{count,expanded,controls,onToggle}){
 const button=element('button','disclosure');button.type='button';button.setAttribute('aria-expanded',String(expanded));if(controls)button.setAttribute('aria-controls',controls);
 button.append(icon(icons.chevron),element('span','disclosure-label',label));
 if(count!==undefined)button.append(element('span','count-pill',String(count)));
 button.onclick=()=>onToggle(!expanded);
 return button;
}

// Status is a shape plus words, never colour alone.
const statusLabels={live:'Live',stale:'Stale',connecting:'Connecting…',offline:'Offline'};
export function statusKind(raw){
 const value=String(raw||'').toLowerCase();
 if(['connected','ready','live','running'].includes(value))return 'live';
 if(value.startsWith('stale'))return 'stale';
 if(['connecting','pending','pairing'].includes(value))return 'connecting';
 if(['disconnected','offline','lost','unavailable'].includes(value))return 'offline';
 return null;
}
export function statusBadge(kind,label){
 const badge=element('span','status');badge.dataset.status=kind;
 badge.append(element('span','status-mark'),element('span','status-label',label||statusLabels[kind]||kind));
 return badge;
}

// One selectable row: optional leading tile, a title, optional meta line and trailing content.
export function listRow({title,meta,tile,accent,trailing,mono=false,label,onSelect}){
 const button=element('button','list-row');button.type='button';button.onclick=onSelect;
 if(label)button.setAttribute('aria-label',label);
 if(tile){const box=element('span','list-row-tile');if(accent)box.dataset.accent=accent;box.append(typeof tile==='string'?icon(tile,16):tile);button.append(box);}
 const text=element('span','list-row-text');
 text.append(element('span','list-row-title'+(mono?' mono-id':''),title));
 // meta may be text or nodes (for example a status badge followed by words).
 if(meta){const line=element('span','list-row-meta');line.append(...[meta].flat());text.append(line);}
 button.append(text);
 if(trailing){const end=element('span','list-row-trailing');end.append(trailing);button.append(end);}
 return button;
}

export function searchField({id,label,placeholder,onInput}){
 const field=element('div','search-field');
 const name=element('label','visually-hidden',label);name.htmlFor=id;
 const input=element('input');input.id=id;input.type='search';input.autocomplete='off';input.placeholder=placeholder;
 const clear=element('button','search-clear');clear.type='button';clear.setAttribute('aria-label','Clear search');clear.hidden=true;clear.append(icon(icons.close,14));
 const sync=()=>{clear.hidden=!input.value;onInput(input.value);};
 input.addEventListener('input',sync);
 clear.onclick=()=>{input.value='';sync();input.focus();};
 field.append(icon(icons.search),name,input,clear);
 return field;
}

export function copyButton(value,label){
 const button=element('button','copy-button');button.type='button';button.setAttribute('aria-label',label);button.append(icon(icons.copy,14));
 button.onclick=async()=>{try{await navigator.clipboard.writeText(value);button.replaceChildren(icon(icons.check,14));button.setAttribute('aria-label','Copied');setTimeout(()=>{button.replaceChildren(icon(icons.copy,14));button.setAttribute('aria-label',label);},1500);}catch{}};
 return button;
}

export function emptyState(text,action){
 const box=element('div','empty-state');box.append(element('p','',text));
 if(action){const button=element('button','text-action',action.label);button.type='button';button.onclick=action.onClick;box.append(button);}
 return box;
}

export function actionButton(label,{variant='secondary',iconPath,onClick,type='button',id}={}){
 const button=element('button','btn btn-'+variant);button.type=type;if(id)button.id=id;
 if(iconPath)button.append(icon(iconPath));
 button.append(element('span','',label));
 if(onClick)button.onclick=onClick;
 return button;
}

let fieldCount=0;
// Label above the control, optional live character counter.
export function formField({label,id=`field-${++fieldCount}`,multiline=false,max,value='',required=false,placeholder=''}){
 const field=element('div','form-field');
 const name=element('label','form-label',label);name.htmlFor=id;
 const input=element(multiline?'textarea':'input','form-control');input.id=id;input.value=value||'';input.required=required;input.placeholder=placeholder;input.autocomplete='off';
 if(max)input.maxLength=max;
 field.append(name,input);
 if(max){const counter=element('span','form-counter');counter.id=id+'-count';const from=Math.min(max-20,Math.round(max*.9));
  const sync=()=>{const near=input.value.length>=from;counter.hidden=!near;counter.textContent=`${input.value.length.toLocaleString()}/${max.toLocaleString()}`;if(near)input.setAttribute('aria-describedby',counter.id);else input.removeAttribute('aria-describedby');};input.addEventListener('input',sync);sync();field.append(counter);}
 if(multiline){const grow=()=>{input.style.height='auto';input.style.height=Math.min(240,Math.max(96,input.scrollHeight+2))+'px';};input.addEventListener('input',grow);requestAnimationFrame(grow);}
 return {field,input};
}

// A polite status message that fades after a few seconds.
export function toast(text){
 let region=document.getElementById('toast');
 if(!region){region=element('div','toast');region.id='toast';region.setAttribute('role','status');document.querySelector('.viewport')?.append(region)||document.body.append(region);}
 region.textContent=text;region.hidden=false;clearTimeout(region.timer);region.timer=setTimeout(()=>{region.hidden=true;},3500);
}
