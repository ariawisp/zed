use crate::{
    geometry::{
        rect::RectF,
        vector::{vec2f, Vector2F},
    },
    platform::{
        self,
        mac::{platform::NSViewLayerContentsRedrawDuringViewResize, renderer::Renderer},
        Event, FontSystem, WindowBounds,
    },
    Scene,
};
use objc2_foundation::{NSPoint, NSRect, NSSize};
use objc2::runtime::AnyObject as Object;
use objc2_app_kit::{NSStatusBar, NSStatusItem};
// Use raw Objective-C object pointers where typed APIs are missing
use ctor::ctor;
use foreign_types::ForeignTypeRef;
use objc2::{class, sel, msg_send};
use super::shims;
use objc2::rc::Retained;
use objc2::runtime::{AnyClass as Objc2AnyClass, AnyObject as Objc2AnyObject, AnyProtocol as Objc2AnyProtocol, ClassBuilder as Objc2ClassBuilder, ProtocolObject};
use std::ffi::CStr;
use std::{
    cell::RefCell,
    ffi::c_void,
    ptr,
    rc::{Rc, Weak},
    sync::Arc,
};

use super::screen::Screen;

const STATE_IVAR: &str = "state";

fn status_item_button_ptr(item: &NSStatusItem) -> *mut Object {
    unsafe { msg_send![&*item, button] }
}

#[ctor]
unsafe fn build_classes() {
    let mut decl = Objc2ClassBuilder::new(
        CStr::from_bytes_with_nul(b"GPUIStatusItemView\0").unwrap(),
        objc2::class!(NSView),
    )
    .expect("failed to allocate GPUIStatusItemView class");
    decl.add_ivar::<*mut c_void>(CStr::from_bytes_with_nul(b"state\0").unwrap());
    unsafe {
        // Register methods using generic extern signatures to satisfy objc2
        decl.add_method(sel!(dealloc), dealloc_view as extern "C" fn(_, _));
        decl.add_method(sel!(mouseDown:), handle_view_event as extern "C" fn(_, _, _));
        decl.add_method(sel!(mouseUp:), handle_view_event as extern "C" fn(_, _, _));
        decl.add_method(sel!(rightMouseDown:), handle_view_event as extern "C" fn(_, _, _));
        decl.add_method(sel!(rightMouseUp:), handle_view_event as extern "C" fn(_, _, _));
        decl.add_method(sel!(otherMouseDown:), handle_view_event as extern "C" fn(_, _, _));
        decl.add_method(sel!(otherMouseUp:), handle_view_event as extern "C" fn(_, _, _));
        decl.add_method(sel!(mouseMoved:), handle_view_event as extern "C" fn(_, _, _));
        decl.add_method(sel!(mouseDragged:), handle_view_event as extern "C" fn(_, _, _));
        decl.add_method(sel!(scrollWheel:), handle_view_event as extern "C" fn(_, _, _));
        decl.add_method(sel!(flagsChanged:), handle_view_event as extern "C" fn(_, _, _));
        decl.add_method(sel!(makeBackingLayer), make_backing_layer as extern "C" fn(_, _) -> _);
        decl.add_method(
            sel!(viewDidChangeEffectiveAppearance),
            view_did_change_effective_appearance as extern "C" fn(_, _),
        );
        if let Some(proto) = Objc2AnyProtocol::get(CStr::from_bytes_with_nul(b"CALayerDelegate\0").unwrap()) {
            decl.add_protocol(proto);
        }
        decl.add_method(sel!(displayLayer:), display_layer as extern "C" fn(_, _, _));
    }
    let _ = decl.register();
}

pub struct StatusItem(Rc<RefCell<StatusItemState>>);

struct StatusItemState {
    native_item: Retained<NSStatusItem>,
    native_view: *mut Object,
    renderer: Renderer,
    scene: Option<Scene>,
    event_callback: Option<Box<dyn FnMut(Event) -> bool>>,
    appearance_changed_callback: Option<Box<dyn FnMut()>>,
}

