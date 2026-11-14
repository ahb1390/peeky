<div align="center">

<img src="docs/idle.png" width="520" alt="Peeky resting at the top of the screen">

# Peeky

**A tiny dynamic island for Windows.** One small pill at the top of your screen that shows what is playing,
catches your notifications, remembers what you copied, holds your files, and blinks at you in between.

[![release](https://img.shields.io/github/v/release/raminturne/peeky?label=release&color=555)](https://github.com/raminturne/peeky/releases/latest)
[![downloads](https://img.shields.io/github/downloads/raminturne/peeky/total?label=downloads&color=555)](https://github.com/raminturne/peeky/releases)
[![build](https://img.shields.io/github/actions/workflow/status/raminturne/peeky/build.yml?branch=main&label=build)](https://github.com/raminturne/peeky/actions/workflows/build.yml)
![size](https://img.shields.io/badge/size-~3%20MB-555)
[![license](https://img.shields.io/badge/license-MIT-555)](LICENSE)

[![Windows installer](https://img.shields.io/badge/WINDOWS-INSTALLER-0a7cd6?style=for-the-badge)](https://github.com/raminturne/peeky/releases/latest/download/Peeky-Setup.exe)
[![Windows portable exe](https://img.shields.io/badge/WINDOWS-PORTABLE%20.EXE-1f2328?style=for-the-badge)](https://github.com/raminturne/peeky/releases/latest/download/Peeky-Portable.exe)
[![Windows zip](https://img.shields.io/badge/WINDOWS-.ZIP-1f2328?style=for-the-badge)](https://github.com/raminturne/peeky/releases/latest/download/Peeky-Windows.zip)

[**Watch the trailer**](docs/trailer.mp4) · [فارسی](#فارسی)

<a href="docs/trailer.mp4"><img src="docs/trailer.gif" width="760" alt="Peeky trailer"></a>

</div>

## What it does

| | |
|---|---|
| <img src="docs/music-nebula.png" width="380"> | **Now playing.** Title, artist, cover, controls and a scrubber for whatever is playing on the system. Films and fullscreen games are left alone. |
| <img src="docs/music-reactor.png" width="380"> | **Themes that move.** Liquid camo skins that drift at rest and flood with colour on the beat (or stay calm, if you switch that off). |
| <img src="docs/venom.png" width="380"> | **Venom.** The pill itself turns to black goo: the outline rolls, tendrils reach out and snap back. |
| <img src="docs/notification.png" width="380"> | **Notifications in one line.** Click to open that chat. While you are using the pill they wait beside it instead of taking it over. |
| <img src="docs/menu.png" width="380"> | **One hover away.** Screenshot, text grab, clipboard history, quick note, passwords, timer, notifications, settings. |
| <img src="docs/smart-copy.png" width="380"> | **Smart copy.** A copied link can be opened, a sum gets its answer, a date its twin in the other calendar (Gregorian ⇄ Shamsi), a colour its swatch. |
| <img src="docs/shelf.png" width="380"> | **File shelf.** Drop files on the pill; they sit there side by side until you drag them out again. |
| <img src="docs/timer.png" width="380"> | **Timer.** Stays in sight while it runs, even over a fullscreen app. |
| <img src="docs/settings.png" width="380"> | **Settings inside the island.** No extra windows. |

And also:

- **Quick notes.** `Ctrl+Alt+N` from anywhere, type one line, Enter. A time in the line (`call Ali in 20m`, `at 18:30 gym`, «۱۰ دقیقه دیگه چای») turns it into a reminder.
- **Passwords.** Generate one, confirm, and it asks which site and which e-mail it is for. Stored encrypted with your Windows account (DPAPI); copies clear themselves from the clipboard after 30 seconds.
- **Text grab.** `Ctrl+Alt+T`, drag a box, and the text in it is on your clipboard. English is read by Windows' own OCR; Persian by a bundled Tesseract. Nothing leaves your machine.
- **Clipboard history** with images and pins.
- **Small signs.** A dot while the microphone or camera is in use, a line when the internet or a VPN goes up or down, Bluetooth devices and the charger.
- **Updates itself.** New releases are downloaded, checked against the project's signing key, and installed with one click.

## Install

Download **[Peeky-Setup.exe](https://github.com/raminturne/peeky/releases/latest/download/Peeky-Setup.exe)** and run it.
It installs for the current user only (no administrator prompt) and starts with Windows; both can be changed in the settings.

Windows 10 or 11 with the WebView2 runtime (already part of Windows 11 and of current Windows 10).

The installer is not code-signed, so Windows SmartScreen may ask once whether to run it.

## Privacy

Peeky makes no network requests except checking this repository for a new release (which can be switched off).
Notifications, clipboard contents, notes and passwords stay on your computer.

## Build it yourself

```
cd src-tauri
cargo build --release
```

The result is a single `peeky.exe` (about 5 MB). The installer is `installer/island.nsi` (NSIS).
Every push is built by [GitHub Actions](.github/workflows/build.yml); a tag `vX.Y.Z` publishes a release.

The trailer is code too: `trailer/trailer.html` is a pure function of time and `trailer/render.mjs` steps a headless
browser through it at 60 frames a second.

## Platforms

Windows today. A Linux version is being worked on; the backend talks to Windows directly for almost everything
(notifications, media session, clipboard, the click-through window), so it is a port, not a rebuild.

## License

[MIT](LICENSE). Bundles tesseract.js and Tesseract's Persian data (Apache 2.0).

---

<div dir="rtl">

## فارسی

**Peeky یک جزیرهٔ کوچک و زنده برای ویندوز است.** یک قرص کوچک بالای صفحه که آهنگ در حال پخش را نشان می‌دهد،
نوتیفیکیشن‌ها را می‌گیرد، چیزهایی که کپی کرده‌ای یادش می‌ماند، فایل‌هایت را نگه می‌دارد و بین این کارها پلک می‌زند.

### امکانات

- **پخش موزیک:** نام آهنگ، خواننده، کاور، دکمه‌های کنترل و نوار جلو و عقب بردن. با فیلم و بازی تمام‌صفحه کاری ندارد.
- **تم‌های متحرک:** کموهای مایع که آرام حرکت می‌کنند و با ضرب آهنگ پر از رنگ می‌شوند (می‌شود واکنش به موزیک را خاموش کرد).
- **ونوم:** خود جزیره به لجن سیاه زنده تبدیل می‌شود.
- **نوتیفیکیشن در یک خط:** با کلیک همان چت باز می‌شود. وقتی مشغول کار با جزیره‌ای، کنار آن منتظر می‌ماند.
- **کپی هوشمند:** لینکِ کپی‌شده باز می‌شود، حساب جواب می‌گیرد، تاریخ میلادی و شمسی به هم تبدیل می‌شوند.
- **نگه‌داشتن فایل:** فایل را روی جزیره رها کن؛ کنار هم می‌مانند تا دوباره بیرونشان بکشی.
- **یادداشت سریع:** `Ctrl+Alt+N` از هر جا. اگر در متن زمان باشد («۱۰ دقیقه دیگه چای») یادآور می‌شود.
- **رمزها:** رمز می‌سازد، سایت و ایمیل را می‌پرسد و ذخیره می‌کند (رمزنگاری‌شده با حساب ویندوز).
- **خواندن متن از صفحه:** `Ctrl+Alt+T`، دور متن کادر بکش؛ انگلیسی و فارسی. همه‌چیز روی سیستم خودت انجام می‌شود.
- **تایمر، تاریخچهٔ کپی با عکس، نشانگر میکروفون و دوربین، وضعیت اینترنت و VPN.**
- **آپدیت خودکار:** نسخهٔ جدید دانلود، با کلید پروژه بررسی و با یک کلیک نصب می‌شود.

### نصب

فایل **[Peeky-Setup.exe](https://github.com/raminturne/peeky/releases/latest/download/Peeky-Setup.exe)** را دانلود و اجرا کن.
فقط برای کاربر فعلی نصب می‌شود و دسترسی ادمین نمی‌خواهد.

### حریم خصوصی

Peeky هیچ درخواست اینترنتی‌ای نمی‌فرستد، به‌جز بررسی همین مخزن برای نسخهٔ جدید (که می‌شود خاموشش کرد).
نوتیفیکیشن‌ها، کلیپ‌بورد، یادداشت‌ها و رمزها روی سیستم خودت می‌مانند.

</div>
