#![cfg(all(target_os = "macos", feature = "macos-swift"))]

use super::{renderer, MacDisplay};
use crate::platform::{
    PlatformWindow, WindowBackgroundAppearance, WindowBounds, WindowControls, WindowDecorations,
    PromptButton, PromptLevel, PlatformDisplay, PlatformAtlas, RequestFrameOptions,
};
use crate::{
    AnyWindowHandle, Bounds, Capslock, DevicePixels, DispatchEventResult, ForegroundExecutor, GpuSpecs, Modifiers,
    Pixels, Point, Result, Scene, Size, WindowAppearance, Keystroke, ScrollDelta, point,
};
use objc2_app_kit as appkit;
use objc2_foundation as foundation;
use objc2::{msg_send, sel};
use objc2::rc::Retained;
use objc2::runtime::AnyObject as ObjcAny;
use objc2_foundation::{NSSize, NSString};
use objc2_app_kit::NSView;
use objc2_app_kit::NSWindow;
use parking_lot::Mutex;
use raw_window_handle as rwh;
use std::ptr;
use std::rc::Rc;
use std::sync::Arc;
use std::cell::RefCell;
use std::collections::HashMap;
use std::path::PathBuf;

pub(crate) struct SwiftMacWindowState {
    handle: AnyWindowHandle,
    executor: ForegroundExecutor,
    ns_window: *mut ObjcAny,
    ns_view: std::ptr::NonNull<ObjcAny>,
    renderer: renderer::Renderer,
    request_frame_callback: Option<Box<dyn FnMut(RequestFrameOptions)>>,
    resize_callback: Option<Box<dyn FnMut(Size<Pixels>, f32)>>,
    event_callback: Option<Box<dyn FnMut(crate::PlatformInput) -> DispatchEventResult>>,
    input_handler: Option<crate::platform::PlatformInputHandler>,
    active_callback: Option<Box<dyn FnMut(bool)>>,
    moved_callback: Option<Box<dyn FnMut()>>,
    hover_callback: Option<Box<dyn FnMut(bool)>>,
    should_close_callback: Option<Box<dyn FnMut() -> bool>>,
    close_callback: Option<Box<dyn FnOnce()>>,
    visibility_callback: Option<Box<dyn FnMut(bool)>>,
    appearance_callback: Option<Box<dyn FnMut()>>,
}

pub(crate) struct SwiftMacWindow(Arc<Mutex<SwiftMacWindowState>>);

impl SwiftMacWindow {
    pub fn open(
        handle: AnyWindowHandle,
        _options: crate::platform::WindowParams,
        executor: ForegroundExecutor,
        renderer_context: renderer::Context,
        swift_handle: *mut ObjcAny,
        cametal_layer: *mut std::ffi::c_void,
    ) -> Self {
        unsafe {
            // Get contentView from NSWindow handle
            let win_ref: &NSWindow = &*(swift_handle as *mut NSWindow);
            let view_ptr: *mut ObjcAny = match win_ref.contentView() {
                Some(v) => Retained::as_ptr(&v) as *mut ObjcAny,
                None => ptr::null_mut(),
            };
            let view = std::ptr::NonNull::new(view_ptr).expect("NSWindow contentView must not be nil");

            // Create renderer that adopts the external CAMetalLayer
            let mut renderer = renderer::new_renderer_with_layer(
                renderer_context,
                cametal_layer,
            );

            // Initialize drawable size from current view frame * scale
            if let Some(vp) = std::ptr::NonNull::new(view.as_ptr() as *mut NSView) {
                let vref: &NSView = unsafe { &*vp.as_ptr() };
                let frame = vref.frame();
                let scale = get_scale_factor(swift_handle as *mut ObjcAny);
                let size = Size {
                    width: DevicePixels((frame.size.width * scale as f64) as i32),
                    height: DevicePixels((frame.size.height * scale as f64) as i32),
                };
                renderer.update_drawable_size(size);
            }

            let window_arc = Arc::new(Mutex::new(SwiftMacWindowState {
                handle,
                executor,
                ns_window: swift_handle,
                ns_view: view,
                renderer,
                request_frame_callback: None,
                resize_callback: None,
                event_callback: None,
                input_handler: None,
                active_callback: None,
                moved_callback: None,
                hover_callback: None,
                should_close_callback: None,
                close_callback: None,
                visibility_callback: None,
                appearance_callback: None,
            }));

            register_window(swift_handle, &window_arc);
            Self(window_arc)
        }
    }
}