impl StatusItem {
    pub fn add(fonts: Arc<dyn FontSystem>) -> Self {
        unsafe {
            let renderer = Renderer::new(false, fonts);
            // Create a status item with square length (-2.0 per AppKit docs)
            let status_bar = NSStatusBar::systemStatusBar();
            let native_item = status_bar.statusItemWithLength(-2.0);

            let button: *mut Object = unsafe { shims::status_item_button_ptr(&native_item) };
            let _: () = objc2::msg_send![button, setHidden: true];

            let view_cls: &Objc2AnyClass = Objc2AnyClass::get(CStr::from_bytes_with_nul(b"GPUIStatusItemView\0").unwrap())
                .expect("GPUIStatusItemView class not registered");
            let native_view_any: *mut Objc2AnyObject = objc2::msg_send![view_cls, alloc];
            let native_view = native_view_any as *mut Object;
            let state = Rc::new(RefCell::new(StatusItemState {
                native_item,
                native_view,
                renderer,
                scene: None,
                event_callback: None,
                appearance_changed_callback: None,
            }));

            let parent_view: *mut Object = objc2::msg_send![button, superview];
            let parent_view: *mut Object = objc2::msg_send![parent_view, superview];
            let parent_frame: NSRect = objc2::msg_send![parent_view, frame];
            let _: *mut Object = objc2::msg_send![native_view, initWithFrame: NSRect::new(NSPoint::new(0., 0.), parent_frame.size)];
            // Set ivar using objc2 helpers
            {
                let ivar_name = CStr::from_bytes_with_nul(b"state\0").unwrap();
                let view_ref: &mut Objc2AnyObject = unsafe { &mut *(native_view_any) };
                let ivar = view_ref.class().instance_variable(ivar_name).expect("state ivar missing");
                unsafe {
                    *ivar.load_mut::<*const c_void>(view_ref) = Weak::into_raw(Rc::downgrade(&state)) as *const c_void;
                }
            }
            let _: () = objc2::msg_send![native_view, setWantsBestResolutionOpenGLSurface: true];
            let _: () = objc2::msg_send![native_view, setWantsLayer: true];
            let _: () = objc2::msg_send![native_view, setLayerContentsRedrawPolicy: NSViewLayerContentsRedrawDuringViewResize];

            {
                let pv: &objc2_app_kit::NSView = &*(parent_view as *mut objc2_app_kit::NSView);
                let nv: &objc2_app_kit::NSView = &*(native_view as *mut objc2_app_kit::NSView);
                pv.addSubview(nv);
            }

            {
                let state = state.borrow();
                let scale_factor = state.scale_factor();
                let size = state.content_size() * scale_factor;
                #[cfg(feature = "macos-metal4")]
                {
                    use objc2_core_foundation::CGSize;
                    use objc2_quartz_core::CAMetalLayer;
                    let layer_ptr = state.renderer.layer();
                    let layer_ref: &CAMetalLayer = unsafe { &*(layer_ptr as *mut CAMetalLayer) };
                    layer_ref.setContentsScale(scale_factor as f64);
                    let cg = CGSize { width: size.x() as f64, height: size.y() as f64 };
                    layer_ref.setDrawableSize(cg);
                }
                #[cfg(not(feature = "macos-metal4"))]
                {
                    let layer = state.renderer.layer();
                    layer.set_contents_scale(scale_factor.into());
                    layer.set_drawable_size(metal::CGSize::new(size.x().into(), size.y().into()));
                }
            }

            Self(state)
        }
    }
}

impl platform::Window for StatusItem {
    fn bounds(&self) -> WindowBounds {
        self.0.borrow().bounds()
    }

    fn content_size(&self) -> Vector2F {
        self.0.borrow().content_size()
    }

    fn scale_factor(&self) -> f32 {
        self.0.borrow().scale_factor()
    }

