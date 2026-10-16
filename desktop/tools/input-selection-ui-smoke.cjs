// Run against the existing Vite preview. All routing uses local fixture data.
const {chromium}=require(process.env.PLAYWRIGHT_MODULE||'playwright');
const assert=require('node:assert/strict');
const {readFile,readdir,mkdir,writeFile}=require('node:fs/promises');
const {join,resolve}=require('node:path');
const root=resolve(__dirname,'../..');
const out=join(root,'artifacts','input-selection');
const base=process.env.JACK_UI_URL||'http://127.0.0.1:1421';

(async()=>{
  await mkdir(out,{recursive:true});
  const assets=join(root,'desktop','dist','assets');
  const css=(await Promise.all((await readdir(assets)).filter(n=>n.startsWith('index-')&&n.endsWith('.css')).map(n=>readFile(join(assets,n),'utf8')))).join('\n');
  const browser=await chromium.launch({channel:'msedge',headless:true});
  const report={cases:[],errors:[]};
  try {
    for(const tray of [false,true])for(const theme of ['dark','light']) {
      const page=await browser.newPage({viewport:{width:tray?360:400,height:850}});
      page.on('pageerror',error=>report.errors.push(error.message));
      await page.goto(`${base}/?sonar&fixture=design&interactive&theme=${theme}&${tray?'view=tray&expanded':'tab=capture'}`);
      await page.addStyleTag({content:css});
      await page.waitForSelector('#device-capture-preview-capture-1');
      await page.waitForTimeout(300);
      const initial=await page.locator('[data-input-list]').evaluate(list=>[...list.querySelectorAll('.device-row')].map(row=>({id:row.id,y:row.getBoundingClientRect().y,height:row.getBoundingClientRect().height,label:row.querySelector('.device-name').getBoundingClientRect().x})));
      await page.locator('#device-capture-preview-capture-1').press('Enter');
      await page.locator('.input-selection-surface').waitFor({state:'attached'});
      const midpoint=await page.locator('[data-input-list]').evaluate(list=>{
        const animations=list.getAnimations({subtree:true}).filter(a=>a.id.startsWith('input-selection-'));
        animations.forEach(a=>{a.pause();a.currentTime=125;});
        const layer=list.querySelector('.input-selection-surface');
        const row=list.querySelector('.selected');
        const rows=[...list.querySelectorAll('.device-row')];
        return {
          y:layer.getBoundingClientRect().y,target:row.getBoundingClientRect().y,origin:rows[0].getBoundingClientRect().y,
          fillWidth:layer.querySelector('.volume-fill').getBoundingClientRect().width,
          stroke:parseFloat(getComputedStyle(row.querySelector('path')).strokeDashoffset),
          status:Number(getComputedStyle(row.querySelector('.input-volume-status')).opacity),
          hiddenLiveSurface:Number(getComputedStyle(row.querySelector('.volume-surface-clip')).opacity),
          durations:animations.map(a=>a.effect.getTiming().duration),
          rows:rows.map(row=>({id:row.id,y:row.getBoundingClientRect().y,height:row.getBoundingClientRect().height,label:row.querySelector('.device-name').getBoundingClientRect().x})),
        };
      });
      assert.ok(midpoint.y>midpoint.origin&&midpoint.y<midpoint.target,'surface travels between rows');
      assert.ok(midpoint.fillWidth>20,'travelling surface retains its volume fill');
      assert.ok(midpoint.stroke>0&&midpoint.stroke<1,'check draws during selection');
      assert.ok(midpoint.status>0,'check feedback starts without a delayed blank slot');
      assert.equal(midpoint.hiddenLiveSurface,0,'only one selection surface is painted');
      assert.deepEqual(midpoint.rows,initial,'labels and hit rectangles remain fixed');
      assert.ok(midpoint.durations.includes(250),'production .25s resolves to 250 milliseconds');
      await page.screenshot({path:join(out,`${tray?'tray':'app'}-${theme}-midpoint.png`)});
      // Retarget while paused mid-flight: the next surface must start at this
      // exact painted position, rather than at either selected row.
      await page.locator('#device-capture-preview-capture-2').press('Enter');
      await page.waitForFunction(()=>document.querySelector('[data-input-list] .device-row.selected')?.dataset.endpoint==='preview-capture-2');
      const resumed=await page.locator('[data-input-list]').evaluate(list=>{
        list.getAnimations({subtree:true}).filter(a=>a.id.startsWith('input-selection-')).forEach(a=>{a.pause();a.currentTime=0;});
        return list.querySelector('.input-selection-surface').getBoundingClientRect().y;
      });
      assert.ok(Math.abs(resumed-midpoint.y)<.1,'rapid selection preserves the painted surface position');
      await page.locator('[data-input-list]').evaluate(list=>list.getAnimations({subtree:true}).forEach(a=>a.finish()));
      await page.locator('.input-selection-surface').waitFor({state:'detached'});
      assert.equal(await page.locator('[data-input-list] .selected').count(),1);
      await page.locator('#device-capture-preview-capture-0').press('Enter');
      await page.locator('.input-selection-surface').waitFor({state:'attached'});
      // Beginning a real volume gesture must immediately hand paint back to the
      // live slider, so the travelling copy never masks the user's adjustment.
      const bounds=await page.locator('#device-capture-preview-capture-0').boundingBox();
      await page.mouse.move(bounds.x+bounds.width*.4,bounds.y+bounds.height/2);
      await page.mouse.down();
      assert.equal(await page.locator('.input-selection-surface').count(),0);
      await page.mouse.up();
      await page.waitForTimeout(400);
      await page.emulateMedia({reducedMotion:'reduce'});
      await page.locator('#device-capture-preview-capture-1').press('Enter');
      await page.waitForFunction(()=>document.querySelector('[data-input-list] .device-row.selected')?.dataset.endpoint==='preview-capture-1');
      assert.equal(await page.locator('.input-selection-surface').count(),0,'reduced motion applies selection immediately');
      await page.emulateMedia({reducedMotion:'no-preference'});
      await page.addStyleTag({content:'[data-input-list] { --input-selection-duration:0s!important; --input-check-duration:0ms!important; --input-status-duration:0s!important; }'});
      await page.locator('#device-capture-preview-capture-2').press('Enter');
      await page.waitForFunction(()=>document.querySelector('[data-input-list] .device-row.selected')?.dataset.endpoint==='preview-capture-2');
      assert.equal(await page.locator('.input-selection-surface').count(),0,'valid zero seconds never falls back');
      report.cases.push({tray,theme,midpoint,resumed,passed:true});
      await page.close();
    }
    assert.deepEqual(report.errors,[]);
    console.log('Input selection: app/tray × dark/light passed with built CSS, fixed labels/hit targets, check draw, retargeting, volume handoff, reduced motion and zero durations.');
  } finally {
    await writeFile(join(out,'report.json'),JSON.stringify(report,null,2));
    await browser.close();
  }
})().catch(error=>{console.error(error);process.exitCode=1;});