// Global registry to resolve callbacks from Swift by window handle.
type Registry = HashMap<usize, std::sync::Weak<Mutex<SwiftMacWindowState>>>;
thread_local! { static WINDOW_REGISTRY: RefCell<Registry> = RefCell::new(HashMap::new()); }

fn register_window(handle: *mut ObjcAny, window: &Arc<Mutex<SwiftMacWindowState>>) {
    WINDOW_REGISTRY.with(|reg| reg.borrow_mut().insert(handle as usize, Arc::downgrade(window)));
}

#[allow(dead_code)]
fn unregister_window(handle: *mut ObjcAny) { WINDOW_REGISTRY.with(|reg| { reg.borrow_mut().remove(&(handle as usize)); }); }

pub(crate) fn handle_window_resized(handle: *mut ObjcAny, width: u32, height: u32, scale: f32) {
    WINDOW_REGISTRY.with(|reg| {
        if let Some(weak) = reg.borrow().get(&(handle as usize)).cloned() {
            if let Some(arc) = weak.upgrade() {
                let mut lock = arc.lock();
                let size_px = Size { width: DevicePixels((width as f32 * scale) as i32), height: DevicePixels((height as f32 * scale) as i32) };
                lock.renderer.update_drawable_size(size_px);
                // Notify GPUI of logical size change if a callback is set
                if let Some(cb) = lock.resize_callback.as_mut() {
                    cb(Size { width: Pixels(width as f32), height: Pixels(height as f32) }, scale);
                }
                if let Some(cb) = lock.request_frame_callback.as_mut() {
                    let _ = cb(crate::platform::RequestFrameOptions::default());
                }
            }
        }
    })
}

pub(crate) fn handle_mouse_event(handle: *mut ObjcAny, ev: &super::swift_ffi::GPUI_MouseEvent) {
    WINDOW_REGISTRY.with(|reg| {
        if let Some(weak) = reg.borrow().get(&(handle as usize)).cloned() {
            if let Some(arc) = weak.upgrade() {
                let mut lock = arc.lock();
                let pos = Point { x: Pixels(ev.x as f32), y: Pixels(ev.y as f32) };
                let mods = modifiers_from_bits(ev.modifiers);
                let platform_input = match ev.r#type {
                    super::swift_ffi::GPUI_MouseType::Down => crate::PlatformInput::MouseDown(crate::MouseDownEvent {
                        button: mouse_button_from(ev.button), position: pos, modifiers: mods, click_count: ev.click_count as usize, first_mouse: false,
                    }),
                    super::swift_ffi::GPUI_MouseType::Up => crate::PlatformInput::MouseUp(crate::MouseUpEvent { button: mouse_button_from(ev.button), position: pos, modifiers: mods, click_count: ev.click_count as usize }),
                    super::swift_ffi::GPUI_MouseType::Move | super::swift_ffi::GPUI_MouseType::Drag => crate::PlatformInput::MouseMove(crate::MouseMoveEvent { position: pos, pressed_button: None, modifiers: mods }),
                    super::swift_ffi::GPUI_MouseType::Scroll => crate::PlatformInput::ScrollWheel(crate::ScrollWheelEvent { position: pos, delta: ScrollDelta::Lines(point(ev.dx as f32, ev.dy as f32)), modifiers: mods, touch_phase: crate::interactive::TouchPhase::Moved }),
                };
                if let Some(cb) = lock.event_callback.as_mut() { let _ = cb(platform_input); }
            }
        }
    });
}

