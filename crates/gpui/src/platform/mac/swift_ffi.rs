#![cfg(target_os = "macos")]

use std::ffi::c_void;
use std::ffi::CString;
use std::os::raw::{c_char, c_uint};
use std::path::PathBuf;
use std::ptr;
use std::sync::{Mutex, OnceLock};

// Keep this in sync with src/platform/mac/gpui_macos_ffi.h
pub type GPUI_WindowHandle = *mut c_void;

#[repr(C)]
pub struct GPUI_WindowParams {
    pub width: u32,
    pub height: u32,
    pub scale: f32,
    pub title: *const c_char,
}

#[repr(C)]
pub struct GPUI_Callbacks {
    pub on_app_will_finish_launching: Option<extern "C" fn()>,
    pub on_app_did_finish_launching: Option<extern "C" fn()>,
    pub on_window_resized:
        Option<extern "C" fn(GPUI_WindowHandle, c_uint, c_uint, f32)>,
    pub on_mouse_event: Option<extern "C" fn(*const GPUI_MouseEvent)>,
    pub on_key_event: Option<extern "C" fn(*const GPUI_KeyEvent)>,
    pub on_menu_action: Option<extern "C" fn(*mut c_void, i32)>,
    pub on_validate_menu: Option<extern "C" fn(*mut c_void, i32) -> bool>,
    pub on_open_panel_result: Option<extern "C" fn(*mut c_void, u64, *const u8, usize)>,
    pub on_save_panel_result: Option<extern "C" fn(*mut c_void, u64, *const u8, usize)>,
    pub on_file_drop_event: Option<extern "C" fn(*mut c_void, GPUI_WindowHandle, i32, f32, f32, *const u8, usize)>,
    pub on_window_active_changed: Option<extern "C" fn(*mut c_void, GPUI_WindowHandle, bool)>,
    pub on_window_moved: Option<extern "C" fn(*mut c_void, GPUI_WindowHandle)>,
    pub on_hover_changed: Option<extern "C" fn(*mut c_void, GPUI_WindowHandle, bool)>,
    pub on_window_visibility_changed: Option<extern "C" fn(*mut c_void, GPUI_WindowHandle, bool)>,
    pub on_window_appearance_changed: Option<extern "C" fn(*mut c_void, GPUI_WindowHandle)>,
    pub on_window_should_close: Option<extern "C" fn(*mut c_void, GPUI_WindowHandle) -> bool>,
    pub on_window_will_close: Option<extern "C" fn(*mut c_void, GPUI_WindowHandle)>,
    // IME
    pub ime_selected_range: Option<extern "C" fn(*mut c_void, GPUI_WindowHandle, *mut u32, *mut u32, *mut bool) -> bool>,
    pub ime_marked_range: Option<extern "C" fn(*mut c_void, GPUI_WindowHandle, *mut u32, *mut u32) -> bool>,
    pub ime_text_for_range: Option<extern "C" fn(*mut c_void, GPUI_WindowHandle, u32, u32, *mut *const u8, *mut usize, *mut u32, *mut u32) -> bool>,
    pub ime_free_text: Option<extern "C" fn(*const u8, usize)>,
    pub ime_replace_text_in_range: Option<extern "C" fn(*mut c_void, GPUI_WindowHandle, bool, u32, u32, *const u8, usize)>,
    pub ime_replace_and_mark_text_in_range: Option<extern "C" fn(*mut c_void, GPUI_WindowHandle, bool, u32, u32, *const u8, usize, bool, u32, u32)>,
    pub ime_unmark_text: Option<extern "C" fn(*mut c_void, GPUI_WindowHandle)>,
    pub ime_bounds_for_range: Option<extern "C" fn(*mut c_void, GPUI_WindowHandle, u32, u32, *mut f32, *mut f32, *mut f32, *mut f32) -> bool>,
}

#[repr(C)]
#[derive(Copy, Clone)]
pub enum GPUI_MouseType { Move = 0, Down = 1, Up = 2, Drag = 3, Scroll = 4 }

#[repr(C)]
#[derive(Copy, Clone)]
pub enum GPUI_MouseButton { Left = 0, Right = 1, Middle = 2 }

