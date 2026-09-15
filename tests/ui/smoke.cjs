// UI contract test: the fixture is deliberately test-only, never bundled in the app.
const { chromium } = require('playwright');
const assert = require('node:assert/strict');
const fs = require('node:fs');
const path = require('node:path');
const http = require('node:http');
const root = path.resolve(__dirname, '../../ui');
const server = http.createServer((req,res) => {
  const name = req.url === '/' ? '/index.html' : req.url;
  if (!/^\/[a-zA-Z0-9._-]+$/.test(name)) { res.writeHead(404).end(); return; }
  const file = path.join(root, name);
  if (!fs.existsSync(file)) { res.writeHead(404).end(); return; }
  res.setHeader('Content-Type', name.endsWith('.js') ? 'text/javascript' : name.endsWith('.css') ? 'text/css' : name.endsWith('.svg') ? 'image/svg+xml' : 'text/html');
  res.end(fs.readFileSync(file));
});
(async () => {
  await new Promise(resolve => server.listen(0,'127.0.0.1',resolve));
  const browser = await chromium.launch({headless:true});
  fs.mkdirSync('screenshots',{recursive:true});
  try {
    for (const viewport of [{width:1140,height:820},{width:390,height:844}]) {
      const context = await browser.newContext({viewport});
      const page = await context.newPage();
      const errors=[];page.on('pageerror',e=>errors.push(e.message));
      await page.addInitScript(() => {
        const id='a'.repeat(64), peer='b'.repeat(64);
        const state={version:'0.1.0',device:{id,name:'My PC',port:53319,addresses:['192.168.1.10:53319']},peers:[{id:peer,name:'Pixel 7a',address:'192.168.1.20:53319',connected:true,trusted:false,ready:false,code:'483291',local_confirmed:false}],trusted:[],messages:[],transfers:[],warnings:[],receive_dir:'/test/received'};
        let clipboard='clipboard test';window.testCalls=[];
        window.LocalNative={invoke(callId,body){
          const request=JSON.parse(body);window.testCalls.push(request);
          let data={};
          switch(request.op){
            case 'snapshot':data=state;break;
            case 'confirm':state.peers[0].ready=true;state.peers[0].trusted=true;state.peers[0].code=null;state.trusted=[{id:peer,name:'Pixel 7a'}];break;
            case 'send_text':state.messages.push({id:String(state.messages.length),peer_id:peer,direction:'out',channel:request.channel,text:request.text,timestamp:Date.now()});break;
            case 'read_clipboard':data=clipboard;break;
            case 'write_clipboard':clipboard=request.text;break;
            case 'set_name':state.device.name=request.name;break;
            case 'pick_file':state.transfers.unshift({id:'file1',peer_id:peer,name:'photo.png',size:12345,bytes:12345,direction:'in',status:'completed',error:'',path:'/test/received/photo.png',timestamp:Date.now()});data={id:'file1'};break;
            case 'export_file':data=true;break;
            case 'clear_history':state.messages=[];break;
            case 'forget':state.trusted=[];state.peers=[];break;
          }
          setTimeout(()=>window.localResolve(callId,JSON.stringify({ok:true,data})),5);
        }};
      });
      await page.goto(`http://127.0.0.1:${server.address().port}`);
      await page.getByText('483291',{exact:true}).waitFor();
      await page.screenshot({path:`screenshots/pairing-${viewport.width}.png`,fullPage:true});
      assert.equal(await page.evaluate(()=>document.documentElement.scrollWidth>innerWidth),false,'Horizontal overflow');
      await page.getByRole('button',{name:'一致しています',exact:true}).click();
      await page.getByText('● ペアリング済み · 暗号化接続').waitFor();
      await page.getByRole('button',{name:'メッセージ',exact:true}).last().click();
      const attack='<img src=x onerror="alert(1)"> 日本語';
      await page.getByPlaceholder('メッセージを入力…').fill(attack);
      await page.getByRole('button',{name:'送信 ↗',exact:true}).click();
      await page.getByText(attack,{exact:true}).waitFor();
      assert.equal(await page.locator('.message img').count(),0,'Message HTML must stay inert');
      await page.getByRole('button',{name:'クリップボードを貼り付け',exact:true}).click();
      await page.waitForFunction(()=>document.getElementById('message').value==='clipboard test');
      assert.equal(await page.evaluate(()=>window.testCalls.filter(c=>c.op==='send_text').length),1,'Pasting must not auto-send');
      await page.getByRole('button',{name:'送信 ↗',exact:true}).click();
      await page.getByText('clipboard test',{exact:true}).waitFor();
      await page.screenshot({path:`screenshots/messages-${viewport.width}.png`,fullPage:true});
      await page.locator('[data-tab="nearby"]').click();
      await page.getByRole('button',{name:'ファイルを送る ↗',exact:true}).click();
      await page.getByRole('heading',{name:'photo.png',exact:true}).waitFor();
      await page.getByRole('button',{name:'端末に保存…',exact:true}).click();
      await page.getByText('ファイルを保存しました',{exact:true}).waitFor();
      await page.screenshot({path:`screenshots/transfers-${viewport.width}.png`,fullPage:true});
      await page.locator('[data-tab="settings"]').click();
      await page.getByLabel('端末の名前',{exact:true}).fill('Test Phone');
      await page.getByRole('button',{name:'保存',exact:true}).click();
      await page.getByText('端末名を保存しました',{exact:true}).waitFor();
      assert.equal(await page.evaluate(()=>document.documentElement.scrollWidth>innerWidth),false);
      assert.deepEqual(errors,[]);
      await context.close();
      console.log(`PASS ${viewport.width}px: pairing, escaped text, explicit clipboard send, file/export, settings, no overflow`);
    }
  } finally {await browser.close();server.close();}
})().catch(e=>{console.error(e);server.close();process.exitCode=1;});

