// Makes the pictures in docs/: the real interface (ui/index.html, with a stand-in for the Rust side) is put on a
// clean scene and photographed by a headless browser.
//   npm i puppeteer-core   &&   node shots/shots.mjs [only-this-shot] [path-to-chrome]
import { existsSync } from 'node:fs';
import { fileURLToPath, pathToFileURL } from 'node:url';
import path from 'node:path';
import { createRequire } from 'node:module';
const puppeteer = createRequire(path.join(process.cwd(), 'package.json'))('puppeteer-core');

const here = path.dirname(fileURLToPath(import.meta.url)), docs = path.join(here, '..', 'docs');
const only = process.argv[2] && !process.argv[2].includes('/') && !process.argv[2].includes('\\') ? process.argv[2] : '';
const chrome = process.argv[3] || ['C:/Program Files/Google/Chrome/Application/chrome.exe', 'C:/Program Files (x86)/Microsoft/Edge/Application/msedge.exe', '/usr/bin/google-chrome'].find(existsSync);

// What the page believes is the Rust side: settings, empty lists, and a way for us to send it events.
const standIn = (skin, zoom) => {
  if (!/ui\/index\.html/.test(location.href)) return;
  const subs = {};
  const settings = { skin, face: 'eyes', calendar: 'gregorian', toured: true, dndHint: true, autoStartOn: true, autoHide: true, version: '2.1.0', appName: 'Peeky', notifSeconds: 4, musicTheme: 'sky', vaultLength: 18 };
  const answers = { settings_get: () => settings, clips_get: () => [], shelf_get: () => [], notes_get: () => [], vault_list: () => ({ items: [] }), set_typing: () => ({}), update_check: () => 'none' };
  window.__TAURI__ = {
    core: { invoke: async (cmd) => (answers[cmd] ? answers[cmd]() : null) },
    event: { listen: async (ev, cb) => { (subs[ev] = subs[ev] || []).push(cb); } },
  };
  // room for the whole list under the pill; and alone on a plain stage the pill needs no halo
  addEventListener('DOMContentLoaded', () => { const s = document.createElement('style'); s.textContent = '#stack-list { max-height: 520px !important; }' + (zoom ? ' #glow { display: none !important; } body { transform: translateY(60px); overflow: visible !important; }' : ''); document.head.appendChild(s); });
  window.emit = (ev, payload) => (subs[ev] || []).forEach((cb) => cb({ payload }));
  // a cover, a file icon: drawn here so the repository holds no pictures that are not its own
  window.cover = (a, b, c) => {
    const cv = document.createElement('canvas'); cv.width = cv.height = 160; const g = cv.getContext('2d');
    const gr = g.createLinearGradient(0, 0, 160, 160); gr.addColorStop(0, a); gr.addColorStop(1, b); g.fillStyle = gr; g.fillRect(0, 0, 160, 160);
    g.fillStyle = c; g.beginPath(); g.arc(112, 54, 34, 0, 7); g.fill();
    g.globalAlpha = .55; g.beginPath(); g.arc(58, 118, 52, 0, 7); g.fill();
    return cv.toDataURL('image/png').split(',')[1];
  };
  window.doc = (color, tag) => {
    const cv = document.createElement('canvas'); cv.width = cv.height = 64; const g = cv.getContext('2d');
    g.fillStyle = '#fff'; g.beginPath(); g.roundRect(12, 4, 40, 56, 7); g.fill();
    g.fillStyle = color; g.beginPath(); g.roundRect(12, 34, 40, 26, [0, 0, 7, 7]); g.fill();
    g.fillStyle = '#fff'; g.font = '700 13px Segoe UI'; g.textAlign = 'center'; g.fillText(tag, 32, 52);
    return cv.toDataURL('image/png').split(',')[1];
  };
  window.play = (title, artist, a, b, c) => {
    emit('helper', { t: 'media', active: true, playing: true, title, artist, pos: 64000, dur: 214000, key: title, app: 'Spotify', kind: 'Music' });
    emit('helper', { t: 'thumb', key: title, data: cover(a, b, c) });
    __qazi.fake(0.85);
  };
  window.hover = (on) => { const r = document.getElementById('island').getBoundingClientRect(); emit('cursor', on ? { x: r.left + r.width / 2, y: r.top + r.height / 2 } : { x: 20, y: 600 }); };
};