#[repr(C)]
#[derive(Copy, Clone)]
pub enum GPUI_KeyPhase { Down = 1, Up = 2, FlagsChanged = 3 }

#[repr(C)]
pub struct GPUI_MouseEvent {
    pub window: GPUI_WindowHandle,
    pub r#type: GPUI_MouseType,
    pub button: GPUI_MouseButton,
    pub x: f32,
    pub y: f32,
    pub dx: f32,
    pub dy: f32,
    pub click_count: u32,
    pub modifiers: u32,
}

#[repr(C)]
pub struct GPUI_KeyEvent {
    pub window: GPUI_WindowHandle,
    pub phase: GPUI_KeyPhase,
    pub key_code: u16,
    pub unicode: u32,
    pub modifiers: u32,
    pub is_repeat: bool,
    pub key: *const c_char,
    pub key_char: *const c_char,
}

type InitFn = unsafe extern "C" fn(*mut c_void, *const GPUI_Callbacks);
type RunFn = unsafe extern "C" fn();
type QuitFn = unsafe extern "C" fn();
type CreateWindowFn = unsafe extern "C" fn(
    *const GPUI_WindowParams,
    *mut GPUI_WindowHandle,
    *mut *mut c_void,
);
type SetMenusFn = unsafe extern "C" fn(*const u8, usize);
type SetDockMenuFn = unsafe extern "C" fn(*const u8, usize);
type OpenPanelFn = unsafe extern "C" fn(*const u8, usize, u64);
type SavePanelFn = unsafe extern "C" fn(*const u8, usize, u64);
type SetCursorFn = unsafe extern "C" fn(i32, bool);
type WindowSetTitleFn = unsafe extern "C" fn(GPUI_WindowHandle, *const u8, usize);
type WindowVoidCmdFn = unsafe extern "C" fn(GPUI_WindowHandle);
type WindowIsFullscreenFn = unsafe extern "C" fn(GPUI_WindowHandle) -> bool;

// Statically linked Swift exports
unsafe extern "C" {
    pub fn gpui_macos_init(user_data: *mut c_void, callbacks: *const GPUI_Callbacks);
    pub fn gpui_macos_run();
    pub fn gpui_macos_quit();
    pub fn gpui_macos_create_window(
        params: *const GPUI_WindowParams,
        out_handle: *mut GPUI_WindowHandle,
        out_metal_layer: *mut *mut c_void,
    );
    pub fn gpui_macos_set_menus(json: *const u8, len: usize);
    pub fn gpui_macos_set_dock_menu(json: *const u8, len: usize);
    pub fn gpui_macos_open_panel(json: *const u8, len: usize, request_id: u64);
    pub fn gpui_macos_save_panel(json: *const u8, len: usize, request_id: u64);
    pub fn gpui_macos_set_cursor(style: i32, hide_until_mouse_moves: bool);
    pub fn gpui_macos_window_set_title(window: GPUI_WindowHandle, utf8: *const u8, len: usize);
    pub fn gpui_macos_window_minimize(window: GPUI_WindowHandle);
    pub fn gpui_macos_window_zoom(window: GPUI_WindowHandle);
    pub fn gpui_macos_window_toggle_fullscreen(window: GPUI_WindowHandle);
    pub fn gpui_macos_window_is_fullscreen(window: GPUI_WindowHandle) -> bool;
}

pub struct SwiftApi {
    _lib: libloading::Library,
    pub init: InitFn,
    pub run: RunFn,
    #[allow(dead_code)]
    pub quit: QuitFn,
    #[allow(dead_code)]
    pub create_window: CreateWindowFn,
    pub set_menus: SetMenusFn,
    pub set_dock_menu: SetDockMenuFn,
    pub open_panel: OpenPanelFn,
    pub save_panel: SavePanelFn,
    pub set_cursor: SetCursorFn,
    pub window_set_title: WindowSetTitleFn,
    pub window_minimize: WindowVoidCmdFn,
    pub window_zoom: WindowVoidCmdFn,
    pub window_toggle_fullscreen: WindowVoidCmdFn,
    pub window_is_fullscreen: WindowIsFullscreenFn,
}

