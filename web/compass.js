// North for the scene on screen, from the place on the Earth a phone gave Home.
// All directions are across the floor, as [x, z] in the scene frame (Y up).

/** Degrees to turn a marker clockwise from the top of the screen so it points at true north. */
export function northTurn(north, screenUp, screenRight) {
 const dot=(a,b)=>a[0]*b[0]+a[1]*b[1];
 return Math.atan2(dot(north,screenRight),dot(north,screenUp))*180/Math.PI;
}

/** A small dial in the viewport: ticks round the rim and a red triangle at north. Hidden until Home knows north. */
export function createCompass(host) {
 const dial=document.createElement('div');dial.id='compass';dial.hidden=true;
 const ring=document.createElement('div');ring.className='compass-ring';
 for(let tick=1;tick<24;tick++){const mark=document.createElement('i');if(tick%6===0)mark.className='cardinal';mark.style.transform=`rotate(${tick*15}deg) translateY(-22px)`;ring.append(mark);}
 const needle=document.createElement('b');needle.style.transform='translateY(-22px)';ring.append(needle);
 const label=document.createElement('span');label.textContent='N';
 dial.append(ring,label);host.append(dial);
 return {
  /** `geo` is the scene's `spatial.phone-geo/v1` reference, or nothing. */
  update(geo, screenUp, screenRight) {
   const north=geo?.north;
   if(!Array.isArray(north)||north.length!==2||!north.every(Number.isFinite)){dial.hidden=true;return;}
   dial.hidden=false;
   const turn=northTurn(north,screenUp,screenRight);
   ring.style.transform=`rotate(${turn}deg)`;
   dial.dataset.turn=turn.toFixed(1);
   dial.title=`True north · scene origin ${geo.latitude.toFixed(6)}, ${geo.longitude.toFixed(6)} ±${Math.round(geo.accuracy_m)} m · compass ±${Math.round(geo.heading_error_deg)}°`;
  },
 };
}