    fn appearance(&self) -> platform::Appearance {
        unsafe {
            let button: *mut Object = if let Some(btn) = self.0.borrow().native_item.button() {
                btn as *const _ as *mut Object
            } else { std::ptr::null_mut() };
            let appearance: *mut Object = objc2::msg_send![button, effectiveAppearance];
            platform::Appearance::from_native(appearance as *mut objc2::runtime::AnyObject)
        }
    }

    fn screen(&self) -> Rc<dyn platform::Screen> {
        unsafe {
            let window = self.0.borrow().native_window();
            Rc::new(Screen { native_screen: objc2::msg_send![window, screen] })
        }
    }

    fn mouse_position(&self) -> Vector2F {
        unimplemented!()
    }

    fn as_any_mut(&mut self) -> &mut dyn std::any::Any {
        self
    }

    fn set_input_handler(&mut self, _: Box<dyn platform::InputHandler>) {}

    fn prompt(
        &self,
        _: crate::platform::PromptLevel,
        _: &str,
        _: &[&str],
    ) -> postage::oneshot::Receiver<usize> {
        unimplemented!()
    }

    fn activate(&self) {
        unimplemented!()
    }

    fn set_title(&mut self, _: &str) {
        unimplemented!()
    }

    fn set_edited(&mut self, _: bool) {
        unimplemented!()
    }

    fn show_character_palette(&self) {
        unimplemented!()
    }

    fn minimize(&self) {
        unimplemented!()
    }

    fn zoom(&self) {
        unimplemented!()
    }

    fn present_scene(&mut self, scene: Scene) {
        self.0.borrow_mut().scene = Some(scene);
        unsafe {
            let nv: &objc2_app_kit::NSView = &*(self.0.borrow().native_view as *mut objc2_app_kit::NSView);
            nv.setNeedsDisplay(true);
        }
    }

    fn toggle_fullscreen(&self) {
        unimplemented!()
    }

    fn on_event(&mut self, callback: Box<dyn FnMut(platform::Event) -> bool>) {
        self.0.borrow_mut().event_callback = Some(callback);
    }

    fn on_active_status_change(&mut self, _: Box<dyn FnMut(bool)>) {}

    fn on_resize(&mut self, _: Box<dyn FnMut()>) {}

    fn on_fullscreen(&mut self, _: Box<dyn FnMut(bool)>) {}

    fn on_moved(&mut self, _: Box<dyn FnMut()>) {}

    fn on_should_close(&mut self, _: Box<dyn FnMut() -> bool>) {}

    fn on_close(&mut self, _: Box<dyn FnOnce()>) {}

    fn on_appearance_changed(&mut self, callback: Box<dyn FnMut()>) {
        self.0.borrow_mut().appearance_changed_callback = Some(callback);
    }

    fn is_topmost_for_position(&self, _: Vector2F) -> bool {
        true
    }
}

impl StatusItemState {
    fn bounds(&self) -> WindowBounds {
        unsafe {
            let window: id = self.native_window();
            let win: &objc2_app_kit::NSWindow = &*(window as *mut objc2_app_kit::NSWindow);
            let screen_frame: NSRect = if let Some(screen) = win.screen() {
                screen.visibleFrame()
            } else {
                NSRect::new(NSPoint::new(0., 0.), NSSize::new(0., 0.))
            };
            let window_frame: NSRect = win.frame();
            let origin = vec2f(
                window_frame.origin.x as f32,
                (window_frame.origin.y - screen_frame.size.height - window_frame.size.height)
                    as f32,
            );
            let size = vec2f(
                window_frame.size.width as f32,
                window_frame.size.height as f32,
            );
            WindowBounds::Fixed(RectF::new(origin, size))
        }
    }

    fn content_size(&self) -> Vector2F {
        unsafe {
            let button: *mut Object = unsafe { shims::status_item_button_ptr(&self.native_item) };
            let parent: *mut Object = objc2::msg_send![button, superview];
            let parent: *mut Object = objc2::msg_send![parent, superview];
            let rect: NSRect = objc2::msg_send![parent, frame];
            let NSSize { width, height } = rect.size;
            vec2f(width as f32, height as f32)
        }
    }