pub(crate) fn handle_key_event(handle: *mut ObjcAny, ev: &super::swift_ffi::GPUI_KeyEvent) {
    WINDOW_REGISTRY.with(|reg| {
        if let Some(weak) = reg.borrow().get(&(handle as usize)).cloned() {
            if let Some(arc) = weak.upgrade() {
                let mut lock = arc.lock();
                let mods = modifiers_from_bits(ev.modifiers);
                let key = unsafe { ev.key.as_ref().map(|p| std::ffi::CStr::from_ptr(p)).and_then(|c| c.to_str().ok()).unwrap_or("") };
                let key_char = unsafe { ev.key_char.as_ref().map(|p| std::ffi::CStr::from_ptr(p)).and_then(|c| c.to_str().ok()).map(|s| s.to_string()) };
                let keystroke = Keystroke { modifiers: mods, key: key.to_string(), key_char };
                let platform_input = match ev.phase {
                    super::swift_ffi::GPUI_KeyPhase::Down => crate::PlatformInput::KeyDown(crate::KeyDownEvent { keystroke, is_held: ev.is_repeat }),
                    super::swift_ffi::GPUI_KeyPhase::Up => crate::PlatformInput::KeyUp(crate::KeyUpEvent { keystroke }),
                    super::swift_ffi::GPUI_KeyPhase::FlagsChanged => crate::PlatformInput::ModifiersChanged(crate::ModifiersChangedEvent { modifiers: mods, capslock: Capslock { on: (ev.modifiers & (1 << 5)) != 0 } }),
                };
                if let Some(cb) = lock.event_callback.as_mut() { let _ = cb(platform_input); }
            }
        }
    })
}

fn modifiers_from_bits(bits: u32) -> Modifiers {
    Modifiers {
        control: (bits & (1 << 2)) != 0,
        alt: (bits & (1 << 3)) != 0,
        shift: (bits & (1 << 0)) != 0,
        platform: (bits & (1 << 1)) != 0,
        function: (bits & (1 << 4)) != 0,
    }
}

fn mouse_button_from(b: super::swift_ffi::GPUI_MouseButton) -> crate::MouseButton {
    match b {
        super::swift_ffi::GPUI_MouseButton::Left => crate::MouseButton::Left,
        super::swift_ffi::GPUI_MouseButton::Right => crate::MouseButton::Right,
        super::swift_ffi::GPUI_MouseButton::Middle => crate::MouseButton::Middle,
    }
}

// ===== IME helpers (window-scoped) =====
pub(crate) fn ime_selected_range(handle: *mut ObjcAny) -> Option<(u32,u32,bool)> {
    WINDOW_REGISTRY.with(|reg| {
        reg.borrow().get(&(handle as usize)).and_then(|w| w.upgrade()).and_then(|arc| {
            let mut lock = arc.lock();
            let ih = lock.input_handler.as_mut()?;
            ih.selected_text_range(false).map(|sel| (sel.range.start as u32, (sel.range.end - sel.range.start) as u32, sel.reversed))
        })
    })
}

pub(crate) fn ime_marked_range(handle: *mut ObjcAny) -> Option<(u32,u32)> {
    WINDOW_REGISTRY.with(|reg| {
        reg.borrow().get(&(handle as usize)).and_then(|w| w.upgrade()).and_then(|arc| {
            let mut lock = arc.lock();
            let ih = lock.input_handler.as_mut()?;
            ih.marked_text_range().map(|r| (r.start as u32, (r.end - r.start) as u32))
        })
    })
}

pub(crate) fn ime_text_for_range(handle: *mut ObjcAny, loc: usize, len: usize) -> Option<(*const u8, usize, Option<(usize,usize)>)> {
    WINDOW_REGISTRY.with(|reg| {
        reg.borrow().get(&(handle as usize)).and_then(|w| w.upgrade()).and_then(|arc| {
            let mut lock = arc.lock();
            let ih = lock.input_handler.as_mut()?;
            let mut adjusted: Option<std::ops::Range<usize>> = None;
            let text = ih.text_for_range(loc..(loc+len), &mut adjusted)?;
            let bytes: Vec<u8> = text.into_bytes();
            let blen = bytes.len();
            let ptr = bytes.as_ptr();
            std::mem::forget(bytes);
            let adj = adjusted.map(|r| (r.start, r.end - r.start));
            Some((ptr, blen, adj))
        })
    })
}

