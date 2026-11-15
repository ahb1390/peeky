// Native Linux backend. X11 and Wayland use the same GTK/WebKit runtime; compositor
// policy controls placement and global-input features on Wayland.
use std::{
    collections::HashSet,
    fs,
    io::{Cursor, Read, Write},
    path::{Path, PathBuf},
    process::{Command, Stdio},
    ptr,
    sync::{atomic::{AtomicBool, AtomicU64, Ordering}, Mutex, OnceLock},
    thread,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

use base64::{engine::general_purpose::STANDARD as BASE64, Engine as _};
use image::ImageFormat;
use gtk::glib::object::Cast;
use serde::{Deserialize, Serialize};
use serde_json::{json, Map, Value};
use tauri::{
    menu::{CheckMenuItemBuilder, MenuBuilder, MenuItemBuilder},
    tray::TrayIconBuilder,
    Emitter, Manager,
};
use gtk::prelude::*;
use gdkx11::X11Window;
use gtk_layer_shell::{Edge, KeyboardMode, Layer, LayerShell};
use cairo::{RectangleInt, Region};
use tauri_plugin_global_shortcut::{Code, GlobalShortcutExt, Modifiers, ShortcutState};

const APP_NAME: &str = "Peeky";
const WIN_W: f64 = 700.0;
const WIN_H: f64 = 430.0;
const MAX_SHELF_BYTES: usize = 200 * 1024 * 1024;

static APP: OnceLock<tauri::AppHandle> = OnceLock::new();
static SETTINGS: OnceLock<Mutex<Map<String, Value>>> = OnceLock::new();
static NOTES: OnceLock<Mutex<Vec<Note>>> = OnceLock::new();
static VAULT: OnceLock<Mutex<Vec<VaultItem>>> = OnceLock::new();
static CLIPS: OnceLock<Mutex<Vec<ClipEntry>>> = OnceLock::new();
static SHELF: OnceLock<Mutex<Vec<ShelfItem>>> = OnceLock::new();
static LAST_CLIPBOARD: OnceLock<Mutex<String>> = OnceLock::new();
static X11_POINTER_ACTIVE: AtomicBool = AtomicBool::new(false);
static X11_LIB: OnceLock<x11_dl::xlib::Xlib> = OnceLock::new();
static INPUT_INTERACTIVE: AtomicBool = AtomicBool::new(false);
static INPUT_HEARTBEAT: AtomicU64 = AtomicU64::new(0);
static TYPING_ACTIVE: AtomicBool = AtomicBool::new(false);
static PREVIOUS_X11_WINDOW: OnceLock<Mutex<Option<String>>> = OnceLock::new();

fn previous_x11_window() -> &'static Mutex<Option<String>> {
    PREVIOUS_X11_WINDOW.get_or_init(|| Mutex::new(None))
}

fn now_ms() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default().as_millis() as u64
}

fn home_dir() -> PathBuf {
    std::env::var_os("HOME").map(PathBuf::from).unwrap_or_else(|| PathBuf::from("."))
}

fn data_dir() -> PathBuf {
    let base = std::env::var_os("XDG_DATA_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| home_dir().join(".local/share"));
    let dir = base.join("peeky");
    let _ = fs::create_dir_all(&dir);
    dir
}

fn config_dir() -> PathBuf {
    let base = std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| home_dir().join(".config"));
    let dir = base.join("peeky");
    let _ = fs::create_dir_all(&dir);
    dir
}

fn is_wayland() -> bool {
    std::env::var_os("WAYLAND_DISPLAY").is_some()
        || std::env::var("XDG_SESSION_TYPE").map(|s| s.eq_ignore_ascii_case("wayland")).unwrap_or(false)
}

fn emit<S: Serialize + Clone>(event: &str, payload: S) {
    if let Some(app) = APP.get() {
        let _ = app.emit(event, payload);
    }
}

fn log(message: &str) {
    let path = data_dir().join("peeky.log");
    if let Ok(mut file) = fs::OpenOptions::new().create(true).append(true).open(path) {
        let _ = writeln!(file, "[{}] {}", now_ms(), message);
    }
}

fn output(program: &str, args: &[&str]) -> Option<String> {
    let result = Command::new(program).args(args).stdout(Stdio::piped()).stderr(Stdio::null()).output().ok()?;
    if result.status.success() {
        Some(String::from_utf8_lossy(&result.stdout).trim().to_string())
    } else {
        None
    }
}

fn command_exists(program: &str) -> bool {
    Command::new("sh")
        .args(["-c", "command -v \"$1\" >/dev/null 2>&1", "peeky", program])
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

fn open_path(path: &str) {
    if !path.is_empty() {
        let _ = Command::new("xdg-open").arg(path).stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null()).spawn();
    }
}

fn write_atomic(path: &Path, bytes: &[u8]) -> bool {
    if let Some(parent) = path.parent() {
        if fs::create_dir_all(parent).is_err() { return false; }
    }
    let tmp = path.with_extension("tmp");
    fs::write(&tmp, bytes).is_ok() && fs::rename(&tmp, path).is_ok()
}

fn write_json(path: &Path, value: &impl Serialize) {
    if let Ok(bytes) = serde_json::to_vec_pretty(value) {
        let _ = write_atomic(path, &bytes);
    }
}

// ---------------------------------------------------------------- settings and XDG autostart
fn settings_defaults() -> Map<String, Value> {
    let mut map = json!({
        "autoHide": true, "dndHint": false, "toured": false, "calendar": "gregorian", "hideVideo": true,
        "copyPreview": true, "clipHistory": true, "deviceAlerts": true, "downloadAlerts": true, "shelf": true,
        "visuals": true, "notifSeconds": 4, "face": "eyes", "noteHotkey": !is_wayland(),
        "vaultLength": 18, "vaultSymbols": true, "lastUser": "", "skin": "black", "musicTheme": "sky",
        "netAlerts": true, "sensorDot": true, "smartCopy": true, "musicReact": true, "autoUpdate": false,
        "autoStart": false
    }).as_object().cloned().unwrap_or_default();
    map.insert("platform".into(), Value::String("linux".into()));
    map
}

fn settings_map() -> &'static Mutex<Map<String, Value>> {
    SETTINGS.get_or_init(|| Mutex::new(settings_defaults()))
}

fn settings_load() {
    let mut map = settings_defaults();
    if let Ok(saved) = fs::read_to_string(config_dir().join("settings.json")) {
        if let Ok(Value::Object(extra)) = serde_json::from_str::<Value>(&saved) {
            for (key, value) in extra { map.insert(key, value); }
        }
    }
    if is_wayland() { map.insert("noteHotkey".into(), false.into()); }
    *settings_map().lock().unwrap() = map;
    settings_save();
}

fn settings_save() {
    let snapshot = settings_map().lock().unwrap().clone();
    write_json(&config_dir().join("settings.json"), &snapshot);
}

fn setting(key: &str) -> Value {
    settings_map().lock().unwrap().get(key).cloned().unwrap_or(Value::Null)
}

fn setting_flag(key: &str) -> bool {
    setting(key).as_bool().unwrap_or(false)
}

fn settings_put(key: &str, value: Value) {
    settings_map().lock().unwrap().insert(key.to_string(), value);
    settings_save();
}

fn autostart_path() -> PathBuf {
    let base = std::env::var_os("XDG_CONFIG_HOME").map(PathBuf::from).unwrap_or_else(|| home_dir().join(".config"));
    base.join("autostart/com.peeky.app.desktop")
}

fn autostart_on() -> bool {
    autostart_path().exists()
}

fn set_autostart(on: bool) {
    let path = autostart_path();
    if !on {
        let _ = fs::remove_file(path);
        return;
    }
    let executable = match std::env::current_exe() {
        Ok(p) => p,
        Err(_) => return,
    };
    let escaped = executable.to_string_lossy().replace('\\', "\\\\").replace('"', "\\\"");
    let body = format!(
        "[Desktop Entry]\nType=Application\nName=Peeky\nComment=Peeky desktop island\nExec=\"{}\"\nTerminal=false\nX-GNOME-Autostart-enabled=true\n",
        escaped
    );
    let _ = write_atomic(&path, body.as_bytes());
}

fn settings_payload() -> Value {
    let mut map = settings_map().lock().unwrap().clone();
    map.insert("autoStartOn".into(), autostart_on().into());
    map.insert("version".into(), env!("CARGO_PKG_VERSION").into());
    map.insert("appName".into(), APP_NAME.into());
    map.insert("displayServer".into(), if is_wayland() { "wayland" } else { "x11" }.into());
    Value::Object(map)
}

fn settings_broadcast() {
    emit("settings", settings_payload());
}

fn settings_apply(key: &str, value: &Value) {
    const BOOLS: &[&str] = &[
        "autoUpdate", "musicReact", "netAlerts", "sensorDot", "smartCopy", "autoHide", "hideVideo",
        "copyPreview", "clipHistory", "deviceAlerts", "downloadAlerts", "shelf", "visuals", "noteHotkey", "vaultSymbols",
    ];
    match key {
        "autoStart" => {
            let on = value.as_bool().unwrap_or(false);
            settings_put("autoStart", on.into());
            set_autostart(on);
        }
        "calendar" if ["gregorian", "shamsi", "shamsi-fa", "both"].contains(&value.as_str().unwrap_or("")) => settings_put(key, value.clone()),
        "face" if ["eyes", "clock"].contains(&value.as_str().unwrap_or("")) => settings_put(key, value.clone()),
        "skin" if ["black", "nebula", "crystal", "ember", "reactor", "venom", "cloud", "water", "lava", "ice"].contains(&value.as_str().unwrap_or("")) => settings_put(key, value.clone()),
        "musicTheme" if value.as_str().map(|v| v == "sky" || ["nebula", "crystal", "ember", "reactor", "venom", "cloud", "water", "lava", "ice"].contains(&v)).unwrap_or(false) => settings_put(key, value.clone()),
        "notifSeconds" if value.as_f64().map(|n| (2.0..=15.0).contains(&n)).unwrap_or(false) => settings_put(key, json!(value.as_f64().unwrap().round() as u64)),
        "vaultLength" if value.as_u64().map(|n| (8..=40).contains(&n)).unwrap_or(false) => settings_put(key, value.clone()),
        "dndHint" | "toured" => settings_put(key, true.into()),
        _ if BOOLS.contains(&key) => {
            let mut on = value.as_bool().unwrap_or(false);
            // The upstream global-hotkey backend is X11-only. Wayland sessions deliberately do not
            // advertise a shortcut that the compositor cannot register reliably.
            if key == "noteHotkey" && is_wayland() { on = false; }
            settings_put(key, on.into());
            if key == "noteHotkey" { update_hotkeys(on); }
        }
        _ => return,
    }
    settings_broadcast();
}

