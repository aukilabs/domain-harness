import { defineConfig } from '@playwright/test';
import { mkdtempSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
const data=join(mkdtempSync(join(tmpdir(),'spatial-browser-')),'home.json');
export default defineConfig({
 testDir:'./tests/browser',workers:1,timeout:120000,expect:{timeout:20000},
 use:{baseURL:'http://127.0.0.1:4318',viewport:{width:1440,height:1060},channel:'chrome',launchOptions:{args:['--enable-webgl','--use-angle=swiftshader','--enable-unsafe-swiftshader']}},
 webServer:{command:'cargo run --locked --offline',url:'http://127.0.0.1:4318/api/scene',reuseExistingServer:false,env:{SPATIAL_DEMO:'1',SPATIAL_PORT:'4318',SPATIAL_DATA:data},timeout:120000}
});
