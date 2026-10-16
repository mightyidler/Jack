// Existing web preview only; fixture actions never access native audio state.
const {chromium}=require(process.env.PLAYWRIGHT_MODULE||'playwright');
const assert=require('node:assert/strict');
const {readFile,readdir,mkdir,writeFile}=require('node:fs/promises');
const {join,resolve}=require('node:path');
const root=resolve(__dirname,'../..'),out=join(root,'artifacts','output-group-motion');
const base=process.env.JACK_UI_URL||'http://127.0.0.1:1421';
const add='#group-add',remove='.sonar-group-remove';
const sample=(page,trigger)=>page.evaluate(async trigger=>{
  const panel=document.querySelector('#panel');
  const samples=[];
  const read=()=>{
    const node=document.querySelector('.sonar-channel[data-channel="aux"]:not([data-output-group-exit] *)');
    const button=document.querySelector('#group-add');
    const exit=document.querySelector('[data-output-group-exit]');
    const rect=(node)=>node?.getBoundingClientRect();
    samples.push({time:performance.now(),scroll:panel.scrollTop,range:panel.scrollHeight-panel.clientHeight,
      groupHeight:node?.parentElement.classList.contains('output-group-slot')?rect(node.parentElement).height:rect(node)?.height,
      groupOpacity:node?Number(getComputedStyle(node).opacity):null,
      buttonOpacity:button?Number(getComputedStyle(button).opacity):null,
      buttonBottom:rect(button)?.bottom,groupBottom:rect(node)?.bottom,panelBottom:rect(panel).bottom,
      exitHeight:rect(exit)?.height,exitOpacity:exit?Number(getComputedStyle(exit.firstElementChild).opacity):null,
      exitInert:exit?.inert,slots:panel.querySelectorAll('.output-group-slot').length});
  };
  read();document.querySelector(trigger).focus({preventScroll:true});document.querySelector(trigger).click();
  const start=performance.now();
  while(performance.now()-start<550) {await new Promise(requestAnimationFrame);read();}
  return samples;
},trigger);

