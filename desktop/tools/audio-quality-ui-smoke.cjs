// Fixture IPC only: exercises the real UI against the running Vite preview.
const {chromium}=require(process.env.PLAYWRIGHT_MODULE||'playwright');
const assert=require('node:assert/strict');
const {mkdir,writeFile}=require('node:fs/promises');
const {resolve}=require('node:path');
const base=process.env.JACK_UI_URL||'http://127.0.0.1:1421';
const out=resolve(__dirname,'../../artifacts/audio-quality');

async function installFixture(page) {
  await page.addInitScript(()=>{
    const channels=['game','chat','media'];
    window.fixture={settings:{theme:'dark',language:'ko',launch_at_login:false,channel_separation_enabled:true,link_group_outputs:false,
      groups:channels.map((sonar_channel,i)=>({id:`group-${i}`,display_name:['게이밍','채팅','미디어'][i],sonar_channel,device_volumes:{usb:.63}})),
      microphone_device_volumes:{mic:.5,mic2:.3}},audio:{endpoints:[],warnings:[]},separation_available:true,
      sonar:{connection:'connected',mode:'classic',message:null,
        volumes:[...channels.map((channel,i)=>({channel,volume:[.63,.44,.81][i],muted:false})),{channel:'microphone',volume:.5,muted:false}],
        routes:[...channels.map(channel=>({channel,device_id:'usb',device_name:'Speakers (USB)'})),{channel:'microphone',device_id:'mic',device_name:'Microphone (Studio)'}],
        devices:[{id:'usb',name:'Speakers (USB)',enabled:true,role:'playback'},{id:'usb2',name:'Headphones (USB)',enabled:true,role:'playback'},
          {id:'mic',name:'Microphone (Studio)',enabled:true,role:'capture'},{id:'mic2',name:'Microphone (Desk)',enabled:true,role:'capture'}],
        sources:channels.map(channel=>({channel,endpoint_id:channel})),
        applications:channels.map((route,i)=>({process_id:i+1,process_name:`app${i}`,display_name:`Application ${i}`,route,activity:'active',routing_error:false}))},
      app_routing:channels.map((requested,i)=>({process_id:i+1,stable_id:`path:app${i}`,requested,origin:'user',pending:false})),routing_warnings:[]};
    const callbacks=new Map(),listeners=new Map();let id=0;
    window.volumeCalls=[];window.fixtureFailure=null;window.reads=0;
    window.publishFixture=()=>listeners.get('jack-visibility')?.({payload:true});
    window.isTauri=true;
    window.__TAURI_INTERNALS__={metadata:{currentWindow:{label:'main'},currentWebview:{label:'main'}},
      transformCallback:callback=>{callbacks.set(++id,callback);return id;},
      invoke:async(command,args)=>{
        if(command==='plugin:event|listen'){listeners.set(args.event,callbacks.get(args.handler));return ++id;}
        if(command==='window_visible')return true;
        if(command==='resize_quick')return {anchor:0,origin:0};
        if(command==='audio_action'){
          if(args.action.kind==='read'){window.reads++;return structuredClone(window.fixture);}
          if(window.fixtureFailure)throw window.fixtureFailure;
          if(args.action.kind==='sonar'){
            const change=args.action.command,route=window.fixture.sonar.routes.find(route=>route.channel===change.channel);
            if(route.device_id!==change.expected)throw {code:'external_change',detail:'stale route'};
            const device=window.fixture.sonar.devices.find(device=>device.id===change.device_id);
            route.device_id=device.id;route.device_name=device.name;
          }
          if(args.action.kind==='assign_application'){
            const action=args.action;
            window.fixture.sonar.applications.find(app=>app.process_id===action.process_id).route=action.channel;
            window.fixture.app_routing.find(app=>app.process_id===action.process_id).requested=action.channel;
            window.fixture.settings.sonar_app_overrides={[`path:app${action.process_id-1}`]:action.channel};
          }
          return structuredClone(window.fixture);
        }
        if(command==='audio_volume'){
          window.volumeCalls.push(structuredClone(args));
          await new Promise(resolve=>setTimeout(resolve,100));
          if(window.fixtureFailure)throw window.fixtureFailure;
          const volume=window.fixture.sonar.volumes.find(volume=>volume.channel===args.target.channel);
          if(Math.abs(volume.volume-args.expected)>.0001)throw {code:'external_change',detail:'stale volume'};
          volume.volume=args.level;return args.level;
        }
        return 0;
      }};
  });
}

