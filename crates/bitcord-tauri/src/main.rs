// Prevents additional console window on Windows in release builds.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

fn main() {
    // Force X11 backend on Linux — Tauri/WebKitGTK doesn't support Wayland.
    #[cfg(target_os = "linux")]
    // SAFETY: called at program startup before any threads are spawned.
    unsafe {
        std::env::set_var("GDK_BACKEND", "x11");
        std::env::set_var("WEBKIT_DISABLE_COMPOSITING_MODE", "1");
    }

    bitcord_tauri_lib::run();
}
