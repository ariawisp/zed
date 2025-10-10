pub mod redwood_panel;

use once_cell::sync::Lazy;
use parking_lot::Mutex;
use std::collections::HashMap;
use gpui::{AnyWindowHandle, AsyncApp, UpdateGlobal as _};

static PANELS: Lazy<Mutex<HashMap<u64, AnyWindowHandle>>> = Lazy::new(|| Mutex::new(HashMap::new()));

pub fn register_panel_window(panel_id: u64, handle: AnyWindowHandle) {
    PANELS.lock().insert(panel_id, handle);
}

pub fn close_panel_window(panel_id: u64, cx: &mut AsyncApp) {
    if let Some(handle) = PANELS.lock().remove(&panel_id) {
        let _ = cx.update_window(handle, |window, cx| { window.close(cx); Ok(()) });
    }
}

