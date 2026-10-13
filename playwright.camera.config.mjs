import {defineConfig} from '@playwright/test';
// Read-only browser checks against an existing Home. Test routes supply modified
// assets and mocked chat/sensor responses; no second Home or scene writes.
export default defineConfig({testDir:'./tests/browser',workers:1,timeout:60000,
 use:{baseURL:process.env.HOME_TEST_URL||'http://127.0.0.1:4329',viewport:{width:1440,height:1060},channel:'chrome',
 launchOptions:{args:['--enable-webgl','--use-angle=swiftshader','--enable-unsafe-swiftshader']}}});
