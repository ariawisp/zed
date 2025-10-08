//! STOPGAP: Static host registration for Redwood->GPUI bridging.
//! This module is compile-time optional; it is enabled when REDWOOD_HOST_LIB_DIR is set
//! so the build script links libredwood_host_gpui.a and defines the `redwood_host` cfg.

#[cfg(redwood_host)]
mod imp {
    use std::ffi::c_char;
    use std::ffi::CStr;
    use std::sync::atomic::{AtomicI64, Ordering};

    type RwdHandle = i64;

    #[repr(C)]
    struct RwdGpuiVTable {
        create_text: extern "C" fn() -> RwdHandle,
        create_button: extern "C" fn() -> RwdHandle,
        create_image: extern "C" fn() -> RwdHandle,
        create_row: extern "C" fn() -> RwdHandle,
        create_column: extern "C" fn() -> RwdHandle,
        destroy: extern "C" fn(RwdHandle),
        append_child: extern "C" fn(RwdHandle, RwdHandle),
        insert_child: extern "C" fn(RwdHandle, i32, RwdHandle),
        remove_child: extern "C" fn(RwdHandle, RwdHandle),
        set_padding: extern "C" fn(RwdHandle, f32, f32, f32, f32),
        set_size: extern "C" fn(RwdHandle, *const f32, *const f32),
        set_spacing: extern "C" fn(RwdHandle, f32),
        set_align: extern "C" fn(RwdHandle, i32, i32),
        set_text: extern "C" fn(RwdHandle, *const c_char, usize),
        set_button_text: extern "C" fn(RwdHandle, *const c_char, usize),
        set_button_enabled: extern "C" fn(RwdHandle, i32),
        set_image_url: extern "C" fn(RwdHandle, *const c_char, usize),
        set_image_fit: extern "C" fn(RwdHandle, i32),
        set_image_radius: extern "C" fn(RwdHandle, f32),
    }

    extern "C" {
        fn redwood_host_set_gpui_vtable(vtable: *const RwdGpuiVTable) -> u64;
        fn redwood_host_init() -> u64;
        fn redwood_host_create_view(view_id: u64) -> u64;
        fn redwood_host_apply_changes(view_id: u64, json_ptr: *const c_char, json_len: usize) -> u64;
        fn redwood_host_preview_protocol_demo(view_id: u64) -> u64;
        fn redwood_host_button_click(view_id: u64, button_handle: RwdHandle);
    }

    // STOPGAP: simple handle allocator + logging stubs. Replace with real GPUI calls.
    use smol::channel::{Sender, Receiver, unbounded};
    use parking_lot::Mutex;
    use std::collections::HashMap;
    use once_cell::sync::Lazy;

    #[derive(Clone, Copy, Debug)]
    pub enum NodeKind { Text, Button, Image, Row, Column }

    #[derive(Debug)]
    pub enum Cmd {
        Create { handle: RwdHandle, kind: NodeKind },
        Destroy { handle: RwdHandle },
        AppendChild { parent: RwdHandle, child: RwdHandle },
        InsertChild { parent: RwdHandle, index: i32, child: RwdHandle },
        RemoveChild { parent: RwdHandle, child: RwdHandle },
        SetText { handle: RwdHandle, text: String },
        SetButtonText { handle: RwdHandle, text: String },
        SetButtonEnabled { handle: RwdHandle, enabled: bool },
        SetImageUrl { handle: RwdHandle, url: String },
        SetImageFit { handle: RwdHandle, fit: i32 },
        SetImageRadius { handle: RwdHandle, radius: f32 },
    }

    static UI_SENDER: Lazy<Mutex<Option<Sender<Cmd>>>> = Lazy::new(|| Mutex::new(None));
    pub fn register_ui_sender(tx: Sender<Cmd>) { *UI_SENDER.lock() = Some(tx); }

    fn send(cmd: Cmd) {
        if let Some(tx) = UI_SENDER.lock().as_ref() { let _ = tx.try_send(cmd); }
    }

    static NEXT_HANDLE: AtomicI64 = AtomicI64::new(10_000);
    fn new_handle() -> RwdHandle { NEXT_HANDLE.fetch_add(1, Ordering::Relaxed) }

