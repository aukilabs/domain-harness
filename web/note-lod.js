// Zoom tiers and note clusters for the scene labels: pure logic, no DOM or three.js.
// The tier comes from one measure for the whole scene, metres per CSS pixel at the nearest label in view (else the
// orbit target), so every label changes form together. Tier 0 shows text, 1 pins and tile-only volume chips, 2 note clusters. Each step has ±5%
// hysteresis, wider than one 5% wheel notch, so a single notch back and forth never flaps.
export const TIERS=[{up:.026,down:.0235},{up:.048,down:.0435}];
export function zoomTier(mpp,previous=0){
 let tier=previous;
 while(tier<TIERS.length&&mpp>=TIERS[tier].up)tier++;
 while(tier>0&&mpp<TIERS[tier-1].down)tier--;
 return tier;
}
// Greedy clusters on the floor plane, seeded in id order: membership depends only on the notes, never on the
// camera, so it changes at tier changes and nowhere else. points: [{id, x, z}] → arrays of members.
export function clusterNotes(points,radius=2){
 const sorted=[...points].sort((a,b)=>a.id<b.id?-1:a.id>b.id?1:0),taken=new Set(),clusters=[];
 for(const seed of sorted){
  if(taken.has(seed.id))continue;
  const members=sorted.filter(p=>!taken.has(p.id)&&Math.hypot(p.x-seed.x,p.z-seed.z)<=radius);
  for(const member of members)taken.add(member.id);
  clusters.push(members);
 }
 return clusters;
}
