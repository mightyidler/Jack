// Opt-in actual Windows dev app verification over its loopback WebView2 CDP port.
// Temporarily lowers (or at zero, slightly raises) only Gaming volume, then
// restores the exact original level while it is still the value this run owns.
const assert=require('node:assert/strict');
const {mkdir,writeFile}=require('node:fs/promises');
const {resolve}=require('node:path');
const out=resolve(__dirname,'../../artifacts/audio-quality');
(async()=>{
  await mkdir(out,{recursive:true});
  const target=(await (await fetch('http://127.0.0.1:9223/json/list')).json()).find(target=>target.url==='http://127.0.0.1:1420/');
  assert.ok(target,'running native dev main WebView');
  const socket=new WebSocket(target.webSocketDebuggerUrl);
  await new Promise((resolve,reject)=>{socket.addEventListener('open',resolve,{once:true});socket.addEventListener('error',reject,{once:true});});
  const pending=new Map();let id=0;
  socket.addEventListener('message',event=>{const response=JSON.parse(event.data);const callback=pending.get(response.id);if(callback){pending.delete(response.id);response.error?callback.reject(Error(response.error.message)):callback.resolve(response.result);}});
  const call=(method,params={})=>new Promise((resolve,reject)=>{const request=++id;pending.set(request,{resolve,reject});socket.send(JSON.stringify({id:request,method,params}));});
  const evaluate=async(fn,arg)=>{
    const result=await call('Runtime.evaluate',{expression:`(${fn})(${JSON.stringify(arg)??''})`,returnByValue:true,awaitPromise:true});
    if(result.exceptionDetails)throw Error(result.exceptionDetails.exception?.description||result.exceptionDetails.text);
    return result.result.value;
  };
  const until=async(fn,arg)=>{const end=Date.now()+10000;while(Date.now()<end){if(await evaluate(fn,arg))return;await new Promise(resolve=>setTimeout(resolve,100));}throw Error('Native condition timed out');};
  const page={evaluate,waitForFunction:until,screenshot:async({path})=>{const result=await call('Page.captureScreenshot');await writeFile(path,Buffer.from(result.data,'base64'));}};
  const invoke=(command,args)=>page.evaluate(({command,args})=>window.__TAURI_INTERNALS__.invoke(command,args),{command,args});
  const read=()=>invoke('audio_action',{action:{kind:'read'}});
  const report={cases:[],restored:false};let original,owned;
  try {
    await call('Page.bringToFront');await until(()=>!!document.querySelector('#sonar-volume-game:not(:disabled)'));
    const before=await read();assert.equal(before.sonar.connection,'connected');assert.equal(before.sonar.mode,'classic');
    original=before.sonar.volumes.find(volume=>volume.channel==='game').volume;
    const muted=before.sonar.volumes.find(volume=>volume.channel==='game').muted;
    owned=original;
    await page.evaluate(()=>{window.__qualityControl=document.getElementById('sonar-volume-game');});
    // Trigger a real external change through the same native broker, then let
    // the visible UI adopt it through its normal polling path.
    const external=Math.max(0,original-.02);
    owned=await invoke('audio_volume',{target:{kind:'sonar',channel:'game'},level:external,expected:owned});
    await page.waitForFunction(level=>document.getElementById('sonar-volume-game').getAttribute('aria-valuenow')===String(Math.round(level*100)),external);
    assert.equal(await page.evaluate(()=>window.__qualityControl===document.getElementById('sonar-volume-game')),true);
    report.cases.push({name:'real external volume sync preserves DOM',passed:true});
    const point=await evaluate(()=>{const rect=document.getElementById('sonar-volume-game').getBoundingClientRect();return {x:rect.x+rect.width/2,y:rect.y+rect.height/2};});
    const wheel=async(deltaY,modifiers=0)=>call('Input.dispatchMouseEvent',{type:'mouseWheel',...point,deltaX:0,deltaY,modifiers});
    const direction=external>=.06?1:-1;
    await call('Input.dispatchMouseEvent',{type:'mouseMoved',...point});await wheel(direction*100);await new Promise(resolve=>setTimeout(resolve,900));
    let state=await read();owned=state.sonar.volumes.find(volume=>volume.channel==='game').volume;
    const normal=Math.round(Math.max(0,Math.min(1,external-direction*.05))*100)/100;
    assert.ok(Math.abs(owned-normal)<.0001,`normal wheel ${owned} vs ${normal}`);
    await wheel(direction*100,8);await new Promise(resolve=>setTimeout(resolve,900));
    state=await read();owned=state.sonar.volumes.find(volume=>volume.channel==='game').volume;
    assert.ok(Math.abs(owned-Math.round(Math.max(0,Math.min(1,normal-direction*.01))*100)/100)<.0001);
    assert.equal(state.sonar.volumes.find(volume=>volume.channel==='game').muted,muted);
    for(const volume of before.sonar.volumes.filter(volume=>volume.channel!=='game')) {
      assert.equal(state.sonar.volumes.find(current=>current.channel===volume.channel)?.volume,volume.volume);
    }
    report.cases.push({name:'native wheel 5%, Shift wheel 1%, independent channels and mute',passed:true});
    const failure=await page.evaluate(async args=>{
      try{await window.__TAURI_INTERNALS__.invoke('audio_volume',args);return null;}catch(error){return error;}
    },{target:{kind:'sonar',channel:'game'},level:owned,expected:owned>.5?0:1});
    assert.equal(failure.code,'external_change');assert.equal(typeof failure.detail,'string');
    assert.equal((await read()).sonar.volumes.find(volume=>volume.channel==='game').volume,owned);
    report.cases.push({name:'native structured conflict error rejects stale write',passed:true});
    await page.screenshot({path:resolve(out,'native-main.png')});
  } finally {
    if(original!==undefined && owned!==undefined) {
      const current=(await read()).sonar.volumes.find(volume=>volume.channel==='game').volume;
      assert.ok(Math.abs(current-owned)<.0001,'external volume change detected; refuse to overwrite');
      await invoke('audio_volume',{target:{kind:'sonar',channel:'game'},level:original,expected:current});
      const restored=(await read()).sonar.volumes.find(volume=>volume.channel==='game').volume;
      assert.ok(Math.abs(restored-original)<.0001);report.restored=true;
    }
    await writeFile(resolve(out,'native-report.json'),JSON.stringify(report,null,2));
    socket.close();
  }
  console.log(JSON.stringify(report));
})().catch(error=>{console.error(error);process.exitCode=1;});