    extern "C" fn vt_create_text() -> RwdHandle { let h=new_handle(); send(Cmd::Create{handle:h,kind:NodeKind::Text}); h }
    static LAST_BUTTON: Lazy<Mutex<Option<RwdHandle>>> = Lazy::new(|| Mutex::new(None));
    extern "C" fn vt_create_button() -> RwdHandle {
        let h=new_handle();
        *LAST_BUTTON.lock() = Some(h);
        send(Cmd::Create{handle:h,kind:NodeKind::Button});
        h
    }
    extern "C" fn vt_create_image() -> RwdHandle { let h=new_handle(); send(Cmd::Create{handle:h,kind:NodeKind::Image}); h }
    extern "C" fn vt_create_row() -> RwdHandle { let h=new_handle(); send(Cmd::Create{handle:h,kind:NodeKind::Row}); h }
    extern "C" fn vt_create_column() -> RwdHandle { let h=new_handle(); send(Cmd::Create{handle:h,kind:NodeKind::Column}); h }
    extern "C" fn vt_destroy(h: RwdHandle) { send(Cmd::Destroy{handle:h}) }
    extern "C" fn vt_append_child(p: RwdHandle, c: RwdHandle) { send(Cmd::AppendChild{parent:p,child:c}) }
    extern "C" fn vt_insert_child(p: RwdHandle, idx: i32, c: RwdHandle) { send(Cmd::InsertChild{parent:p,index:idx,child:c}) }
    extern "C" fn vt_remove_child(p: RwdHandle, c: RwdHandle) { send(Cmd::RemoveChild{parent:p,child:c}) }
    extern "C" fn vt_set_padding(_h: RwdHandle, _l:f32,_t:f32,_r:f32,_b:f32) { /* TODO */ }
    extern "C" fn vt_set_size(_h: RwdHandle, _w:*const f32, _hh:*const f32) { /* TODO */ }
    extern "C" fn vt_set_spacing(_h: RwdHandle, _gap:f32) { /* TODO */ }
    extern "C" fn vt_set_align(_h: RwdHandle, _main:i32,_cross:i32) { /* TODO */ }
    extern "C" fn vt_set_text(h: RwdHandle, s:*const c_char, n:usize) { let s = unsafe{std::slice::from_raw_parts(s as *const u8,n)}; let s=String::from_utf8_lossy(s).to_string(); send(Cmd::SetText{handle:h,text:s}) }
    extern "C" fn vt_set_button_text(h: RwdHandle, s:*const c_char, n:usize) { let s=unsafe{std::slice::from_raw_parts(s as *const u8,n)}; let s=String::from_utf8_lossy(s).to_string(); send(Cmd::SetButtonText{handle:h,text:s}) }
    extern "C" fn vt_set_button_enabled(h: RwdHandle, en:i32) { send(Cmd::SetButtonEnabled{handle:h,enabled: en!=0}) }
    extern "C" fn vt_set_image_url(h: RwdHandle, s:*const c_char, n:usize) { let s=unsafe{std::slice::from_raw_parts(s as *const u8,n)}; let s=String::from_utf8_lossy(s).to_string(); send(Cmd::SetImageUrl{handle:h,url:s}) }
    extern "C" fn vt_set_image_fit(h: RwdHandle, fit:i32) { send(Cmd::SetImageFit{handle:h,fit}) }
    extern "C" fn vt_set_image_radius(h: RwdHandle, r:f32) { send(Cmd::SetImageRadius{handle:h,radius:r}) }

    static VTABLE: RwdGpuiVTable = RwdGpuiVTable{
        create_text: vt_create_text,
        create_button: vt_create_button,
        create_image: vt_create_image,
        create_row: vt_create_row,
        create_column: vt_create_column,
        destroy: vt_destroy,
        append_child: vt_append_child,
        insert_child: vt_insert_child,
        remove_child: vt_remove_child,
        set_padding: vt_set_padding,
        set_size: vt_set_size,
        set_spacing: vt_set_spacing,
        set_align: vt_set_align,
        set_text: vt_set_text,
        set_button_text: vt_set_button_text,
        set_button_enabled: vt_set_button_enabled,
        set_image_url: vt_set_image_url,
        set_image_fit: vt_set_image_fit,
        set_image_radius: vt_set_image_radius,
    };

    pub fn try_register() {
        // NOTE(stopgap): If the static lib is not linked, this won't compile. Build.rs sets the
        // `redwood_host` cfg only when REDWOOD_HOST_LIB_DIR is set and linking is configured.
        unsafe {
            let rc = redwood_host_set_gpui_vtable(&VTABLE as *const _);
            if rc != 0 { log::warn!("redwood_host_set_gpui_vtable failed rc={}", rc); }
            let rc2 = redwood_host_init();
            if rc2 != 0 { log::warn!("redwood_host_init failed rc={}", rc2); }
        }
    }

    /// STOPGAP: If ZED_REDWOOD_PREVIEW=1 is set, create a demo view and apply a tiny JSON
    /// change-list the K/N host understands. Replace with real Redwood protocol once decoder lands.
    pub fn preview_demo_if_env() {
        let preview_env = std::env::var("ZED_REDWOOD_PREVIEW").ok().as_deref() == Some("1");
        let zedline_manifest = std::env::var("ZEDLINE_MANIFEST_PATH").ok().is_some();
        if !preview_env && !zedline_manifest { return; }
        unsafe {
            let _ = redwood_host_create_view(1);
            let rc = redwood_host_preview_protocol_demo(1);
            if rc != 0 { log::warn!("redwood_host_preview_protocol_demo failed rc={}", rc); }
            // If a button was created, dispatch a synthetic click to prove event flow.
            if let Some(h) = *LAST_BUTTON.lock() {
                redwood_host_button_click(1, h);
            }
        }
    }

    pub fn click(handle: RwdHandle) {
        unsafe { redwood_host_button_click(1, handle); }
    }
}

#[cfg(not(redwood_host))]
mod imp { pub fn try_register() { /* no-op: static lib not linked */ } pub fn click(_h: i64) {} }

pub use imp::{try_register, preview_demo_if_env, register_ui_sender, Cmd, NodeKind, click};