const wait = (ms) => new Promise((r) => setTimeout(r, ms));
const SHOTS = [
  { name: 'hero', skin: 'black', scene: { mode: 'laptop', kicker: 'Peeky', title: 'A tiny island for your desktop', sub: 'Music, notifications, clipboard and files. One small pill at the top of the screen.' },
    run: () => { play('Midnight Drive', 'Neon Coast', '#ff7a59', '#7c3aed', '#ffd166'); }, ms: 2600 },
  { name: 'music', skin: 'nebula', scene: { mode: 'zoom', kicker: 'Now playing', title: 'Your music, one glance away', sub: 'Cover, title, controls and a scrubber for whatever is playing.' },
    run: () => { play('Midnight Drive', 'Neon Coast', '#ff7a59', '#7c3aed', '#ffd166'); }, ms: 3200 },
  { name: 'notification', skin: 'black', scene: { mode: 'laptop', kicker: 'Notifications', title: 'One line. Then it gets out of the way', sub: 'Click it to open that chat. While you are busy with the pill, it waits beside it.' },
    run: () => { __qazi.onNotif({ id: 1, app: 'Telegram', title: 'Sara', body: 'Are we still on for tonight? I booked the table for eight' }); }, ms: 1500 },
  { name: 'menu', skin: 'black', scene: { mode: 'zoom', kicker: 'One hover away', title: 'Everything in a single line', sub: 'Screenshot, text grab, clipboard, notes, passwords, timer, notifications.' },
    run: () => { hover(true); }, ms: 1600 },
  { name: 'smart-copy', skin: 'black', scene: { mode: 'zoom', kicker: 'Smart copy', title: 'Copy a sum, get the answer', sub: 'Links open, dates switch between Gregorian and Shamsi, colours show their swatch.' },
    run: () => { emit('clip', { kind: 'text', text: '1250 * 12 + 499', preview: '1250 * 12 + 499' }); }, ms: 1200 },
  { name: 'shelf', skin: 'black', scene: { mode: 'laptop', kicker: 'File shelf', title: 'Drop files on it. Pick them up later', sub: 'Up to five sit side by side on the pill until you drag them out again.' },
    run: () => { emit('shelf', [['Invoice.pdf', '#e5484d', 'PDF'], ['Budget.xlsx', '#2f9e5b', 'XLS'], ['Notes.docx', '#3b74e6', 'DOC'], ['Deck.pptx', '#e8703a', 'PPT']].map(([name, c, t]) => ({ name, path: 'C:/' + name, icon: doc(c, t) }))); }, ms: 1500 },
  { name: 'venom', skin: 'venom', scene: { mode: 'laptop', scale: 1.7, kicker: 'Themes', title: 'Or let it come alive', sub: 'Liquid skins that move with the beat, and Venom, where the pill itself turns to goo.' },
    run: () => { play('Symbiote', 'Black Tide', '#111827', '#4b5563', '#e5e7eb'); }, ms: 3400 },
  { name: 'tour', skin: 'black', scene: { mode: 'laptop', kicker: 'First start', title: 'It shows you around', sub: 'Every feature in one list. Click a row to try it.' },
    run: () => { __qazi.st.holdUntil = Date.now() + 60000; __qazi.setStack(true, 'tour'); }, ms: 1600 },
  { name: 'timer', skin: 'black', scene: { mode: 'zoom', kicker: 'Timer', title: 'Stays in sight while it runs', sub: 'Even over a fullscreen app.' },
    run: () => { hover(true); setTimeout(() => document.querySelector('[data-act="timer"]').click(), 400); }, ms: 1700 },
  { name: 'settings', skin: 'reactor', scene: { mode: 'laptop', kicker: 'Settings', title: 'No extra windows', sub: 'Every switch lives inside the island.' },
    run: () => { __qazi.st.holdUntil = Date.now() + 60000; __qazi.setStack(true, 'settings'); }, ms: 1600 },
];

const browser = await puppeteer.launch({ executablePath: chrome, headless: true, args: ['--hide-scrollbars', '--allow-file-access-from-files'] });
for (const shot of SHOTS) {
  if (only && shot.name !== only) continue;
  const page = await browser.newPage();
  await page.setViewport({ width: 1440, height: 900, deviceScaleFactor: 2 });
  await page.evaluateOnNewDocument(standIn, shot.skin, shot.scene.mode === 'zoom');
  await page.goto(pathToFileURL(path.join(here, 'scene.html')).href);
  await page.evaluate((o) => window.setScene(o), shot.scene);
  const app = page.frames().find((f) => /ui\/index\.html/.test(f.url()));
  await wait(900);
  await app.evaluate(shot.run);
  await wait(shot.ms);
  await page.screenshot({ path: path.join(docs, shot.name + '.jpg'), type: 'jpeg', quality: 90 });
  await page.close();
  console.log(shot.name);
}
await browser.close();