impl SwiftApi {
    fn new(lib: libloading::Library) -> anyhow::Result<Self> {
        unsafe {
            let init_sym: libloading::Symbol<InitFn> = lib.get(b"gpui_macos_init")?;
            let run_sym: libloading::Symbol<RunFn> = lib.get(b"gpui_macos_run")?;
            let quit_sym: libloading::Symbol<QuitFn> = lib.get(b"gpui_macos_quit")?;
            let create_window_sym: libloading::Symbol<CreateWindowFn> = lib.get(b"gpui_macos_create_window")?;
            let set_menus_sym: libloading::Symbol<SetMenusFn> = lib.get(b"gpui_macos_set_menus")?;
            let set_dock_menu_sym: libloading::Symbol<SetDockMenuFn> = lib.get(b"gpui_macos_set_dock_menu")?;
            let init = *init_sym;
            let run = *run_sym;
            let quit = *quit_sym;
            let create_window = *create_window_sym;
            let set_menus = *set_menus_sym;
            let set_dock_menu = *set_dock_menu_sym;
            let open_panel_sym: libloading::Symbol<OpenPanelFn> = lib.get(b"gpui_macos_open_panel")?;
            let save_panel_sym: libloading::Symbol<SavePanelFn> = lib.get(b"gpui_macos_save_panel")?;
            let open_panel = *open_panel_sym;
            let save_panel = *save_panel_sym;
            let set_cursor_sym: libloading::Symbol<SetCursorFn> = lib.get(b"gpui_macos_set_cursor")?;
            let set_cursor = *set_cursor_sym;
            let window_set_title_sym: libloading::Symbol<WindowSetTitleFn> = lib.get(b"gpui_macos_window_set_title")?;
            let window_minimize_sym: libloading::Symbol<WindowVoidCmdFn> = lib.get(b"gpui_macos_window_minimize")?;
            let window_zoom_sym: libloading::Symbol<WindowVoidCmdFn> = lib.get(b"gpui_macos_window_zoom")?;
            let window_toggle_fullscreen_sym: libloading::Symbol<WindowVoidCmdFn> = lib.get(b"gpui_macos_window_toggle_fullscreen")?;
            let window_is_fullscreen_sym: libloading::Symbol<WindowIsFullscreenFn> = lib.get(b"gpui_macos_window_is_fullscreen")?;
            let window_set_title = *window_set_title_sym;
            let window_minimize = *window_minimize_sym;
            let window_zoom = *window_zoom_sym;
            let window_toggle_fullscreen = *window_toggle_fullscreen_sym;
            let window_is_fullscreen = *window_is_fullscreen_sym;
            Ok(SwiftApi {
                _lib: lib,
                init, run, quit, create_window,
                set_menus, set_dock_menu,
                open_panel, save_panel,
                set_cursor,
                window_set_title,
                window_minimize,
                window_zoom,
                window_toggle_fullscreen,
                window_is_fullscreen,
            })
        }
    }
}

// Expose a stable set of Swift function pointers for use across modules.
pub(crate) struct SwiftFns {
    pub set_cursor: SetCursorFn,
    pub window_set_title: WindowSetTitleFn,
    pub window_minimize: WindowVoidCmdFn,
    pub window_zoom: WindowVoidCmdFn,
    pub window_toggle_fullscreen: WindowVoidCmdFn,
    pub window_is_fullscreen: WindowIsFullscreenFn,
}

static SWIFT_FNS: OnceLock<SwiftFns> = OnceLock::new();

pub(crate) fn install_api(api: &SwiftApi) {
    let _ = SWIFT_FNS.set(SwiftFns {
        set_cursor: api.set_cursor,
        window_set_title: api.window_set_title,
        window_minimize: api.window_minimize,
        window_zoom: api.window_zoom,
        window_toggle_fullscreen: api.window_toggle_fullscreen,
        window_is_fullscreen: api.window_is_fullscreen,
    });
}

pub(crate) fn with_fns<R>(f: impl FnOnce(&SwiftFns) -> R) -> Option<R> {
    SWIFT_FNS.get().map(f)
}

extern "C" fn on_app_will_finish_launching() {}
extern "C" fn on_app_did_finish_launching() {}
extern "C" fn on_window_resized(
    _handle: GPUI_WindowHandle,
    _width: c_uint,
    _height: c_uint,
    _scale: f32,
) {
    #[cfg(feature = "macos-swift")]
    unsafe {
        crate::platform::mac::swift_window::handle_window_resized(
            _handle as *mut objc2::runtime::AnyObject,
            _width as u32,
            _height as u32,
            _scale as f32,
        );
    }
}

