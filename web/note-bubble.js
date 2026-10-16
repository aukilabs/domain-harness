import * as THREE from 'three';

// iPhone Theme: fixed light paper/plum ink, rounded type and speech-bubble tail.
// Unlike the phone's fixed AR frame, Home sizes each bubble to its content.
export function bubbleLayout(text,measure,maxWidth=720){
 const lines=[];let line='';
 for(const word of String(text||'Note name').split(/\s+/)){const candidate=line?line+' '+word:word;if(line&&measure(candidate)>maxWidth){lines.push(line);line=word;}else line=candidate;}
 if(line)lines.push(line);
 const width=Math.ceil(Math.max(24,...lines.map(measure))+40);
 return {lines,width,height:lines.length*38+30+10};
}
export function bubbleColors(peer=false){return peer?{background:'#472652',foreground:'#fff7fc'}:{background:'#fff7fc',foreground:'#472652'};}
// A title stays readable through the scene: the sprite itself is a faint copy drawn
// without depth testing, and a full-strength twin drawn after it is hidden by whatever
// stands in front. Unobstructed titles look solid; titles behind voxels show as a ghost.
// The twin inherits the sprite's scale, so screen-size fixes must skip it (userData.ghostTwin).
export const GHOST_OPACITY=.3;
export function ghostWhenHidden(sprite){
 const material=sprite.material,solid=material.clone();
 solid.depthTest=true;solid.depthWrite=false;
 material.depthTest=false;material.depthWrite=false;material.transparent=true;material.opacity=GHOST_OPACITY;
 material.addEventListener('dispose',()=>solid.dispose());
 const twin=new THREE.Sprite(solid);twin.center.copy(sprite.center);twin.renderOrder=sprite.renderOrder+1;twin.raycast=()=>{};twin.userData.ghostTwin=true;
 sprite.add(twin);return sprite;
}
export function createNoteBubble(text,{peer=false}={}){
 const canvas=document.createElement('canvas'),ctx=canvas.getContext('2d');
 const font='600 32px ui-rounded, "SF Pro Rounded", system-ui, sans-serif';ctx.font=font;
 const layout=bubbleLayout(text,t=>ctx.measureText(t).width);
 canvas.width=layout.width;canvas.height=layout.height;ctx.font=font;
 const bodyHeight=layout.height-10;
 const colors=bubbleColors(peer);
 ctx.fillStyle=colors.background;ctx.beginPath();ctx.roundRect(1,1,layout.width-2,bodyHeight-2,Math.min(22,bodyHeight/2));ctx.fill();
 ctx.beginPath();ctx.moveTo(layout.width/2-9,bodyHeight-3);ctx.lineTo(layout.width/2,layout.height-1);ctx.lineTo(layout.width/2+9,bodyHeight-3);ctx.closePath();ctx.fill();
 ctx.fillStyle=colors.foreground;ctx.textAlign='center';ctx.textBaseline='middle';
 layout.lines.forEach((line,i)=>ctx.fillText(line,layout.width/2,15+19+i*38));
 const texture=new THREE.CanvasTexture(canvas);texture.colorSpace=THREE.SRGBColorSpace;
 const material=new THREE.SpriteMaterial({map:texture,transparent:true,depthTest:false,depthWrite:false,sizeAttenuation:false,toneMapped:false});
 const sprite=new THREE.Sprite(material);const height=.026*(layout.height/78);
 sprite.scale.set(height*layout.width/layout.height,height,1);sprite.center.set(.5,0);sprite.renderOrder=80;
 sprite.userData.bubble={text:String(text),width:layout.width,height:layout.height};return ghostWhenHidden(sprite);
}