    fn scale_factor(&self) -> f32 {
        unsafe {
            let button: *mut Object = objc2::msg_send![&*self.native_item, button];
            let window: *mut Object = objc2::msg_send![button, window];
            let win: &objc2_app_kit::NSWindow = &*(window as *mut objc2_app_kit::NSWindow);
            if let Some(screen) = win.screen() {
                screen.backingScaleFactor() as f32
            } else {
                2.0
            }
        }
    }

    pub fn native_window(&self) -> *mut Object {
        unsafe {
            let button: *mut Object = if let Some(btn) = self.native_item.button() {
                btn as *const _ as *mut Object
            } else { std::ptr::null_mut() };
            objc2::msg_send![button, window]
        }
    }
}

extern "C" fn dealloc_view(this: &Object, _: Sel) {
    unsafe {
        drop_state(this);

        let _: () = objc2::msg_send![super(this, objc2::class!(NSView)), dealloc];
    }
}

extern "C" fn handle_view_event(this: &Object, _: Sel, native_event: *mut Object) {
    unsafe {
        if let Some(state) = get_state(this).upgrade() {
            let mut state_borrow = state.as_ref().borrow_mut();
            let ev: &objc2_app_kit::NSEvent = &*(native_event as *mut objc2_app_kit::NSEvent);
            if let Some(event) = Event::from_native(ev, Some(state_borrow.content_size().y())) {
                if let Some(mut callback) = state_borrow.event_callback.take() {
                    drop(state_borrow);
                    callback(event);
                    state.borrow_mut().event_callback = Some(callback);
                }
            }
        }
    }
}

extern "C" fn make_backing_layer(this: &Object, _: Sel) -> *mut Object {
    if let Some(state) = unsafe { get_state(this).upgrade() } {
        let state = state.borrow();
        state.renderer.layer().as_ptr() as *mut Object
    } else {
        std::ptr::null_mut()
    }
}

extern "C" fn display_layer(this: &Object, _: Sel, _: *mut Object) {
    unsafe {
        if let Some(state) = get_state(this).upgrade() {
            let mut state = state.borrow_mut();
            if let Some(scene) = state.scene.take() {
                state.renderer.render(&scene);
            }
        }
    }
}

extern "C" fn view_did_change_effective_appearance(this: &Object, _: Sel) {
    unsafe {
        if let Some(state) = get_state(this).upgrade() {
            let mut state_borrow = state.as_ref().borrow_mut();
            if let Some(mut callback) = state_borrow.appearance_changed_callback.take() {
                drop(state_borrow);
                callback();
                state.borrow_mut().appearance_changed_callback = Some(callback);
            }
        }
    }
}

unsafe fn get_state(object: &Object) -> Weak<RefCell<StatusItemState>> {
    let ivar_name = CStr::from_bytes_with_nul(b"state\0").unwrap();
    let ivar = object.class().instance_variable(ivar_name).expect("state ivar missing");
    let raw: *const c_void = *ivar.load::<*const c_void>(object);
    let weak1 = Weak::from_raw(raw as *const RefCell<StatusItemState>);
    let weak2 = weak1.clone();
    let _ = Weak::into_raw(weak1);
    weak2
}

unsafe fn drop_state(object: &Object) {
    let ivar_name = CStr::from_bytes_with_nul(b"state\0").unwrap();
    let ivar = object.class().instance_variable(ivar_name).expect("state ivar missing");
    let raw: *const c_void = *ivar.load::<*const c_void>(object);
    Weak::from_raw(raw as *const RefCell<StatusItemState>);
}
extern_methods! {
    unsafe impl objc2_app_kit::NSStatusItem {
        #[unsafe(method(button))]
        fn button_ptr(&self) -> *mut Object;
    }
}