pub(crate) fn ime_replace_text_in_range(handle: *mut ObjcAny, range: Option<(usize,usize)>, text: &str) {
    WINDOW_REGISTRY.with(|reg| {
        if let Some(arc) = reg.borrow().get(&(handle as usize)).and_then(|w| w.upgrade()) {
            let mut lock = arc.lock();
            if let Some(ih) = lock.input_handler.as_mut() {
                ih.replace_text_in_range(range.map(|(l,n)| l..(l+n)), text);
            }
        }
    });
}

pub(crate) fn ime_replace_and_mark_text_in_range(handle: *mut ObjcAny, range: Option<(usize,usize)>, text: &str, sel: Option<(usize,usize)>) {
    WINDOW_REGISTRY.with(|reg| {
        if let Some(arc) = reg.borrow().get(&(handle as usize)).and_then(|w| w.upgrade()) {
            let mut lock = arc.lock();
            if let Some(ih) = lock.input_handler.as_mut() {
                ih.replace_and_mark_text_in_range(range.map(|(l,n)| l..(l+n)), text, sel.map(|(l,n)| l..(l+n)));
            }
        }
    });
}

pub(crate) fn ime_unmark_text(handle: *mut ObjcAny) {
    WINDOW_REGISTRY.with(|reg| {
        if let Some(arc) = reg.borrow().get(&(handle as usize)).and_then(|w| w.upgrade()) {
            let mut lock = arc.lock();
            if let Some(ih) = lock.input_handler.as_mut() {
                ih.unmark_text();
            }
        }
    });
}

pub(crate) fn ime_bounds_for_range(handle: *mut ObjcAny, loc: usize, len: usize) -> Option<(f32,f32,f32,f32)> {
    WINDOW_REGISTRY.with(|reg| {
        reg.borrow().get(&(handle as usize)).and_then(|w| w.upgrade()).and_then(|arc| {
            let mut lock = arc.lock();
            let ih = lock.input_handler.as_mut()?;
            ih.bounds_for_range(loc..(loc+len)).map(|b| (b.origin.x.0 as f32, b.origin.y.0 as f32, b.size.width.0 as f32, b.size.height.0 as f32))
        })
    })
}

pub(crate) fn handle_file_drop_event(handle: *mut ObjcAny, phase: i32, x: f32, y: f32, paths: Option<Vec<PathBuf>>) {
    WINDOW_REGISTRY.with(|reg| {
        if let Some(weak) = reg.borrow().get(&(handle as usize)).cloned() {
            if let Some(arc) = weak.upgrade() {
                let mut lock = arc.lock();
                let pos = Point { x: Pixels(x), y: Pixels(y) };
                use crate::interactive::FileDropEvent;
                let input = match phase {
                    0 => {
                        let paths = paths.unwrap_or_default();
                        let external = crate::ExternalPaths(paths.into_iter().collect());
                        crate::PlatformInput::FileDrop(FileDropEvent::Entered { position: pos, paths: external })
                    }
                    1 => crate::PlatformInput::FileDrop(FileDropEvent::Pending { position: pos }),
                    2 => crate::PlatformInput::FileDrop(FileDropEvent::Exited),
                    3 => crate::PlatformInput::FileDrop(FileDropEvent::Submit { position: pos }),
                    _ => return,
                };
                if let Some(cb) = lock.event_callback.as_mut() { let _ = cb(input); }
            }
        }
    });
}

pub(crate) fn handle_active_changed(handle: *mut ObjcAny, active: bool) {
    WINDOW_REGISTRY.with(|reg| {
        if let Some(weak) = reg.borrow().get(&(handle as usize)).cloned() {
            if let Some(arc) = weak.upgrade() {
                if let Some(cb) = arc.lock().active_callback.as_mut() { cb(active); }
            }
        }
    });
}

pub(crate) fn handle_window_moved(handle: *mut ObjcAny) {
    WINDOW_REGISTRY.with(|reg| {
        if let Some(weak) = reg.borrow().get(&(handle as usize)).cloned() {
            if let Some(arc) = weak.upgrade() {
                if let Some(cb) = arc.lock().moved_callback.as_mut() { cb(); }
            }
        }
    });
}

