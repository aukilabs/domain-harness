// Read-only browser tests against the existing Home; never starts another server.
import { defineConfig } from '@playwright/test';
export default defineConfig({testDir:'./tests/browser',workers:1,timeout:60000,
 use:{baseURL:process.env.HOME_TEST_URL||'http://127.0.0.1:4329',viewport:{width:1440,height:1060},channel:'chrome',
 launchOptions:{args:['--enable-webgl','--use-angle=swiftshader','--enable-unsafe-swiftshader']}}});
