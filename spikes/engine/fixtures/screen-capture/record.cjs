// Capture the compositor's output for a newly created, isolated Playwright page.
// Only this local page is a CDP target. No screen, desktop, audio or user profile is requested.
const { chromium } = require(process.env.PLAYWRIGHT_MODULE || 'playwright');
const fs = require('node:fs/promises');
const path = require('node:path');
const { performance } = require('node:perf_hooks');

async function record(directory, seconds) {
  await fs.mkdir(directory);
  const browser = await chromium.launch({ channel: 'chrome', headless: true });
  try {
    const context = await browser.newContext({ viewport: {width:640,height:480}, deviceScaleFactor:1, offline:true });
    const page = await context.newPage();
    // Serve exactly two repository files from an intercepted origin, fully offline.
    // A shared origin lets Chromium load the original fixture font with ordinary web rules.
    await page.route('https://n4.invalid/**', async (route) => {
      const files = {
        '/capture/app.html': [path.join(__dirname, 'app.html'), 'text/html; charset=utf-8'],
        '/fonts/N4GridSans.ttf': [path.join(__dirname, '../fonts/N4GridSans.ttf'), 'font/ttf'],
      };
      const file = files[new URL(route.request().url()).pathname];
      if (!file) return route.abort();
      return route.fulfill({path: file[0], contentType: file[1]});
    });
    await page.goto('https://n4.invalid/capture/app.html');
    const hasFont = await page.evaluate(async () =>
      (await document.fonts.load('16px N4Grid')).some(face => face.status === 'loaded')
    );
    if (!hasFont) throw new Error('the fixture font did not load');
    const cdp = await context.newCDPSession(page);
    const records = [];
    let latest = Promise.resolve();
    cdp.on('Page.screencastFrame', event => {
      latest = latest.then(async () => {
        const timestamp = Math.round(event.metadata.timestamp * 1000);
        const origin = records[0]?.origin ?? timestamp;
        const elapsed = timestamp - origin;
        // H.264 uses a 1 ms media clock. Collapse captures inside one clock tick.
        if (!records.length || elapsed > records.at(-1).ms) {
          const file = String(records.length).padStart(5,'0') + '.png';
          await fs.writeFile(path.join(directory,file), Buffer.from(event.data,'base64'));
          records.push({origin,ms:elapsed,file});
        }
        await cdp.send('Page.screencastFrameAck',{sessionId:event.sessionId});
      });
    });
    await cdp.send('Page.startScreencast',{format:'png',everyNthFrame:1});
    const start = performance.now();
    // Short interactive bursts alternate with real holds. Each actual button activation
    // updates the same DOM state a person would see, and Chrome timestamps the painted picture.
    while ((performance.now()-start) < seconds*1000) {
      const phase = (performance.now()-start) % 12000;
      const delay = phase < 1600 ? 85 : phase < 5400 ? 330 : 1200;
      await page.waitForTimeout(delay);
      await page.locator('#review').dispatchEvent('click');
    }
    await page.waitForTimeout(150);
    await cdp.send('Page.stopScreencast');
    await latest;
    const metadata = { browser: browser.version(), requested_seconds:seconds,
      frames:records.map(({ms,file})=>({ms,file})) };
    await fs.writeFile(path.join(directory,'capture.json'), JSON.stringify(metadata));
    console.log('Captured', records.length,'frames over', records.at(-1).ms,'ms using Chrome',metadata.browser);
  } finally { await browser.close(); }
}
const [, , target, duration='180'] = process.argv;
if (!target || !(Number(duration)>0)) throw new Error('usage: node record.cjs NEW_CAPTURE_DIR [SECONDS]');
record(path.resolve(target), Number(duration)).catch(err=>{console.error(err);process.exitCode=1;});
