// Renders trailer.html to docs/trailer.mp4 at 60 frames a second: a headless browser lays out one frame at a
// time (the page is a pure function of time) and the pictures are piped straight into ffmpeg.
//   npm i puppeteer-core   &&   node trailer/render.mjs [path-to-chrome]      (ffmpeg must be on the PATH)
import { spawn } from 'node:child_process';
import { existsSync } from 'node:fs';
import { fileURLToPath, pathToFileURL } from 'node:url';
import path from 'node:path';
import { createRequire } from 'node:module';
// puppeteer-core is looked up from the folder the command is run in, so it need not be installed inside the repository
const puppeteer = createRequire(path.join(process.cwd(), 'package.json'))('puppeteer-core');

const here = path.dirname(fileURLToPath(import.meta.url)), FPS = 60;
const chrome = process.argv[2] || ['C:/Program Files/Google/Chrome/Application/chrome.exe', 'C:/Program Files (x86)/Microsoft/Edge/Application/msedge.exe', '/usr/bin/google-chrome'].find(existsSync);
const out = path.join(here, '..', 'docs', 'trailer.mp4');
const browser = await puppeteer.launch({ executablePath: chrome, headless: true, args: ['--force-device-scale-factor=1', '--hide-scrollbars'] });
const page = await browser.newPage();
await page.setViewport({ width: 1920, height: 1080 });
await page.goto(pathToFileURL(path.join(here, 'trailer.html')).href);
await page.evaluate(() => Promise.all([...document.images].map((i) => i.decode().catch(() => {}))));
const frames = Math.round((await page.evaluate(() => window.DURATION)) * FPS);
const ff = spawn('ffmpeg', ['-y', '-loglevel', 'error', '-f', 'image2pipe', '-framerate', String(FPS), '-c:v', 'mjpeg', '-i', '-', '-c:v', 'libx264', '-preset', 'slow', '-crf', '17', '-pix_fmt', 'yuv420p', '-movflags', '+faststart', out], { stdio: ['pipe', 'inherit', 'inherit'] });
for (let f = 0; f < frames; f++) {
  await page.evaluate((t) => window.seek(t), f / FPS);
  const jpg = await page.screenshot({ type: 'jpeg', quality: 95 });
  if (!ff.stdin.write(jpg)) await new Promise((r) => ff.stdin.once('drain', r));
  if (f % 300 === 0) console.log(`frame ${f} / ${frames}`);
}
ff.stdin.end();
await new Promise((r) => ff.on('close', r));
await browser.close();
console.log('wrote ' + out);