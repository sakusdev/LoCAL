const { chromium } = require('playwright');
const assert = require('node:assert/strict');
const fs = require('node:fs');
const path = require('node:path');
const http = require('node:http');
const root = path.resolve(__dirname, '../../ui');
const server = http.createServer((req,res) => {
  const name = req.url === '/' ? '/index.html' : req.url;
  if (!/^\/[a-zA-Z0-9._-]+$/.test(name)) return res.writeHead(404).end();
  const file = path.join(root,name);
  if (!fs.existsSync(file)) return res.writeHead(404).end();
  res.setHeader('Content-Type',name.endsWith('.js') ? 'text/javascript' : name.endsWith('.css') ? 'text/css' : name.endsWith('.svg') ? 'image/svg+xml' : 'text/html');
  res.end(fs.readFileSync(file));
});

(async () => {
  await new Promise(resolve => server.listen(0,'127.0.0.1',resolve));
  const browser = await chromium.launch({headless:true});
  try {
    const page = await browser.newPage();
    const errors=[]; page.on('pageerror',error=>errors.push(error.message));
    await page.addInitScript(() => {
      const local='a'.repeat(64),peer='b'.repeat(64);
      const state={version:'0.3.0',device:{id:local,name:'PC',port:53319,addresses:['192.168.1.10:53319']},peers:[{id:peer,name:'Phone',address:'192.168.1.20:53319',connected:true,trusted:true,ready:true,capabilities:['text','file','clipboard','audio']}],trusted:[],messages:[],transfers:[],screen_sessions:[],warnings:[],receive_dir:'/tmp'};
      let sequence=0; const incoming=[]; window.sentSignals=[]; window.testTracks=[]; window.testPcs=[];
      const description=(type,sdp)=>({type,sdp,toJSON(){return {type,sdp};}});
      class FakePeerConnection {
        constructor(){this.connectionState='new';this.remoteDescription=null;this.localDescription=null;this.candidates=[];window.testPcs.push(this);}
        addTrack(){}
        async createOffer(){return description('offer','outgoing-offer');}
        async createAnswer(){return description('answer','incoming-answer');}
        async setLocalDescription(value){this.localDescription=description(value.type,value.sdp);}
        async setRemoteDescription(value){this.remoteDescription=value;}
        async addIceCandidate(value){this.candidates.push(value);}
        close(){this.connectionState='closed';}
      }
      window.RTCPeerConnection=FakePeerConnection;
      window.MediaStream=class { constructor(tracks){this.tracks=tracks;} };
      Object.defineProperty(navigator,'mediaDevices',{value:{getUserMedia:async()=>{
        const track={enabled:true,stopped:false,stop(){this.stopped=true;}};window.testTracks.push(track);
        return {getTracks:()=>[track],getAudioTracks:()=>[track]};
      }}});
      window.queueSignal=(kind,data,callId='11111111-1111-4111-8111-111111111111')=>incoming.push({sequence:++sequence,call_id:callId,peer_id:peer,kind,data:data ? JSON.stringify(data) : '',timestamp:Date.now()});
      window.LocalNative={invoke(id,body){
        const request=JSON.parse(body); let data={};
        if(request.op==='snapshot') data=state;
        else if(request.op==='received_files') data={files:[],total:0,offset:0,limit:50};
        else if(request.op==='audio_signals') { const events=incoming.filter(event=>event.sequence>request.after); data={events,latest:events.at(-1)?.sequence || request.after}; }
        else if(request.op==='send_audio_signal') window.sentSignals.push(request);
        setTimeout(()=>window.localResolve(id,JSON.stringify({ok:true,data})),1);
      }};
    });
    await page.goto(`http://127.0.0.1:${server.address().port}`);
    await page.locator('#calls-nav').waitFor({state:'visible'});
    await page.getByRole('button',{name:'通話',exact:true}).last().click();
    await page.waitForFunction(()=>window.sentSignals.some(signal=>signal.kind==='offer'));
    assert.equal(await page.locator('#call-state-text').textContent(),'呼び出しています…');
    const callId=await page.evaluate(()=>window.sentSignals.find(signal=>signal.kind==='offer').call_id);
    await page.evaluate(({callId})=>window.queueSignal('answer',{type:'answer',sdp:'remote-answer'},callId),{callId});
    await page.waitForFunction(()=>window.testPcs[0].remoteDescription?.type==='answer');
    await page.evaluate(()=>{const pc=window.testPcs[0];pc.connectionState='connected';pc.onconnectionstatechange();});
    await page.getByText('通話中',{exact:true}).waitFor();
    await page.getByRole('button',{name:'ミュート',exact:true}).click();
    assert.equal(await page.evaluate(()=>window.testTracks[0].enabled),false);
    await page.getByRole('button',{name:'通話を終了',exact:true}).click();
    await page.waitForFunction(()=>window.sentSignals.some(signal=>signal.kind==='end'));
    assert.equal(await page.evaluate(()=>window.testTracks[0].stopped),true);

    await page.evaluate(()=>window.queueSignal('offer',{type:'offer',sdp:'incoming-offer'}));
    await page.getByText('Phoneから音声通話です',{exact:true}).waitFor();
    await page.getByRole('button',{name:'応答',exact:true}).click();
    await page.waitForFunction(()=>window.sentSignals.some(signal=>signal.kind==='answer'));
    assert.equal(await page.evaluate(()=>window.testPcs[1].remoteDescription.sdp),'incoming-offer');
    await page.getByRole('button',{name:'通話を終了',exact:true}).click();
    assert.deepEqual(errors,[]);
    console.log('PASS calls: outgoing answer, mute/end, incoming accept, authenticated signaling UI');
  } finally { await browser.close(); server.close(); }
})().catch(error=>{console.error(error);server.close();process.exitCode=1;});