pub(crate) fn handle_hover_changed(handle: *mut ObjcAny, hovered: bool) {
    WINDOW_REGISTRY.with(|reg| {
        if let Some(weak) = reg.borrow().get(&(handle as usize)).cloned() {
            if let Some(arc) = weak.upgrade() {
                if let Some(cb) = arc.lock().hover_callback.as_mut() { cb(hovered); }
            }
        }
    });
}

pub(crate) fn handle_should_close(handle: *mut ObjcAny) -> bool {
    WINDOW_REGISTRY.with(|reg| {
        if let Some(weak) = reg.borrow().get(&(handle as usize)).cloned() {
            if let Some(arc) = weak.upgrade() {
                let mut lock = arc.lock();
                if let Some(cb) = lock.should_close_callback.as_mut() { return cb(); }
            }
        }
        true
    })
}

pub(crate) fn handle_window_closed(handle: *mut ObjcAny) {
    WINDOW_REGISTRY.with(|reg| {
        if let Some(weak) = reg.borrow().get(&(handle as usize)).cloned() {
            if let Some(arc) = weak.upgrade() {
                let mut lock = arc.lock();
                if let Some(cb) = lock.close_callback.take() { cb(); }
            }
        }
    })
}

pub(crate) fn handle_visibility_changed(handle: *mut ObjcAny, visible: bool) {
    WINDOW_REGISTRY.with(|reg| {
        if let Some(weak) = reg.borrow().get(&(handle as usize)).cloned() {
            if let Some(arc) = weak.upgrade() {
                if let Some(cb) = arc.lock().visibility_callback.as_mut() { cb(visible); }
            }
        }
    });
}

pub(crate) fn handle_appearance_changed(handle: *mut ObjcAny) {
    WINDOW_REGISTRY.with(|reg| {
        if let Some(weak) = reg.borrow().get(&(handle as usize)).cloned() {
            if let Some(arc) = weak.upgrade() {
                if let Some(cb) = arc.lock().appearance_callback.as_mut() { cb(); }
            }
        }
    });
}

impl PlatformWindow for SwiftMacWindow {
    fn bounds(&self) -> Bounds<Pixels> {
        unsafe {
            let win: &NSWindow = &*(self.0.lock().ns_window as *mut NSWindow);
            let frame = win.frame();
            Bounds::new(
                Point { x: Pixels(frame.origin.x as f32), y: Pixels(frame.origin.y as f32) },
                Size { width: Pixels(frame.size.width as f32), height: Pixels(frame.size.height as f32) },
            )
        }
    }

    fn is_maximized(&self) -> bool { false }
    fn window_bounds(&self) -> WindowBounds { WindowBounds::Windowed(self.bounds()) }

    fn content_size(&self) -> Size<Pixels> {
        unsafe {
            let win: &NSWindow = &*(self.0.lock().ns_window as *mut NSWindow);
            let r = win.contentLayoutRect();
            Size { width: Pixels(r.size.width as f32), height: Pixels(r.size.height as f32) }
        }
    }

    fn resize(&mut self, size: Size<Pixels>) {
        let window = self.0.lock().ns_window;
        self.0.lock().executor.spawn(async move {
            let win: &NSWindow = unsafe { &*(window as *mut NSWindow) };
            let new_size = NSSize::new(size.width.0 as f64, size.height.0 as f64);
            win.setContentSize(new_size);
        }).detach();
    }

    fn scale_factor(&self) -> f32 { get_scale_factor(self.0.lock().ns_window) }

    fn appearance(&self) -> WindowAppearance {
        unsafe {
            let native: &ObjcAny = &*self.0.lock().ns_window;
            let appearance: *mut ObjcAny = msg_send![native, effectiveAppearance];
            WindowAppearance::from_native(appearance)
        }
    }

    fn display(&self) -> Option<Rc<dyn PlatformDisplay>> { Some(Rc::new(MacDisplay::primary())) }

    fn mouse_position(&self) -> Point<Pixels> { Point { x: Pixels(0.0), y: Pixels(0.0) } }
    fn modifiers(&self) -> Modifiers { Modifiers::default() }
    fn capslock(&self) -> Capslock { Capslock { on: false } }

    fn set_input_handler(&mut self, input_handler: crate::platform::PlatformInputHandler) {
        self.0.lock().input_handler = Some(input_handler);
    }
    fn take_input_handler(&mut self) -> Option<crate::platform::PlatformInputHandler> {
        self.0.lock().input_handler.take()
    }