fn update_hotkeys(on: bool) {
    if is_wayland() { return; }
    if let Some(app) = APP.get() {
        let shortcuts = app.global_shortcut();
        if on {
            if let Err(e) = shortcuts.register("ctrl+alt+n") { log(&format!("Could not register Ctrl+Alt+N: {e}")); }
            if let Err(e) = shortcuts.register("ctrl+alt+t") { log(&format!("Could not register Ctrl+Alt+T: {e}")); }
        } else {
            let _ = shortcuts.unregister("ctrl+alt+n");
            let _ = shortcuts.unregister("ctrl+alt+t");
        }
    }
}

// ---------------------------------------------------------------- clipboard, history and private copy
#[derive(Clone, Serialize, Deserialize)]
struct ClipEntry {
    id: u64,
    #[serde(default)]
    text: String,
    pinned: bool,
    at: u64,
    #[serde(default)]
    image: Option<String>,
    #[serde(default)]
    preview: String,
    #[serde(default)]
    thumb: String,
    #[serde(skip)]
    image_bytes: Option<Vec<u8>>,
}

fn clips() -> &'static Mutex<Vec<ClipEntry>> {
    CLIPS.get_or_init(|| Mutex::new(Vec::new()))
}

fn last_clipboard() -> &'static Mutex<String> {
    LAST_CLIPBOARD.get_or_init(|| Mutex::new(String::new()))
}

fn clipboard_read() -> Option<String> {
    if is_wayland() {
        output("wl-paste", &["--no-newline", "--type", "text/plain"])
    } else {
        output("xclip", &["-selection", "clipboard", "-o", "-t", "UTF8_STRING"])
            .or_else(|| output("xsel", &["--clipboard", "--output"]))
    }
}

fn clipboard_write(text: &str, private: bool) -> bool {
    let wayland = is_wayland();
    let program = if wayland { "wl-copy" } else if command_exists("xclip") { "xclip" } else { "xsel" };
    let mut cmd = Command::new(program);
    if wayland {
        if private { cmd.arg("--sensitive"); }
    } else if program == "xclip" {
        cmd.args(["-selection", "clipboard", "-i"]);
    } else {
        cmd.args(["--clipboard", "--input"]);
    }
    let Ok(mut child) = cmd.stdin(Stdio::piped()).stdout(Stdio::null()).stderr(Stdio::null()).spawn() else { return false; };
    let Some(mut stdin) = child.stdin.take() else { let _ = child.kill(); return false; };
    if stdin.write_all(text.as_bytes()).is_err() { let _ = child.kill(); return false; }
    drop(stdin);
    // Clipboard owners must remain alive until another application requests/replaces the
    // selection. Reap them on a background thread; never block the Tauri command on this.
    thread::spawn(move || { let _ = child.wait(); });
    *last_clipboard().lock().unwrap() = text.to_string();
    true
}

fn clip_image_dir() -> PathBuf {
    let dir = data_dir().join("clips");
    let _ = fs::create_dir_all(&dir);
    dir
}

fn clip_image_path(name: &str) -> Option<PathBuf> {
    let path = Path::new(name);
    if path.components().count() != 1 || path.file_name()?.to_str()? != name {
        return None;
    }
    Some(clip_image_dir().join(path))
}

fn image_hash(bytes: &[u8]) -> u64 {
    let mut hash = 0xcbf29ce484222325u64;
    for byte in bytes {
        hash ^= *byte as u64;
        hash = hash.wrapping_mul(0x100000001b3);
    }
    hash
}

fn build_image_clip(
    id: u64,
    pinned: bool,
    at: u64,
    bytes: Vec<u8>,
    stored_name: Option<String>,
) -> Option<ClipEntry> {
    if bytes.is_empty() || bytes.len() > 20 * 1024 * 1024 {
        return None;
    }
    let decoded = image::load_from_memory(&bytes).ok()?;
    let (width, height) = (decoded.width(), decoded.height());
    if width == 0 || height == 0 || (width as u64) * (height as u64) > 40_000_000 {
        return None;
    }

    let mut png_cursor = Cursor::new(Vec::new());
    decoded.write_to(&mut png_cursor, ImageFormat::Png).ok()?;
    let png = png_cursor.into_inner();

    let thumbnail = decoded.thumbnail(200, 60).to_rgba8();
    let mut thumb_cursor = Cursor::new(Vec::new());
    image::DynamicImage::ImageRgba8(thumbnail)
        .write_to(&mut thumb_cursor, ImageFormat::Png)
        .ok()?;

    let filename = stored_name.unwrap_or_else(|| format!("{:016x}.png", image_hash(&png)));
    Some(ClipEntry {
        id,
        text: String::new(),
        pinned,
        at,
        image: Some(filename),
        preview: format!("{width} × {height}"),
        thumb: BASE64.encode(thumb_cursor.into_inner()),
        image_bytes: Some(png),
    })
}

fn clipboard_read_image() -> Option<Vec<u8>> {
    let candidates: Vec<(&str, Vec<&str>)> = if is_wayland() {
        vec![
            ("wl-paste", vec!["--no-newline", "--type", "image/png"]),
            ("wl-paste", vec!["--no-newline", "--type", "image/jpeg"]),
        ]
    } else {
        vec![
            ("xclip", vec!["-selection", "clipboard", "-o", "-t", "image/png"]),
            ("xclip", vec!["-selection", "clipboard", "-o", "-t", "image/jpeg"]),
        ]
    };

    for (program, args) in candidates {
        let result = Command::new(program)
            .args(args)
            .stdin(Stdio::null())
            .stderr(Stdio::null())
            .output()
            .ok()?;
        if result.status.success() && !result.stdout.is_empty() {
            return Some(result.stdout);
        }
    }
    None
}

fn clipboard_read_file_urls() -> Option<String> {
    let (program, args): (&str, &[&str]) = if is_wayland() {
        ("wl-paste", &["--no-newline", "--type", "text/uri-list"])
    } else {
        ("xclip", &["-selection", "clipboard", "-o", "-t", "text/uri-list"])
    };
    let value = output(program, args)?;
    let urls = value.lines().filter(|line| {
        let line = line.trim();
        !line.is_empty() && !line.starts_with('#')
    }).collect::<Vec<_>>();
    if urls.is_empty() { None } else { Some(urls.join("\n")) }
}

fn emit_clip_preview(text: &str) {
    let preview = text.chars().take(400).collect::<String>()
        .split_whitespace().collect::<Vec<_>>().join(" ");
    emit("clip", json!({ "kind": "text", "text": text, "preview": preview }));
}

fn clip_payload() -> Value {
    Value::Array(clips().lock().unwrap().iter().map(|clip| {
        if clip.image.is_some() {
            json!({
                "id": clip.id, "kind": "image", "preview": clip.preview,
                "len": 0, "pinned": clip.pinned, "thumb": clip.thumb
            })
        } else {
            let preview = clip.text.chars().take(400).collect::<String>()
                .split_whitespace().collect::<Vec<_>>().join(" ");
            json!({
                "id": clip.id, "kind": "text", "preview": preview,
                "len": clip.text.chars().count(), "pinned": clip.pinned
            })
        }
    }).collect())
}

fn save_clips() {
    let mut pinned: Vec<ClipEntry> = clips().lock().unwrap().iter()
        .filter(|clip| clip.pinned).cloned().collect();
    let dir = clip_image_dir();
    let mut keep = HashSet::new();

    for clip in &mut pinned {
        if let (Some(name), Some(bytes)) = (clip.image.as_deref(), clip.image_bytes.as_deref()) {
            if let Some(path) = clip_image_path(name) {
                if !path.exists() {
                    let _ = write_atomic(&path, bytes);
                }
                keep.insert(name.to_string());
            }
        } else if let Some(name) = clip.image.as_deref() {
            if clip_image_path(name).is_some_and(|path| path.exists()) {
                keep.insert(name.to_string());
            }
        }
        clip.image_bytes = None;
    }

    write_json(&data_dir().join("clips.json"), &pinned);
    if let Ok(entries) = fs::read_dir(&dir) {
        for entry in entries.flatten() {
            let name = entry.file_name().to_string_lossy().to_string();
            if !keep.contains(&name) {
                let _ = fs::remove_file(entry.path());
            }
        }
    }
}

fn enforce_clip_limits(list: &mut Vec<ClipEntry>) {
    list.sort_by(|a, b| b.pinned.cmp(&a.pinned).then(b.at.cmp(&a.at)));
    let mut loose = 0usize;
    let mut loose_images = 0usize;
    list.retain(|clip| {
        if clip.pinned { return true; }
        loose += 1;
        if clip.image.is_some() {
            loose_images += 1;
            if loose_images > 10 { return false; }
        }
        loose <= 30
    });
}

fn add_clip(text: String) {
    let text = text.trim_end_matches('\0').to_string();
    if text.trim().is_empty() || text.chars().count() > 100_000 { return; }
    {
        let mut list = clips().lock().unwrap();
        if let Some(found) = list.iter_mut().find(|clip| clip.image.is_none() && clip.text == text) {
            found.at = now_ms();
        } else {
            let id = list.iter().map(|clip| clip.id).max().unwrap_or(0) + 1;
            list.push(ClipEntry {
                id, text, pinned: false, at: now_ms(),
                image: None, preview: String::new(), thumb: String::new(), image_bytes: None,
            });
        }
        enforce_clip_limits(&mut list);
    }
    save_clips();
    emit("clips", clip_payload());
}

fn add_clip_image(bytes: Vec<u8>) {
    let next_id = clips().lock().unwrap().iter().map(|clip| clip.id).max().unwrap_or(0) + 1;
    let Some(entry) = build_image_clip(next_id, false, now_ms(), bytes, None) else { return; };
    {
        let mut list = clips().lock().unwrap();
        if let Some(filename) = entry.image.as_deref() {
            if let Some(found) = list.iter_mut().find(|clip| clip.image.as_deref() == Some(filename)) {
                found.at = now_ms();
                found.image_bytes = entry.image_bytes;
                found.preview = entry.preview;
                found.thumb = entry.thumb;
            } else {
                list.push(entry);
            }
        }
        enforce_clip_limits(&mut list);
    }
    save_clips();
    emit("clips", clip_payload());
}