pub fn callbacks() -> GPUI_Callbacks {
    GPUI_Callbacks {
        on_app_will_finish_launching: Some(on_app_will_finish_launching),
        on_app_did_finish_launching: Some(on_app_did_finish_launching),
        on_window_resized: Some(on_window_resized),
        on_mouse_event: Some(on_mouse_event),
        on_key_event: Some(on_key_event),
        on_menu_action: Some(crate::platform::mac::platform::swift_on_menu_action),
        on_validate_menu: Some(crate::platform::mac::platform::swift_on_validate_menu),
        on_open_panel_result: Some(crate::platform::mac::platform::swift_on_open_panel_result),
        on_save_panel_result: Some(crate::platform::mac::platform::swift_on_save_panel_result),
        on_file_drop_event: Some(on_file_drop_event),
        on_window_active_changed: Some(on_window_active_changed),
        on_window_moved: Some(on_window_moved),
        on_hover_changed: Some(on_hover_changed),
        on_window_visibility_changed: Some(on_window_visibility_changed),
        on_window_appearance_changed: Some(on_window_appearance_changed),
        on_window_should_close: Some(on_window_should_close),
        on_window_will_close: Some(on_window_will_close),
        ime_selected_range: Some(ime_selected_range),
        ime_marked_range: Some(ime_marked_range),
        ime_text_for_range: Some(ime_text_for_range),
        ime_free_text: Some(ime_free_text),
        ime_replace_text_in_range: Some(ime_replace_text_in_range),
        ime_replace_and_mark_text_in_range: Some(ime_replace_and_mark_text_in_range),
        ime_unmark_text: Some(ime_unmark_text),
        ime_bounds_for_range: Some(ime_bounds_for_range),
    }
}

extern "C" fn on_mouse_event(ev: *const GPUI_MouseEvent) {
    if ev.is_null() { return; }
    let ev = unsafe { &*ev };
    #[cfg(feature = "macos-swift")]
    {
        crate::platform::mac::swift_window::handle_mouse_event(
            ev.window as *mut objc2::runtime::AnyObject,
            ev,
        );
    }
}

extern "C" fn on_key_event(ev: *const GPUI_KeyEvent) {
    if ev.is_null() { return; }
    let ev = unsafe { &*ev };
    #[cfg(feature = "macos-swift")]
    {
        crate::platform::mac::swift_window::handle_key_event(
            ev.window as *mut objc2::runtime::AnyObject,
            ev,
        );
    }
}

// ===== IME callbacks (Swift -> Rust) =====
extern "C" fn ime_selected_range(
    _ctx: *mut c_void,
    window: GPUI_WindowHandle,
    out_loc: *mut u32,
    out_len: *mut u32,
    out_reversed: *mut bool,
) -> bool {
    #[cfg(feature = "macos-swift")]
    {
        if let Some((loc, len, rev)) = crate::platform::mac::swift_window::ime_selected_range(window as *mut objc2::runtime::AnyObject) {
            unsafe {
                if !out_loc.is_null() { *out_loc = loc; }
                if !out_len.is_null() { *out_len = len; }
                if !out_reversed.is_null() { *out_reversed = rev; }
            }
            return true;
        }
    }
    false
}

extern "C" fn ime_marked_range(
    _ctx: *mut c_void,
    window: GPUI_WindowHandle,
    out_loc: *mut u32,
    out_len: *mut u32,
) -> bool {
    #[cfg(feature = "macos-swift")]
    {
        if let Some((loc, len)) = crate::platform::mac::swift_window::ime_marked_range(window as *mut objc2::runtime::AnyObject) {
            unsafe {
                if !out_loc.is_null() { *out_loc = loc; }
                if !out_len.is_null() { *out_len = len; }
            }
            return true;
        }
    }
    false
}

