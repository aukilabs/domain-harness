import {test,expect} from '@playwright/test';

test('AR and picking reach sensor validation; legacy following stays blocked',async({request})=>{
 for(const option of ['ar','pick']){
  const response=await request.get(`/api/sensors?sensor=missing-camera&${option}=true`);
  expect(response.ok()).toBe(false);
  const body=await response.json();
  expect(body.error).not.toContain('legacy pose updates');
  expect(body.error).toMatch(/sensor|camera/i);
 }
 const response=await request.get('/api/sensors?sensor=missing-camera&follow=true');
 expect(response.ok()).toBe(false);
 expect((await response.json()).error).toContain('Home camera following disabled');
});