fn clips_load() {
    if let Ok(raw) = fs::read_to_string(data_dir().join("clips.json")) {
        if let Ok(saved) = serde_json::from_str::<Vec<ClipEntry>>(&raw) {
            let mut loaded = Vec::new();
            for clip in saved {
                if let Some(name) = clip.image.as_deref() {
                    let Some(path) = clip_image_path(name) else { continue; };
                    let Ok(bytes) = fs::read(path) else { continue; };
                    if let Some(mut image) = build_image_clip(clip.id, clip.pinned, clip.at, bytes, Some(name.to_string())) {
                        loaded.push(image);
                    }
                } else if !clip.text.is_empty() {
                    loaded.push(ClipEntry { image: None, image_bytes: None, preview: String::new(), thumb: String::new(), ..clip });
                }
            }
            *clips().lock().unwrap() = loaded;
        }
    }

    let current_text = clipboard_read().filter(|text| !text.trim().is_empty());
    let current_image = if current_text.is_none() {
        clipboard_read_image().map(|bytes| format!("image:{:016x}", image_hash(&bytes)))
    } else { None };
    *last_clipboard().lock().unwrap() = current_text.or(current_image).unwrap_or_default();
}

fn copy_text_impl(text: &str) -> bool {
    if !clipboard_write(text, false) { return false; }
    if setting_flag("clipHistory") { add_clip(text.to_string()); }
    if setting_flag("copyPreview") { emit_clip_preview(text); }
    true
}

fn copy_secret_impl(text: &str) -> bool {
    if !clipboard_write(text, true) { return false; }
    let expected = text.to_string();
    thread::spawn(move || {
        thread::sleep(Duration::from_secs(30));
        if clipboard_read().as_deref() == Some(expected.as_str()) {
            let _ = clipboard_write("", true);
        }
    });
    true
}

fn clipboard_watch() {
    thread::spawn(|| loop {
        thread::sleep(Duration::from_millis(900));

        if let Some(text) = clipboard_read().filter(|text| !text.trim().is_empty()) {
            let changed = {
                let mut last = last_clipboard().lock().unwrap();
                if *last == text { false } else { *last = text.clone(); true }
            };
            if changed {
                if setting_flag("clipHistory") { add_clip(text.clone()); }
                if setting_flag("copyPreview") { emit_clip_preview(&text); }
            }
            continue;
        }

        if let Some(image) = clipboard_read_image() {
            let signature = format!("image:{:016x}", image_hash(&image));
            let changed = {
                let mut last = last_clipboard().lock().unwrap();
                if *last == signature { false } else { *last = signature; true }
            };
            if changed {
                if setting_flag("clipHistory") { add_clip_image(image); }
                if setting_flag("copyPreview") { emit("clip", json!({ "kind": "image" })); }
            }
            continue;
        }

        if let Some(files) = clipboard_read_file_urls() {
            let signature = format!("files:{files}");
            let changed = {
                let mut last = last_clipboard().lock().unwrap();
                if *last == signature { false } else { *last = signature; true }
            };
            if changed && setting_flag("copyPreview") {
                let count = files.lines().count();
                emit("clip", json!({ "kind": "files", "preview": format!("{count} file(s)") }));
            }
        }
    });
}


// ---------------------------------------------------------------- notes and Secret Service vault (GNOME Keyring / KWallet via libsecret)
#[derive(Clone, Serialize, Deserialize)]
struct Note {
    id: u64,
    text: String,
    #[serde(default)]
    pinned: bool,
    at: u64,
    #[serde(default)]
    remind: u64,
    #[serde(default)]
    fired: bool,
}

fn notes() -> &'static Mutex<Vec<Note>> {
    NOTES.get_or_init(|| Mutex::new(Vec::new()))
}

fn notes_load() {
    if let Ok(raw) = fs::read_to_string(data_dir().join("notes.json")) {
        if let Ok(list) = serde_json::from_str::<Vec<Note>>(&raw) { *notes().lock().unwrap() = list; }
    }
}

fn notes_payload(list: &[Note]) -> Value {
    serde_json::to_value(list).unwrap_or(Value::Array(Vec::new()))
}

fn notes_save_and_emit(list: &mut Vec<Note>) -> Value {
    list.sort_by(|a, b| b.pinned.cmp(&a.pinned).then(b.at.cmp(&a.at)));
    write_json(&data_dir().join("notes.json"), list);
    let value = notes_payload(list);
    emit("notes", value.clone());
    value
}

fn note_save_impl(id: Option<u64>, text: &str, remind: u64) -> Value {
    let text = text.trim();
    if text.is_empty() || text.len() > 20_000 { return notes_payload(&notes().lock().unwrap()); }
    let mut list = notes().lock().unwrap();
    if let Some(found) = id.and_then(|id| list.iter_mut().find(|n| n.id == id)) {
        found.text = text.to_string();
        found.remind = remind;
        found.fired = false;
    } else {
        let id = list.iter().map(|n| n.id).max().unwrap_or(0) + 1;
        list.push(Note { id, text: text.to_string(), pinned: false, at: now_ms(), remind, fired: false });
    }
    if list.len() > 300 {
        if let Some(index) = list.iter().rposition(|n| !n.pinned) { list.remove(index); }
    }
    notes_save_and_emit(&mut list)
}

fn note_pin_impl(id: u64) -> Value {
    let mut list = notes().lock().unwrap();
    if let Some(note) = list.iter_mut().find(|n| n.id == id) { note.pinned = !note.pinned; }
    notes_save_and_emit(&mut list)
}

fn note_fired_impl(id: u64) -> Value {
    let mut list = notes().lock().unwrap();
    if let Some(note) = list.iter_mut().find(|n| n.id == id) { note.fired = true; }
    notes_save_and_emit(&mut list)
}

fn note_remove_impl(id: u64) -> Value {
    let mut list = notes().lock().unwrap();
    list.retain(|n| n.id != id);
    notes_save_and_emit(&mut list)
}

#[derive(Clone, Serialize, Deserialize)]
struct VaultItem {
    id: u64,
    site: String,
    user: String,
    at: u64,
}

fn vault() -> &'static Mutex<Vec<VaultItem>> {
    VAULT.get_or_init(|| Mutex::new(Vec::new()))
}

fn vault_load() {
    if let Ok(raw) = fs::read_to_string(data_dir().join("vault-index.json")) {
        if let Ok(items) = serde_json::from_str::<Vec<VaultItem>>(&raw) { *vault().lock().unwrap() = items; }
    }
}

fn secret_tool_available() -> bool { command_exists("secret-tool") }

fn secret_service_available() -> bool {
    if !secret_tool_available() { return false; }

    if command_exists("busctl") {
        if Command::new("busctl")
            .args(["--user", "status", "org.freedesktop.secrets"])
            .stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null())
            .status().map(|status| status.success()).unwrap_or(false)
        {
            return true;
        }
    }
    if command_exists("gdbus") {
        return Command::new("gdbus")
            .args(["introspect", "--session", "--dest", "org.freedesktop.secrets", "--object-path", "/org/freedesktop/secrets"])
            .stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null())
            .status().map(|status| status.success()).unwrap_or(false);
    }
    false
}

fn vault_payload() -> Value {
    let locked = !secret_service_available();
    let items = vault().lock().unwrap();
    json!({
        "locked": locked,
        "items": items.iter().map(|v| json!({
            "id": v.id, "site": v.site, "user": v.user,
            "len": if locked { 0 } else { secret_lookup(v.id).map(|s| s.chars().count()).unwrap_or(0) },
            "at": v.at
        })).collect::<Vec<_>>()
    })
}

fn secret_store(id: u64, secret: &str, label: &str) -> bool {
    let mut child = match Command::new("secret-tool")
        .args(["store", "--label", label, "application", "peeky", "entry", &id.to_string()])
        .stdin(Stdio::piped()).stdout(Stdio::null()).stderr(Stdio::null()).spawn() {
            Ok(c) => c, Err(_) => return false,
        };
    if let Some(mut stdin) = child.stdin.take() {
        if stdin.write_all(secret.as_bytes()).is_err() {
            let _ = child.kill();
            return false;
        }
    }

    // Secret Service can wait for an unlock prompt. Never let a missing desktop
    // keyring agent block Peeky's UI command forever.
    let started = Instant::now();
    loop {
        match child.try_wait() {
            Ok(Some(status)) => return status.success(),
            Ok(None) if started.elapsed() < Duration::from_secs(12) => {
                thread::sleep(Duration::from_millis(50));
            }
            _ => {
                let _ = child.kill();
                let _ = child.wait();
                log("Secret Service store timed out or could not be reached");
                return false;
            }
        }
    }
}

fn secret_lookup(id: u64) -> Option<String> {
    output("secret-tool", &["lookup", "application", "peeky", "entry", &id.to_string()])
}

fn vault_save_impl(site: &str, user: &str, pass: &str) -> u64 {
    let site = site.trim();
    let user = user.trim();
    if pass.is_empty() || pass.len() > 1000 || site.len() > 300 || user.len() > 300 || !secret_service_available() {
        return 0;
    }
    let mut items = vault().lock().unwrap();
    let id = items.iter().map(|v| v.id).max().unwrap_or(0) + 1;
    if !secret_store(id, pass, &format!("Peeky password for {}", if site.is_empty() { "Untitled" } else { site })) {
        return 0;
    }
    items.push(VaultItem { id, site: if site.is_empty() { "Untitled".into() } else { site.into() }, user: user.into(), at: now_ms() });
    write_json(&data_dir().join("vault-index.json"), &*items);
    drop(items);
    emit("vault", vault_payload());
    id
}

fn vault_edit_impl(id: u64, site: &str, user: &str) -> bool {
    let site = site.trim();
    let user = user.trim();
    if site.len() > 300 || user.len() > 300 { return false; }
    let mut items = vault().lock().unwrap();
    let Some(item) = items.iter_mut().find(|entry| entry.id == id) else { return false; };
    item.site = if site.is_empty() { "Untitled".into() } else { site.into() };
    item.user = user.into();
    write_json(&data_dir().join("vault-index.json"), &*items);
    drop(items);
    emit("vault", vault_payload());
    true
}

fn vault_remove_impl(id: u64) -> bool {
    if !secret_service_available() { return false; }
    let _ = Command::new("secret-tool").args(["clear", "application", "peeky", "entry", &id.to_string()])
        .stdout(Stdio::null()).stderr(Stdio::null()).status();
    let mut items = vault().lock().unwrap();
    items.retain(|v| v.id != id);
    write_json(&data_dir().join("vault-index.json"), &*items);
    drop(items);
    emit("vault", vault_payload());
    true
}

fn vault_reveal_impl(id: u64) -> Option<String> {
    secret_lookup(id)
}

fn vault_copy_impl(id: u64, what: &str) -> bool {
    let value = {
        let items = vault().lock().unwrap();
        let Some(item) = items.iter().find(|v| v.id == id) else { return false; };
        if what == "user" { item.user.clone() }
        else if what == "pass" { match secret_lookup(id) { Some(s) => s, None => return false } }
        else { return false; }
    };
    copy_secret_impl(&value)
}