extern "C" fn ime_text_for_range(
    _ctx: *mut c_void,
    window: GPUI_WindowHandle,
    loc: u32,
    len: u32,
    out_ptr: *mut *const u8,
    out_len: *mut usize,
    out_adj_loc: *mut u32,
    out_adj_len: *mut u32,
) -> bool {
    #[cfg(feature = "macos-swift")]
    {
        if let Some((ptr, blen, adj)) = crate::platform::mac::swift_window::ime_text_for_range(window as *mut objc2::runtime::AnyObject, loc as usize, len as usize) {
            unsafe {
                if !out_ptr.is_null() { *out_ptr = ptr; }
                if !out_len.is_null() { *out_len = blen; }
                if let Some((al, an)) = adj {
                    if !out_adj_loc.is_null() { *out_adj_loc = al as u32; }
                    if !out_adj_len.is_null() { *out_adj_len = an as u32; }
                }
            }
            return true;
        }
    }
    false
}

extern "C" fn ime_free_text(ptr: *const u8, len: usize) {
    if ptr.is_null() || len == 0 { return; }
    unsafe {
        // reconstruct Vec to free
        let _ = Vec::from_raw_parts(ptr as *mut u8, len, len);
    }
}

extern "C" fn ime_replace_text_in_range(
    _ctx: *mut c_void,
    window: GPUI_WindowHandle,
    has_range: bool,
    loc: u32,
    len: u32,
    text_ptr: *const u8,
    text_len: usize,
) {
    #[cfg(feature = "macos-swift")]
    {
        let text = if !text_ptr.is_null() && text_len > 0 {
            unsafe { std::slice::from_raw_parts(text_ptr, text_len) }
        } else { &[] };
        let s = std::str::from_utf8(text).unwrap_or("");
        crate::platform::mac::swift_window::ime_replace_text_in_range(
            window as *mut objc2::runtime::AnyObject,
            if has_range { Some((loc as usize, len as usize)) } else { None },
            s,
        );
    }
}

extern "C" fn ime_replace_and_mark_text_in_range(
    _ctx: *mut c_void,
    window: GPUI_WindowHandle,
    has_range: bool,
    loc: u32,
    len: u32,
    text_ptr: *const u8,
    text_len: usize,
    has_sel: bool,
    sel_loc: u32,
    sel_len: u32,
) {
    #[cfg(feature = "macos-swift")]
    {
        let text = if !text_ptr.is_null() && text_len > 0 {
            unsafe { std::slice::from_raw_parts(text_ptr, text_len) }
        } else { &[] };
        let s = std::str::from_utf8(text).unwrap_or("");
        crate::platform::mac::swift_window::ime_replace_and_mark_text_in_range(
            window as *mut objc2::runtime::AnyObject,
            if has_range { Some((loc as usize, len as usize)) } else { None },
            s,
            if has_sel { Some((sel_loc as usize, sel_len as usize)) } else { None },
        );
    }
}

extern "C" fn ime_unmark_text(
    _ctx: *mut c_void,
    window: GPUI_WindowHandle,
) {
    #[cfg(feature = "macos-swift")]
    {
        crate::platform::mac::swift_window::ime_unmark_text(window as *mut objc2::runtime::AnyObject);
    }
}

extern "C" fn ime_bounds_for_range(
    _ctx: *mut c_void,
    window: GPUI_WindowHandle,
    loc: u32,
    len: u32,
    out_x: *mut f32,
    out_y: *mut f32,
    out_w: *mut f32,
    out_h: *mut f32,
) -> bool {
    #[cfg(feature = "macos-swift")]
    {
        if let Some((x,y,w,h)) = crate::platform::mac::swift_window::ime_bounds_for_range(window as *mut objc2::runtime::AnyObject, loc as usize, len as usize) {
            unsafe {
                if !out_x.is_null() { *out_x = x; }
                if !out_y.is_null() { *out_y = y; }
                if !out_w.is_null() { *out_w = w; }
                if !out_h.is_null() { *out_h = h; }
            }
            return true;
        }
    }
    false
}

