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
      const a='b'.repeat(64),b='c'.repeat(64);
      window.clipboardValue='before-enable';window.clipboardRevision=0;window.clipboardCalls=[];
      window.clipboardState={version:'0.4.1',device:{id:'a'.repeat(64),name:'Desktop',port:53319,addresses:[]},
        peers:[a,b].map((id,index)=>({id,name:index?'Tablet':'Phone',address:'10.0.0.'+(index+2)+':53319',ready:true,capabilities:['text','clipboard']})),
        trusted:[],messages:[{id:'old',peer_id:a,direction:'in',channel:'mesh.clipboard',text:'old-history',timestamp:1}],transfers:[],warnings:[],receive_dir:'/tmp',screen_sessions:[]};
      window.LocalNative={invoke(callId,body){
        const request=JSON.parse(body);window.clipboardCalls.push(request);let data={};
        if(request.op==='snapshot') data=window.clipboardState;
        else if(request.op==='received_files') data={files:[],total:0,offset:0,limit:50};
        else if(request.op==='clipboard_revision') data=window.clipboardRevision;
        else if(request.op==='read_clipboard') data=window.clipboardValue;
        else if(request.op==='write_clipboard') {window.clipboardValue=request.text;window.clipboardRevision++;}
        else if(request.op==='send_text') data={id:String(Date.now())};
        else if(request.op==='audio_signals') data={events:[],latest:0};
        setTimeout(()=>window.localResolve(callId,JSON.stringify({ok:true,data})),1);
      }};
    });
    await page.goto('http://127.0.0.1:' + server.address().port);
    await page.locator('[data-tab="clipboard"]').click();
    await page.locator('#clipboard-enabled').check();
    await page.getByText('2台と同期中',{exact:false}).waitFor();
    await page.waitForTimeout(1400);
    assert.equal(await page.evaluate(()=>window.clipboardValue),'before-enable','History must not overwrite clipboard when sync starts');
    await page.evaluate(()=>{window.clipboardValue='copied locally';window.clipboardRevision++;});
    await page.waitForFunction(()=>window.clipboardCalls.filter(call=>call.op==='send_text'&&call.text==='copied locally').length===2);
    await page.evaluate(() => window.clipboardState.messages.unshift({id:'new',peer_id:'b'.repeat(64),direction:'in',channel:'mesh.clipboard',text:'from phone',timestamp:Date.now()}));
    await page.waitForFunction(()=>window.clipboardValue==='from phone');
    await page.waitForTimeout(1400);
    assert.equal(await page.evaluate(()=>window.clipboardCalls.filter(call=>call.op==='send_text'&&call.text==='from phone').length),0,'Received clipboard must not echo back');
    await page.locator('#clipboard-enabled').uncheck();
    await page.evaluate(()=>{window.clipboardValue='disabled change';window.clipboardRevision++;});
    await page.waitForTimeout(1400);
    assert.equal(await page.evaluate(()=>window.clipboardCalls.filter(call=>call.op==='send_text'&&call.text==='disabled change').length),0);
    assert.deepEqual(errors,[]);
    console.log('PASS clipboard: explicit enable, selected peers, history isolation, receive/write and loop prevention');
  } finally { await browser.close();server.close(); }
})().catch(error=>{console.error(error);server.close();process.exitCode=1;});