fn vault_generate_impl(len: usize, symbols: bool) -> String {
    const ALPHA: &[u8] = b"abcdefghijkmnopqrstuvwxyzABCDEFGHJKLMNPQRSTUVWXYZ23456789";
    const EXTRA: &[u8] = b"!@#$%^&*()-_=+[]{}:,.?";
    let mut random = vec![0u8; len.clamp(8, 64)];
    if fs::File::open("/dev/urandom").and_then(|mut f| f.read_exact(&mut random)).is_err() {
        return String::new();
    }
    let mut alphabet = ALPHA.to_vec();
    if symbols { alphabet.extend_from_slice(EXTRA); }
    random.iter().map(|b| alphabet[*b as usize % alphabet.len()] as char).collect()
}

// ---------------------------------------------------------------- file shelf, downloads and desktop integration
#[derive(Clone, Serialize, Deserialize)]
struct ShelfItem {
    path: String,
    name: String,
    icon: Option<String>,
}

fn shelf() -> &'static Mutex<Vec<ShelfItem>> {
    SHELF.get_or_init(|| Mutex::new(Vec::new()))
}

fn shelf_payload() -> Value {
    Value::Array(shelf().lock().unwrap().iter().map(|f| json!({
        "path": f.path, "name": f.name, "icon": f.icon
    })).collect())
}

fn shelf_save() {
    write_json(&data_dir().join("shelf.json"), &*shelf().lock().unwrap());
    emit("shelf", shelf_payload());
}

fn shelf_load() {
    if let Ok(raw) = fs::read_to_string(data_dir().join("shelf.json")) {
        if let Ok(items) = serde_json::from_str::<Vec<ShelfItem>>(&raw) {
            *shelf().lock().unwrap() = items.into_iter().filter(|i| Path::new(&i.path).exists()).collect();
        }
    }
}

fn shelf_put(path: &Path) -> bool {
    if !path.is_file() { return false; }
    let path = match path.canonicalize() { Ok(p) => p, Err(_) => return false };
    let text = path.to_string_lossy().to_string();
    let mut list = shelf().lock().unwrap();
    if list.iter().any(|i| i.path == text) { return false; }
    let name = path.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_else(|| text.clone());
    list.push(ShelfItem { path: text, name, icon: None });
    if list.len() > 40 { list.remove(0); }
    true
}

fn shelf_add_impl(paths: Vec<String>) -> Value {
    let mut changed = false;
    for p in paths {
        if !p.is_empty() { changed |= shelf_put(Path::new(&p)); }
    }
    if changed { shelf_save(); }
    shelf_payload()
}

fn safe_filename(name: &str) -> String {
    let cleaned: String = name.chars().filter(|c| !c.is_control() && !['/', '\\', ':'].contains(c)).take(120).collect();
    if cleaned.trim().is_empty() { format!("peeky-{}", now_ms()) } else { cleaned }
}

fn shelf_add_data_impl(name: &str, mime: &str, data: &[u8]) -> Value {
    if data.is_empty() || data.len() > MAX_SHELF_BYTES { return shelf_payload(); }
    let mut filename = safe_filename(name);
    if Path::new(&filename).extension().is_none() {
        let ext = match mime.to_ascii_lowercase().as_str() {
            "image/png" => "png", "image/jpeg" => "jpg", "image/webp" => "webp",
            "image/gif" => "gif", "application/pdf" => "pdf", _ => "bin"
        };
        filename.push('.');
        filename.push_str(ext);
    }
    let dir = data_dir().join("shelf-files");
    if fs::create_dir_all(&dir).is_err() { return shelf_payload(); }
    let dest = dir.join(format!("{}-{}", now_ms(), filename));
    if fs::write(&dest, data).is_ok() {
        let _ = shelf_put(&dest);
        shelf_save();
    }
    shelf_payload()
}