(async()=>{
  await mkdir(out,{recursive:true});
  const browser=await chromium.launch({channel:'msedge',headless:true});
  const report={cases:[],errors:[]};
  try {
    for(const tray of [false,true]) {
      const page=await browser.newPage({viewport:{width:tray?360:400,height:800}});
      page.on('pageerror',error=>report.errors.push(error.message));await installFixture(page);
      await page.goto(`${base}/?fixture=design${tray?'&view=tray':''}`);
      const slider=page.locator('#sonar-volume-game');await slider.waitFor();await page.waitForTimeout(450);
      await page.evaluate(()=>{window.keptVolume=document.getElementById('sonar-volume-game');window.keptApp=document.getElementById('sonar-app-3');window.keptPanel=document.querySelector('#panel')||document.querySelector('.quick-content');});
      await page.evaluate(()=>{window.fixture.sonar.volumes.find(volume=>volume.channel==='game').volume=.4;window.publishFixture();});
      await page.waitForFunction(()=>document.getElementById('sonar-volume-game').getAttribute('aria-valuenow')==='40');
      assert.equal(await page.evaluate(()=>window.keptVolume===document.getElementById('sonar-volume-game')),true);
      await slider.hover();await page.mouse.wheel(0,-100);await page.waitForTimeout(700);
      assert.equal(await slider.getAttribute('aria-valuenow'),'45');
      await page.keyboard.down('Shift');await page.mouse.wheel(0,-100);await page.keyboard.up('Shift');await page.waitForTimeout(700);
      assert.equal(await slider.getAttribute('aria-valuenow'),'46');
      for(let i=0;i<6;i++)await page.mouse.wheel(0,-100);
      await page.waitForTimeout(800);assert.equal(await slider.getAttribute('aria-valuenow'),'76');
      const calls=await page.evaluate(()=>window.volumeCalls);assert.equal(calls[0].expected,.4);assert.ok(calls.length<8,'wheel burst must coalesce');
      assert.equal(await page.evaluate(()=>window.keptVolume===document.getElementById('sonar-volume-game')),true);

      if(!tray) {
        await page.evaluate(()=>{window.fixture.sonar.applications[0].route='chat';window.fixture.app_routing[0].requested='chat';window.publishFixture();});
        await page.waitForFunction(()=>document.querySelector('[data-channel=chat] #sonar-app-1'));
        assert.equal(await page.evaluate(()=>window.keptApp===document.getElementById('sonar-app-3') && window.keptVolume===document.getElementById('sonar-volume-game')),true);
        await page.locator('#sonar-app-2').focus();await page.keyboard.press('Shift+ArrowDown');
        await page.waitForFunction(()=>!!document.querySelector('[data-channel=media] #sonar-app-2'));
        assert.equal(await page.evaluate(()=>window.keptVolume===document.getElementById('sonar-volume-game')),true);
        await page.locator('#panel').evaluate(panel=>panel.scrollTop=0);await page.waitForTimeout(500);
        const source=await page.locator('#sonar-app-1').boundingBox(),target=await page.locator('[data-channel=game]').boundingBox();
        await page.mouse.move(source.x+source.width/2,source.y+source.height/2);await page.mouse.down();
        await page.mouse.move(target.x+target.width/2,target.y+target.height-12,{steps:12});await page.mouse.up();
        await page.waitForFunction(()=>!!document.querySelector('[data-channel=game] #sonar-app-1') && !document.querySelector('.app-drag-ghost'),null,{timeout:5000}).catch(async error=>{
          await page.screenshot({path:resolve(out,'drag-failure.png')});
          console.log(await page.evaluate(()=>({route:window.fixture.sonar.applications[0].route,classes:document.getElementById('sonar-app-1')?.className,ghosts:document.querySelectorAll('.app-drag-ghost').length})));throw error;
        });
        assert.equal(await page.evaluate(()=>window.keptVolume===document.getElementById('sonar-volume-game')),true);
        await page.evaluate(()=>window.keptApp=document.getElementById('sonar-app-3'));
      }
      await page.locator('#sonar-output-game').click();
      const prefix=tray?'quick':'main';
      await page.locator(`#${prefix}-output-game-usb2`).waitFor({state:'visible'});
      assert.equal(await page.locator(`#${prefix}-output-game-usb .device-name`).textContent(),'Speakers');
      assert.equal(await page.locator(`#${prefix}-output-game-usb2 .device-name`).textContent(),'Headphones');
      assert.equal(await page.locator(`#${prefix}-output-game-usb2`).getAttribute('title'),'Headphones (USB)');
      if(!tray)await page.locator('#sonar-output-menu').screenshot({path:resolve(out,'corrected-device-names.png')});
      await page.locator(`#${prefix}-output-game-usb2`).click();await page.waitForTimeout(500);
      assert.equal(await slider.getAttribute('data-endpoint'),'usb2');
      if(!tray)assert.equal(await page.evaluate(()=>window.keptApp===document.getElementById('sonar-app-3')),true);

      await page.evaluate(()=>{
        window.fixture.settings.groups[0].device_volumes.usb2=.76;
        window.fixture.sonar.devices.find(device=>device.id==='usb2').enabled=false;window.publishFixture();
      });
      await page.waitForSelector('#sonar-volume-game[data-volume-unavailable]');
      assert.equal(await slider.isDisabled(),true);assert.match(await slider.textContent(),/연결 해제/);
      assert.equal(await page.locator('#sonar-output-game').isEnabled(),true);
      assert.equal(await page.locator('#sonar-output-game>.icon').evaluate(node=>getComputedStyle(node).opacity),'1');
      assert.equal(await slider.locator('.device-name').evaluate(node=>getComputedStyle(node).opacity),'0.35');
      assert.equal(await slider.locator('.icon').evaluate(node=>getComputedStyle(node).opacity),'0.35');
      const picker=page.locator('[data-channel=game] .sonar-picker-surface');
      assert.equal(await picker.count(),1,'disconnected output keeps the painted picker button');
      const paint=await picker.evaluate(node=>{const style=getComputedStyle(node),rect=node.getBoundingClientRect();return {background:style.backgroundColor,shadow:style.boxShadow,width:rect.width,height:rect.height};});
      assert.notEqual(paint.background,'rgba(0, 0, 0, 0)');assert.notEqual(paint.shadow,'none');
      assert.ok(paint.width>40 && paint.height>40,'picker background covers its button');
      const typography=await page.evaluate(()=>{
        const read=node=>{const style=getComputedStyle(node);return ['fontFamily','fontSize','fontWeight','lineHeight','letterSpacing'].map(key=>style[key]);};
        return {status:read(document.querySelector('[data-channel=game] .output-unavailable')),percent:read(document.querySelector('[data-channel=chat] .sonar-percent'))};
      });
      assert.equal(typography.status[2],'300','disconnection status uses Light weight');
      assert.deepEqual(typography.status.filter((_,i)=>i!==2),typography.percent.filter((_,i)=>i!==2),'disconnection status keeps percentage size and spacing');
      assert.equal(await slider.locator('.sonar-percent').count(),0);assert.match(await slider.getAttribute('title'),/76%/);
      const count=await page.evaluate(()=>window.volumeCalls.length);await slider.dispatchEvent('wheel',{deltaY:-100});
      assert.equal(await page.evaluate(()=>window.volumeCalls.length),count);
      await page.screenshot({path:resolve(out,tray?'tray-disconnected.png':'main-disconnected.png')});
      await slider.locator('xpath=ancestor::*[@data-channel="game"]').screenshot({path:resolve(out,tray?'corrected-disconnected-tray.png':'corrected-disconnected-main.png')});
      await page.locator('#sonar-output-game').click();
      await page.locator(`#${prefix}-output-game-usb`).click();
      await page.waitForFunction(()=>document.getElementById('sonar-volume-game').dataset.endpoint==='usb' && !document.getElementById('sonar-volume-game').disabled);
      await page.evaluate(()=>{
        window.fixture.sonar.devices.find(device=>device.id==='usb2').enabled=true;
        const route=window.fixture.sonar.routes.find(route=>route.channel==='game');route.device_id='usb2';route.device_name='Headphones (USB)';window.publishFixture();
      });
      await page.waitForSelector('#sonar-volume-game:not([data-volume-unavailable])');
      await page.evaluate(()=>window.fixtureFailure={code:'settings_save',detail:'Access denied C:\\private\\settings.json'});
      await slider.hover();await page.mouse.wheel(0,-100);await page.waitForTimeout(750);
      assert.match(await page.locator('#toast').textContent(),/설정.*저장.*권한/);
      assert.doesNotMatch(await page.locator('#toast').textContent(),/private|denied|object Object/);
      assert.equal(await slider.getAttribute('aria-valuenow'),'76','failed optimistic change returns to confirmed state');
      report.cases.push({tray,passed:true,volumeCalls:calls.length});await page.close();
    }
    const microphone=await browser.newPage({viewport:{width:400,height:680}});
    microphone.on('pageerror',error=>report.errors.push(error.message));await installFixture(microphone);
    await microphone.goto(`${base}/?tab=capture`);
    await microphone.waitForSelector('[data-input-device=mic][role=slider]');
    await microphone.evaluate(()=>{window.fixture.settings.theme='light';window.fixture.settings.language='en';window.publishFixture();});
    await microphone.waitForFunction(()=>document.documentElement.lang==='en' && document.documentElement.dataset.theme==='light');
    const mic=microphone.locator('[data-input-device=mic]');await mic.hover();await microphone.mouse.wheel(0,-100);await microphone.waitForTimeout(750);
    assert.equal(await mic.getAttribute('aria-valuenow'),'55');
    await microphone.evaluate(()=>{window.fixture.settings.microphone_device_volumes.mic2=0;window.publishFixture();});
    await microphone.waitForFunction(()=>document.querySelector('[data-input-device=mic2] .sonar-percent').textContent==='0%');
    await microphone.screenshot({path:resolve(out,'microphone-light-english.png')});
    await microphone.locator('#tab-render').click();await microphone.locator('#sonar-output-game').click();
    await microphone.locator('#main-output-game-usb2').waitFor();
    await microphone.evaluate(()=>window.fixtureFailure={code:'settings_save',detail:'private path'});
    await microphone.locator('#main-output-game-usb2').click();
    await microphone.waitForFunction(()=>document.getElementById('toast')?.textContent.includes('Could not save settings'));
    await microphone.locator('#output-sync-toggle').click();
    await microphone.waitForFunction(()=>document.getElementById('output-sync-toggle').getAttribute('aria-checked')==='false' && !document.getElementById('output-sync-toggle').disabled);
    report.cases.push({name:'microphone wheel, remembered zero, light theme and English errors',passed:true});await microphone.close();
    assert.deepEqual(report.errors,[]);
    await writeFile(resolve(out,'report.json'),JSON.stringify(report,null,2));console.log(JSON.stringify(report));
  }finally{await browser.close();}
})().catch(error=>{console.error(error);process.exitCode=1;});
