// Fully intercepted fixtures: never start a second Home or contact live peers.
export default {
 testDir:'./tests/browser',testMatch:'slash-commands.spec.js',workers:1,timeout:30000,
 use:{viewport:{width:1440,height:1060},channel:'chrome',launchOptions:{args:['--enable-webgl','--use-angle=swiftshader','--enable-unsafe-swiftshader']}}
};
