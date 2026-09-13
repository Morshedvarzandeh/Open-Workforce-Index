// Record the actual offline example. All pasted output is explicitly illustrative.
const {chromium} = require('playwright');
const fs = require('node:fs');
const path = require('node:path');
const assert = require('node:assert/strict');
(async()=>{
 const root=path.resolve(__dirname,'..');
 const browser=await chromium.launch({headless:true,executablePath:process.env.OWI_CHROMIUM_PATH});
 const context=await browser.newContext({viewport:{width:1280,height:900},recordVideo:{dir:path.join(root,'docs/promotion/recording'),size:{width:1280,height:900}}});
 const page=await context.newPage();const errors=[];
 page.on('pageerror',e=>errors.push(e.message));
 await page.route('https://**/*',r=>r.abort());
 await page.goto('file://'+path.join(root,'demo/index.html'));
 async function stage(message,seconds=7){
  await page.evaluate(text=>{let e=document.getElementById('video-caption');if(!e){e=document.createElement('div');e.id='video-caption';e.style='position:fixed;bottom:0;left:0;right:0;padding:20px;background:#12372a;color:white;font:22px system-ui;text-align:center;z-index:10000';document.body.append(e);}e.textContent=text;},message);
  await page.waitForTimeout(seconds*1000);
 }
 await stage('OWI · Compare AI models, then check the result. Browser example.');
 await page.locator('#q').fill('Rewrite politely: Our order is late. Please confirm delivery by Friday.');
 await page.locator('[aria-controls="checkOptions"]').click();
 await page.locator('#checks').fill('contains:Friday');
 await stage('1. Describe your task and what a good answer must contain.');
 await page.locator('#chooseButton').click();
 await page.locator('#answer').scrollIntoViewIfNeeded();
 assert(await page.locator('#copybtn').isVisible());
 await stage('2. Compare estimates. This demo uses sample prices and assumed abilities.');
 await page.locator('#reviewDetails > summary').click();
 await page.locator('#pasteback').fill('Could you please confirm whether our order will arrive by Friday? Thank you.');
 await stage('3. Paste an answer to check. This answer is illustrative; no model was called.');
 await page.locator('#checkbtn').click();
 await page.locator('#checkrep').scrollIntoViewIfNeeded();
 assert.match(await page.locator('#checkrep').textContent(),/Friday/);
 await stage('4. Review the answer. Passing a phrase check does not guarantee quality.');
 await page.evaluate(()=>window.scrollTo(0,0));
 await stage('Try OWI from GitHub. Connect your own models when you are ready.',10);
 assert.deepEqual(errors,[]);
 const video=page.video();await context.close();
 const videoPath=await video.path();
 fs.writeFileSync(path.join(root,'docs/promotion/recording-path.txt'),videoPath);
 await browser.close();console.log('Offline demo flow passed; video recorded. No provider calls.');
})().catch(e=>{console.error(e);process.exit(1)});
