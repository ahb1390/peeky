#![cfg_attr(target_os = "windows", windows_subsystem = "windows")]

// Platform backends are separate compilation units. Win32/WebView2 code is never compiled on Linux.
#[cfg(target_os = "windows")]
include!("main_windows.rs");

#[cfg(target_os = "linux")]
include!("main_linux.rs");
