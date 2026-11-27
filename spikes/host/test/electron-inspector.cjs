// Chromium decides which element receives a physical Space key after a pointer grab.
const { app } = require('electron');
const assert = require('node:assert/strict');
const { resolve } = require('node:path');
const { setTimeout: delay } = require('node:timers/promises');

async function checkFocus(win) {
  app.focus({ steal: true });
  win.show();
  win.focus();
  const wc = win.webContents;
  const canvas = "document.querySelector('[data-role=timeline]')";
  const cap = "document.querySelector('[data-role=playhead]')";
  const play = "document.querySelector('[data-role=play]')";
  async function waitFor(expression, message) {
    const deadline = Date.now() + 15000;
    while (Date.now() < deadline) {
      if (await wc.executeJavaScript(expression)) return;
      await delay(30);
    }
    throw Error(message);
  }
  function key(keyCode) {
    wc.sendInputEvent({ type: 'keyDown', keyCode });
    wc.sendInputEvent({ type: 'keyUp', keyCode });
  }
  await waitFor("document.querySelector('[data-role=preview]')?.dataset.frame !== undefined", 'Native picture did not arrive');
  await wc.executeJavaScript(canvas + '.focus()');
  key('Tab');
  await waitFor('document.activeElement !== ' + canvas, 'Tab did not leave the timeline');
  assert.equal(await wc.executeJavaScript('document.activeElement === ' + cap), false, 'Tab focused the playhead cap');

  const { x, y } = await wc.executeJavaScript(`(() => {
    const bounds = ${cap}.getBoundingClientRect();
    return { x: Math.round(bounds.x + bounds.width / 2), y: Math.round(bounds.y + bounds.height / 2) };
  })()`);
  wc.sendInputEvent({ type: 'mouseDown', button: 'left', clickCount: 1, x, y });
  wc.sendInputEvent({ type: 'mouseMove', x: x + 12, y, modifiers: ['leftButtonDown'] });
  wc.sendInputEvent({ type: 'mouseUp', button: 'left', clickCount: 1, x: x + 12, y });
  await waitFor('document.activeElement === ' + canvas, 'Grabbing the playhead did not focus the timeline');
  key('Space');
  await waitFor(play + ".textContent.trim() === 'Pause'", 'Space did not start playback');
  key('Space');
  await waitFor(play + ".textContent.trim() === 'Play'", 'Space did not pause playback');
}

app.once('browser-window-created', (_, win) => {
  win.webContents.once('did-finish-load', () => {
    checkFocus(win).then(() => {
      console.log('The native playhead grab leaves Space available to play and pause.');
      win.close();
    }).catch(error => { console.error(error); app.exit(1); });
  });
});
if (process.argv.length === 2) process.argv.push(resolve(__dirname, '../../examples/signal-atlas'));
require('../harness/dist/main.cjs');
