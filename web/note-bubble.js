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
 sprite.userData.bubble={text:String(text),width:layout.width,height:layout.height};return sprite;
}

// Far-away notes: a round pin in the bubble's colours with a note glyph, or (count > 1) a stacked pin holding
// the number of notes it stands for. Same units as the bubbles, so sizes match on screen.
const NOTE_GLYPH='M4 2h10v14H4ZM6 6h6M6 9h6M6 12h4';
export function createNotePin({peer=false,count=1}={}){
 const canvas=document.createElement('canvas'),ctx=canvas.getContext('2d'),colors=bubbleColors(peer),stacked=count>1;
 const size=stacked?72:60,tail=10,pad=4,width=size+pad*2+(stacked?8:0),height=size+tail+pad*2;canvas.width=width;canvas.height=height;
 const cx=width/2-(stacked?4:0),cy=pad+size/2;
 const disc=(x,y,r,fill,ring)=>{ctx.beginPath();ctx.arc(x,y,r,0,Math.PI*2);ctx.fillStyle=fill;ctx.fill();if(ring){ctx.lineWidth=2;ctx.strokeStyle=ring;ctx.stroke();}};
 if(stacked)disc(cx+8,cy-2,size/2-3,colors.foreground,colors.background);
 ctx.beginPath();ctx.moveTo(cx-8,cy+size/2-5);ctx.lineTo(cx,cy+size/2+tail);ctx.lineTo(cx+8,cy+size/2-5);ctx.closePath();ctx.fillStyle=colors.background;ctx.fill();
 disc(cx,cy,size/2-1,colors.background,stacked?colors.foreground:null);
 ctx.fillStyle=colors.foreground;ctx.strokeStyle=colors.foreground;
 if(stacked){ctx.font='700 30px ui-rounded, "SF Pro Rounded", system-ui, sans-serif';ctx.textAlign='center';ctx.textBaseline='middle';ctx.fillText(count>99?'99+':String(count),cx,cy+1);}
 else{ctx.save();ctx.translate(cx-15,cy-15);ctx.scale(30/18,30/18);ctx.lineWidth=1.7;ctx.lineJoin='round';ctx.lineCap='round';ctx.stroke(new Path2D(NOTE_GLYPH));ctx.restore();}
 const texture=new THREE.CanvasTexture(canvas);texture.colorSpace=THREE.SRGBColorSpace;
 const material=new THREE.SpriteMaterial({map:texture,transparent:true,depthTest:false,depthWrite:false,sizeAttenuation:false,toneMapped:false});
 const sprite=new THREE.Sprite(material),scale=.026/78;sprite.scale.set(width*scale,height*scale,1);sprite.center.set(cx/width,pad/height);sprite.renderOrder=81;
 return sprite;
}
