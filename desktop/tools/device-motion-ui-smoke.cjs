// Run Vite first. Exercises real fixture actions and the shipped minified CSS.
const {chromium}=require(process.env.PLAYWRIGHT_MODULE||'playwright');
const assert=require('node:assert/strict');
const {readFile,readdir,mkdir,writeFile}=require('node:fs/promises');
const {join,resolve}=require('node:path');
const root=resolve(__dirname,'../..');
const out=join(root,'artifacts','device-motion');

(async()=>{
  await mkdir(out,{recursive:true});
  const assets=join(root,'desktop','dist','assets');
  const css=(await Promise.all((await readdir(assets)).filter(n=>n.endsWith('.css')).map(n=>readFile(join(assets,n),'utf8')))).join('\n');
  const browser=await chromium.launch({channel:'msedge',headless:true});
  const report={cases:[],errors:[]};
  try {
    for(const tray of [false,true])for(const theme of ['dark','light']) {
      const page=await browser.newPage({viewport:{width:tray?360:400,height:850}});
      page.on('pageerror',error=>report.errors.push(error.message));
      await page.addInitScript(()=>{
        window.__deviceSwaps=[];
        const animate=Element.prototype.animate;
        Element.prototype.animate=function(frames,options) {
          if(this.matches('.device-label-layer,.device-icon-layer')) {
            const row=this.closest('.device-row');
            window.__deviceSwaps.push({row:row?.id,device:this.dataset.deviceLayerId,icon:this.classList.contains('device-icon-layer'),frames,duration:options.duration});
          }
          return animate.call(this,frames,options);
        };
      });
      await page.goto(`${process.env.JACK_UI_URL||'http://127.0.0.1:1420'}/?sonar&fixture=design&interactive&theme=${theme}${tray?'&view=tray':''}`);
      await page.waitForSelector('#sonar-volume-game');
      await page.addStyleTag({content:css});
      await page.waitForTimeout(400);
      const selections=[];
      let oldDevice='preview-output';
      // Use the actual picker in both directions, including the tray's detail
      // page/return. A forward-only component test cannot catch fixed direction.
      for(const [target,direction] of [['preview-output-1',-1],['preview-output',1]]) {
        await page.evaluate(()=>{window.__deviceSwaps=[];});
        await page.locator('#sonar-output-game').click();
        await page.locator(`#${tray?'quick':'main'}-output-game-${target}`).click();
        await page.waitForFunction(({target,direction,tray})=>{
          const row=document.querySelector('#sonar-volume-game');
          return row?.dataset.endpoint===target&&(tray||row.dataset.deviceMotionDirection===String(direction));
        },{target,direction,tray});
        const swaps=await page.evaluate(()=>window.__deviceSwaps.filter(swap=>swap.row==='sonar-volume-game'));
        if(tray)assert.equal(swaps.length,0,'tray detail navigation must not replay the main window device push');
        for(const icon of tray?[]:[false,true]) {
          const incoming=swaps.find(swap=>swap.icon===icon&&swap.device===target&&swap.frames[0].transform!==undefined);
          const outgoing=swaps.find(swap=>swap.icon===icon&&swap.device===oldDevice&&swap.frames[0].transform!==undefined);
          const incomingPaint=swaps.find(swap=>swap.icon===icon&&swap.device===target&&swap.frames[0].opacity!==undefined);
          const outgoingPaint=swaps.find(swap=>swap.icon===icon&&swap.device===oldDevice&&swap.frames[0].opacity!==undefined);
          assert.ok(incoming&&outgoing,'actual selection must animate text and icon on both app and tray');
          assert.equal(incoming.frames[0].transform,`translateY(calc(100% * ${-direction}))`);
          assert.equal(outgoing.frames.at(-1).transform,`translateY(calc(100% * ${direction}))`);
          assert.equal(incomingPaint.frames[0].filter,'blur(2px)');
          assert.equal(outgoingPaint.frames.at(-1).opacity,0);
          assert.equal(incoming.duration,250);
        }
        selections.push({from:oldDevice,to:target,direction,swaps});oldDevice=target;
        await page.waitForTimeout(350);
      }
      if(tray) {
        await page.locator('#input-expand').click();
        await page.waitForTimeout(50);
        const rotation=await page.locator('.t-acc-chevron').evaluate(node=>{
          const matrix=new DOMMatrix(getComputedStyle(node).transform);
          return {angle:Math.atan2(matrix.b,matrix.a)*180/Math.PI,determinant:matrix.a*matrix.d-matrix.b*matrix.c};
        });
        assert.ok(rotation.angle>0&&rotation.angle<180,'chevron must rotate during disclosure');
        assert.ok(Math.abs(rotation.determinant-1)<.01,'rotation must preserve the chevron shape');
        await page.waitForTimeout(350);
      } else {
        await page.locator('#sonar-output-game').click();
        await page.locator('#main-output-game-preview-output-1').click();
        await page.waitForSelector('#sonar-volume-game .device-label-layer');
      }
      const samples=await page.evaluate(async(tray)=>{
        const {captureDevicePaint,animateDeviceChange}=await import('/src/device-transition.ts');
        let row=document.querySelector('#sonar-volume-game');
        if(tray) {
          const paint=captureDevicePaint(row.parentElement);
          row.dataset.deviceMotionOrder=JSON.stringify([row.dataset.endpoint,'motion-test-device']);
          row.dataset.endpoint='motion-test-device';
          row.querySelector('.device-name').textContent='Changed device';
          animateDeviceChange(row.parentElement,paint);
        }
        const animations=()=>row.getAnimations({subtree:true}).filter(animation=>animation.id==='device-content-swap');
        const lanes=animations();
        lanes.forEach(animation=>{animation.pause();animation.currentTime=125;});
        const read=()=>[...row.querySelectorAll('.device-label-layer,.device-icon-layer')].map(node=>{
          const style=getComputedStyle(node);return {text:node.textContent,icon:node.classList.contains('device-icon-layer'),y:new DOMMatrix(style.transform).m42,opacity:Number(style.opacity),blur:style.filter};
        });
        const exit=read();
        lanes.forEach(animation=>animation.currentTime=187.5);
        const enter=read();
        // An unrelated render must carry the currently painted position forward.
        const before=read();const paint=captureDevicePaint(row.parentElement);
        const replacement=row.cloneNode(true);
        replacement.querySelector('.device-name').textContent=row.querySelector('.device-name').dataset.deviceLabel||'Changed device';
        const slot=replacement.querySelector('.icon[data-icon-asset]');
        slot.replaceChildren();slot.style.removeProperty('background-color');
        slot.style.maskImage=`url("${slot.dataset.iconAsset}")`;
        row.replaceWith(replacement);row=replacement;
        animateDeviceChange(row.parentElement,paint);
        animations().forEach(animation=>{animation.pause();animation.currentTime=0;});
        const interrupted=read();
        animations().forEach(animation=>animation.finish());
        await Promise.resolve();await Promise.resolve();await Promise.resolve();
        return {exit,enter,before,interrupted,durations:lanes.map(animation=>animation.effect.getTiming().duration)};
      },tray);
      for(const icon of [false,true]) {
        assert.ok(samples.exit.some(layer=>layer.icon===icon&&layer.y<0&&layer.opacity>0),'old device must slide upward');
        assert.ok(samples.enter.some(layer=>layer.icon===icon&&layer.y>0&&layer.opacity>0),'new device must enter from below');
        const layers=samples.exit.filter(layer=>layer.icon===icon);
        assert.equal(layers.length,2);
        assert.ok(layers.some(layer=>layer.y<0&&layer.opacity>0&&layer.opacity<1),'outgoing fades while leaving');
        assert.ok(layers.some(layer=>layer.y>0&&layer.opacity>0&&layer.opacity<1),'incoming fades in at the same midpoint');
        assert.ok(layers.every(layer=>layer.blur!=='none'&&layer.blur!=='blur(0px)'),'both layers interpolate blur during the push');
      }
      assert.ok(samples.durations.every(duration=>duration===250),'minified .25s must resolve to one shared 250ms push');
      samples.before.filter(layer=>layer.opacity>.001).forEach(layer=>assert.ok(samples.interrupted.some(next=>next.icon===layer.icon&&next.text===layer.text&&Math.abs(next.y-layer.y)<.01),'render must preserve current vertical position'));
      await page.screenshot({path:join(out,`${tray?'tray':'app'}-${theme}.png`)});
      await page.emulateMedia({reducedMotion:'reduce'});
      const reduced=await page.evaluate(async()=>{
        const {captureDevicePaint,animateDeviceChange}=await import('/src/device-transition.ts');
        const row=document.querySelector('#sonar-volume-game');
        const paint=captureDevicePaint(row.parentElement);
        row.dataset.endpoint='reduced-device';row.querySelector('.device-name').textContent='Reduced device';
        animateDeviceChange(row.parentElement,paint);
        return row.querySelectorAll('.device-label-layer,.device-icon-layer').length;
      });
      assert.equal(reduced,0,'reduced motion must commit immediately');
      await page.emulateMedia({reducedMotion:'no-preference'});
      const zero=await page.evaluate(async()=>{
        const {captureDevicePaint,animateDeviceChange}=await import('/src/device-transition.ts');
        const row=document.querySelector('#sonar-volume-game');
        const paint=captureDevicePaint(row.parentElement);
        row.style.setProperty('--device-text-duration','0s');
        row.dataset.endpoint='zero-device';row.querySelector('.device-name').textContent='Zero duration';
        animateDeviceChange(row.parentElement,paint);
        return row.querySelectorAll('.device-label-layer,.device-icon-layer').length;
      });
      assert.equal(zero,0,'valid zero seconds must not fall back to 150ms');
      report.cases.push({tray,theme,selections,samples});await page.close();
    }
    assert.deepEqual(report.errors,[]);
    console.log(`Device motion: ${report.cases.length} app/tray/theme cases passed with production CSS, including interrupted renders and reduced motion.`);
  } finally {
    await writeFile(join(out,'report.json'),JSON.stringify(report,null,2));await browser.close();
  }
})().catch(error=>{console.error(error);process.exitCode=1;});
