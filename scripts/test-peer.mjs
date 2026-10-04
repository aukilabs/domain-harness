// Two explicitly simulated sources over the localhost fixture API, not Auki P2P.
const base=process.env.SPATIAL_URL||'http://127.0.0.1:4319';
async function request(path,body){const r=await fetch(base+path,body?{method:'POST',headers:{'Content-Type':'application/json'},body:JSON.stringify(body)}:undefined);const data=await r.json();if(!r.ok)throw new Error(JSON.stringify(data));return data;}
const f=await request('/api/fixtures');
for(const c of f.contributions)console.log(c.peer,await request('/api/contributions',c));
console.log('book detection',await request('/api/detections',f.detection));
const before=await request('/api/scene');
console.log('replay',await request('/api/contributions',f.contributions[0]));
const after=await request('/api/scene');
if(JSON.stringify(before)!==JSON.stringify(after))throw new Error('Replay changed the scene');
console.log({occupiedVoxels:after.voxels.length,observations:after.integrated_observations,objects:after.objects.length,frame:after.frame});