extern "C" fn on_file_drop_event(
    _ctx: *mut c_void,
    window: GPUI_WindowHandle,
    phase: i32,
    x: f32,
    y: f32,
    json: *const u8,
    len: usize,
) {
    #[cfg(feature = "macos-swift")]
    {
        let paths: Option<Vec<std::path::PathBuf>> = if !json.is_null() && len > 0 {
            let slice = unsafe { std::slice::from_raw_parts(json, len) };
            match serde_json::from_slice::<Vec<String>>(slice) {
                Ok(vec) => Some(vec.into_iter().map(std::path::PathBuf::from).collect()),
                Err(_) => None,
            }
        } else { None };
        crate::platform::mac::swift_window::handle_file_drop_event(
            window as *mut objc2::runtime::AnyObject,
            phase,
            x,
            y,
            paths,
        );
    }
}

extern "C" fn on_window_active_changed(_ctx: *mut c_void, window: GPUI_WindowHandle, active: bool) {
    #[cfg(feature = "macos-swift")]
    {
        crate::platform::mac::swift_window::handle_active_changed(window as *mut objc2::runtime::AnyObject, active);
    }
}

extern "C" fn on_window_moved(_ctx: *mut c_void, window: GPUI_WindowHandle) {
    #[cfg(feature = "macos-swift")]
    {
        crate::platform::mac::swift_window::handle_window_moved(window as *mut objc2::runtime::AnyObject);
    }
}

extern "C" fn on_hover_changed(_ctx: *mut c_void, window: GPUI_WindowHandle, hovered: bool) {
    #[cfg(feature = "macos-swift")]
    {
        crate::platform::mac::swift_window::handle_hover_changed(window as *mut objc2::runtime::AnyObject, hovered);
    }
}

extern "C" fn on_window_visibility_changed(_ctx: *mut c_void, window: GPUI_WindowHandle, visible: bool) {
    #[cfg(feature = "macos-swift")]
    {
        crate::platform::mac::swift_window::handle_visibility_changed(window as *mut objc2::runtime::AnyObject, visible);
    }
}

extern "C" fn on_window_appearance_changed(_ctx: *mut c_void, window: GPUI_WindowHandle) {
    #[cfg(feature = "macos-swift")]
    {
        crate::platform::mac::swift_window::handle_appearance_changed(window as *mut objc2::runtime::AnyObject);
    }
}

extern "C" fn on_window_should_close(_ctx: *mut c_void, window: GPUI_WindowHandle) -> bool {
    #[cfg(feature = "macos-swift")]
    {
        return crate::platform::mac::swift_window::handle_should_close(window as *mut objc2::runtime::AnyObject);
    }
    true
}

extern "C" fn on_window_will_close(_ctx: *mut c_void, window: GPUI_WindowHandle) {
    #[cfg(feature = "macos-swift")]
    {
        crate::platform::mac::swift_window::handle_window_closed(window as *mut objc2::runtime::AnyObject);
    }
}

pub fn try_load() -> Option<SwiftApi> {
    // Resolve dylib path: env override or common names next to the bundle.
    // 1) GPUI_SWIFT_LIB env var (absolute path)
    if let Ok(path) = std::env::var("GPUI_SWIFT_LIB") {
        if let Ok(lib) = unsafe { libloading::Library::new(&path) } {
            return SwiftApi::new(lib).ok();
        }
    }

    // 2) Current working dir (development convenience)
    let candidates = [
        "libGPUIAppKit.dylib",
        "GPUIAppKit.framework/GPUIAppKit",
    ];
    for name in candidates.iter() {
        if let Ok(lib) = unsafe { libloading::Library::new(name) } {
            if let Ok(api) = SwiftApi::new(lib) {
                return Some(api);
            }
        }
    }

    // 3) Relative to crate’s Swift package build dir (when built via swift build)
    if let Ok(manifest_dir) = std::env::var("CARGO_MANIFEST_DIR") {
        let mut p = PathBuf::from(manifest_dir);
        p.push("src/platform/mac/swift/.build/release/libGPUIAppKit.dylib");
        if let Ok(lib) = unsafe { libloading::Library::new(&p) } {
            if let Ok(api) = SwiftApi::new(lib) {
                return Some(api);
            }
        }
    }

    None
}

// Convenience to build a minimal window params struct
#[allow(dead_code)]
pub fn window_params(width: u32, height: u32, scale: f32, title: &str) -> GPUI_WindowParams {
    GPUI_WindowParams {
        width,
        height,
        scale,
        title: CString::new(title).ok().map_or(ptr::null(), |s| s.into_raw()),
    }
}