    fn prompt(&self, level: PromptLevel, msg: &str, detail: Option<&str>, answers: &[PromptButton]) -> Option<futures::channel::oneshot::Receiver<usize>> {
        use futures::channel::oneshot;
        let (tx, rx) = oneshot::channel::<usize>();
        // Mirror behavior from objc2 path: compute index only to avoid borrowing
        let latest_non_cancel_index = answers
            .iter()
            .enumerate()
            .rev()
            .find(|&(_, ref label)| !label.is_cancel())
            .map(|(ix, _)| ix)
            .filter(|&ix| ix > 0);
        let sender = std::rc::Rc::new(std::cell::RefCell::new(Some(tx)));
        let executor = self.0.lock().executor.clone();
        let win_ptr = self.0.lock().ns_window as *mut NSWindow;
        executor.spawn({
            let sender = sender.clone();
            let msg = msg.to_string();
            let detail = detail.map(|s| s.to_string());
            let answers = answers.to_vec();
            let latest_non_cancel_index = latest_non_cancel_index;
            async move {
                let mtm = objc2::MainThreadMarker::new().expect("NSAlert on main thread");
                let alert = objc2_app_kit::NSAlert::new(mtm);
                let style = match level {
                    PromptLevel::Info => objc2_app_kit::NSAlertStyle::Informational,
                    PromptLevel::Warning => objc2_app_kit::NSAlertStyle::Warning,
                    PromptLevel::Critical => objc2_app_kit::NSAlertStyle::Critical,
                };
                alert.setAlertStyle(style);
                alert.setMessageText(&objc2_foundation::NSString::from_str(&msg));
                if let Some(detail) = detail.as_ref() {
                    alert.setInformativeText(&objc2_foundation::NSString::from_str(detail));
                }
                for (ix, answer) in answers.iter().enumerate().filter(|(ix, _)| Some(*ix) != latest_non_cancel_index) {
                    let title = objc2_foundation::NSString::from_str(answer.label());
                    let button = alert.addButtonWithTitle(&title);
                    let bref: &objc2_app_kit::NSButton = &*button;
                    bref.setTag(ix as objc2_foundation::NSInteger);
                }
                if let Some(ix) = latest_non_cancel_index {
                    let title = objc2_foundation::NSString::from_str(answers[ix].label());
                    let button = alert.addButtonWithTitle(&title);
                    let bref: &objc2_app_kit::NSButton = &*button;
                    bref.setTag(ix as objc2_foundation::NSInteger);
                }
                let block = block2::StackBlock::new(move |answer: objc2_app_kit::NSModalResponse| {
                    if let Some(tx) = sender.borrow_mut().take() {
                        let idx: usize = (answer as isize).try_into().unwrap_or(0);
                        let _ = tx.send(idx);
                    }
                }).copy();
                let win_ref: &NSWindow = unsafe { &*win_ptr };
                alert.beginSheetModalForWindow_completionHandler(win_ref, Some(&block));
            }
        }).detach();
        Some(rx)
    }
    fn activate(&self) {
        unsafe {
            let win: &NSWindow = &*(self.0.lock().ns_window as *mut NSWindow);
            win.makeKeyAndOrderFront(None);
        }
    }
    fn is_active(&self) -> bool { true }
    fn is_hovered(&self) -> bool { true }
    fn set_title(&mut self, title: &str) {
        if let Some(win) = std::ptr::NonNull::new(self.0.lock().ns_window as *mut ObjcAny) {
            let bytes = title.as_bytes();
            unsafe { crate::platform::mac::swift_ffi::gpui_macos_window_set_title(win.as_ptr() as *mut std::ffi::c_void, bytes.as_ptr(), bytes.len()) }
        }
    }
    fn set_background_appearance(&self, _background_appearance: WindowBackgroundAppearance) {}
    fn minimize(&self) {
        if let Some(win) = std::ptr::NonNull::new(self.0.lock().ns_window as *mut ObjcAny) {
            unsafe { crate::platform::mac::swift_ffi::gpui_macos_window_minimize(win.as_ptr() as *mut std::ffi::c_void) }
        }
    }
    fn zoom(&self) {
        if let Some(win) = std::ptr::NonNull::new(self.0.lock().ns_window as *mut ObjcAny) {
            unsafe { crate::platform::mac::swift_ffi::gpui_macos_window_zoom(win.as_ptr() as *mut std::ffi::c_void) }
        }
    }
    fn toggle_fullscreen(&self) {
        if let Some(win) = std::ptr::NonNull::new(self.0.lock().ns_window as *mut ObjcAny) {
            unsafe { crate::platform::mac::swift_ffi::gpui_macos_window_toggle_fullscreen(win.as_ptr() as *mut std::ffi::c_void) }
        }
    }
    fn is_fullscreen(&self) -> bool {
        if let Some(win) = std::ptr::NonNull::new(self.0.lock().ns_window as *mut ObjcAny) {
            return unsafe { crate::platform::mac::swift_ffi::gpui_macos_window_is_fullscreen(win.as_ptr() as *mut std::ffi::c_void) };
        }
        false
    }
    fn on_request_frame(&self, callback: Box<dyn FnMut(RequestFrameOptions)>) {
        self.0.lock().request_frame_callback = Some(callback);
    }
    fn on_input(&self, callback: Box<dyn FnMut(crate::PlatformInput) -> crate::DispatchEventResult>) {
        self.0.lock().event_callback = Some(callback);
    }
    fn on_active_status_change(&self, callback: Box<dyn FnMut(bool)>) {
        self.0.lock().active_callback = Some(callback);
    }
    fn on_hover_status_change(&self, callback: Box<dyn FnMut(bool)>) {
        self.0.lock().hover_callback = Some(callback);
    }
    fn on_resize(&self, callback: Box<dyn FnMut(Size<Pixels>, f32)>) {
        self.0.lock().resize_callback = Some(callback);
    }
    fn on_moved(&self, callback: Box<dyn FnMut()>) {
        self.0.lock().moved_callback = Some(callback);
    }
    fn on_should_close(&self, callback: Box<dyn FnMut() -> bool>) {
        self.0.lock().should_close_callback = Some(callback);
    }
    fn on_close(&self, callback: Box<dyn FnOnce()>) {
        self.0.lock().close_callback = Some(callback);
    }
    fn on_hit_test_window_control(&self, _callback: Box<dyn FnMut() -> Option<crate::WindowControlArea>>) {}
    fn on_appearance_changed(&self, callback: Box<dyn FnMut()>) { self.0.lock().appearance_callback = Some(callback); }
    fn on_visibility_changed(&self, callback: Box<dyn FnMut(bool)>) { self.0.lock().visibility_callback = Some(callback); }

