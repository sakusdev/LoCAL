const { chromium } = require('playwright');
const assert = require('node:assert/strict');
const fs = require('node:fs');
const path = require('node:path');
const http = require('node:http');
const root = path.resolve(__dirname, '../../ui');
const server = http.createServer((req,res) => {
  const name = req.url === '/' ? '/index.html' : req.url;
  if (!/^\/[a-zA-Z0-9._-]+$/.test(name)) { res.writeHead(404).end(); return; }
  const file = path.join(root,name);
  if (!fs.existsSync(file)) { res.writeHead(404).end(); return; }
  res.setHeader('Content-Type',name.endsWith('.js') ? 'text/javascript' : name.endsWith('.css') ? 'text/css' : 'text/html');
  res.end(fs.readFileSync(file));
});
(async () => {
  await new Promise(resolve => server.listen(0,'127.0.0.1',resolve));
  const browser = await chromium.launch({headless:true});
  try {
    const page = await browser.newPage();
    const errors=[];page.on('pageerror',error=>errors.push(error.message));
    await page.addInitScript(() => {
      Object.defineProperty(navigator,'userAgent',{get:()=> 'LoCAL Windows WebView2'});
      const peer='b'.repeat(64), local='a'.repeat(64), incoming='52c26a8a-25d8-4751-802e-57e8bfe92731';
      const offers={};
      offers[incoming]={id:incoming,peer_id:peer,direction:'in',status:'offered',error:'',
        offer:{id:incoming,resource:'screen/display/display-1',codec:'h264',width:640,height:360,fps:15}};
      const state={version:'0.3.0',device:{id:local,name:'My PC',port:53319,addresses:[],
        screen:{decode:{codecs:['h264'],max_width:1920,max_height:1080,max_fps:30}}},
        peers:[{id:peer,name:'<img src=x onerror=alert(1)> peer',address:'10.1.1.2:53319',
          ready:true,screen:{decode:{codecs:['h264'],max_width:1920,max_height:1080,max_fps:30}}}],
        trusted:[],messages:[],transfers:[],warnings:[],receive_dir:'/tmp',screen_sessions:Object.values(offers)};
      const listeners=new Map();window.screenCalls=[];window.decodedChunks=[];
      window.testScreenEvent=(name,payload)=>listeners.get(name)?.({payload});
      class MockVideoDecoder {
        static async isConfigSupported(config) { return {supported:config.avc?.format==='annexb'}; }
        constructor(handlers) { this.handlers=handlers;this.state='unconfigured'; }
        configure(config) { this.config=config;this.state='configured'; }
        decode(chunk) { window.decodedChunks.push(chunk); }
        close() { this.state='closed'; }
      }
      window.VideoDecoder=MockVideoDecoder;
      window.EncodedVideoChunk=class {constructor(data){Object.assign(this,data);}};
      window.__TAURI__={event:{async listen(name,listener){listeners.set(name,listener);return ()=>listeners.delete(name);}},
        core:{async invoke(name,{request}){
          if(name!=='local_command') throw Error('Unexpected native command');
          window.screenCalls.push(request);
          switch(request.op){
            case 'snapshot':return state;
            case 'enable_screen_view':return {enabled:true};
            case 'screen_sources':return {sources:[{id:'display-1',name:'<b>Primary</b>',width:640,height:360}]};
            case 'accept_screen':offers[request.id].status=request.accept?'active':'rejected';return {};
            case 'watch_screen':return {watching:true};
            case 'stop_screen':offers[request.id].status='stopped';return {};
            case 'share_screen':return {id:'outgoing',accepted:true};
            case 'received_files':return {files:[],total:0,offset:0,limit:50};
            default:return {};
          }
        }}};
    });
    await page.goto('http://127.0.0.1:' + server.address().port);
    await page.locator('#screen-nav').waitFor({state:'visible'});
    await page.getByRole('button',{name:'画面を見る',exact:true}).click();
    await page.locator('[data-tab="screen"]').click();
    await page.getByRole('button',{name:'表示する',exact:true}).click();
    await page.locator('#screen-viewer').waitFor({state:'visible'});
    await page.evaluate(() => window.testScreenEvent('local-screen-frame',{
      id:'52c26a8a-25d8-4751-802e-57e8bfe92731',keyframe:true,timestamp_us:42,
      data:btoa('encoded-frame'),
    }));
    await page.waitForFunction(() => window.decodedChunks.length===1);
    assert.equal(await page.evaluate(() => window.decodedChunks[0].timestamp),42);
    assert.equal(await page.locator('#screen-sessions img').count(),0,'Remote peer name must be escaped');
    assert.equal(await page.locator('#screen-source b').count(),0,'Capture source must be escaped');
    await page.getByRole('button',{name:'表示を終了',exact:true}).click();
    await page.locator('#screen-viewer').waitFor({state:'hidden'});
    await page.getByRole('button',{name:'共有を申し込む',exact:true}).click();
    await page.waitForFunction(() => window.screenCalls.some(call => call.op==='share_screen'));
    assert.equal(await page.evaluate(() => window.screenCalls.find(call => call.op==='share_screen').control),undefined);
    assert.deepEqual(errors,[]);
    await page.close();
    console.log('PASS Windows screen: capability detection, consent, decoded frame dispatch, stop and safe source labels');

    const androidPage = await browser.newPage();
    const androidErrors=[];androidPage.on('pageerror',error=>androidErrors.push(error.message));
    await androidPage.addInitScript(() => {
      const peer='b'.repeat(64),local='a'.repeat(64),id='52c26a8a-25d8-4751-802e-57e8bfe92732';
      const state={version:'0.3.0',device:{id:local,name:'Phone',port:53319,addresses:[]},peers:[{id:peer,name:'Windows PC',address:'10.1.1.2:53319',ready:true,screen:{encode:{codecs:['h264'],max_width:1920,max_height:1080,max_fps:30}}}],trusted:[],messages:[],transfers:[],warnings:[],receive_dir:'/tmp',screen_sessions:[{id,peer_id:peer,direction:'in',status:'active',error:'',offer:{id,resource:'screen/display/display-1',codec:'h264',width:640,height:360,fps:15}}]};
      window.androidDecoded=[];let polled=false;
      class MockVideoDecoder {
        static async isConfigSupported(){return {supported:true};}
        constructor(){this.state='unconfigured';this.decodeQueueSize=0;}
        configure(){this.state='configured';}
        decode(chunk){window.androidDecoded.push(chunk);}
        close(){this.state='closed';}
      }
      window.VideoDecoder=MockVideoDecoder;
      window.EncodedVideoChunk=class {constructor(data){Object.assign(this,data);}};
      window.LocalNative={invoke(callId,body){
        const request=JSON.parse(body);let data={};
        if(request.op==='snapshot') data=state;
        else if(request.op==='received_files') data={files:[],total:0,offset:0,limit:50};
        else if(request.op==='enable_screen_view') data={enabled:true};
        else if(request.op==='watch_screen') data={watching:true};
        else if(request.op==='poll_screen_frame') {data=polled?{ended:true,frame:null}:{frame:{sequence:1,timestamp_us:77,keyframe:true,data:btoa('android-frame')}};polled=true;}
        else if(request.op==='audio_signals') data={events:[],latest:0};
        setTimeout(()=>window.localResolve(callId,JSON.stringify({ok:true,data})),1);
      }};
    });
    await androidPage.goto('http://127.0.0.1:' + server.address().port);
    await androidPage.locator('#screen-nav').waitFor({state:'visible'});
    await androidPage.locator('[data-tab="screen"]').click();
    await androidPage.getByRole('button',{name:'表示する',exact:true}).click();
    await androidPage.waitForFunction(()=>window.androidDecoded.length===1);
    assert.equal(await androidPage.evaluate(()=>window.androidDecoded[0].timestamp),77);
    assert.equal(await androidPage.locator('#screen-share-panel').isHidden(),true);
    assert.deepEqual(androidErrors,[]);
    await androidPage.close();
    console.log('PASS Android screen: WebCodecs capability, native frame polling, H.264 decode and viewer-only UI');
  } finally {await browser.close();server.close();}
})().catch(error=>{console.error(error);server.close();process.exitCode=1;});