fn percent_decode(input: &str) -> String {
    let bytes = input.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' && i + 2 < bytes.len() {
            if let (Some(a), Some(b)) = ((bytes[i + 1] as char).to_digit(16), (bytes[i + 2] as char).to_digit(16)) {
                out.push((a * 16 + b) as u8);
                i += 3;
                continue;
            }
        }
        out.push(bytes[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

fn shelf_add_url_impl(url: &str) -> Value {
    let url = url.trim();
    if let Some(rest) = url.strip_prefix("file://") {
        let path = percent_decode(rest);
        return shelf_add_impl(vec![path]);
    }
    if url.to_ascii_lowercase().starts_with("data:image/") {
        if let Some((header, body)) = url.split_once(',') {
            let mime = header.strip_prefix("data:").unwrap_or("").split(';').next().unwrap_or("");
            let bytes = if header.ends_with(";base64") {
                use base64::Engine as _;
                base64::engine::general_purpose::STANDARD.decode(body).ok()
            } else {
                Some(percent_decode(body).into_bytes())
            };
            if let Some(bytes) = bytes {
                return shelf_add_data_impl("image", mime, &bytes);
            }
        }
        return shelf_payload();
    }
    if !(url.starts_with("https://") || url.starts_with("http://")) || url.len() > 4000
        || url.chars().any(|c| c.is_control() || c.is_whitespace()) || !command_exists("curl") {
        return shelf_payload();
    }
    let name = url.split(['?', '#']).next().unwrap_or("download").rsplit('/').next().unwrap_or("download");
    let filename = safe_filename(&percent_decode(name));
    let dir = data_dir().join("shelf-files");
    if fs::create_dir_all(&dir).is_err() { return shelf_payload(); }
    let dest = dir.join(format!("{}-{}", now_ms(), filename));
    let status = Command::new("curl").args([
        "--fail", "--location", "--max-filesize", "209715200", "--connect-timeout", "10",
        "--max-time", "45", "--output"
    ]).arg(&dest).arg(url).stdout(Stdio::null()).stderr(Stdio::null()).status();
    if status.map(|s| s.success()).unwrap_or(false) {
        let _ = shelf_put(&dest);
        shelf_save();
    } else {
        let _ = fs::remove_file(dest);
    }
    shelf_payload()
}

fn shelf_remove_impl(path: &str) {
    shelf().lock().unwrap().retain(|f| f.path != path);
    shelf_save();
}

fn shelf_clear_impl() {
    shelf().lock().unwrap().clear();
    shelf_save();
}

fn shelf_reveal_impl(path: &str) {
    let target = Path::new(path).parent().unwrap_or(Path::new(path));
    open_path(target.to_string_lossy().as_ref());
}

fn shelf_drag_impl(_paths: Vec<String>) {
    // GTK/WebKit file drag-out is not exposed by the current Tauri 2 window API. Files remain
    // available through the shelf's reveal/open actions instead of faking a drag operation.
}

fn size_label(bytes: u64) -> String {
    if bytes >= 1024 * 1024 { format!("{:.1} MB", bytes as f64 / (1024.0 * 1024.0)) }
    else if bytes >= 1024 { format!("{:.0} KB", bytes as f64 / 1024.0) }
    else { format!("{bytes} B") }
}

fn downloads_watch() {
    thread::spawn(|| {
        let dir = home_dir().join("Downloads");
        let mut known = HashSet::<PathBuf>::new();
        if let Ok(entries) = fs::read_dir(&dir) {
            for entry in entries.flatten() { known.insert(entry.path()); }
        }
        loop {
            thread::sleep(Duration::from_secs(2));
            let Ok(entries) = fs::read_dir(&dir) else { continue; };
            for entry in entries.flatten() {
                let path = entry.path();
                if known.contains(&path) { continue; }
                let Ok(meta) = entry.metadata() else { continue; };
                if !meta.is_file() || meta.len() == 0 { continue; }
                thread::sleep(Duration::from_millis(700));
                let Ok(after) = fs::metadata(&path) else { continue; };
                if after.len() != meta.len() { continue; }
                known.insert(path.clone());
                let name = path.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
                let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("").to_ascii_lowercase();
                let risky = ["deb", "rpm", "appimage", "sh", "run", "bin", "desktop"].contains(&ext.as_str());
                emit("download", json!({
                    "name": name, "size": size_label(after.len()), "risky": risky,
                    "path": path.to_string_lossy(), "icon": Value::Null
                }));
            }
        }
    });
}

fn open_download_impl(path: &str) {
    let ext = Path::new(path).extension().and_then(|e| e.to_str()).unwrap_or("").to_ascii_lowercase();
    if ["deb", "rpm", "appimage", "sh", "run", "bin", "desktop"].contains(&ext.as_str()) {
        shelf_reveal_impl(path);
    } else {
        open_path(path);
    }
}

// ---------------------------------------------------------------- screenshot/OCR
fn screenshot_path() -> PathBuf {
    let dir = std::env::temp_dir().join("peeky");
    let _ = fs::create_dir_all(&dir);
    dir.join(format!("capture-{}.png", now_ms()))
}

fn run_capture_command(mut command: Command, dest: &Path, tool: &str) -> Result<Vec<u8>, String> {
    let status = command
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map_err(|error| format!("Could not start {tool}: {error}"))?;
    if !status.success() {
        return Err(format!("{tool} did not complete the capture"));
    }
    fs::read(dest).map_err(|error| format!("{tool} did not create a readable screenshot: {error}"))
}

fn capture_selection_slurp(dest: &Path) -> Result<Vec<u8>, String> {
    if !command_exists("slurp") || !command_exists("grim") {
        return Err("For this Wayland session install grim and slurp, or use the desktop screenshot tool".into());
    }
    let selection = Command::new("slurp")
        .stdin(Stdio::null())
        .stderr(Stdio::null())
        .output()
        .map_err(|error| format!("Could not start slurp: {error}"))?;
    if !selection.status.success() {
        return Err("Selection cancelled".into());
    }
    let geometry = String::from_utf8_lossy(&selection.stdout).trim().to_string();
    if geometry.is_empty() {
        return Err("Selection cancelled".into());
    }
    let mut command = Command::new("grim");
    command.args(["-g", &geometry]).arg(dest);
    run_capture_command(command, dest, "grim")
}

fn desktop_name() -> String {
    std::env::var("XDG_CURRENT_DESKTOP")
        .unwrap_or_default()
        .to_ascii_lowercase()
}

fn capture_selection() -> Result<Vec<u8>, String> {
    let dest = screenshot_path();
    let desktop = desktop_name();
    let result = if is_wayland() {
        if desktop.contains("kde") && command_exists("spectacle") {
            let mut command = Command::new("spectacle");
            command.args(["--region", "--background", "--nonotify", "--output"]).arg(&dest);
            run_capture_command(command, &dest, "Spectacle")
        } else if desktop.contains("gnome") && command_exists("gnome-screenshot") {
            let mut command = Command::new("gnome-screenshot");
            command.args(["--area", "--file"]).arg(&dest);
            run_capture_command(command, &dest, "GNOME Screenshot")
        } else if command_exists("grim") && command_exists("slurp") {
            capture_selection_slurp(&dest)
        } else if command_exists("spectacle") {
            let mut command = Command::new("spectacle");
            command.args(["--region", "--background", "--nonotify", "--output"]).arg(&dest);
            run_capture_command(command, &dest, "Spectacle")
        } else if command_exists("gnome-screenshot") {
            let mut command = Command::new("gnome-screenshot");
            command.args(["--area", "--file"]).arg(&dest);
            run_capture_command(command, &dest, "GNOME Screenshot")
        } else {
            Err("No Wayland selector is installed. Install grim + slurp, GNOME Screenshot, or Spectacle".into())
        }
    } else if command_exists("flameshot") {
        let mut command = Command::new("flameshot");
        command.args(["gui", "--path"]).arg(&dest);
        run_capture_command(command, &dest, "Flameshot")
    } else if command_exists("gnome-screenshot") {
        let mut command = Command::new("gnome-screenshot");
        command.args(["--area", "--file"]).arg(&dest);
        run_capture_command(command, &dest, "GNOME Screenshot")
    } else if command_exists("spectacle") {
        let mut command = Command::new("spectacle");
        command.args(["--region", "--background", "--nonotify", "--output"]).arg(&dest);
        run_capture_command(command, &dest, "Spectacle")
    } else if command_exists("maim") {
        let mut command = Command::new("maim");
        command.arg("-s").arg(&dest);
        run_capture_command(command, &dest, "maim")
    } else {
        Err("No X11 screen selector is installed. Install Flameshot, GNOME Screenshot, Spectacle, or maim".into())
    };
    let _ = fs::remove_file(&dest);
    result
}

fn capture_fullscreen() -> Result<Vec<u8>, String> {
    let dest = screenshot_path();
    let desktop = desktop_name();
    let result = if is_wayland() && desktop.contains("kde") && command_exists("spectacle") {
        let mut command = Command::new("spectacle");
        command.args(["--fullscreen", "--background", "--nonotify", "--output"]).arg(&dest);
        run_capture_command(command, &dest, "Spectacle")
    } else if is_wayland() && desktop.contains("gnome") && command_exists("gnome-screenshot") {
        let mut command = Command::new("gnome-screenshot");
        command.args(["--file"]).arg(&dest);
        run_capture_command(command, &dest, "GNOME Screenshot")
    } else if is_wayland() && command_exists("grim") {
        let mut command = Command::new("grim");
        command.arg(&dest);
        run_capture_command(command, &dest, "grim")
    } else if command_exists("spectacle") {
        let mut command = Command::new("spectacle");
        command.args(["--fullscreen", "--background", "--nonotify", "--output"]).arg(&dest);
        run_capture_command(command, &dest, "Spectacle")
    } else if command_exists("gnome-screenshot") {
        let mut command = Command::new("gnome-screenshot");
        command.args(["--file"]).arg(&dest);
        run_capture_command(command, &dest, "GNOME Screenshot")
    } else if command_exists("maim") {
        let mut command = Command::new("maim");
        command.arg(&dest);
        run_capture_command(command, &dest, "maim")
    } else {
        Err("No full-screen capture tool is installed. Install grim, GNOME Screenshot, Spectacle, or maim".into())
    };
    let _ = fs::remove_file(&dest);
    result
}

fn clipboard_write_png(bytes: &[u8]) -> bool {
    let (program, args): (&str, &[&str]) = if is_wayland() {
        ("wl-copy", &["--type", "image/png"])
    } else {
        ("xclip", &["-selection", "clipboard", "-t", "image/png", "-i"])
    };
    let Ok(mut child) = Command::new(program).args(args)
        .stdin(Stdio::piped()).stdout(Stdio::null()).stderr(Stdio::null()).spawn() else {
            return false;
        };
    let Some(mut stdin) = child.stdin.take() else { let _ = child.kill(); return false; };
    if stdin.write_all(bytes).is_err() { let _ = child.kill(); return false; }
    drop(stdin);
    *last_clipboard().lock().unwrap() = format!("image:{:016x}", image_hash(bytes));
    thread::spawn(move || { let _ = child.wait(); });
    true
}

fn ocr_start() {
    match capture_selection() {
        Ok(png) => {
            let encoded = {
                const TABLE: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
                let mut out = String::with_capacity((png.len() + 2) / 3 * 4);
                for chunk in png.chunks(3) {
                    let a = chunk[0]; let b = *chunk.get(1).unwrap_or(&0); let c = *chunk.get(2).unwrap_or(&0);
                    out.push(TABLE[(a >> 2) as usize] as char);
                    out.push(TABLE[(((a & 3) << 4) | (b >> 4)) as usize] as char);
                    out.push(if chunk.len() > 1 { TABLE[(((b & 15) << 2) | (c >> 6)) as usize] as char } else { '=' });
                    out.push(if chunk.len() > 2 { TABLE[(c & 63) as usize] as char } else { '=' });
                }
                out
            };
            emit("ocr-read", json!({ "png": encoded }));
        }
        Err(why) => emit("ocr", json!({ "ok": false, "why": why })),
    }
}

fn ocr_done_impl(text: &str, _conf: f64) {
    let clean = text.trim();
    if clean.is_empty() {
        emit("ocr", json!({ "ok": false, "why": "No text found" }));
    } else if copy_text_impl(clean) {
        emit("ocr", json!({ "ok": true, "text": clean }));
    } else {
        emit("ocr", json!({ "ok": false, "why": "Could not write to the clipboard" }));
    }
}

// ---------------------------------------------------------------- media and audio through MPRIS / PipeWire / PulseAudio command-line clients
fn playerctl(args: &[&str]) {
    let _ = Command::new("playerctl").args(args).stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null()).status();
}

fn media_metadata() -> Value {
    let raw = output("playerctl", &["metadata", "--format", "{{status}}\n{{title}}\n{{artist}}\n{{position}}\n{{mpris:length}}\n{{playerName}}"]);
    let Some(raw) = raw else {
        return json!({ "active": false, "playing": false, "title": "", "artist": "", "pos": 0, "dur": 0, "at": 0, "key": "", "video": false });
    };
    let mut lines = raw.lines();
    let status = lines.next().unwrap_or("");
    let title = lines.next().unwrap_or("").to_string();
    let artist = lines.next().unwrap_or("").to_string();
    let pos = lines.next().unwrap_or("0").parse::<f64>().unwrap_or(0.0);
    let duration = lines.next().unwrap_or("0").parse::<f64>().unwrap_or(0.0);
    let app = lines.next().unwrap_or("").to_string();
    json!({
        "active": true, "playing": status.eq_ignore_ascii_case("playing"),
        "title": title, "artist": artist, "pos": (pos * 1000.0) as u64,
        "dur": (duration / 1000.0) as u64, "at": now_ms(),
        "key": format!("{app}|{title}|{artist}"), "kind": "Music", "app": app, "video": false
    })
}

fn volume_state() -> (f32, bool) {
    if let Some(raw) = output("wpctl", &["get-volume", "@DEFAULT_AUDIO_SINK@"]) {
        let value = raw.split_whitespace().find_map(|w| w.parse::<f32>().ok()).unwrap_or(0.0);
        return (value.clamp(0.0, 1.0), raw.contains("MUTED"));
    }
    if let Some(raw) = output("pactl", &["get-sink-volume", "@DEFAULT_SINK@"]) {
        let value = raw.split_whitespace().find_map(|w| w.strip_suffix('%').and_then(|n| n.parse::<f32>().ok())).unwrap_or(0.0) / 100.0;
        let muted = output("pactl", &["get-sink-mute", "@DEFAULT_SINK@"]).map(|s| s.contains("yes")).unwrap_or(false);
        return (value.clamp(0.0, 1.0), muted);
    }
    (0.0, false)
}

fn set_volume_value(value: f32) {
    let value = value.clamp(0.0, 1.0);
    if command_exists("wpctl") {
        let _ = Command::new("wpctl").args(["set-volume", "@DEFAULT_AUDIO_SINK@", &format!("{:.2}", value)]).status();
    } else {
        let _ = Command::new("pactl").args(["set-sink-volume", "@DEFAULT_SINK@", &format!("{:.0}%", value * 100.0)]).status();
    }
}

fn toggle_mute_value() {
    if command_exists("wpctl") {
        let _ = Command::new("wpctl").args(["set-mute", "@DEFAULT_AUDIO_SINK@", "toggle"]).status();
    } else {
        let _ = Command::new("pactl").args(["set-sink-mute", "@DEFAULT_SINK@", "toggle"]).status();
    }
}

fn net_state() -> bool {
    fs::read_dir("/sys/class/net").ok().map(|entries| entries.flatten().any(|entry| {
        let name = entry.file_name().to_string_lossy().to_string();
        name != "lo" && fs::read_to_string(entry.path().join("operstate")).map(|s| s.trim() == "up").unwrap_or(false)
    })).unwrap_or(false)
}

fn x11_window_geometry(id: &str) -> Option<(i32, i32, i32, i32)> {
    let raw = output("xdotool", &["getwindowgeometry", "--shell", id])?;
    let mut x = None;
    let mut y = None;
    let mut width = None;
    let mut height = None;
    for line in raw.lines() {
        if let Some(value) = line.strip_prefix("X=") { x = value.parse::<i32>().ok(); }
        if let Some(value) = line.strip_prefix("Y=") { y = value.parse::<i32>().ok(); }
        if let Some(value) = line.strip_prefix("WIDTH=") { width = value.parse::<i32>().ok(); }
        if let Some(value) = line.strip_prefix("HEIGHT=") { height = value.parse::<i32>().ok(); }
    }
    Some((x?, y?, width?, height?))
}

// Detect the external X11 window underneath Peeky. When Peeky temporarily owns focus for
// a text field, retain the last external window instead of treating Peeky as the covered app.
fn x11_cover_state(last_external: &mut Option<String>) -> Option<(bool, bool)> {
    if is_wayland() || !X11_POINTER_ACTIVE.load(Ordering::SeqCst) { return None; }
    let active = output("xdotool", &["getactivewindow"])?;
    let active_pid = x11_window_pid(&active);
    let target = if active_pid == Some(std::process::id()) {
        last_external.clone()?
    } else {
        let title = output("xdotool", &["getwindowname", &active]).unwrap_or_default();
        if title.trim().is_empty() {
            *last_external = None;
            return Some((false, false));
        }
        *last_external = Some(active.clone());
        active
    };

    let title = output("xdotool", &["getwindowname", &target]).unwrap_or_default();
    if title.trim().is_empty() {
        *last_external = None;
        return Some((false, false));
    }
    let Some((x, y, width, height)) = x11_window_geometry(&target) else {
        // The previously focused application may have been closed while Peeky had focus.
        // Do not leave a stale "covered" state hiding the island.
        *last_external = None;
        return Some((false, false));
    };
    if width <= 0 || height <= 0 { return Some((false, false)); }

    let Some(window) = APP.get().and_then(|app| app.get_webview_window("main")) else { return None; };
    let Ok(position) = window.outer_position() else { return None; };
    let Ok(size) = window.outer_size() else { return None; };
    let Ok(scale) = window.scale_factor() else { return None; };
    let center_x = position.x + size.width as i32 / 2;
    let center_y = position.y + (26.0 * scale.max(0.5)).round() as i32;

    let right = x.saturating_add(width);
    let bottom = y.saturating_add(height);
    // X11 geometry describes the client surface; a decorated application's title bar can
    // sit above it. Include a small top frame allowance so maximized browsers still count
    // as covering the pill and its top-edge controls.
    let decorated_top = y.saturating_sub(56);
    let covered = x <= center_x
        && center_x < right
        && decorated_top <= center_y
        && center_y < bottom;

    let monitors = window.available_monitors().ok()?;
    let Some(monitor) = monitors.into_iter().find(|monitor| {
        let p = monitor.position();
        let s = monitor.size();
        center_x >= p.x
            && center_x < p.x + s.width as i32
            && center_y >= p.y
            && center_y < p.y + s.height as i32
    }) else {
        return Some((covered, false));
    };
    let monitor_position = monitor.position();
    let monitor_size = monitor.size();
    let full = x <= monitor_position.x
        && y <= monitor_position.y
        && right >= monitor_position.x + monitor_size.width as i32
        && bottom >= monitor_position.y + monitor_size.height as i32;
    Some((covered, full))
}

fn poll_start() {
    thread::spawn(|| {
        let mut previous_media = Value::Null;
        let mut last_online: Option<bool> = None;
        let mut last_volume = String::new();
        let mut net_tick = 0u8;
        let mut volume_tick = 0u8;
        let mut media_tick = 0u8;
        let mut window_tick = 0u8;
        let mut last_external_window = None;
        let mut window_state_candidate: Option<(bool, bool)> = None;
        let mut window_state_sent: Option<(bool, bool)> = None;
        let mut window_state_count = 0u8;
        loop {
            thread::sleep(Duration::from_millis(250));
            media_tick = media_tick.wrapping_add(1);
            volume_tick = volume_tick.wrapping_add(1);
            net_tick = net_tick.wrapping_add(1);
            window_tick = window_tick.wrapping_add(1);
            if media_tick >= 2 {
                media_tick = 0;
                let state = media_metadata();
                if state != previous_media {
                    emit("helper", json!({ "t": "media", "active": state["active"], "playing": state["playing"], "title": state["title"], "artist": state["artist"], "pos": state["pos"], "dur": state["dur"], "at": state["at"], "key": state["key"], "kind": state["kind"], "app": state["app"] }));
                    previous_media = state;
                }
            }
            if volume_tick >= 2 {
                volume_tick = 0;
                let (level, mute) = volume_state();
                let key = format!("{level:.2}|{mute}");
                if key != last_volume {
                    last_volume = key;
                    emit("helper", json!({ "t": "vol", "level": level, "mute": mute }));
                }
                emit("levels", vec![0.0f32; 24]);
            }
            if net_tick >= 20 {
                net_tick = 0;
                let online = net_state();
                if last_online != Some(online) {
                    emit("helper", json!({ "t": "net", "online": online, "vpn": "", "proxy": std::env::var_os("http_proxy").is_some() || std::env::var_os("https_proxy").is_some(), "first": last_online.is_none() }));
                    last_online = Some(online);
                }
            }
            // X11 can inspect the active window geometry. Report only after two matching samples
            // so rapid focus/raise changes do not make the island flicker over applications.
            if window_tick >= 2 {
                window_tick = 0;
                if let Some(state) = x11_cover_state(&mut last_external_window) {
                    if window_state_candidate == Some(state) {
                        window_state_count = window_state_count.saturating_add(1);
                    } else {
                        window_state_candidate = Some(state);
                        window_state_count = 1;
                    }
                    if window_state_count >= 2 && window_state_sent != Some(state) {
                        emit("helper", json!({ "t": "cover", "covered": state.0, "full": state.1 }));
                        window_state_sent = Some(state);
                    }
                }
            }
        }
    });
}

// ---------------------------------------------------------------- window, tray and application commands
#[derive(Clone, Copy, Deserialize)]
struct InputRect {
    x: f64,
    y: f64,
    width: f64,
    height: f64,
}

fn default_input_regions() -> Vec<InputRect> {
    vec![InputRect {
        x: (WIN_W - 84.0) / 2.0,
        y: 0.0,
        width: 84.0,
        height: 44.0,
    }]
}

fn legacy_input_regions(
    x: Option<f64>,
    y: Option<f64>,
    width: Option<f64>,
    height: Option<f64>,
) -> Option<Vec<InputRect>> {
    match (x, y, width, height) {
        (Some(x), Some(y), Some(width), Some(height)) => Some(vec![InputRect {
            x, y, width, height,
        }]),
        _ => None,
    }
}

fn apply_linux_input_region(
    window: tauri::WebviewWindow,
    input_regions: Vec<InputRect>,
) {
    let target = window.clone();
    let _ = window.run_on_main_thread(move || {
        let Ok(gtk_window) = target.gtk_window() else {
            log("Could not access GTK window while applying input regions");
            return;
        };

        // Keep the hit-test areas disjoint. A single union bounding box would also
        // intercept clicks in the transparent space between the face, action strip,
        // notification stack and side chip.
        let mut rectangles: Vec<RectangleInt> = input_regions
            .into_iter()
            .take(12)
            .filter_map(|rect| {
                if !rect.x.is_finite()
                    || !rect.y.is_finite()
                    || !rect.width.is_finite()
                    || !rect.height.is_finite()
                    || rect.width <= 0.0
                    || rect.height <= 0.0
                {
                    return None;
                }

                let left = rect.x.max(0.0).min(WIN_W).floor();
                let top = rect.y.max(0.0).min(WIN_H).floor();
                let right = (rect.x + rect.width).max(0.0).min(WIN_W).ceil();
                let bottom = (rect.y + rect.height).max(0.0).min(WIN_H).ceil();
                if right <= left || bottom <= top {
                    return None;
                }

                Some(RectangleInt::new(
                    left as i32,
                    top as i32,
                    (right - left) as i32,
                    (bottom - top) as i32,
                ))
            })
            .collect();

        // Never leave the user with an unclickable window if the renderer sends bad
        // geometry during startup or while a view is being replaced.
        if rectangles.is_empty() {
            rectangles.push(RectangleInt::new(308, 0, 84, 44));
        }

        let region = Region::create_rectangles(&rectangles);
        gtk_window.input_shape_combine_region(Some(&region));
        gtk_window.queue_draw();
    });
}

fn set_input_interactive(
    window: tauri::WebviewWindow,
    on: bool,
    input_regions: Option<Vec<InputRect>>,
) {
    INPUT_INTERACTIVE.store(on, Ordering::SeqCst);
    INPUT_HEARTBEAT.store(now_ms(), Ordering::SeqCst);

    // The GTK input shape is enforced on both X11 and Wayland. The "interactive"
    // state renews the watchdog; it never expands the native window to 700x430.
    apply_linux_input_region(window, input_regions.unwrap_or_else(default_input_regions));
}

fn set_interactive_window(
    on: bool,
    x: Option<f64>,
    y: Option<f64>,
    width: Option<f64>,
    height: Option<f64>,
    regions: Option<Vec<InputRect>>,
) {
    if let Some(window) = APP.get().and_then(|app| app.get_webview_window("main")) {
        set_input_interactive(
            window,
            on,
            regions.or_else(|| legacy_input_regions(x, y, width, height)),
        );
    }
}

#[tauri::command]
fn set_interactive(
    on: bool,
    x: Option<f64>,
    y: Option<f64>,
    width: Option<f64>,
    height: Option<f64>,
    regions: Option<Vec<InputRect>>,
) {
    set_interactive_window(on, x, y, width, height, regions);
}

#[tauri::command]
fn cursor_update(x: f64, y: f64) {
    if x.is_finite() && y.is_finite() {
        emit("cursor", json!({ "x": x, "y": y }));
    }
}

fn x11_window_pid(id: &str) -> Option<u32> {
    output("xdotool", &["getwindowpid", id]).and_then(|value| value.parse::<u32>().ok())
}

fn x11_window_context(id: &str) -> Value {
    let title = output("xdotool", &["getwindowname", id]).unwrap_or_default();
    let exe = x11_window_pid(id)
        .and_then(|pid| fs::read_link(format!("/proc/{pid}/exe")).ok())
        .and_then(|path| path.file_name().map(|name| name.to_string_lossy().to_string()))
        .unwrap_or_default();
    json!({ "title": title, "exe": exe })
}

fn capture_previous_x11_window() -> Value {
    if is_wayland() { return json!({}); }
    let Some(id) = output("xdotool", &["getactivewindow"]) else { return json!({}); };
    // Do not replace the saved external window with Peeky itself when an input flow
    // advances from one field to the next.
    if x11_window_pid(&id) == Some(std::process::id()) { return json!({}); }
    *previous_x11_window().lock().unwrap() = Some(id.clone());
    x11_window_context(&id)
}

fn restore_previous_x11_window() {
    if is_wayland() { return; }
    let Some(previous) = previous_x11_window().lock().unwrap().take() else { return; };
    let Some(active) = output("xdotool", &["getactivewindow"]) else { return; };
    // If the user deliberately switched to another app, never steal focus back.
    if x11_window_pid(&active) != Some(std::process::id()) { return; }
    if x11_window_pid(&previous).is_none() { return; }

    let activated = Command::new("xdotool")
        .args(["windowactivate", "--sync", &previous])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map(|status| status.success())
        .unwrap_or(false);
    if !activated {
        let _ = Command::new("xdotool")
            .args(["windowfocus", "--sync", &previous])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status();
    }
}

#[tauri::command]
fn set_typing(
    window: tauri::WebviewWindow,
    on: bool,
    restore: bool,
    x: Option<f64>,
    y: Option<f64>,
    width: Option<f64>,
    height: Option<f64>,
    regions: Option<Vec<InputRect>>,
) -> Value {
    let context = if on { capture_previous_x11_window() } else { json!({}) };
    TYPING_ACTIVE.store(on, Ordering::SeqCst);
    set_input_interactive(
        window.clone(),
        on,
        regions.or_else(|| legacy_input_regions(x, y, width, height)),
    );
    if on {
        if let Err(error) = window.set_focus() {
            log(&format!("Could not focus the Peeky input field: {error}"));
        }
    } else if restore {
        restore_previous_x11_window();
    } else {
        previous_x11_window().lock().unwrap().take();
    }
    context
}

fn interaction_watchdog_start() {
    thread::spawn(|| loop {
        thread::sleep(Duration::from_millis(250));
        if INPUT_INTERACTIVE.load(Ordering::SeqCst)
            && now_ms().saturating_sub(INPUT_HEARTBEAT.load(Ordering::SeqCst)) > 4_000
        {
            let was_typing = TYPING_ACTIVE.swap(false, Ordering::SeqCst);
            // A dead renderer cannot complete its input flow. Drop the saved X11 focus target
            // rather than unexpectedly restoring a stale application during a later session.
            previous_x11_window().lock().unwrap().take();
            if let Some(window) = APP.get().and_then(|app| app.get_webview_window("main")) {
                set_input_interactive(window, false, None);
            }
            if was_typing {
                log("The UI stopped renewing input capture during a text flow; released pointer input");
            } else {
                log("The UI stopped renewing input capture; released pointer input");
            }
        }
    });
}

#[tauri::command]
fn lost_click(info: String) { log(&format!("missed click: {info}")); }

#[tauri::command]
fn media(cmd: String) {
    match cmd.as_str() {
        "toggle" => playerctl(&["play-pause"]),
        "next" => playerctl(&["next"]),
        "prev" => playerctl(&["previous"]),
        _ => {}
    }
}

#[tauri::command]
fn seek(ms: f64) {
    if (0.0..86_400_000.0).contains(&ms) {
        let seconds = format!("{:.3}", ms / 1000.0);
        playerctl(&["position", &seconds]);
    }
}

#[tauri::command]
fn set_volume(v: f64) { set_volume_value(v.clamp(0.0, 1.0) as f32); }

#[tauri::command]
fn toggle_mute() { toggle_mute_value(); }

#[tauri::command]
fn set_viz(_on: bool) {}

#[tauri::command]
fn notif_act(_id: String, _aumid: String) {}

#[tauri::command]
fn notif_dismiss(_id: String) {}

#[tauri::command]
fn notif_clear() {}

#[tauri::command]
fn clips_get() -> Value { clip_payload() }

#[tauri::command]
fn clip_copy(id: u64) -> bool {
    let entry = clips().lock().unwrap().iter().find(|clip| clip.id == id).cloned();
    let Some(entry) = entry else { return false; };
    if let Some(image_name) = entry.image.as_deref() {
        let bytes = entry.image_bytes.clone().or_else(|| clip_image_path(image_name).and_then(|path| fs::read(path).ok()));
        let Some(bytes) = bytes else { return false; };
        let copied = clipboard_write_png(&bytes);
        if copied && setting_flag("copyPreview") {
            emit("clip", json!({ "kind": "image" }));
        }
        copied
    } else {
        copy_text_impl(&entry.text)
    }
}

#[tauri::command]
fn clip_pin(id: u64) {
    if let Some(item) = clips().lock().unwrap().iter_mut().find(|c| c.id == id) { item.pinned = !item.pinned; }
    save_clips();
    emit("clips", clip_payload());
}

#[tauri::command]
fn clip_remove(id: u64) {
    clips().lock().unwrap().retain(|c| c.id != id);
    save_clips();
    emit("clips", clip_payload());
}

#[tauri::command]
fn clips_clear() {
    clips().lock().unwrap().retain(|c| c.pinned);
    save_clips();
    emit("clips", clip_payload());
}

#[tauri::command]
fn copy_text(text: String) -> bool { copy_text_impl(&text) }

#[tauri::command]
fn shelf_get() -> Value { shelf_payload() }

#[tauri::command]
fn shelf_add(paths: Vec<String>) -> Value { shelf_add_impl(paths) }

#[tauri::command]
fn shelf_add_data(request: tauri::ipc::Request<'_>) -> Value {
    let header = |key: &str| request.headers().get(key).and_then(|v| v.to_str().ok()).map(percent_decode).unwrap_or_default();
    match request.body() {
        tauri::ipc::InvokeBody::Raw(bytes) => shelf_add_data_impl(&header("x-name"), &header("x-type"), bytes),
        _ => shelf_payload(),
    }
}

#[tauri::command]
fn shelf_add_url(url: String) -> Value { shelf_add_url_impl(&url) }

#[tauri::command]
fn shelf_remove(path: String) { shelf_remove_impl(&path); }

#[tauri::command]
fn shelf_clear() { shelf_clear_impl(); }

#[tauri::command]
fn shelf_reveal(path: String) { shelf_reveal_impl(&path); }

#[tauri::command]
fn shelf_drag(paths: Vec<String>) { shelf_drag_impl(paths); }

#[tauri::command]
fn open_download(path: String) { open_download_impl(&path); }

#[tauri::command]
fn settings_get() -> Value { settings_payload() }

#[tauri::command]
fn settings_set(key: String, value: Value) { settings_apply(&key, &value); }

#[tauri::command]
fn action(name: String) {
    match name.as_str() {
        "screenshot" => {
            match capture_fullscreen() {
                Ok(png) if clipboard_write_png(&png) => emit("clip", json!({ "kind": "image" })),
                Ok(_) => emit("helper", json!({ "t": "capture-error", "message": "Could not put the screenshot on the clipboard. Check wl-clipboard or xclip." })),
                Err(why) => {
                    log(&format!("Screenshot failed: {why}"));
                    emit("helper", json!({ "t": "capture-error", "message": why }));
                }
            }
        }
        "dnd" => {
            if command_exists("gnome-control-center") { let _ = Command::new("gnome-control-center").arg("notifications").spawn(); }
            else if command_exists("systemsettings") { let _ = Command::new("systemsettings").arg("kcm_notifications").spawn(); }
        }
        "ocr" => ocr_start(),
        _ => {}
    }
}

#[tauri::command]
fn open_url(url: String) {
    let value = url.trim();
    if (value.starts_with("https://") || value.starts_with("http://"))
        && value.len() < 2000
        && !value.chars().any(|c| c.is_whitespace() || c.is_control() || c == '"' || c == '\'') {
        open_path(value);
    }
}

#[tauri::command]
fn update_check() -> String { "manual".into() }

#[tauri::command]
fn update_install() -> bool { false }

#[tauri::command]
fn speedo_cmd(_cmd: String, _id: Option<String>, _on: Option<bool>) -> Value {
    json!({ "ok": false, "present": false })
}

#[tauri::command]
fn ocr_done(text: String, conf: f64) { ocr_done_impl(&text, conf); }

#[tauri::command]
fn notes_get() -> Value { notes_payload(&notes().lock().unwrap()) }

#[tauri::command]
fn note_save(id: Option<u64>, text: String, remind: Option<f64>) -> Value {
    note_save_impl(id, &text, remind.unwrap_or(0.0).max(0.0) as u64)
}

#[tauri::command]
fn note_pin(id: u64) -> Value { note_pin_impl(id) }

#[tauri::command]
fn note_fired(id: u64) -> Value { note_fired_impl(id) }

#[tauri::command]
fn note_remove(id: u64) -> Value { note_remove_impl(id) }

#[tauri::command]
fn vault_list() -> Value { vault_payload() }

#[tauri::command]
fn vault_generate(len: usize, symbols: bool) -> String { vault_generate_impl(len, symbols) }

#[tauri::command]
fn vault_save(site: String, user: String, pass: String) -> u64 {
    let id = vault_save_impl(&site, &user, &pass);
    if id != 0 && !user.trim().is_empty() {
        settings_put("lastUser", user.trim().into());
        settings_broadcast();
    }
    id
}

#[tauri::command]
fn vault_edit(id: u64, site: String, user: String) -> bool {
    let ok = vault_edit_impl(id, &site, &user);
    if ok && !user.trim().is_empty() {
        settings_put("lastUser", user.trim().into());
        settings_broadcast();
    }
    ok
}

#[tauri::command]
fn fg_context() -> Value {
    if is_wayland() { return json!({}); }
    let Some(id) = output("xdotool", &["getactivewindow"]) else { return json!({}); };
    if x11_window_pid(&id) == Some(std::process::id()) { return json!({}); }
    x11_window_context(&id)
}

#[tauri::command]
fn vault_copy(id: u64, what: String) -> bool { vault_copy_impl(id, &what) }

#[tauri::command]
fn copy_secret(text: String) -> bool { copy_secret_impl(&text) }

#[tauri::command]
fn vault_reveal(id: u64) -> Option<String> { vault_reveal_impl(id) }

#[tauri::command]
fn vault_remove(id: u64) -> bool { vault_remove_impl(id) }

#[tauri::command]
fn debug_out(text: String) { log(&text); }

fn tray_rgba() -> Vec<u8> {
    const S: i32 = 32;
    let mut buf = vec![0u8; (S * S * 4) as usize];
    for y in 0..S {
        for x in 0..S {
            let i = ((y * S + x) * 4) as usize;
            let cx = x.clamp(9, 23);
            if (x - cx).pow(2) + (y - 16).pow(2) <= 64 {
                buf[i..i + 4].copy_from_slice(&[12, 12, 14, 255]);
            }
            for ex in [11.5f32, 20.5] {
                if ((x as f32 - ex) / 2.6).powi(2) + ((y as f32 - 16.0) / 3.4).powi(2) <= 1.0 {
                    buf[i..i + 4].copy_from_slice(&[255, 255, 255, 255]);
                }
            }
        }
    }
    buf
}

fn create_tray(app: &tauri::App) -> tauri::Result<()> {
    let title = MenuItemBuilder::with_id("title", format!("{} {}", APP_NAME, env!("CARGO_PKG_VERSION"))).enabled(false).build(app)?;
    let auto = CheckMenuItemBuilder::with_id("autostart", "Start with Linux").checked(autostart_on()).build(app)?;
    let menu = MenuBuilder::new(app)
        .item(&title).separator()
        .text("settings", "Settings…")
        .item(&auto)
        .text("restart", "Restart")
        .separator()
        .text("quit", "Quit")
        .build()?;
    TrayIconBuilder::with_id("main")
        .icon(tauri::image::Image::new_owned(tray_rgba(), 32, 32))
        .tooltip(APP_NAME)
        .menu(&menu)
        .show_menu_on_left_click(true)
        .on_menu_event(move |app, event| match event.id().as_ref() {
            "settings" => { let _ = app.emit("open", "settings"); }
            "autostart" => {
                let on = auto.is_checked().unwrap_or(false);
                settings_put("autoStart", on.into());
                set_autostart(on);
                settings_broadcast();
            }
            "restart" => app.restart(),
            "quit" => app.exit(0),
            _ => {}
        })
        .build(app)?;
    Ok(())
}

fn position_x11_window(window: &tauri::WebviewWindow) {
    // Explicit positioning is supported by X11 window managers.
    if let Ok(monitors) = window.available_monitors() {
        if let Some(monitor) = monitors.first() {
            let p = monitor.position();
            let size = monitor.size();
            let x = p.x + (size.width as i32 - WIN_W as i32) / 2;
            let _ = window.set_position(tauri::PhysicalPosition::new(x, p.y));
        }
    }
}

fn initialize_x11_threading() {
    if is_wayland() { return; }
    let Ok(xlib) = x11_dl::xlib::Xlib::open() else {
        log("libX11 is unavailable; using a normal interactive window without X11 pointer tracking");
        return;
    };
    // GTK/WebKit also use Xlib. XInitThreads must be called before either library opens
    // its display connection; the X11 polling thread then owns a separate Display handle.
    let initialized = unsafe { (xlib.XInitThreads)() };
    if initialized == 0 {
        log("XInitThreads failed; using a normal interactive window without X11 pointer tracking");
        return;
    }
    let _ = X11_LIB.set(xlib);
}

fn x11_native_window_id(window: &tauri::WebviewWindow) -> Option<x11_dl::xlib::Window> {
    let gtk_window = window.gtk_window().ok()?;
    let gdk_window = gtk_window.window()?;
    let x11_window = gdk_window.downcast::<X11Window>().ok()?;
    Some(x11_window.xid() as _)
}

unsafe fn x11_root_level_window(
    xlib: &x11_dl::xlib::Xlib,
    display: *mut x11_dl::xlib::Display,
    mut current: x11_dl::xlib::Window,
) -> Option<x11_dl::xlib::Window> {
    // Window managers may reparent client windows inside decoration/frame windows.
    // Walk upwards so the pointer's root child can be compared with Peeky's real top-level.
    for _ in 0..32 {
        let mut root = 0;
        let mut parent = 0;
        let mut children: *mut x11_dl::xlib::Window = ptr::null_mut();
        let mut child_count = 0u32;
        let ok = (xlib.XQueryTree)(
            display,
            current,
            &mut root,
            &mut parent,
            &mut children,
            &mut child_count,
        );
        if !children.is_null() {
            (xlib.XFree)(children.cast());
        }
        if ok == 0 || root == 0 || parent == 0 || parent == current {
            return None;
        }
        if parent == root {
            return Some(current);
        }
        current = parent;
    }
    None
}

fn x11_pointer_watch() {
    if is_wayland() { return; }
    let Some(xlib) = X11_LIB.get() else {
        X11_POINTER_ACTIVE.store(false, Ordering::SeqCst);
        // The compact GTK input region remains active; the WebView bridge supplies
        // local pointer events even when global X11 polling is unavailable.
        return;
    };
    let Some(window) = APP.get().and_then(|app| app.get_webview_window("main")) else { return; };
    let Some(own_window_id) = x11_native_window_id(&window) else {
        log("Could not obtain Peeky's native X11 window id; pointer tracking is disabled");
        X11_POINTER_ACTIVE.store(false, Ordering::SeqCst);
        // Keep the measured GTK input shape. DOM pointer events remain the fallback.
        return;
    };

    thread::spawn(move || unsafe {
        let display = (xlib.XOpenDisplay)(ptr::null());
        if display.is_null() {
            log("XOpenDisplay failed; pointer tracking is disabled");
            X11_POINTER_ACTIVE.store(false, Ordering::SeqCst);
            return;
        }
        let root = (xlib.XDefaultRootWindow)(display);
        let Some(own_root_child) = x11_root_level_window(xlib, display, own_window_id) else {
            log("Could not resolve Peeky's top-level X11 window; pointer tracking is disabled");
            (xlib.XCloseDisplay)(display);
            X11_POINTER_ACTIVE.store(false, Ordering::SeqCst);
            return;
        };

        X11_POINTER_ACTIVE.store(true, Ordering::SeqCst);
        let mut was_near = false;
        let mut last_emit = std::time::Instant::now();
        let mut last_down = false;
        let mut press_belongs_to_peeky = false;
        let mut last_x = -1000.0;
        let mut last_y = -1000.0;

        loop {
            thread::sleep(Duration::from_millis(16));

            let mut pointer_root = 0;
            let mut pointer_child = 0;
            let mut root_x = 0;
            let mut root_y = 0;
            let mut window_x = 0;
            let mut window_y = 0;
            let mut mask = 0u32;
            let queried = (xlib.XQueryPointer)(
                display,
                root,
                &mut pointer_root,
                &mut pointer_child,
                &mut root_x,
                &mut root_y,
                &mut window_x,
                &mut window_y,
                &mut mask,
            );
            if queried == 0 { continue; }

            let (Ok(position), Ok(scale)) = (window.outer_position(), window.scale_factor()) else { continue; };
            let scale = scale.max(0.5);
            let x = (root_x - position.x) as f64 / scale;
            let y = (root_y - position.y) as f64 / scale;
            let near = x > -80.0 && x < WIN_W + 80.0 && y > -80.0 && y < WIN_H + 80.0;

            let pointer_moved = (x - last_x).abs() > 0.1 || (y - last_y).abs() > 0.1;
            if (near || was_near)
                && (pointer_moved || last_emit.elapsed() >= Duration::from_millis(32))
            {
                emit("cursor", json!({ "x": (x * 10.0).round() / 10.0, "y": (y * 10.0).round() / 10.0 }));
                last_emit = std::time::Instant::now();
                last_x = x;
                last_y = y;
            }
            was_near = near;

            // Match the Windows hit-test fallback: only announce a press when the actual
            // top-level window under the pointer is Peeky, not merely when the coordinates
            // overlap its 700x430 transparent WebView rectangle.
            let left_button_down = mask & (1 << 8) != 0; // X11 Button1Mask
            if left_button_down
                && !last_down
                && INPUT_INTERACTIVE.load(Ordering::SeqCst)
                && near
                && pointer_child == own_root_child
            {
                press_belongs_to_peeky = true;
                emit("press", json!({ "x": x, "y": y }));
            } else if !left_button_down && last_down && press_belongs_to_peeky {
                press_belongs_to_peeky = false;
                emit("release", json!({ "x": x, "y": y }));
            }
            last_down = left_button_down;
        }
    });
}

fn register_x11_hotkeys(app: &tauri::App) {
    if !is_wayland() && setting_flag("noteHotkey") {
        if let Err(e) = app.global_shortcut().register("ctrl+alt+n") { log(&format!("Ctrl+Alt+N unavailable: {e}")); }
        if let Err(e) = app.global_shortcut().register("ctrl+alt+t") { log(&format!("Ctrl+Alt+T unavailable: {e}")); }
    }
}

fn main() {
    initialize_x11_threading();
    settings_load();
    notes_load();
    vault_load();
    clips_load();
    shelf_load();

    let builder = tauri::Builder::default()
        .plugin(tauri_plugin_single_instance::init(|app, _args, _cwd| {
            let _ = app.emit("open", "settings");
            if let Some(window) = app.get_webview_window("main") { let _ = window.set_focus(); }
        }))
        .plugin(tauri_plugin_global_shortcut::Builder::new().with_handler(|app, shortcut, event| {
            if event.state != ShortcutState::Pressed { return; }
            if shortcut.matches(Modifiers::CONTROL | Modifiers::ALT, Code::KeyN) {
                let _ = app.emit("hotkey", "note");
            } else if shortcut.matches(Modifiers::CONTROL | Modifiers::ALT, Code::KeyT) {
                ocr_start();
            }
        }).build());

    builder
        .invoke_handler(tauri::generate_handler![
            set_interactive, cursor_update, set_typing, lost_click, media, seek, set_volume, toggle_mute, set_viz, notif_act, notif_dismiss, notif_clear,
            clips_get, clip_copy, clip_pin, clip_remove, clips_clear, copy_text,
            shelf_get, shelf_add, shelf_add_data, shelf_add_url, shelf_remove, shelf_clear, shelf_reveal, shelf_drag, open_download,
            settings_get, settings_set, action, open_url, ocr_done, speedo_cmd, update_check, update_install,
            notes_get, note_save, note_pin, note_fired, note_remove,
            vault_list, vault_generate, vault_save, vault_edit, fg_context, vault_copy, copy_secret, vault_reveal, vault_remove, debug_out
        ])
        .setup(|app| {
            let _ = APP.set(app.handle().clone());
            if !is_wayland() { register_x11_hotkeys(app); }

            // gtk-layer-shell gives Wayland compositors that implement wlr-layer-shell a true
            // top-layer, top-centred surface. GNOME's default Wayland compositor does not expose
            // this protocol, so it gets a conventional, movable application window instead.
            let layer_shell = is_wayland() && gtk_layer_shell::is_supported();
            if is_wayland() && !layer_shell {
                log("This Wayland compositor has no layer-shell protocol; using a regular desktop window");
            }

            let mut builder = tauri::WebviewWindowBuilder::new(app, "main", tauri::WebviewUrl::App("index.html".into()))
                .title(APP_NAME)
                .inner_size(WIN_W, WIN_H)
                .decorations(is_wayland() && !layer_shell)
                .transparent(true)
                .shadow(false)
                .resizable(is_wayland() && !layer_shell)
                .maximizable(false)
                .minimizable(true)
                .always_on_top(!is_wayland() || layer_shell)
                .skip_taskbar(!is_wayland() || layer_shell)
                .focused(false)
                .visible(false)
                .disable_drag_drop_handler();
            if !is_wayland() {
                builder = builder.position(0.0, 0.0);
            }
            let window = builder.build()?;

            if layer_shell {
                if let Ok(gtk_window) = window.gtk_window() {
                    gtk_window.init_layer_shell();
                    gtk_window.set_layer(Layer::Top);
                    gtk_window.set_anchor(Edge::Top, true);
                    // CSS positions the pill 8px below the surface origin; do not add a second layer margin.
                    gtk_window.set_layer_shell_margin(Edge::Top, 0);
                    gtk_window.set_exclusive_zone(-1);
                    gtk_window.set_keyboard_mode(KeyboardMode::OnDemand);
                    gtk_window.set_namespace("peeky");
                }
            } else if is_wayland() {
                let _ = window.center();
            } else {
                position_x11_window(&window);
            }

            let _ = window.show();
            // Start with a compact hit target on every Linux display server. The bridge
            // expands it only around controls that are actually visible.
            set_input_interactive(window.clone(), false, Some(default_input_regions()));

            create_tray(app)?;
            if setting_flag("autoStart") { set_autostart(true); }
            clipboard_watch();
            downloads_watch();
            x11_pointer_watch();
            interaction_watchdog_start();
            poll_start();
            settings_broadcast();
            emit("notes", notes_payload(&notes().lock().unwrap()));
            emit("vault", vault_payload());
            emit("shelf", shelf_payload());
            emit("clips", clip_payload());
            emit("helper", json!({ "t": "cover", "covered": false, "full": false }));
            emit("speedo", json!({ "present": false }));
            Ok(())
        })
        .run(tauri::generate_context!())
        .expect("error while running Peeky on Linux");
}