    fn draw(&self, scene: &Scene) {
        self.0.lock().renderer.draw(scene);
    }
    fn sprite_atlas(&self) -> Arc<dyn PlatformAtlas> {
        self.0.lock().renderer.sprite_atlas().clone()
    }
    fn gpu_specs(&self) -> Option<GpuSpecs> { None }
    fn update_ime_position(&self, _bounds: Bounds<Pixels>) {}
}

impl rwh::HasWindowHandle for SwiftMacWindow {
    fn window_handle(&self) -> Result<rwh::WindowHandle<'_>, rwh::HandleError> {
        unsafe {
            Ok(rwh::WindowHandle::borrow_raw(rwh::RawWindowHandle::AppKit(
                rwh::AppKitWindowHandle::new(self.0.lock().ns_view.cast()),
            )))
        }
    }
}

impl rwh::HasDisplayHandle for SwiftMacWindow {
    fn display_handle(&self) -> Result<rwh::DisplayHandle<'_>, rwh::HandleError> {
        unsafe { Ok(rwh::DisplayHandle::borrow_raw(rwh::AppKitDisplayHandle::new().into())) }
    }
}

fn get_scale_factor(native_window: *mut ObjcAny) -> f32 {
    let factor = unsafe {
        let win: &NSWindow = &*(native_window as *mut NSWindow);
        if let Some(screen) = win.screen() { screen.backingScaleFactor() as f32 } else { 2.0 }
    };
    if factor == 0.0 { 2.0 } else { factor }
}
