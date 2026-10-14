// Browser leg only. Peer transport and authentication remain SDK-owned.
export async function readCameraResponse(response) {
 const type=(response.headers.get('content-type')||'').split(';')[0];
 if(type!=='application/vnd.spatial-harness.camera-frame'){
  const value=await response.json();
  if(!response.ok)throw Error(value.error||'Camera unavailable');
  // Compatibility with a browser connecting to an older Home, never emitted by
  // the new Home. Even legacy peer Products are forwarded as binary by new Home.
  if(typeof value.data==='string')value.bytes=Uint8Array.from(atob(value.data),c=>c.charCodeAt(0));
  return value;
 }
 if(!response.ok)throw Error('Camera request failed');
 const buffer=await response.arrayBuffer();
 if(buffer.byteLength<4||buffer.byteLength>4+1024*1024+8*1024*1024)throw Error('Invalid camera packet length');
 const length=new DataView(buffer).getUint32(0,true);
 if(!length||length>1024*1024||length+4>=buffer.byteLength)throw Error('Invalid camera metadata length');
 const metadata=JSON.parse(new TextDecoder('utf-8',{fatal:true}).decode(new Uint8Array(buffer,4,length)));
 if(!metadata||typeof metadata!=='object'||Array.isArray(metadata)||'data' in metadata)throw Error('Invalid camera metadata');
 const bytes=new Uint8Array(buffer,4+length);
 if(bytes.length>8*1024*1024)throw Error('Camera image exceeds limits');
 return {...metadata,bytes};
}