(async()=>{
  await mkdir(out,{recursive:true});
  const assets=join(root,'desktop','dist','assets');
  const css=(await Promise.all((await readdir(assets)).filter(n=>n.startsWith('index-')&&n.endsWith('.css')).map(n=>readFile(join(assets,n),'utf8')))).join('\n');
  const browser=await chromium.launch({channel:'msedge',headless:true});
  const report={cases:[],errors:[]};
  try {
    for(const theme of ['dark','light'])for(const height of [680,480,1100]) {
      const page=await browser.newPage({viewport:{width:400,height}});
      page.on('pageerror',error=>report.errors.push(error.message));
      await page.goto(`${base}/?sonar&fixture=design&interactive&grouped-apps&theme=${theme}`);
      await page.waitForSelector(add);await page.addStyleTag({content:css});
      await page.locator('#panel').evaluate(panel=>panel.scrollTop=panel.scrollHeight);
      const entering=await sample(page,add),last=entering.at(-1);
      assert.ok(entering.some(frame=>frame.groupOpacity>0&&frame.groupOpacity<1),'group must fade in');
      assert.ok(entering.some(frame=>frame.groupHeight>0&&frame.groupHeight<110),'group space must expand');
      assert.ok(entering.some(frame=>frame.exitOpacity>0&&frame.exitOpacity<1),'add button must fade out');
      assert.ok(entering.filter(frame=>frame.exitHeight!==undefined).every(frame=>frame.exitInert),'exit paint must be inert');
      assert.equal(await page.locator(add).count(),0,'capacity must remove the add control');
      assert.equal(last.slots,0,'entrance wrappers must be cleaned up');
      if(last.range>0)assert.ok(Math.abs(last.groupBottom-(last.panelBottom-12))<2,'new group must settle at the bottom inset');
      if(height<1000)assert.ok(last.scroll>entering[0].scroll+10,'insertion must animate scroll');
      entering.slice(1).forEach((frame,index)=>assert.ok(frame.scroll>=entering[index].scroll-1,'insertion must never reverse scroll direction'));
      await page.screenshot({path:join(out,`${theme}-${height}-added.png`)});
      const leaving=await sample(page,remove),end=leaving.at(-1);
      assert.ok(leaving.some(frame=>frame.exitOpacity>0&&frame.exitOpacity<1),'group must fade out before removal');
      assert.ok(leaving.some(frame=>frame.buttonOpacity>0&&frame.buttonOpacity<1),'add control must fade back in');
      assert.equal(await page.locator('.sonar-channel[data-channel="aux"]').count(),0);
      assert.equal(end.slots,0);
      if(end.range>0)assert.ok(Math.abs(end.buttonBottom-(end.panelBottom-12))<2,'add button must settle at bottom inset');
      assert.equal(await page.locator(add).isEnabled(),true);
      await page.screenshot({path:join(out,`${theme}-${height}-removed.png`)});

      // Reverse a live entrance, then interrupt another entrance with a real
      // unrelated settings render. Neither may resurrect an exit or leave slots.
      await page.locator(add).evaluate(button=>button.click());await page.waitForTimeout(65);
      await page.locator(remove).evaluate(button=>button.click());await page.waitForTimeout(450);
      assert.equal(await page.locator('.output-group-slot').count(),0);
      assert.equal(await page.locator(add).count(),1);
      await page.locator(add).evaluate(button=>button.click());await page.waitForTimeout(70);
      const before=await page.locator('.sonar-channel[data-channel="aux"]').evaluate(node=>({opacity:Number(getComputedStyle(node).opacity),height:node.parentElement.getBoundingClientRect().height}));
      await page.locator('#output-sync-toggle').evaluate(button=>button.click());
      const after=await page.locator('.sonar-channel[data-channel="aux"]:not([data-output-group-exit] *)').evaluate(node=>({opacity:Number(getComputedStyle(node).opacity),height:node.parentElement.getBoundingClientRect().height}));
      assert.ok(after.opacity>=before.opacity-.05,'rerender must preserve current paint');
      assert.ok(after.height>=before.height-2,'rerender must preserve expansion');
      await page.waitForTimeout(450);
      assert.equal(await page.locator('.output-group-slot').count(),0);
      if(theme==='dark'&&height===680) {
        await page.locator('#sonar-app-104').press('Shift+ArrowDown');await page.waitForTimeout(400);
        assert.equal(await page.locator('.sonar-channel[data-channel="aux"] #sonar-app-104').count(),1);
        await page.locator('#panel').evaluate(panel=>panel.scrollTop=panel.scrollHeight);
        const occupiedExit=await sample(page,remove);
        assert.equal(await page.locator('.sonar-channel[data-channel="game"] #sonar-app-104').count(),1,'deleting occupied group must retain its app in the destination');
        assert.equal(occupiedExit.at(-1).slots,0);
        await page.locator(add).evaluate(button=>button.click());await page.waitForTimeout(60);
        await page.locator('#panel').evaluate(panel=>{
          panel.dispatchEvent(new WheelEvent('wheel',{deltaY:-200,bubbles:true}));panel.scrollTop=0;
        });
        await page.locator('#output-sync-toggle').evaluate(button=>button.click());await page.waitForTimeout(450);
        assert.equal(await page.locator('#panel').evaluate(panel=>panel.scrollTop),0,'user scroll must cancel automatic follow, including across a render');
      }
      await page.locator(remove).evaluate(button=>button.click());await page.waitForTimeout(450);
      await page.emulateMedia({reducedMotion:'reduce'});
      await page.locator(add).evaluate(button=>button.click());
      assert.equal(await page.locator('.output-group-slot').count(),0,'reduced motion must commit immediately');
      assert.equal(await page.locator(add).count(),0);
      if(height<1000)assert.ok(await page.locator('#panel').evaluate(panel=>Math.abs(panel.scrollTop-(panel.scrollHeight-panel.clientHeight))<2),'reduced motion must still reveal the new group');
      await page.locator(remove).evaluate(button=>button.click());
      assert.equal(await page.locator(add).count(),1);
      if(height<1000)assert.ok(await page.locator('#panel').evaluate(panel=>Math.abs(panel.scrollTop-(panel.scrollHeight-panel.clientHeight))<2),'reduced motion must still reveal the add button');
      await page.emulateMedia({reducedMotion:'no-preference'});
      await page.addStyleTag({content:':root{--output-group-open-duration:0s;--output-group-close-duration:0ms}'});
      await page.locator(add).evaluate(button=>button.click());
      assert.equal(await page.locator('.output-group-slot').count(),0,'valid zero seconds must stay zero');
      await page.locator(remove).evaluate(button=>button.click());
      assert.equal(await page.locator('.output-group-slot').count(),0,'valid zero milliseconds must stay zero');
      report.cases.push({theme,height,entering,leaving,before,after});await page.close();
    }
    assert.deepEqual(report.errors,[]);
    console.log(`Output group motion: ${report.cases.length} theme/height cases passed with minified CSS, reversal, interrupted render, reduced motion and zero timing.`);
  } finally {await writeFile(join(out,'report.json'),JSON.stringify(report,null,2));await browser.close();}
})().catch(error=>{console.error(error);process.exitCode=1;});
