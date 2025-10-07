use super::{BoolExt, MacDisplay, NSRange, renderer};
use crate::{
    AnyWindowHandle, Bounds, Capslock, DisplayLink, ExternalPaths, FileDropEvent,
    ForegroundExecutor, KeyDownEvent, Keystroke, Modifiers, ModifiersChangedEvent, MouseButton,
    MouseDownEvent, MouseMoveEvent, MouseUpEvent, Pixels, PlatformAtlas, PlatformDisplay,
    PlatformInput, PlatformWindow, Point, PromptButton, PromptLevel, RequestFrameOptions,
    SharedString, Size, SystemWindowTab, Timer, WindowAppearance, WindowBackgroundAppearance,
    WindowBounds, WindowControlArea, WindowKind, WindowParams, dispatch_get_main_queue,
    dispatch_sys::dispatch_async_f, platform::PlatformInputHandler, point, px, size,
};
use block2::StackBlock;
use cocoa::{
    appkit::{
        NSBackingStoreBuffered,
        NSWindowButton,
    },
    base::{id, nil},
    foundation::NSInteger,
};
use objc2_app_kit::{
    NSAppKitVersionNumber, NSAppKitVersionNumber12_0, NSDraggingInfo as ObjNSDraggingInfo,
    NSPasteboardTypeFileURL, NSTrackingAreaOptions, NSViewLayerContentsRedrawPolicy,
    NSEventModifierFlags as ObjcNSEventModifierFlags,
};
use objc2::rc::autoreleasepool;
use objc2::runtime::ProtocolObject;
use objc2_foundation::{NSArray as ObjNSArray, NSURL as ObjNSURL, NSString as ObjNSString, NSPoint, NSRect, NSSize, NSUserDefaults as ObjNSUserDefaults, NSUInteger};
use objc2_quartz_core::CALayer;
/* objc2 dynamic class migration pending for window.rs */

use core_graphics::display::{CGDirectDisplayID, CGPoint, CGRect};
use ctor::ctor;
use futures::channel::oneshot;
use objc::{
    class,
    msg_send,
    runtime::{BOOL, Class, NO, Object, Protocol, Sel, YES},
    sel, sel_impl,
};
use parking_lot::Mutex;
use raw_window_handle as rwh;
use smallvec::SmallVec;
use std::{
    cell::RefCell,
    ffi::c_void,
    mem,
    ops::Range,
    path::PathBuf,
    ptr::{self, NonNull},
    rc::Rc,
    sync::{Arc, Weak},
    time::Duration,
};
use util::ResultExt;

const WINDOW_STATE_IVAR: &str = "windowState";

static mut WINDOW_CLASS: *const Class = ptr::null();
static mut PANEL_CLASS: *const Class = ptr::null();
static mut VIEW_CLASS: *const Class = ptr::null();
static mut BLURRED_VIEW_CLASS: *const Class = ptr::null();

// Deprecated Cocoa constants: removed; using typed equivalents where needed
// https://developer.apple.com/documentation/appkit/nsdragoperation
type NSDragOperation = NSUInteger;
#[allow(non_upper_case_globals)]
const NSDragOperationNone: NSDragOperation = 0;
#[allow(non_upper_case_globals)]
const NSDragOperationCopy: NSDragOperation = 1;
#[derive(PartialEq)]
pub enum UserTabbingPreference {
    Never,
    Always,
    InFullScreen,
}

#[link(name = "CoreGraphics", kind = "framework")]
unsafe extern "C" {
    // Widely used private APIs; Apple uses them for their Terminal.app.
    fn CGSMainConnectionID() -> id;
    fn CGSSetWindowBackgroundBlurRadius(
        connection_id: id,
        window_id: NSInteger,
        radius: i64,
    ) -> i32;
}

#[ctor]
unsafe fn build_classes() {
    unsafe {
        WINDOW_CLASS = build_window_class("GPUIWindow", class!(NSWindow));
        PANEL_CLASS = build_window_class("GPUIPanel", class!(NSPanel));
        VIEW_CLASS = {
            let mut decl = ClassDecl::new("GPUIView", class!(NSView)).unwrap();
            decl.add_ivar::<*mut c_void>(WINDOW_STATE_IVAR);
            unsafe {
                decl.add_method(sel!(dealloc), dealloc_view as extern "C" fn(&Object, Sel));

                decl.add_method(
                    sel!(performKeyEquivalent:),
                    handle_key_equivalent as extern "C" fn(&Object, Sel, id) -> BOOL,
                );
                decl.add_method(sel!(keyDown:), handle_key_down as extern "C" fn(&Object, Sel, id));
                decl.add_method(sel!(keyUp:), handle_key_up as extern "C" fn(&Object, Sel, id));
                decl.add_method(sel!(mouseDown:), handle_view_event as extern "C" fn(&Object, Sel, id));
                decl.add_method(sel!(mouseUp:), handle_view_event as extern "C" fn(&Object, Sel, id));
                decl.add_method(
                    sel!(rightMouseDown:),
                    handle_view_event as extern "C" fn(&Object, Sel, id),
                );
                decl.add_method(
                    sel!(rightMouseUp:),
                    handle_view_event as extern "C" fn(&Object, Sel, id),
                );
                decl.add_method(sel!(otherMouseDown:), handle_view_event as extern "C" fn(&Object, Sel, id));
                decl.add_method(sel!(otherMouseUp:), handle_view_event as extern "C" fn(&Object, Sel, id));
                decl.add_method(sel!(mouseMoved:), handle_view_event as extern "C" fn(&Object, Sel, id));
                decl.add_method(sel!(mouseExited:), handle_view_event as extern "C" fn(&Object, Sel, id));
                decl.add_method(sel!(mouseDragged:), handle_view_event as extern "C" fn(&Object, Sel, id));
                decl.add_method(sel!(scrollWheel:), handle_view_event as extern "C" fn(&Object, Sel, id));
                decl.add_method(sel!(swipeWithEvent:), handle_view_event as extern "C" fn(&Object, Sel, id));
                decl.add_method(sel!(flagsChanged:), handle_view_event as extern "C" fn(&Object, Sel, id));

                decl.add_method(
                    sel!(makeBackingLayer),
                    make_backing_layer as extern "C" fn(&Object, Sel) -> id,
                );

                decl.add_protocol(Protocol::get("CALayerDelegate").unwrap());
                decl.add_method(
                    sel!(viewDidChangeBackingProperties),
                    view_did_change_backing_properties as extern "C" fn(&Object, Sel),
                );
                decl.add_method(
                    sel!(setFrameSize:),
                    set_frame_size as extern "C" fn(&Object, Sel, cocoa::foundation::NSSize),
                );
                decl.add_method(sel!(displayLayer:), display_layer as extern "C" fn(&Object, Sel, id));

                decl.add_protocol(Protocol::get("NSTextInputClient").unwrap());
                decl.add_method(
                    sel!(validAttributesForMarkedText),
                    valid_attributes_for_marked_text as extern "C" fn(&Object, Sel) -> id,
                );
                decl.add_method(sel!(hasMarkedText), has_marked_text as extern "C" fn(&Object, Sel) -> BOOL);
                decl.add_method(sel!(markedRange), marked_range as extern "C" fn(&Object, Sel) -> NSRange);
                decl.add_method(sel!(selectedRange), selected_range as extern "C" fn(&Object, Sel) -> NSRange);
                decl.add_method(
                    sel!(firstRectForCharacterRange:actualRange:),
                    first_rect_for_character_range as extern "C" fn(&Object, Sel, NSRange, id) -> cocoa::foundation::NSRect,
                );
                decl.add_method(
                    sel!(insertText:replacementRange:),
                    insert_text as extern "C" fn(&Object, Sel, id, NSRange),
                );
                decl.add_method(
                    sel!(setMarkedText:selectedRange:replacementRange:),
                    set_marked_text as extern "C" fn(&Object, Sel, id, NSRange, NSRange),
                );
                decl.add_method(sel!(unmarkText), unmark_text as extern "C" fn(&Object, Sel));
                decl.add_method(
                    sel!(attributedSubstringForProposedRange:actualRange:),
                    attributed_substring_for_proposed_range as extern "C" fn(&Object, Sel, NSRange, *mut c_void) -> id,
                );
                decl.add_method(
                    sel!(viewDidChangeEffectiveAppearance),
                    view_did_change_effective_appearance as extern "C" fn(&Object, Sel),
                );

                decl.add_method(
                    sel!(doCommandBySelector:),
                    do_command_by_selector as extern "C" fn(&Object, Sel, Sel),
                );
                decl.add_method(
                    sel!(acceptsFirstMouse:),
                    accepts_first_mouse as extern "C" fn(&Object, Sel, id) -> BOOL,
                );
                decl.add_method(
                    sel!(characterIndexForPoint:),
                    character_index_for_point as extern "C" fn(&Object, Sel, cocoa::foundation::NSPoint) -> u64,
                );
            }
            decl.register()
        };
        BLURRED_VIEW_CLASS = {
            let mut decl = ClassDecl::new("BlurredView", class!(NSVisualEffectView)).unwrap();
            unsafe {
                decl.add_method(
                    sel!(initWithFrame:),
                    blurred_view_init_with_frame as extern "C" fn(&Object, Sel, cocoa::foundation::NSRect) -> id,
                );
                decl.add_method(sel!(updateLayer), blurred_view_update_layer as extern "C" fn(&Object, Sel));
                decl.register()
            }
        };
        return;
        /* Register GPUIWindow (objc2 version commented out)
        let mut window_decl = Objc2ClassBuilder::new(
            CStr::from_bytes_with_nul(b"GPUIWindow\0").unwrap(),
            objc2::class!(NSWindow),
        )
        .expect("allocate GPUIWindow");
        if let Some(proto) = Objc2AnyProtocol::get(CStr::from_bytes_with_nul(b"NSWindowDelegate\0").unwrap()) {
            window_decl.add_protocol(proto);
        }
        window_decl.add_ivar::<*mut c_void>(CStr::from_bytes_with_nul(b"windowState\0").unwrap());
        window_decl.add_method(sel!(dealloc), dealloc_window as extern "C" fn(_, _));
        window_decl.add_method(sel!(canBecomeMainWindow), yes as extern "C" fn(_, _) -> _);
        window_decl.add_method(sel!(canBecomeKeyWindow), yes as extern "C" fn(_, _) -> _);
        window_decl.add_method(sel!(windowDidResize:), window_did_resize as extern "C" fn(_, _, _));
        window_decl.add_method(sel!(windowDidChangeOcclusionState:), window_did_change_occlusion_state as extern "C" fn(_, _, _));
        window_decl.add_method(sel!(windowWillEnterFullScreen:), window_will_enter_fullscreen as extern "C" fn(_, _, _));
        window_decl.add_method(sel!(windowWillExitFullScreen:), window_will_exit_fullscreen as extern "C" fn(_, _, _));
        window_decl.add_method(sel!(windowDidMove:), window_did_move as extern "C" fn(_, _, _));
        window_decl.add_method(sel!(windowDidChangeScreen:), window_did_change_screen as extern "C" fn(_, _, _));
        window_decl.add_method(sel!(windowDidBecomeKey:), window_did_change_key_status as extern "C" fn(_, _, _));
        window_decl.add_method(sel!(windowDidResignKey:), window_did_change_key_status as extern "C" fn(_, _, _));
        window_decl.add_method(sel!(windowShouldClose:), window_should_close as extern "C" fn(_, _, _) -> _);
        window_decl.add_method(sel!(close), close_window as extern "C" fn(_, _));
        window_decl.add_method(sel!(draggingEntered:), dragging_entered as extern "C" fn(_, _, _) -> _);
        window_decl.add_method(sel!(draggingUpdated:), dragging_updated as extern "C" fn(_, _, _) -> _);
        window_decl.add_method(sel!(draggingExited:), dragging_exited as extern "C" fn(_, _, _));
        window_decl.add_method(sel!(performDragOperation:), perform_drag_operation as extern "C" fn(_, _, _) -> _);
        window_decl.add_method(sel!(concludeDragOperation:), conclude_drag_operation as extern "C" fn(_, _, _));
        window_decl.add_method(sel!(addTitlebarAccessoryViewController:), add_titlebar_accessory_view_controller as extern "C" fn(_, _, _));
        window_decl.add_method(sel!(moveTabToNewWindow:), move_tab_to_new_window as extern "C" fn(_, _, _));
        window_decl.add_method(sel!(mergeAllWindows:), merge_all_windows as extern "C" fn(_, _, _));
        window_decl.add_method(sel!(selectNextTab:), select_next_tab as extern "C" fn(_, _, _));
        window_decl.add_method(sel!(selectPreviousTab:), select_previous_tab as extern "C" fn(_, _, _));
        window_decl.add_method(sel!(toggleTabBar:), toggle_tab_bar as extern "C" fn(_, _, _));
        WINDOW_CLASS = window_decl.register();

        // Register GPUIPanel
        let mut panel_decl = Objc2ClassBuilder::new(
            CStr::from_bytes_with_nul(b"GPUIPanel\0").unwrap(),
            objc2::class!(NSPanel),
        )
        .expect("allocate GPUIPanel");
        if let Some(proto) = Objc2AnyProtocol::get(CStr::from_bytes_with_nul(b"NSWindowDelegate\0").unwrap()) {
            panel_decl.add_protocol(proto);
        }
        panel_decl.add_ivar::<*mut c_void>(CStr::from_bytes_with_nul(b"windowState\0").unwrap());
        panel_decl.add_method(sel!(dealloc), dealloc_window as extern "C" fn(_, _));
        panel_decl.add_method(sel!(canBecomeMainWindow), yes as extern "C" fn(_, _) -> _);
        panel_decl.add_method(sel!(canBecomeKeyWindow), yes as extern "C" fn(_, _) -> _);
        panel_decl.add_method(sel!(windowDidResize:), window_did_resize as extern "C" fn(_, _, _));
        panel_decl.add_method(sel!(windowDidChangeOcclusionState:), window_did_change_occlusion_state as extern "C" fn(_, _, _));
        panel_decl.add_method(sel!(windowWillEnterFullScreen:), window_will_enter_fullscreen as extern "C" fn(_, _, _));
        panel_decl.add_method(sel!(windowWillExitFullScreen:), window_will_exit_fullscreen as extern "C" fn(_, _, _));
        panel_decl.add_method(sel!(windowDidMove:), window_did_move as extern "C" fn(_, _, _));
        panel_decl.add_method(sel!(windowDidChangeScreen:), window_did_change_screen as extern "C" fn(_, _, _));
        panel_decl.add_method(sel!(windowDidBecomeKey:), window_did_change_key_status as extern "C" fn(_, _, _));
        panel_decl.add_method(sel!(windowDidResignKey:), window_did_change_key_status as extern "C" fn(_, _, _));
        panel_decl.add_method(sel!(windowShouldClose:), window_should_close as extern "C" fn(_, _, _) -> _);
        panel_decl.add_method(sel!(close), close_window as extern "C" fn(_, _));
        panel_decl.add_method(sel!(draggingEntered:), dragging_entered as extern "C" fn(_, _, _) -> _);
        panel_decl.add_method(sel!(draggingUpdated:), dragging_updated as extern "C" fn(_, _, _) -> _);
        panel_decl.add_method(sel!(draggingExited:), dragging_exited as extern "C" fn(_, _, _));
        panel_decl.add_method(sel!(performDragOperation:), perform_drag_operation as extern "C" fn(_, _, _) -> _);
        panel_decl.add_method(sel!(concludeDragOperation:), conclude_drag_operation as extern "C" fn(_, _, _));
        panel_decl.add_method(sel!(addTitlebarAccessoryViewController:), add_titlebar_accessory_view_controller as extern "C" fn(_, _, _));
        panel_decl.add_method(sel!(moveTabToNewWindow:), move_tab_to_new_window as extern "C" fn(_, _, _));
        panel_decl.add_method(sel!(mergeAllWindows:), merge_all_windows as extern "C" fn(_, _, _));
        panel_decl.add_method(sel!(selectNextTab:), select_next_tab as extern "C" fn(_, _, _));
        panel_decl.add_method(sel!(selectPreviousTab:), select_previous_tab as extern "C" fn(_, _, _));
        panel_decl.add_method(sel!(toggleTabBar:), toggle_tab_bar as extern "C" fn(_, _, _));
        PANEL_CLASS = panel_decl.register();

        // Register GPUIView
        let mut view_decl = Objc2ClassBuilder::new(
            CStr::from_bytes_with_nul(b"GPUIView\0").unwrap(),
            objc2::class!(NSView),
        )
        .expect("allocate GPUIView");
        view_decl.add_ivar::<*mut c_void>(CStr::from_bytes_with_nul(b"windowState\0").unwrap());
        view_decl.add_method(sel!(dealloc), dealloc_view as extern "C" fn(_, _));
        view_decl.add_method(sel!(performKeyEquivalent:), handle_key_equivalent as extern "C" fn(_, _, _) -> _);
        view_decl.add_method(sel!(keyDown:), handle_key_down as extern "C" fn(_, _, _));
        view_decl.add_method(sel!(keyUp:), handle_key_up as extern "C" fn(_, _, _));
        view_decl.add_method(sel!(mouseDown:), handle_view_event as extern "C" fn(_, _, _));
        view_decl.add_method(sel!(mouseUp:), handle_view_event as extern "C" fn(_, _, _));
        view_decl.add_method(sel!(rightMouseDown:), handle_view_event as extern "C" fn(_, _, _));
        view_decl.add_method(sel!(rightMouseUp:), handle_view_event as extern "C" fn(_, _, _));
        view_decl.add_method(sel!(otherMouseDown:), handle_view_event as extern "C" fn(_, _, _));
        view_decl.add_method(sel!(otherMouseUp:), handle_view_event as extern "C" fn(_, _, _));
        view_decl.add_method(sel!(mouseMoved:), handle_view_event as extern "C" fn(_, _, _));
        view_decl.add_method(sel!(mouseExited:), handle_view_event as extern "C" fn(_, _, _));
        view_decl.add_method(sel!(mouseDragged:), handle_view_event as extern "C" fn(_, _, _));
        view_decl.add_method(sel!(scrollWheel:), handle_view_event as extern "C" fn(_, _, _));
        view_decl.add_method(sel!(swipeWithEvent:), handle_view_event as extern "C" fn(_, _, _));
        view_decl.add_method(sel!(flagsChanged:), handle_view_event as extern "C" fn(_, _, _));
        view_decl.add_method(sel!(makeBackingLayer), make_backing_layer as extern "C" fn(_, _) -> _);
        if let Some(proto) = Objc2AnyProtocol::get(CStr::from_bytes_with_nul(b"CALayerDelegate\0").unwrap()) {
            view_decl.add_protocol(proto);
        }
        view_decl.add_method(sel!(viewDidChangeBackingProperties), view_did_change_backing_properties as extern "C" fn(_, _));
        view_decl.add_method(sel!(setFrameSize:), set_frame_size as extern "C" fn(_, _, _));
        view_decl.add_method(sel!(displayLayer:), display_layer as extern "C" fn(_, _, _));
        if let Some(proto) = Objc2AnyProtocol::get(CStr::from_bytes_with_nul(b"NSTextInputClient\0").unwrap()) {
            view_decl.add_protocol(proto);
        }
        view_decl.add_method(sel!(validAttributesForMarkedText), valid_attributes_for_marked_text as extern "C" fn(_, _) -> _);
        view_decl.add_method(sel!(hasMarkedText), has_marked_text as extern "C" fn(_, _) -> _);
        view_decl.add_method(sel!(markedRange), marked_range as extern "C" fn(_, _) -> _);
        view_decl.add_method(sel!(selectedRange), selected_range as extern "C" fn(_, _) -> _);
        view_decl.add_method(sel!(firstRectForCharacterRange:actualRange:), first_rect_for_character_range as extern "C" fn(_, _, _, _) -> _);
        view_decl.add_method(sel!(insertText:replacementRange:), insert_text as extern "C" fn(_, _, _, _));
        view_decl.add_method(sel!(setMarkedText:selectedRange:replacementRange:), set_marked_text as extern "C" fn(_, _, _, _, _));
        view_decl.add_method(sel!(unmarkText), unmark_text as extern "C" fn(_, _));
        view_decl.add_method(sel!(attributedSubstringForProposedRange:actualRange:), attributed_substring_for_proposed_range as extern "C" fn(_, _, _, _) -> _);
        view_decl.add_method(sel!(viewDidChangeEffectiveAppearance), view_did_change_effective_appearance as extern "C" fn(_, _));
        view_decl.add_method(sel!(doCommandBySelector:), do_command_by_selector as extern "C" fn(_, _, _));
        view_decl.add_method(sel!(acceptsFirstMouse:), accepts_first_mouse as extern "C" fn(_, _, _) -> _);
        view_decl.add_method(sel!(characterIndexForPoint:), character_index_for_point as extern "C" fn(_, _, _) -> _);
        VIEW_CLASS = view_decl.register();

        // Register BlurredView
        let mut blur_decl = Objc2ClassBuilder::new(
            CStr::from_bytes_with_nul(b"BlurredView\0").unwrap(),
            objc2::class!(NSVisualEffectView),
        )
        .expect("allocate BlurredView");
        blur_decl.add_method(sel!(initWithFrame:), blurred_view_init_with_frame as extern "C" fn(_, _, _) -> _);
        blur_decl.add_method(sel!(updateLayer), blurred_view_update_layer as extern "C" fn(_, _));
        BLURRED_VIEW_CLASS = blur_decl.register();
        */
    }
}

pub(crate) fn convert_mouse_position(position: objc2_foundation::NSPoint, window_height: Pixels) -> Point<Pixels> {
    point(px(position.x as f32), window_height - px(position.y as f32))
}

// build_window_class helper removed; class registration done in build_classes.

struct MacWindowState {
    handle: AnyWindowHandle,
    executor: ForegroundExecutor,
    native_window: id,
    native_view: NonNull<Object>,
    blurred_view: Option<id>,
    display_link: Option<DisplayLink>,
    renderer: renderer::Renderer,
    request_frame_callback: Option<Box<dyn FnMut(RequestFrameOptions)>>,
    event_callback: Option<Box<dyn FnMut(PlatformInput) -> crate::DispatchEventResult>>,
    activate_callback: Option<Box<dyn FnMut(bool)>>,
    resize_callback: Option<Box<dyn FnMut(Size<Pixels>, f32)>>,
    moved_callback: Option<Box<dyn FnMut()>>,
    visibility_callback: Option<Box<dyn FnMut(bool)>>,
    should_close_callback: Option<Box<dyn FnMut() -> bool>>,
    close_callback: Option<Box<dyn FnOnce()>>,
    appearance_changed_callback: Option<Box<dyn FnMut()>>,
    input_handler: Option<PlatformInputHandler>,
    last_key_equivalent: Option<KeyDownEvent>,
    synthetic_drag_counter: usize,
    traffic_light_position: Option<Point<Pixels>>,
    transparent_titlebar: bool,
    previous_modifiers_changed_event: Option<PlatformInput>,
    keystroke_for_do_command: Option<Keystroke>,
    do_command_handled: Option<bool>,
    external_files_dragged: bool,
    // Whether the next left-mouse click is also the focusing click.
    first_mouse: bool,
    fullscreen_restore_bounds: Bounds<Pixels>,
    move_tab_to_new_window_callback: Option<Box<dyn FnMut()>>,
    merge_all_windows_callback: Option<Box<dyn FnMut()>>,
    select_next_tab_callback: Option<Box<dyn FnMut()>>,
    select_previous_tab_callback: Option<Box<dyn FnMut()>>,
    toggle_tab_bar_callback: Option<Box<dyn FnMut()>>,
    activated_least_once: bool,
}

impl MacWindowState {
    fn move_traffic_light(&self) {
        if let Some(traffic_light_position) = self.traffic_light_position {
            if self.is_fullscreen() {
                // Moving traffic lights while fullscreen doesn't work,
                // see https://github.com/zed-industries/zed/issues/4712
                return;
            }

            let titlebar_height = self.titlebar_height();

            unsafe {
                let close_button: id = msg_send![
                    self.native_window,
                    standardWindowButton: NSWindowButton::NSWindowCloseButton
                ];
                let min_button: id = msg_send![
                    self.native_window,
                    standardWindowButton: NSWindowButton::NSWindowMiniaturizeButton
                ];
                let zoom_button: id = msg_send![
                    self.native_window,
                    standardWindowButton: NSWindowButton::NSWindowZoomButton
                ];

                let mut close_button_frame: CGRect = msg_send![close_button, frame];
                let mut min_button_frame: CGRect = msg_send![min_button, frame];
                let mut zoom_button_frame: CGRect = msg_send![zoom_button, frame];
                let mut origin = point(
                    traffic_light_position.x,
                    titlebar_height
                        - traffic_light_position.y
                        - px(close_button_frame.size.height as f32),
                );
                let button_spacing =
                    px((min_button_frame.origin.x - close_button_frame.origin.x) as f32);

                close_button_frame.origin = CGPoint::new(origin.x.into(), origin.y.into());
                let _: () = msg_send![close_button, setFrame: close_button_frame];
                origin.x += button_spacing;

                min_button_frame.origin = CGPoint::new(origin.x.into(), origin.y.into());
                let _: () = msg_send![min_button, setFrame: min_button_frame];
                origin.x += button_spacing;

                zoom_button_frame.origin = CGPoint::new(origin.x.into(), origin.y.into());
                let _: () = msg_send![zoom_button, setFrame: zoom_button_frame];
                origin.x += button_spacing;
            }
        }
    }

    fn start_display_link(&mut self) {
        self.stop_display_link();
        {
            let win: &objc2_app_kit::NSWindow = unsafe { &*(self.native_window as *mut objc2_app_kit::NSWindow) };
            if !win.occlusionState().contains(objc2_app_kit::NSWindowOcclusionState::Visible) {
                return;
            }
        }
        // Use typed NSWindow/NSScreen to determine display ID
        let display_id = unsafe {
            let win: &objc2_app_kit::NSWindow = &*(self.native_window as *mut objc2_app_kit::NSWindow);
            if let Some(screen) = win.screen() {
                display_id_for_typed_screen(&screen)
            } else {
                // Fallback to primary display if no screen
                MacDisplay::primary().0
            }
        };
        if let Some(mut display_link) =
            DisplayLink::new(display_id, self.native_view.as_ptr() as *mut c_void, step).log_err()
        {
            display_link.start().log_err();
            self.display_link = Some(display_link);
        }
    }

    fn stop_display_link(&mut self) {
        self.display_link = None;
    }

    fn is_maximized(&self) -> bool {
        unsafe {
            let bounds = self.bounds();
            // Use typed NSWindow/NSScreen to compute visible frame size
            let win: &objc2_app_kit::NSWindow = &*(self.native_window as *mut objc2_app_kit::NSWindow);
            let screen_size = if let Some(screen) = win.screen() {
                let vf = screen.visibleFrame();
                crate::platform::mac::size(px(vf.size.width as f32), px(vf.size.height as f32))
            } else {
                crate::DEFAULT_WINDOW_SIZE
            };
            bounds.size == screen_size
        }
    }

    fn is_fullscreen(&self) -> bool {
        let ev: &objc2::runtime::AnyObject = unsafe { &*(self.native_window as *mut objc2::runtime::AnyObject) };
        let style_mask: objc2_app_kit::NSWindowStyleMask = unsafe { objc2::msg_send![ev, styleMask] };
        style_mask.contains(objc2_app_kit::NSWindowStyleMask::FullScreen)
    }

    fn bounds(&self) -> Bounds<Pixels> {
        // Use typed NSWindow/NSScreen for geometry
        let win: &objc2_app_kit::NSWindow = unsafe { &*(self.native_window as *mut objc2_app_kit::NSWindow) };
        let mut window_frame: objc2_foundation::NSRect = win.frame();
        let Some(screen) = win.screen() else {
            return Bounds::new(point(px(0.), px(0.)), crate::DEFAULT_WINDOW_SIZE);
        };
        let screen_frame: objc2_foundation::NSRect = screen.frame();

        // Flip the y coordinate to be top-left origin
        window_frame.origin.y =
            screen_frame.size.height - window_frame.origin.y - window_frame.size.height;

        Bounds::new(
            point(
                px((window_frame.origin.x - screen_frame.origin.x) as f32),
                px((window_frame.origin.y + screen_frame.origin.y) as f32),
            ),
            size(
                px(window_frame.size.width as f32),
                px(window_frame.size.height as f32),
            ),
        )
    }

    fn content_size(&self) -> Size<Pixels> {
        let (width, height) = unsafe {
            let content_view: id = msg_send![self.native_window, contentView];
            let rect: objc2_foundation::NSRect = msg_send![content_view, frame];
            (rect.size.width, rect.size.height)
        };
        size(px(width as f32), px(height as f32))
    }

    fn scale_factor(&self) -> f32 {
        get_scale_factor(self.native_window)
    }

    fn titlebar_height(&self) -> Pixels {
        // Use typed NSWindow frame and contentLayoutRect
        let win: &objc2_app_kit::NSWindow = unsafe { &*(self.native_window as *mut objc2_app_kit::NSWindow) };
        let frame: objc2_foundation::NSRect = win.frame();
        let content_layout_rect: CGRect = unsafe { msg_send![self.native_window, contentLayoutRect] };
        px((frame.size.height - content_layout_rect.size.height) as f32)
    }

    fn window_bounds(&self) -> WindowBounds {
        if self.is_fullscreen() {
            WindowBounds::Fullscreen(self.fullscreen_restore_bounds)
        } else {
            WindowBounds::Windowed(self.bounds())
        }
    }
}

unsafe impl Send for MacWindowState {}

pub(crate) struct MacWindow(Arc<Mutex<MacWindowState>>);

impl MacWindow {
    pub fn open(
        handle: AnyWindowHandle,
        WindowParams {
            bounds,
            titlebar,
            kind,
            is_movable,
            is_resizable,
            is_minimizable,
            focus,
            show,
            display_id,
            window_min_size,
            tabbing_identifier,
        }: WindowParams,
        executor: ForegroundExecutor,
        renderer_context: renderer::Context,
    ) -> Self {
        unsafe {
            let window = autoreleasepool(|_| {

            let allows_automatic_window_tabbing = tabbing_identifier.is_some();
            if allows_automatic_window_tabbing {
                let () = msg_send![class!(NSWindow), setAllowsAutomaticWindowTabbing: YES];
            } else {
                let () = msg_send![class!(NSWindow), setAllowsAutomaticWindowTabbing: NO];
            }

            let mut style_mask: objc2_app_kit::NSWindowStyleMask;
            if let Some(titlebar) = titlebar.as_ref() {
                style_mask = objc2_app_kit::NSWindowStyleMask::Closable | objc2_app_kit::NSWindowStyleMask::Titled;

                if is_resizable {
                    style_mask |= objc2_app_kit::NSWindowStyleMask::Resizable;
                }

                if is_minimizable {
                    style_mask |= objc2_app_kit::NSWindowStyleMask::Miniaturizable;
                }

                if titlebar.appears_transparent {
                    style_mask |= objc2_app_kit::NSWindowStyleMask::FullSizeContentView;
                }
            } else {
                style_mask = objc2_app_kit::NSWindowStyleMask::Titled
                    | objc2_app_kit::NSWindowStyleMask::FullSizeContentView;
            }

            let native_window: id = match kind {
                WindowKind::Normal => msg_send![WINDOW_CLASS, alloc],
                WindowKind::PopUp => {
                    style_mask |= objc2_app_kit::NSWindowStyleMask::NonactivatingPanel;
                    msg_send![PANEL_CLASS, alloc]
                }
            };

            let display = display_id
                .and_then(MacDisplay::find_by_id)
                .unwrap_or_else(MacDisplay::primary);

            let mut target_screen = nil;
            let mut screen_frame = None;

            let screens_id: id = msg_send![class!(NSScreen), screens];
            let screens: &objc2_foundation::NSArray<objc2_app_kit::NSScreen> =
                unsafe { &*(screens_id as *mut objc2_foundation::NSArray<objc2_app_kit::NSScreen>) };
            for i in 0..screens.len() {
                let sref = screens.objectAtIndex(i);
                let frame: objc2_foundation::NSRect = sref.frame();
                let display_id = display_id_for_typed_screen(&*sref);
                if display_id == display.0 {
                    screen_frame = Some(frame);
                    let sid: id = objc2::rc::Retained::as_ptr(&sref) as *mut objc::runtime::Object;
                    target_screen = sid;
                }
            }

            let screen_frame = screen_frame.unwrap_or_else(|| {
                let screen: id = msg_send![class!(NSScreen), mainScreen];
                target_screen = screen;
                let sref: &objc2_app_kit::NSScreen = unsafe { &*(screen as *mut objc2_app_kit::NSScreen) };
                sref.frame()
            });

            let window_rect = objc2_foundation::NSRect::new(
                objc2_foundation::NSPoint::new(
                    screen_frame.origin.x + bounds.origin.x.0 as f64,
                    screen_frame.origin.y
                        + (display.bounds().size.height - bounds.origin.y).0 as f64,
                ),
                objc2_foundation::NSSize::new(bounds.size.width.0 as f64, bounds.size.height.0 as f64),
            );

            let native_window: id = msg_send![
                native_window,
                initWithContentRect: window_rect
                styleMask: style_mask
                backing: NSBackingStoreBuffered
                defer: NO
                screen: target_screen
            ];
            assert!(!native_window.is_null());
            // Register for file URL drags using typed objc2 API
            let win_ref: &objc2_app_kit::NSWindow = &*(native_window as *mut objc2_app_kit::NSWindow);
            let types = ObjNSArray::arrayWithObject(NSPasteboardTypeFileURL);
            win_ref.registerForDraggedTypes(&types);
            let () = msg_send![
                native_window,
                setReleasedWhenClosed: NO
            ];

            let content_view: id = msg_send![native_window, contentView];
            let native_view: id = msg_send![VIEW_CLASS, alloc];
            let content_bounds: objc2_foundation::NSRect = msg_send![content_view, bounds];
            let native_view: id = msg_send![native_view, initWithFrame: content_bounds];
            assert!(!native_view.is_null());

            let mut window = Self(Arc::new(Mutex::new(MacWindowState {
                handle,
                executor,
                native_window,
                native_view: NonNull::new_unchecked(native_view),
                blurred_view: None,
                display_link: None,
                renderer: renderer::new_renderer(
                    renderer_context,
                    native_window as *mut _,
                    native_view as *mut _,
                    bounds.size.map(|pixels| pixels.0),
                    false,
                ),
                request_frame_callback: None,
                event_callback: None,
                activate_callback: None,
                resize_callback: None,
                moved_callback: None,
                visibility_callback: None,
                should_close_callback: None,
                close_callback: None,
                appearance_changed_callback: None,
                input_handler: None,
                last_key_equivalent: None,
                synthetic_drag_counter: 0,
                traffic_light_position: titlebar
                    .as_ref()
                    .and_then(|titlebar| titlebar.traffic_light_position),
                transparent_titlebar: titlebar
                    .as_ref()
                    .is_none_or(|titlebar| titlebar.appears_transparent),
                previous_modifiers_changed_event: None,
                keystroke_for_do_command: None,
                do_command_handled: None,
                external_files_dragged: false,
                first_mouse: false,
                fullscreen_restore_bounds: Bounds::default(),
                move_tab_to_new_window_callback: None,
                merge_all_windows_callback: None,
                select_next_tab_callback: None,
                select_previous_tab_callback: None,
                toggle_tab_bar_callback: None,
                activated_least_once: false,
            })));

            (*native_window).set_ivar(
                WINDOW_STATE_IVAR,
                Arc::into_raw(window.0.clone()) as *const c_void,
            );
            // Set typed delegate to our window subclass instance
            {
                let wref: &objc2_app_kit::NSWindow = &*(native_window as *mut objc2_app_kit::NSWindow);
                let delegate_ref: &ProtocolObject<dyn objc2_app_kit::NSWindowDelegate> =
                    &*(native_window as *mut ProtocolObject<dyn objc2_app_kit::NSWindowDelegate>);
                wref.setDelegate(Some(delegate_ref));
            }
            (*native_view).set_ivar(
                WINDOW_STATE_IVAR,
                Arc::into_raw(window.0.clone()) as *const c_void,
            );

            if let Some(title) = titlebar
                .as_ref()
                .and_then(|t| t.title.as_ref().map(AsRef::as_ref))
            {
                window.set_title(title);
            }

            {
                let win: &objc2_app_kit::NSWindow = &*(native_window as *mut objc2_app_kit::NSWindow);
                win.setMovable(is_movable);
            }

            if let Some(window_min_size) = window_min_size {
                let win: &objc2_app_kit::NSWindow = &*(native_window as *mut objc2_app_kit::NSWindow);
                win.setContentMinSize(objc2_foundation::NSSize::new(
                    window_min_size.width.to_f64(),
                    window_min_size.height.to_f64(),
                ));
            }

            if titlebar.is_none_or(|titlebar| titlebar.appears_transparent) {
                let win: &objc2_app_kit::NSWindow = &*(native_window as *mut objc2_app_kit::NSWindow);
                win.setTitlebarAppearsTransparent(true);
                win.setTitleVisibility(objc2_app_kit::NSWindowTitleVisibility::Hidden);
            }

            let mask: objc2_app_kit::NSAutoresizingMaskOptions =
                objc2_app_kit::NSAutoresizingMaskOptions::ViewWidthSizable
                | objc2_app_kit::NSAutoresizingMaskOptions::ViewHeightSizable;
            let _: () = msg_send![native_view, setAutoresizingMask: mask];
            let _: () = msg_send![native_view, setWantsBestResolutionOpenGLSurface: true];

            // From winit crate: On Mojave, views automatically become layer-backed shortly after
            // being added to a native_window. Changing the layer-backedness of a view breaks the
            // association between the view and its associated OpenGL context. To work around this,
            // on we explicitly make the view layer-backed up front so that AppKit doesn't do it
            // itself and break the association with its context.
            {
                let vref: &objc2_app_kit::NSView = &*(native_view as *mut objc2_app_kit::NSView);
                vref.setWantsLayer(true);
            }
            {
                let vref: &objc2_app_kit::NSView = &*(native_view as *mut objc2_app_kit::NSView);
                vref.setLayerContentsRedrawPolicy(NSViewLayerContentsRedrawPolicy::DuringViewResize);
            }

            {
                let cview: &objc2_app_kit::NSView = &*(content_view as *mut objc2_app_kit::NSView);
                let vref: &objc2_app_kit::NSView = &*(native_view as *mut objc2_app_kit::NSView);
                cview.addSubview(vref);
            }
            {
                let wref: &objc2_app_kit::NSWindow = &*(native_window as *mut objc2_app_kit::NSWindow);
                let vref: &objc2_app_kit::NSView = &*(native_view as *mut objc2_app_kit::NSView);
                wref.makeFirstResponder(Some(vref.as_ref()));
            }

            match kind {
                WindowKind::Normal => {
                    let win: &objc2_app_kit::NSWindow = &*(native_window as *mut objc2_app_kit::NSWindow);
                    win.setLevel(objc2_app_kit::NSNormalWindowLevel);
                    win.setAcceptsMouseMovedEvents(true);

                    if let Some(tabbing_identifier) = tabbing_identifier {
                        let tabbing_id = objc2_foundation::NSString::from_str(tabbing_identifier.as_str());
                        let tabbing_id_ref: &objc2_foundation::NSString = &*tabbing_id;
                        let _: () = msg_send![native_window, setTabbingIdentifier: tabbing_id_ref];
                    } else {
                        let _: () = msg_send![native_window, setTabbingIdentifier: std::ptr::null_mut::<objc::runtime::Object>()];
                    }
                }
                WindowKind::PopUp => {
                    // Use a tracking area to allow receiving MouseMoved events even when
                    // the window or application aren't active, which is often the case
                    // e.g. for notification windows.
                    let rect = NSRect::new(NSPoint::new(0., 0.), NSSize::new(0., 0.));
                    let options = NSTrackingAreaOptions::MouseEnteredAndExited
                        | NSTrackingAreaOptions::MouseMoved
                        | NSTrackingAreaOptions::ActiveAlways
                        | NSTrackingAreaOptions::InVisibleRect;
                    let tracking_area: id = msg_send![class!(NSTrackingArea), alloc];
                    let tracking_area: id = msg_send![
                        tracking_area,
                        initWithRect: rect
                        options: options
                        owner: native_view
                        userInfo: nil
                    ];
                    let _: () = msg_send![tracking_area, autorelease];
                    let _: () = msg_send![native_view, addTrackingArea: tracking_area];

                    {
                        let win: &objc2_app_kit::NSWindow = &*(native_window as *mut objc2_app_kit::NSWindow);
                        win.setLevel(objc2_app_kit::NSPopUpMenuWindowLevel);
                        win.setAnimationBehavior(objc2_app_kit::NSWindowAnimationBehavior::UtilityWindow);
                        let behavior = objc2_app_kit::NSWindowCollectionBehavior::CanJoinAllSpaces
                            | objc2_app_kit::NSWindowCollectionBehavior::FullScreenAuxiliary;
                        win.setCollectionBehavior(behavior);
                    }
                }
            }

            let app: *mut objc2::runtime::AnyObject =
                objc2::msg_send![objc2::class!(NSApplication), sharedApplication];
            let main_window: *mut objc2::runtime::AnyObject = objc2::msg_send![&*app, mainWindow];
            if allows_automatic_window_tabbing
                && !main_window.is_null()
                && (main_window as *mut objc::runtime::Object) != native_window
            {
                let main_window_is_fullscreen = {
                    let ev: &objc2::runtime::AnyObject = &*(main_window as *mut objc2::runtime::AnyObject);
                    let sm: objc2_app_kit::NSWindowStyleMask = objc2::msg_send![ev, styleMask];
                    sm.contains(objc2_app_kit::NSWindowStyleMask::FullScreen)
                };
                let user_tabbing_preference = Self::get_user_tabbing_preference()
                    .unwrap_or(UserTabbingPreference::InFullScreen);
                let should_add_as_tab = user_tabbing_preference == UserTabbingPreference::Always
                    || user_tabbing_preference == UserTabbingPreference::InFullScreen
                        && main_window_is_fullscreen;

                if should_add_as_tab {
                    let main_window_can_tab: bool =
                        objc2::msg_send![&*main_window, respondsToSelector: objc2::sel!(addTabbedWindow:ordered:)];
                    let main_window_visible: bool = objc2::msg_send![&*main_window, isVisible];

                    if main_window_can_tab && main_window_visible {
                        let win_obj_objc: &objc::runtime::Object = &*(main_window as *mut objc::runtime::Object);
                        // Pass primitive for NSWindowOrderingMode::Above (1)
                        let _: () = msg_send![win_obj_objc, addTabbedWindow: native_window ordered: 1_i64];

                        // Ensure the window is visible immediately after adding the tab, since the tab bar is updated with a new entry at this point.
                        // Note: Calling orderFront here can break fullscreen mode (makes fullscreen windows exit fullscreen), so only do this if the main window is not fullscreen.
                        if !main_window_is_fullscreen {
                            let wref: &objc2_app_kit::NSWindow = &*(native_window as *mut objc2_app_kit::NSWindow);
                            wref.orderFront(None);
                        }
                    }
                }
            }

            if focus && show {
                let win: &objc2_app_kit::NSWindow = &*(native_window as *mut objc2_app_kit::NSWindow);
                win.makeKeyAndOrderFront(None);
            } else if show {
                let win: &objc2_app_kit::NSWindow = &*(native_window as *mut objc2_app_kit::NSWindow);
                win.orderFront(None);
            }

            // Set the initial position of the window to the specified origin.
            // Although we already specified the position using `initWithContentRect_styleMask_backing_defer_screen_`,
            // the window position might be incorrect if the main screen (the screen that contains the window that has focus)
            //  is different from the primary screen.
            {
                let win: &objc2_app_kit::NSWindow = &*(native_window as *mut objc2_app_kit::NSWindow);
                win.setFrameTopLeftPoint(window_rect.origin);
            }
            window.0.lock().move_traffic_light();

            // Return the constructed window from the autoreleasepool scope
            window
            });

            window
        }
    }

    pub fn active_window() -> Option<AnyWindowHandle> {
        unsafe {
            let app: *mut objc2::runtime::AnyObject = objc2::msg_send![objc2::class!(NSApplication), sharedApplication];
            let main_window: *mut objc2::runtime::AnyObject = objc2::msg_send![&*app, mainWindow];
            if main_window.is_null() {
                return None;
            }

            let win_obj: &objc::runtime::Object = &*(main_window as *mut objc::runtime::Object);
            if msg_send![win_obj, isKindOfClass: WINDOW_CLASS] {
                let handle = get_window_state(win_obj).lock().handle;
                Some(handle)
            } else {
                None
            }
        }
    }

    pub fn ordered_windows() -> Vec<AnyWindowHandle> {
        unsafe {
            let app: *mut objc2::runtime::AnyObject = objc2::msg_send![objc2::class!(NSApplication), sharedApplication];
            let windows_id: *mut objc2::runtime::AnyObject = objc2::msg_send![&*app, orderedWindows];
            let windows: &ObjNSArray<objc2_app_kit::NSWindow> =
                &*(windows_id as *mut ObjNSArray<objc2_app_kit::NSWindow>);

            let mut window_handles = Vec::new();
            for i in 0..windows.len() {
                let window = windows.objectAtIndex(i as objc2_foundation::NSUInteger);
                let win_obj: &objc::runtime::Object = &*(
                    (&*window as *const objc2_app_kit::NSWindow)
                        as *mut objc::runtime::Object
                );
                if msg_send![win_obj, isKindOfClass: WINDOW_CLASS] {
                    let handle = get_window_state(win_obj).lock().handle;
                    window_handles.push(handle);
                }
            }

            window_handles
        }
    }

    pub fn get_user_tabbing_preference() -> Option<UserTabbingPreference> {
        let defaults = ObjNSUserDefaults::standardUserDefaults();
        let dict = defaults.dictionaryRepresentation();
        let key = ObjNSString::from_str("AppleWindowTabbingMode");
        let value = dict.objectForKey(&key);
        let value_str = if let Some(any) = value {
            if let Ok(ns) = any.downcast::<ObjNSString>() {
                objc2::rc::autoreleasepool(|pool| unsafe { ns.to_str(pool).to_owned() })
            } else {
                String::new()
            }
        } else {
            String::new()
        };

        match value_str.as_str() {
            "manual" => Some(UserTabbingPreference::Never),
            "always" => Some(UserTabbingPreference::Always),
            _ => Some(UserTabbingPreference::InFullScreen),
        }
    }
}

impl Drop for MacWindow {
    fn drop(&mut self) {
        let mut this = self.0.lock();
        this.renderer.destroy();
        let window = this.native_window;
        this.display_link.take();
        {
            let wref: &objc2_app_kit::NSWindow = unsafe { &*(this.native_window as *mut objc2_app_kit::NSWindow) };
            wref.setDelegate(None);
        }
        this.input_handler.take();
        this.executor
            .spawn(async move {
                let win: &objc2_app_kit::NSWindow = unsafe { &*(window as *mut objc2_app_kit::NSWindow) };
                win.close();
                unsafe { let _: () = msg_send![window, autorelease]; }
            })
            .detach();
    }
}

impl PlatformWindow for MacWindow {
    fn bounds(&self) -> Bounds<Pixels> {
        self.0.as_ref().lock().bounds()
    }

    fn window_bounds(&self) -> WindowBounds {
        self.0.as_ref().lock().window_bounds()
    }

    fn is_maximized(&self) -> bool {
        self.0.as_ref().lock().is_maximized()
    }

    fn content_size(&self) -> Size<Pixels> {
        self.0.as_ref().lock().content_size()
    }

    fn resize(&mut self, size: Size<Pixels>) {
        let this = self.0.lock();
        let window = this.native_window;
        this.executor
            .spawn(async move {
                let win: &objc2_app_kit::NSWindow = unsafe { &*(window as *mut objc2_app_kit::NSWindow) };
                let new_size = NSSize::new(size.width.0 as f64, size.height.0 as f64);
                win.setContentSize(new_size);
            })
            .detach();
    }

    fn merge_all_windows(&self) {
        let native_window = self.0.lock().native_window;
        unsafe extern "C" fn merge_windows_async(context: *mut std::ffi::c_void) {
            let native_window = context as id;
            let win: &objc2_app_kit::NSWindow = unsafe { &*(native_window as *mut objc2_app_kit::NSWindow) };
            win.mergeAllWindows(None);
        }

        unsafe {
            dispatch_async_f(
                dispatch_get_main_queue(),
                native_window as *mut std::ffi::c_void,
                Some(merge_windows_async),
            );
        }
    }

    fn move_tab_to_new_window(&self) {
        let native_window = self.0.lock().native_window;
        unsafe extern "C" fn move_tab_async(context: *mut std::ffi::c_void) {
            let native_window = context as id;
            let win: &objc2_app_kit::NSWindow = unsafe { &*(native_window as *mut objc2_app_kit::NSWindow) };
            win.moveTabToNewWindow(None);
            win.makeKeyAndOrderFront(None);
        }

        unsafe {
            dispatch_async_f(
                dispatch_get_main_queue(),
                native_window as *mut std::ffi::c_void,
                Some(move_tab_async),
            );
        }
    }

    fn toggle_window_tab_overview(&self) {
        let native_window = self.0.lock().native_window;
        unsafe {
            let _: () = msg_send![native_window, toggleTabOverview:nil];
        }
    }

    fn set_tabbing_identifier(&self, tabbing_identifier: Option<String>) {
        let native_window = self.0.lock().native_window;
        unsafe {
            let allows_automatic_window_tabbing = tabbing_identifier.is_some();
            if allows_automatic_window_tabbing {
                let () = msg_send![class!(NSWindow), setAllowsAutomaticWindowTabbing: YES];
            } else {
                let () = msg_send![class!(NSWindow), setAllowsAutomaticWindowTabbing: NO];
            }

            if let Some(tabbing_identifier) = tabbing_identifier {
                let tabbing_id = objc2_foundation::NSString::from_str(tabbing_identifier.as_str());
                let tabbing_id_ref: &objc2_foundation::NSString = &*tabbing_id;
                let _: () = msg_send![native_window, setTabbingIdentifier: tabbing_id_ref];
            } else {
                let _: () = msg_send![native_window, setTabbingIdentifier: std::ptr::null_mut::<objc::runtime::Object>()];
            }
        }
    }

    fn scale_factor(&self) -> f32 {
        self.0.as_ref().lock().scale_factor()
    }

    fn appearance(&self) -> WindowAppearance {
        unsafe {
            // Treat the native window as an objc2 object to fetch the appearance
            let native: *mut objc2::runtime::AnyObject = self.0.lock().native_window as *mut _;
            let appearance: *mut objc2::runtime::AnyObject = objc2::msg_send![&*native, effectiveAppearance];
            WindowAppearance::from_native(appearance)
        }
    }

    fn display(&self) -> Option<Rc<dyn PlatformDisplay>> {
        unsafe {
            let native_window = self.0.lock().native_window;
            let screen: id = msg_send![native_window, screen];
            if screen.is_null() {
                return None;
            }
            let sref: &objc2_app_kit::NSScreen = &*(screen as *mut objc2_app_kit::NSScreen);
            // Use typed deviceDescription but fetch key via msg_send for NSScreenNumber
            let dict = sref.deviceDescription();
            let key = objc2_foundation::NSString::from_str("NSScreenNumber");
            let val = dict.objectForKey_unchecked(&key);
            if let Some(any) = val {
                let any_ref: &objc2::runtime::AnyObject = any;
                let screen_number: u32 = objc2::msg_send![any_ref, unsignedIntValue];
                Some(Rc::new(MacDisplay(screen_number)))
            } else {
                None
            }
        }
    }

    fn mouse_position(&self) -> Point<Pixels> {
        let native_window = self.0.lock().native_window;
        // Use typed NSWindow API for mouse location
        let win: &objc2_app_kit::NSWindow = unsafe { &*(native_window as *mut objc2_app_kit::NSWindow) };
        let position = win.mouseLocationOutsideOfEventStream();
        convert_mouse_position(position, self.content_size().height)
    }

    fn modifiers(&self) -> Modifiers {
        unsafe {
            let modifiers: ObjcNSEventModifierFlags = msg_send![class!(NSEvent), modifierFlags];

            let control = modifiers.contains(ObjcNSEventModifierFlags::Control);
            let alt = modifiers.contains(ObjcNSEventModifierFlags::Option);
            let shift = modifiers.contains(ObjcNSEventModifierFlags::Shift);
            let command = modifiers.contains(ObjcNSEventModifierFlags::Command);
            let function = modifiers.contains(ObjcNSEventModifierFlags::Function);

            Modifiers {
                control,
                alt,
                shift,
                platform: command,
                function,
            }
        }
    }

    fn capslock(&self) -> Capslock {
        unsafe {
            let modifiers: ObjcNSEventModifierFlags = msg_send![class!(NSEvent), modifierFlags];

            Capslock {
                on: modifiers.contains(ObjcNSEventModifierFlags::CapsLock),
            }
        }
    }

    fn set_input_handler(&mut self, input_handler: PlatformInputHandler) {
        self.0.as_ref().lock().input_handler = Some(input_handler);
    }

    fn take_input_handler(&mut self) -> Option<PlatformInputHandler> {
        self.0.as_ref().lock().input_handler.take()
    }

    fn prompt(
        &self,
        level: PromptLevel,
        msg: &str,
        detail: Option<&str>,
        answers: &[PromptButton],
    ) -> Option<oneshot::Receiver<usize>> {
        // macOs applies overrides to modal window buttons after they are added.
        // Two most important for this logic are:
        // * Buttons with "Cancel" title will be displayed as the last buttons in the modal
        // * Last button added to the modal via `addButtonWithTitle` stays focused
        // * Focused buttons react on "space"/" " keypresses
        // * Usage of `keyEquivalent`, `makeFirstResponder` or `setInitialFirstResponder` does not change the focus
        //
        // See also https://developer.apple.com/documentation/appkit/nsalert/1524532-addbuttonwithtitle#discussion
        // ```
        // By default, the first button has a key equivalent of Return,
        // any button with a title of “Cancel” has a key equivalent of Escape,
        // and any button with the title “Don’t Save” has a key equivalent of Command-D (but only if it’s not the first button).
        // ```
        //
        // To avoid situations when the last element added is "Cancel" and it gets the focus
        // (hence stealing both ESC and Space shortcuts), we find and add one non-Cancel button
        // last, so it gets focus and a Space shortcut.
        // This way, "Save this file? Yes/No/Cancel"-ish modals will get all three buttons mapped with a key.
        let latest_non_cancel_label = answers
            .iter()
            .enumerate()
            .rev()
            .find(|(_, label)| !label.is_cancel())
            .filter(|&(label_index, _)| label_index > 0);

        unsafe {
            let mtm = objc2::MainThreadMarker::new().expect("NSAlert must be used on main thread");
            let alert = objc2_app_kit::NSAlert::new(mtm);
            let style = match level {
                PromptLevel::Info => objc2_app_kit::NSAlertStyle::Informational,
                PromptLevel::Warning => objc2_app_kit::NSAlertStyle::Warning,
                PromptLevel::Critical => objc2_app_kit::NSAlertStyle::Critical,
            };
            alert.setAlertStyle(style);
            let msg_str = objc2_foundation::NSString::from_str(msg);
            alert.setMessageText(&msg_str);
            if let Some(detail) = detail {
                let detail_str = objc2_foundation::NSString::from_str(detail);
                alert.setInformativeText(&detail_str);
            }

            for (ix, answer) in answers
                .iter()
                .enumerate()
                .filter(|&(ix, _)| Some(ix) != latest_non_cancel_label.map(|(ix, _)| ix))
            {
                let title = objc2_foundation::NSString::from_str(answer.label());
                let button = alert.addButtonWithTitle(&title);
                let bref: &objc2_app_kit::NSButton = &*button;
                bref.setTag(ix as objc2_foundation::NSInteger);

                if answer.is_cancel() {
                    // Bind Escape Key to Cancel Button
                    if let Some(key) = std::char::from_u32(super::events::ESCAPE_KEY as u32) {
                        let key_str = objc2_foundation::NSString::from_str(&key.to_string());
                        bref.setKeyEquivalent(&key_str);
                    }
                }
            }
            if let Some((ix, answer)) = latest_non_cancel_label {
                let title = objc2_foundation::NSString::from_str(answer.label());
                let button = alert.addButtonWithTitle(&title);
                let bref: &objc2_app_kit::NSButton = &*button;
                bref.setTag(ix as objc2_foundation::NSInteger);
            }

            let (done_tx, done_rx) = oneshot::channel();
            let done_tx = Rc::new(RefCell::new(Some(done_tx)));
            let block = StackBlock::new(move |answer: objc2_app_kit::NSModalResponse| {
                if let Some(done_tx) = done_tx.borrow_mut().take() {
                    let _ = done_tx.send((answer as isize).try_into().unwrap());
                }
            });
            let block = block.copy();
            let native_window = self.0.lock().native_window;
            let executor = self.0.lock().executor.clone();
            executor
                .spawn(async move {
                    let win: &objc2_app_kit::NSWindow = &*(native_window as *mut objc2_app_kit::NSWindow);
                    alert.beginSheetModalForWindow_completionHandler(win, Some(&block));
                })
                .detach();

            Some(done_rx)
        }
    }

    fn activate(&self) {
        let window = self.0.lock().native_window;
        let executor = self.0.lock().executor.clone();
        executor
            .spawn(async move {
                let win: &objc2_app_kit::NSWindow = unsafe { &*(window as *mut objc2_app_kit::NSWindow) };
                win.makeKeyAndOrderFront(None);
            })
            .detach();
    }

    fn is_active(&self) -> bool {
        let win: &objc2_app_kit::NSWindow = unsafe { &*(self.0.lock().native_window as *mut objc2_app_kit::NSWindow) };
        win.isKeyWindow()
    }

    // is_hovered is unused on macOS. See Window::is_window_hovered.
    fn is_hovered(&self) -> bool {
        false
    }

    fn set_title(&mut self, title: &str) {
        unsafe {
            let app: *mut objc2::runtime::AnyObject = objc2::msg_send![objc2::class!(NSApplication), sharedApplication];
            let window = self.0.lock().native_window;
            let win_any = window as *mut objc2::runtime::AnyObject;
            let ns_title = objc2_foundation::NSString::from_str(title);
            let ns_title_ref: &objc2_foundation::NSString = &*ns_title;
            let _: () = objc2::msg_send![&*app, changeWindowsItem: win_any, title: ns_title_ref, filename: false];
            let win: &objc2_app_kit::NSWindow = &*(window as *mut objc2_app_kit::NSWindow);
            win.setTitle(ns_title_ref);
            self.0.lock().move_traffic_light();
        }
    }

    fn get_title(&self) -> String {
        unsafe {
            let title: id = msg_send![self.0.lock().native_window, title];
            if title.is_null() {
                "".to_string()
            } else {
                let sref: &objc2_foundation::NSString = &*(title as *mut objc2_foundation::NSString);
                objc2::rc::autoreleasepool(|pool| {
                    let s = unsafe { sref.to_str(pool) };
                    s.to_owned()
                })
            }
        }
    }

    fn set_app_id(&mut self, _app_id: &str) {}

    fn set_background_appearance(&self, background_appearance: WindowBackgroundAppearance) {
        let mut this = self.0.as_ref().lock();

        let opaque = background_appearance == WindowBackgroundAppearance::Opaque;
        this.renderer.update_transparency(!opaque);

        unsafe {
            let win: &objc2_app_kit::NSWindow = &*(this.native_window as *mut objc2_app_kit::NSWindow);
            win.setOpaque(opaque);
            let background_color = if opaque {
                objc2_app_kit::NSColor::colorWithSRGBRed_green_blue_alpha(0., 0., 0., 1.)
            } else {
                // Not using clearColor to avoid broken shadow.
                objc2_app_kit::NSColor::colorWithSRGBRed_green_blue_alpha(0., 0., 0., 0.0001)
            };
            win.setBackgroundColor(Some(&background_color));

            if NSAppKitVersionNumber < NSAppKitVersionNumber12_0 {
                // Whether `-[NSVisualEffectView respondsToSelector:@selector(_updateProxyLayer)]`.
                // On macOS Catalina/Big Sur `NSVisualEffectView` doesn’t own concrete sublayers
                // but uses a `CAProxyLayer`. Use the legacy WindowServer API.
                let blur_radius = if background_appearance == WindowBackgroundAppearance::Blurred {
                    80
                } else {
                    0
                };

                let win_any: &objc2::runtime::AnyObject = &*(this.native_window as *mut objc2::runtime::AnyObject);
                let window_number: NSInteger = objc2::msg_send![win_any, windowNumber];
                CGSSetWindowBackgroundBlurRadius(CGSMainConnectionID(), window_number, blur_radius);
            } else {
                // On newer macOS `NSVisualEffectView` manages the effect layer directly. Using it
                // could have a better performance (it downsamples the backdrop) and more control
                // over the effect layer.
                if background_appearance != WindowBackgroundAppearance::Blurred {
                    if let Some(blur_view) = this.blurred_view {
                        let view: &objc2_app_kit::NSView = &*(blur_view as *mut objc2_app_kit::NSView);
                        view.removeFromSuperview();
                        this.blurred_view = None;
                    }
                } else if this.blurred_view.is_none() {
                    let win_any_objc: &objc::runtime::Object = &*(this.native_window as *mut objc::runtime::Object);
                    let content_view: id = msg_send![win_any_objc, contentView];
                    let frame: objc2_foundation::NSRect = msg_send![content_view, bounds];
                    let mut blur_view: id = msg_send![BLURRED_VIEW_CLASS, alloc];
                    // Initialize our NSVisualEffectView subclass with the typed NSRect using msg_send
                    // to avoid mixing cocoa's NSRect with objc2's.
                    blur_view = msg_send![blur_view, initWithFrame: frame];
                    let mask: objc2_app_kit::NSAutoresizingMaskOptions =
                        objc2_app_kit::NSAutoresizingMaskOptions::ViewWidthSizable
                        | objc2_app_kit::NSAutoresizingMaskOptions::ViewHeightSizable;
                    let _: () = msg_send![blur_view, setAutoresizingMask: mask];

                    {
                        let cv: &objc2_app_kit::NSView = &*(content_view as *mut objc2_app_kit::NSView);
                        let bv: &objc2_app_kit::NSView = &*(blur_view as *mut objc2_app_kit::NSView);
                        cv.addSubview_positioned_relativeTo(
                            bv,
                            objc2_app_kit::NSWindowOrderingMode::Below,
                            None,
                        );
                    }
                    let _: () = msg_send![blur_view, autorelease];
                    this.blurred_view = Some(blur_view);
                }
            }
        }
    }

    fn set_edited(&mut self, edited: bool) {
        unsafe {
            let window = self.0.lock().native_window;
            msg_send![window, setDocumentEdited: edited as BOOL]
        }

        // Changing the document edited state resets the traffic light position,
        // so we have to move it again.
        self.0.lock().move_traffic_light();
    }

    fn show_character_palette(&self) {
        let this = self.0.lock();
        let window = this.native_window;
        this.executor
            .spawn(async move {
                unsafe {
                    let app: *mut objc2::runtime::AnyObject = objc2::msg_send![objc2::class!(NSApplication), sharedApplication];
                    let win_any = window as *mut objc2::runtime::AnyObject;
                    let _: () = objc2::msg_send![&*app, orderFrontCharacterPalette: win_any];
                }
            })
            .detach();
    }

    fn minimize(&self) {
        let window = self.0.lock().native_window;
        unsafe {
            let win: &objc2_app_kit::NSWindow = &*(window as *mut objc2_app_kit::NSWindow);
            win.miniaturize(None);
        }
    }

    fn zoom(&self) {
        let this = self.0.lock();
        let window = this.native_window;
        this.executor
            .spawn(async move {
                let win: &objc2_app_kit::NSWindow = unsafe { &*(window as *mut objc2_app_kit::NSWindow) };
                win.zoom(None);
            })
            .detach();
    }

    fn toggle_fullscreen(&self) {
        let this = self.0.lock();
        let window = this.native_window;
        this.executor
            .spawn(async move {
                unsafe {
                    let win: &objc2_app_kit::NSWindow = &*(window as *mut objc2_app_kit::NSWindow);
                    let sender: Option<&objc2::runtime::AnyObject> = None;
                    win.toggleFullScreen(sender);
                }
            })
            .detach();
    }

    fn is_fullscreen(&self) -> bool {
        let this = self.0.lock();
        let window = this.native_window;

        unsafe {
            let ev: &objc2::runtime::AnyObject = &*(window as *mut objc2::runtime::AnyObject);
            let sm: objc2_app_kit::NSWindowStyleMask = objc2::msg_send![ev, styleMask];
            sm.contains(objc2_app_kit::NSWindowStyleMask::FullScreen)
        }
    }

    fn on_request_frame(&self, callback: Box<dyn FnMut(RequestFrameOptions)>) {
        self.0.as_ref().lock().request_frame_callback = Some(callback);
    }

    fn on_input(&self, callback: Box<dyn FnMut(PlatformInput) -> crate::DispatchEventResult>) {
        self.0.as_ref().lock().event_callback = Some(callback);
    }

    fn on_active_status_change(&self, callback: Box<dyn FnMut(bool)>) {
        self.0.as_ref().lock().activate_callback = Some(callback);
    }

    fn on_hover_status_change(&self, _: Box<dyn FnMut(bool)>) {}

    fn on_visibility_changed(&self, callback: Box<dyn FnMut(bool)>) {
        self.0.as_ref().lock().visibility_callback = Some(callback);
    }

    fn on_resize(&self, callback: Box<dyn FnMut(Size<Pixels>, f32)>) {
        self.0.as_ref().lock().resize_callback = Some(callback);
    }

    fn on_moved(&self, callback: Box<dyn FnMut()>) {
        self.0.as_ref().lock().moved_callback = Some(callback);
    }

    fn on_should_close(&self, callback: Box<dyn FnMut() -> bool>) {
        self.0.as_ref().lock().should_close_callback = Some(callback);
    }

    fn on_close(&self, callback: Box<dyn FnOnce()>) {
        self.0.as_ref().lock().close_callback = Some(callback);
    }

    fn on_hit_test_window_control(&self, _callback: Box<dyn FnMut() -> Option<WindowControlArea>>) {
    }

    fn on_appearance_changed(&self, callback: Box<dyn FnMut()>) {
        self.0.lock().appearance_changed_callback = Some(callback);
    }

    fn tabbed_windows(&self) -> Option<Vec<SystemWindowTab>> {
        unsafe {
            let windows_id: id = msg_send![self.0.lock().native_window, tabbedWindows];
            if windows_id.is_null() {
                return None;
            }

            let windows: &ObjNSArray<objc2_app_kit::NSWindow> =
                &*(windows_id as *mut ObjNSArray<objc2_app_kit::NSWindow>);
            let mut result = Vec::new();
            for i in 0..windows.len() {
                let window = windows.objectAtIndex(i as objc2_foundation::NSUInteger);
                let win_obj: &objc::runtime::Object = &*(
                    (&*window as *const objc2_app_kit::NSWindow)
                        as *mut objc::runtime::Object
                );
            {
                if msg_send![win_obj, isKindOfClass: WINDOW_CLASS] {
                    let handle = get_window_state(win_obj).lock().handle;
                    let title: id = msg_send![win_obj, title];
                    let title = {
                        let sref: &objc2_foundation::NSString = &*(title as *mut objc2_foundation::NSString);
                        let s = objc2::rc::autoreleasepool(|pool| unsafe { sref.to_str(pool).to_owned() });
                        SharedString::from(s)
                    };
                    result.push(SystemWindowTab::new(title, handle));
                }
            }
            }

            Some(result)
        }
    }

    fn tab_bar_visible(&self) -> bool {
        unsafe {
            let tab_group: id = msg_send![self.0.lock().native_window, tabGroup];
            if tab_group.is_null() {
                false
            } else {
                let tab_bar_visible: BOOL = msg_send![tab_group, isTabBarVisible];
                tab_bar_visible == YES
            }
        }
    }

    fn on_move_tab_to_new_window(&self, callback: Box<dyn FnMut()>) {
        self.0.as_ref().lock().move_tab_to_new_window_callback = Some(callback);
    }

    fn on_merge_all_windows(&self, callback: Box<dyn FnMut()>) {
        self.0.as_ref().lock().merge_all_windows_callback = Some(callback);
    }

    fn on_select_next_tab(&self, callback: Box<dyn FnMut()>) {
        self.0.as_ref().lock().select_next_tab_callback = Some(callback);
    }

    fn on_select_previous_tab(&self, callback: Box<dyn FnMut()>) {
        self.0.as_ref().lock().select_previous_tab_callback = Some(callback);
    }

    fn on_toggle_tab_bar(&self, callback: Box<dyn FnMut()>) {
        self.0.as_ref().lock().toggle_tab_bar_callback = Some(callback);
    }

    fn draw(&self, scene: &crate::Scene) {
        let mut this = self.0.lock();
        this.renderer.draw(scene);
    }

    fn sprite_atlas(&self) -> Arc<dyn PlatformAtlas> {
        self.0.lock().renderer.sprite_atlas().clone()
    }

    fn gpu_specs(&self) -> Option<crate::GpuSpecs> {
        None
    }

    fn update_ime_position(&self, _bounds: Bounds<Pixels>) {
        let executor = self.0.lock().executor.clone();
        executor
            .spawn(async move {
                unsafe {
                    let input_context: id =
                        msg_send![class!(NSTextInputContext), currentInputContext];
                    if input_context.is_null() {
                        return;
                    }
                    let _: () = msg_send![input_context, invalidateCharacterCoordinates];
                }
            })
            .detach()
    }

    fn titlebar_double_click(&self) {
        let this = self.0.lock();
        let window = this.native_window;
        this.executor
            .spawn(async move {
                unsafe {
                    let defaults = ObjNSUserDefaults::standardUserDefaults();
                    let dict = defaults.dictionaryRepresentation();
                    let key = ObjNSString::from_str("AppleActionOnDoubleClick");
                    let action_obj = dict.objectForKey(&key);
                    let action_str: String = if let Some(any) = action_obj {
                        if let Ok(ns) = any.downcast::<ObjNSString>() {
                            objc2::rc::autoreleasepool(|pool| unsafe { ns.to_str(pool).to_owned() })
                        } else {
                            String::new()
                        }
                    } else {
                        String::new()
                    };

                    let win: &objc2_app_kit::NSWindow = &*(window as *mut objc2_app_kit::NSWindow);
                    match action_str.as_str() {
                        "None" => {
                            // "Do Nothing" selected, so do no action
                        }
                        "Minimize" => {
                            win.miniaturize(None);
                        }
                        "Maximize" => {
                            win.zoom(None);
                        }
                        "Fill" => {
                            // There is no documented API for "Fill" action, so we'll just zoom the window
                            win.zoom(None);
                        }
                        _ => {
                            win.zoom(None);
                        }
                    }
                }
            })
            .detach();
    }
}

impl rwh::HasWindowHandle for MacWindow {
    fn window_handle(&self) -> Result<rwh::WindowHandle<'_>, rwh::HandleError> {
        // SAFETY: The AppKitWindowHandle is a wrapper around a pointer to an NSView
        unsafe {
            Ok(rwh::WindowHandle::borrow_raw(rwh::RawWindowHandle::AppKit(
                rwh::AppKitWindowHandle::new(self.0.lock().native_view.cast()),
            )))
        }
    }
}

impl rwh::HasDisplayHandle for MacWindow {
    fn display_handle(&self) -> Result<rwh::DisplayHandle<'_>, rwh::HandleError> {
        // SAFETY: This is a no-op on macOS
        unsafe {
            Ok(rwh::DisplayHandle::borrow_raw(
                rwh::AppKitDisplayHandle::new().into(),
            ))
        }
    }
}

fn get_scale_factor(native_window: id) -> f32 {
        let factor = unsafe {
            let screen: id = msg_send![native_window, screen];
            if screen.is_null() {
                return 2.0;
            }
            let sref: &objc2_app_kit::NSScreen = &*(screen as *mut objc2_app_kit::NSScreen);
            sref.backingScaleFactor() as f32
        };

    // We are not certain what triggers this, but it seems that sometimes
    // this method would return 0 (https://github.com/zed-industries/zed/issues/6412)
    // It seems most likely that this would happen if the window has no screen
    // (if it is off-screen), though we'd expect to see viewDidChangeBackingProperties before
    // it was rendered for real.
    // Regardless, attempt to avoid the issue here.
    if factor == 0.0 { 2. } else { factor }
}

unsafe fn get_window_state(object: &Object) -> Arc<Mutex<MacWindowState>> {
    unsafe {
        let raw: *mut c_void = *object.get_ivar(WINDOW_STATE_IVAR);
        let rc1 = Arc::from_raw(raw as *mut Mutex<MacWindowState>);
        let rc2 = rc1.clone();
        mem::forget(rc1);
        rc2
    }
}

unsafe fn drop_window_state(object: &Object) {
    unsafe {
        let raw: *mut c_void = *object.get_ivar(WINDOW_STATE_IVAR);
        Arc::from_raw(raw as *mut Mutex<MacWindowState>);
    }
}

extern "C" fn yes(_: &Object, _: Sel) -> BOOL {
    YES
}

extern "C" fn dealloc_window(this: &Object, _: Sel) {
    unsafe {
        drop_window_state(this);
        let _: () = msg_send![super(this, class!(NSWindow)), dealloc];
    }
}

extern "C" fn dealloc_view(this: &Object, _: Sel) {
    unsafe {
        drop_window_state(this);
        let _: () = msg_send![super(this, class!(NSView)), dealloc];
    }
}

extern "C" fn handle_key_equivalent(this: &Object, _: Sel, native_event: id) -> BOOL {
    handle_key_event(this, native_event, true)
}

extern "C" fn handle_key_down(this: &Object, _: Sel, native_event: id) {
    handle_key_event(this, native_event, false);
}

extern "C" fn handle_key_up(this: &Object, _: Sel, native_event: id) {
    handle_key_event(this, native_event, false);
}

// Things to test if you're modifying this method:
//  U.S. layout:
//   - The IME consumes characters like 'j' and 'k', which makes paging through `less` in
//     the terminal behave incorrectly by default. This behavior should be patched by our
//     IME integration
//   - `alt-t` should open the tasks menu
//   - In vim mode, this keybinding should work:
//     ```
//        {
//          "context": "Editor && vim_mode == insert",
//          "bindings": {"j j": "vim::NormalBefore"}
//        }
//     ```
//     and typing 'j k' in insert mode with this keybinding should insert the two characters
//  Brazilian layout:
//   - `" space` should create an unmarked quote
//   - `" backspace` should delete the marked quote
//   - `" "`should create an unmarked quote and a second marked quote
//   - `" up` should insert a quote, unmark it, and move up one line
//   - `" cmd-down` should insert a quote, unmark it, and move to the end of the file
//   - `cmd-ctrl-space` and clicking on an emoji should type it
//  Czech (QWERTY) layout:
//   - in vim mode `option-4`  should go to end of line (same as $)
//  Japanese (Romaji) layout:
//   - type `a i left down up enter enter` should create an unmarked text "愛"
extern "C" fn handle_key_event(this: &Object, native_event: id, key_equivalent: bool) -> BOOL {
    let window_state = unsafe { get_window_state(this) };
    let mut lock = window_state.as_ref().lock();

    let window_height = lock.content_size().height;
    let ev: &objc2_app_kit::NSEvent = unsafe { &*(native_event as *mut objc2_app_kit::NSEvent) };
    let event = PlatformInput::from_native(ev, Some(window_height));

    let Some(event) = event else {
        return NO;
    };

    let run_callback = |event: PlatformInput| -> BOOL {
        let mut callback = window_state.as_ref().lock().event_callback.take();
        let handled: BOOL = if let Some(callback) = callback.as_mut() {
            !callback(event).propagate as BOOL
        } else {
            NO
        };
        window_state.as_ref().lock().event_callback = callback;
        handled
    };

    match event {
        PlatformInput::KeyDown(mut key_down_event) => {
            // For certain keystrokes, macOS will first dispatch a "key equivalent" event.
            // If that event isn't handled, it will then dispatch a "key down" event. GPUI
            // makes no distinction between these two types of events, so we need to ignore
            // the "key down" event if we've already just processed its "key equivalent" version.
            if key_equivalent {
                lock.last_key_equivalent = Some(key_down_event.clone());
            } else if lock.last_key_equivalent.take().as_ref() == Some(&key_down_event) {
                return NO;
            }

            drop(lock);

            let is_composing =
                with_input_handler(this, |input_handler| input_handler.marked_text_range())
                    .flatten()
                    .is_some();

            // If we're composing, send the key to the input handler first;
            // otherwise we only send to the input handler if we don't have a matching binding.
            // The input handler may call `do_command_by_selector` if it doesn't know how to handle
            // a key. If it does so, it will return YES so we won't send the key twice.
            // We also do this for non-printing keys (like arrow keys and escape) as the IME menu
            // may need them even if there is no marked text;
            // however we skip keys with control or the input handler adds control-characters to the buffer.
            // and keys with function, as the input handler swallows them.
            if is_composing
                || (key_down_event.keystroke.key_char.is_none()
                    && !key_down_event.keystroke.modifiers.control
                    && !key_down_event.keystroke.modifiers.function)
            {
                {
                    let mut lock = window_state.as_ref().lock();
                    lock.keystroke_for_do_command = Some(key_down_event.keystroke.clone());
                    lock.do_command_handled.take();
                    drop(lock);
                }

                let handled: BOOL = unsafe {
                    let input_context: id = msg_send![this, inputContext];
                    msg_send![input_context, handleEvent: native_event]
                };
                window_state.as_ref().lock().keystroke_for_do_command.take();
                if let Some(handled) = window_state.as_ref().lock().do_command_handled.take() {
                    return handled as BOOL;
                } else if handled == YES {
                    return YES;
                }

                let handled = run_callback(PlatformInput::KeyDown(key_down_event));
                return handled;
            }

            let handled = run_callback(PlatformInput::KeyDown(key_down_event.clone()));
            if handled == YES {
                return YES;
            }

            if key_down_event.is_held
                && let Some(key_char) = key_down_event.keystroke.key_char.as_ref()
            {
                let handled = with_input_handler(this, |input_handler| {
                    if !input_handler.apple_press_and_hold_enabled() {
                        input_handler.replace_text_in_range(None, key_char);
                        return YES;
                    }
                    NO
                });
                if handled == Some(YES) {
                    return YES;
                }
            }

            // Don't send key equivalents to the input handler,
            // or macOS shortcuts like cmd-` will stop working.
            if key_equivalent {
                return NO;
            }

            unsafe {
                let input_context: id = msg_send![this, inputContext];
                msg_send![input_context, handleEvent: native_event]
            }
        }

        PlatformInput::KeyUp(_) => {
            drop(lock);
            run_callback(event)
        }

        _ => NO,
    }
}

extern "C" fn handle_view_event(this: &Object, _: Sel, native_event: id) {
    let window_state = unsafe { get_window_state(this) };
    let weak_window_state = Arc::downgrade(&window_state);
    let mut lock = window_state.as_ref().lock();
    let window_height = lock.content_size().height;
    let ev: &objc2_app_kit::NSEvent = unsafe { &*(native_event as *mut objc2_app_kit::NSEvent) };
    let event = PlatformInput::from_native(ev, Some(window_height));

    if let Some(mut event) = event {
        match &mut event {
            PlatformInput::MouseDown(
                event @ MouseDownEvent {
                    button: MouseButton::Left,
                    modifiers: Modifiers { control: true, .. },
                    ..
                },
            ) => {
                // On mac, a ctrl-left click should be handled as a right click.
                *event = MouseDownEvent {
                    button: MouseButton::Right,
                    modifiers: Modifiers {
                        control: false,
                        ..event.modifiers
                    },
                    click_count: 1,
                    ..*event
                };
            }

            // Handles focusing click.
            PlatformInput::MouseDown(
                event @ MouseDownEvent {
                    button: MouseButton::Left,
                    ..
                },
            ) if (lock.first_mouse) => {
                *event = MouseDownEvent {
                    first_mouse: true,
                    ..*event
                };
                lock.first_mouse = false;
            }

            // Because we map a ctrl-left_down to a right_down -> right_up let's ignore
            // the ctrl-left_up to avoid having a mismatch in button down/up events if the
            // user is still holding ctrl when releasing the left mouse button
            PlatformInput::MouseUp(
                event @ MouseUpEvent {
                    button: MouseButton::Left,
                    modifiers: Modifiers { control: true, .. },
                    ..
                },
            ) => {
                *event = MouseUpEvent {
                    button: MouseButton::Right,
                    modifiers: Modifiers {
                        control: false,
                        ..event.modifiers
                    },
                    click_count: 1,
                    ..*event
                };
            }

            _ => {}
        };

        match &event {
            PlatformInput::MouseDown(_) => {
                drop(lock);
                unsafe {
                    let input_context: id = msg_send![this, inputContext];
                    msg_send![input_context, handleEvent: native_event]
                }
                lock = window_state.as_ref().lock();
            }
            PlatformInput::MouseMove(
                event @ MouseMoveEvent {
                    pressed_button: Some(_),
                    ..
                },
            ) => {
                // Synthetic drag is used for selecting long buffer contents while buffer is being scrolled.
                // External file drag and drop is able to emit its own synthetic mouse events which will conflict
                // with these ones.
                if !lock.external_files_dragged {
                    lock.synthetic_drag_counter += 1;
                    let executor = lock.executor.clone();
                    executor
                        .spawn(synthetic_drag(
                            weak_window_state,
                            lock.synthetic_drag_counter,
                            event.clone(),
                        ))
                        .detach();
                }
            }

            PlatformInput::MouseUp(MouseUpEvent { .. }) => {
                lock.synthetic_drag_counter += 1;
            }

            PlatformInput::ModifiersChanged(ModifiersChangedEvent {
                modifiers,
                capslock,
            }) => {
                // Only raise modifiers changed event when they have actually changed
                if let Some(PlatformInput::ModifiersChanged(ModifiersChangedEvent {
                    modifiers: prev_modifiers,
                    capslock: prev_capslock,
                })) = &lock.previous_modifiers_changed_event
                    && prev_modifiers == modifiers
                    && prev_capslock == capslock
                {
                    return;
                }

                lock.previous_modifiers_changed_event = Some(event.clone());
            }

            _ => {}
        }

        if let Some(mut callback) = lock.event_callback.take() {
            drop(lock);
            callback(event);
            window_state.lock().event_callback = Some(callback);
        }
    }
}

extern "C" fn window_did_change_occlusion_state(this: &Object, _: Sel, _: id) {
    let window_state = unsafe { get_window_state(this) };
    let mut lock = window_state.lock();
    let win: &objc2_app_kit::NSWindow = unsafe { &*(lock.native_window as *mut objc2_app_kit::NSWindow) };
    let visible = win
        .occlusionState()
        .contains(objc2_app_kit::NSWindowOcclusionState::Visible);
    if visible {
        lock.move_traffic_light();
        lock.start_display_link();
    } else {
        lock.stop_display_link();
    }

    // Notify visibility callback if any
    let mut lock = window_state.as_ref().lock();
    if let Some(mut cb) = lock.visibility_callback.take() {
        drop(lock);
        cb(visible);
        window_state.as_ref().lock().visibility_callback = Some(cb);
    }
}

extern "C" fn window_did_resize(this: &Object, _: Sel, _: id) {
    let window_state = unsafe { get_window_state(this) };
    window_state.as_ref().lock().move_traffic_light();
}

extern "C" fn window_will_enter_fullscreen(this: &Object, _: Sel, _: id) {
    let window_state = unsafe { get_window_state(this) };
    let mut lock = window_state.as_ref().lock();
    lock.fullscreen_restore_bounds = lock.bounds();

    let min_version = objc2_foundation::NSOperatingSystemVersion { majorVersion: 15, minorVersion: 3, patchVersion: 0 };

    if is_macos_version_at_least(min_version) {
        let wref: &objc2_app_kit::NSWindow = unsafe { &*(lock.native_window as *mut objc2_app_kit::NSWindow) };
        wref.setTitlebarAppearsTransparent(false);
    }
}

extern "C" fn window_will_exit_fullscreen(this: &Object, _: Sel, _: id) {
    let window_state = unsafe { get_window_state(this) };
    let mut lock = window_state.as_ref().lock();

    let min_version = objc2_foundation::NSOperatingSystemVersion { majorVersion: 15, minorVersion: 3, patchVersion: 0 };

    if is_macos_version_at_least(min_version) && lock.transparent_titlebar {
        let wref: &objc2_app_kit::NSWindow = unsafe { &*(lock.native_window as *mut objc2_app_kit::NSWindow) };
        wref.setTitlebarAppearsTransparent(true);
    }
}

pub(crate) fn is_macos_version_at_least(version: objc2_foundation::NSOperatingSystemVersion) -> bool {
    let pi = objc2_foundation::NSProcessInfo::processInfo();
    // Call typed `isOperatingSystemAtLeastVersion:` via msg_send
    unsafe { objc2::msg_send![&*pi, isOperatingSystemAtLeastVersion: version] }
}

extern "C" fn window_did_move(this: &Object, _: Sel, _: id) {
    let window_state = unsafe { get_window_state(this) };
    let mut lock = window_state.as_ref().lock();
    if let Some(mut callback) = lock.moved_callback.take() {
        drop(lock);
        callback();
        window_state.lock().moved_callback = Some(callback);
    }
}

extern "C" fn window_did_change_screen(this: &Object, _: Sel, _: id) {
    let window_state = unsafe { get_window_state(this) };
    let mut lock = window_state.as_ref().lock();
    lock.start_display_link();
}

extern "C" fn window_did_change_key_status(this: &Object, selector: Sel, _: id) {
    let window_state = unsafe { get_window_state(this) };
    let mut lock = window_state.lock();
    let wref: &objc2_app_kit::NSWindow = unsafe { &*(lock.native_window as *mut objc2_app_kit::NSWindow) };
    let is_active = wref.isKeyWindow();

    // When opening a pop-up while the application isn't active, Cocoa sends a spurious
    // `windowDidBecomeKey` message to the previous key window even though that window
    // isn't actually key. This causes a bug if the application is later activated while
    // the pop-up is still open, making it impossible to activate the previous key window
    // even if the pop-up gets closed. The only way to activate it again is to de-activate
    // the app and re-activate it, which is a pretty bad UX.
    // The following code detects the spurious event and invokes `resignKeyWindow`:
    // in theory, we're not supposed to invoke this method manually but it balances out
    // the spurious `becomeKeyWindow` event and helps us work around that bug.
    if selector == sel!(windowDidBecomeKey:) && !is_active {
        unsafe {
            let _: () = msg_send![lock.native_window, resignKeyWindow];
            return;
        }
    }

    let executor = lock.executor.clone();
    drop(lock);

    // When a window becomes active, trigger an immediate synchronous frame request to prevent
    // tab flicker when switching between windows in native tabs mode.
    //
    // This is only done on subsequent activations (not the first) to ensure the initial focus
    // path is properly established. Without this guard, the focus state would remain unset until
    // the first mouse click, causing keybindings to be non-functional.
    if selector == sel!(windowDidBecomeKey:) && is_active {
        let window_state = unsafe { get_window_state(this) };
        let mut lock = window_state.lock();

        if lock.activated_least_once {
            if let Some(mut callback) = lock.request_frame_callback.take() {
                #[cfg(not(feature = "macos-blade"))]
                lock.renderer.set_presents_with_transaction(true);
                lock.stop_display_link();
                drop(lock);
                callback(Default::default());

                let mut lock = window_state.lock();
                lock.request_frame_callback = Some(callback);
                #[cfg(not(feature = "macos-blade"))]
                lock.renderer.set_presents_with_transaction(false);
                lock.start_display_link();
            }
        } else {
            lock.activated_least_once = true;
        }
    }

    executor
        .spawn(async move {
            let mut lock = window_state.as_ref().lock();
            if is_active {
                lock.move_traffic_light();
            }

            if let Some(mut callback) = lock.activate_callback.take() {
                drop(lock);
                callback(is_active);
                window_state.lock().activate_callback = Some(callback);
            };
        })
        .detach();
}

extern "C" fn window_should_close(this: &Object, _: Sel, _: id) -> BOOL {
    let window_state = unsafe { get_window_state(this) };
    let mut lock = window_state.as_ref().lock();
    if let Some(mut callback) = lock.should_close_callback.take() {
        drop(lock);
        let should_close = callback();
        window_state.lock().should_close_callback = Some(callback);
        should_close as BOOL
    } else {
        YES
    }
}

extern "C" fn close_window(this: &Object, _: Sel) {
    unsafe {
        let close_callback = {
            let window_state = get_window_state(this);
            let mut lock = window_state.as_ref().lock();
            lock.close_callback.take()
        };

        if let Some(callback) = close_callback {
            callback();
        }

        let _: () = msg_send![super(this, class!(NSWindow)), close];
    }
}

extern "C" fn make_backing_layer(this: &Object, _: Sel) -> id {
    let window_state = unsafe { get_window_state(this) };
    let window_state = window_state.as_ref().lock();
    window_state.renderer.layer_ptr() as id
}

extern "C" fn view_did_change_backing_properties(this: &Object, _: Sel) {
    let window_state = unsafe { get_window_state(this) };
    let mut lock = window_state.as_ref().lock();

    let scale_factor = lock.scale_factor();
    let size = lock.content_size();
    let drawable_size = size.to_device_pixels(scale_factor);
    unsafe {
        let _: () = msg_send![
            lock.renderer.layer(),
            setContentsScale: scale_factor as f64
        ];
    }

    lock.renderer.update_drawable_size(drawable_size);

    if let Some(mut callback) = lock.resize_callback.take() {
        let content_size = lock.content_size();
        let scale_factor = lock.scale_factor();
        drop(lock);
        callback(content_size, scale_factor);
        window_state.as_ref().lock().resize_callback = Some(callback);
    };
}

extern "C" fn set_frame_size(this: &Object, _: Sel, size: cocoa::foundation::NSSize) {
    let window_state = unsafe { get_window_state(this) };
    let mut lock = window_state.as_ref().lock();

    // Convert Cocoa NSSize to typed objc2_foundation::NSSize before field access
    let typed_size: objc2_foundation::NSSize = unsafe { std::mem::transmute(size) };
    let new_size = Size::<Pixels>::new(px(typed_size.width as f32), px(typed_size.height as f32));
    let old_size = unsafe {
        let old_frame: objc2_foundation::NSRect = msg_send![this, frame];
        Size::<Pixels>::from(old_frame.size)
    };

    if old_size == new_size {
        return;
    }

    unsafe {
        let _: () = msg_send![super(this, class!(NSView)), setFrameSize: size];
    }

    let scale_factor = lock.scale_factor();
    let drawable_size = new_size.to_device_pixels(scale_factor);
    lock.renderer.update_drawable_size(drawable_size);

    if let Some(mut callback) = lock.resize_callback.take() {
        let content_size = lock.content_size();
        let scale_factor = lock.scale_factor();
        drop(lock);
        callback(content_size, scale_factor);
        window_state.lock().resize_callback = Some(callback);
    };
}

extern "C" fn display_layer(this: &Object, _: Sel, _: id) {
    let window_state = unsafe { get_window_state(this) };
    let mut lock = window_state.lock();
    if let Some(mut callback) = lock.request_frame_callback.take() {
        #[cfg(not(feature = "macos-blade"))]
        lock.renderer.set_presents_with_transaction(true);
        lock.stop_display_link();
        drop(lock);
        callback(Default::default());

        let mut lock = window_state.lock();
        lock.request_frame_callback = Some(callback);
        #[cfg(not(feature = "macos-blade"))]
        lock.renderer.set_presents_with_transaction(false);
        lock.start_display_link();
    }
}

unsafe extern "C" fn step(view: *mut c_void) {
    let view = view as id;
    let window_state = unsafe { get_window_state(&*view) };
    let mut lock = window_state.lock();

    if let Some(mut callback) = lock.request_frame_callback.take() {
        drop(lock);
        callback(Default::default());
        window_state.lock().request_frame_callback = Some(callback);
    }
}

extern "C" fn valid_attributes_for_marked_text(_: &Object, _: Sel) -> id {
    unsafe { msg_send![class!(NSArray), array] }
}

extern "C" fn has_marked_text(this: &Object, _: Sel) -> BOOL {
    let has_marked_text_result =
        with_input_handler(this, |input_handler| input_handler.marked_text_range()).flatten();

    has_marked_text_result.is_some() as BOOL
}

extern "C" fn marked_range(this: &Object, _: Sel) -> NSRange {
    let marked_range_result =
        with_input_handler(this, |input_handler| input_handler.marked_text_range()).flatten();

    marked_range_result.map_or(NSRange::invalid(), |range| range.into())
}

extern "C" fn selected_range(this: &Object, _: Sel) -> NSRange {
    let selected_range_result = with_input_handler(this, |input_handler| {
        input_handler.selected_text_range(false)
    })
    .flatten();

    selected_range_result.map_or(NSRange::invalid(), |selection| selection.range.into())
}

extern "C" fn first_rect_for_character_range(
    this: &Object,
    _: Sel,
    range: NSRange,
    _: id,
) -> cocoa::foundation::NSRect {
    let frame = get_frame(this);
    with_input_handler(this, |input_handler| {
        input_handler.bounds_for_range(range.to_range()?)
    })
    .flatten()
    .map_or_else(
        || unsafe { std::mem::transmute::<objc2_foundation::NSRect, cocoa::foundation::NSRect>(objc2_foundation::NSRect::new(objc2_foundation::NSPoint::new(0., 0.), objc2_foundation::NSSize::new(0., 0.))) },
        |bounds| {
            let rect_typed = objc2_foundation::NSRect::new(
                objc2_foundation::NSPoint::new(
                    frame.origin.x + bounds.origin.x.0 as f64,
                    frame.origin.y + frame.size.height
                        - bounds.origin.y.0 as f64
                        - bounds.size.height.0 as f64,
                ),
                objc2_foundation::NSSize::new(
                    bounds.size.width.0 as f64,
                    bounds.size.height.0 as f64,
                ),
            );
            unsafe { std::mem::transmute::<objc2_foundation::NSRect, cocoa::foundation::NSRect>(rect_typed) }
        },
    )
}

fn get_frame(this: &Object) -> objc2_foundation::NSRect {
    unsafe {
        let state = get_window_state(this);
        let lock = state.lock();
        let wref: &objc2_app_kit::NSWindow = &*(lock.native_window as *mut objc2_app_kit::NSWindow);
        let mut frame: objc2_foundation::NSRect = objc2::msg_send![wref, frame];
        let content_layout_rect: objc2_foundation::NSRect = wref.contentLayoutRect();
        let style_mask: objc2_app_kit::NSWindowStyleMask = wref.styleMask();
        if !style_mask.contains(objc2_app_kit::NSWindowStyleMask::FullSizeContentView) {
            frame.origin.y -= frame.size.height - content_layout_rect.size.height;
        }
        frame
    }
}

extern "C" fn insert_text(this: &Object, _: Sel, text: id, replacement_range: NSRange) {
    unsafe {
        let is_attributed_string: BOOL =
            msg_send![text, isKindOfClass: [class!(NSAttributedString)]];
        let text: id = if is_attributed_string == YES {
            msg_send![text, string]
        } else {
            text
        };

        let text_string = {
            let sref: &objc2_foundation::NSString = &*(text as *mut objc2_foundation::NSString);
            objc2::rc::autoreleasepool(|pool| unsafe { sref.to_str(pool).to_owned() })
        };
        let replacement_range = replacement_range.to_range();
        with_input_handler(this, |input_handler| {
            input_handler.replace_text_in_range(replacement_range, &text_string)
        });
    }
}

extern "C" fn set_marked_text(
    this: &Object,
    _: Sel,
    text: id,
    selected_range: NSRange,
    replacement_range: NSRange,
) {
    unsafe {
        let is_attributed_string: BOOL =
            msg_send![text, isKindOfClass: [class!(NSAttributedString)]];
        let text: id = if is_attributed_string == YES {
            msg_send![text, string]
        } else {
            text
        };
        let selected_range = selected_range.to_range();
        let replacement_range = replacement_range.to_range();
        let text_string = {
            let sref: &objc2_foundation::NSString = &*(text as *mut objc2_foundation::NSString);
            objc2::rc::autoreleasepool(|pool| unsafe { sref.to_str(pool).to_owned() })
        };
        with_input_handler(this, |input_handler| {
            input_handler.replace_and_mark_text_in_range(replacement_range, &text_string, selected_range)
        });
    }
}
extern "C" fn unmark_text(this: &Object, _: Sel) {
    with_input_handler(this, |input_handler| input_handler.unmark_text());
}

extern "C" fn attributed_substring_for_proposed_range(
    this: &Object,
    _: Sel,
    range: NSRange,
    actual_range: *mut c_void,
) -> id {
    with_input_handler(this, |input_handler| {
        let range = range.to_range()?;
        if range.is_empty() {
            return None;
        }
        let mut adjusted: Option<Range<usize>> = None;

        let selected_text = input_handler.text_for_range(range.clone(), &mut adjusted)?;
        if let Some(adjusted) = adjusted
            && adjusted != range
        {
            unsafe { (actual_range as *mut NSRange).write(NSRange::from(adjusted)) };
        }
        unsafe {
            let string: id = msg_send![class!(NSAttributedString), alloc];
            let ns = objc2_foundation::NSString::from_str(&selected_text);
            let string: id = msg_send![string, initWithString: &*ns];
            Some(string)
        }
    })
    .flatten()
    .unwrap_or(nil)
}

// We ignore which selector it asks us to do because the user may have
// bound the shortcut to something else.
extern "C" fn do_command_by_selector(this: &Object, _: Sel, _: Sel) {
    let state = unsafe { get_window_state(this) };
    let mut lock = state.as_ref().lock();
    let keystroke = lock.keystroke_for_do_command.take();
    let mut event_callback = lock.event_callback.take();
    drop(lock);

    if let Some((keystroke, mut callback)) = keystroke.zip(event_callback.as_mut()) {
        let handled = (callback)(PlatformInput::KeyDown(KeyDownEvent {
            keystroke,
            is_held: false,
        }));
        state.as_ref().lock().do_command_handled = Some(!handled.propagate);
    }

    state.as_ref().lock().event_callback = event_callback;
}

extern "C" fn view_did_change_effective_appearance(this: &Object, _: Sel) {
    unsafe {
        let state = get_window_state(this);
        let mut lock = state.as_ref().lock();
        if let Some(mut callback) = lock.appearance_changed_callback.take() {
            drop(lock);
            callback();
            state.lock().appearance_changed_callback = Some(callback);
        }
    }
}

extern "C" fn accepts_first_mouse(this: &Object, _: Sel, _: id) -> BOOL {
    let window_state = unsafe { get_window_state(this) };
    let mut lock = window_state.as_ref().lock();
    lock.first_mouse = true;
    YES
}

extern "C" fn character_index_for_point(
    this: &Object,
    _: Sel,
    position: cocoa::foundation::NSPoint,
) -> u64 {
    let position = screen_point_to_gpui_point(this, position);
    with_input_handler(this, |input_handler| {
        input_handler.character_index_for_point(position)
    })
    .flatten()
    .map(|index| index as u64)
    .unwrap_or(objc2_foundation::NSNotFound as u64)
}

fn screen_point_to_gpui_point(
    this: &Object,
    position: cocoa::foundation::NSPoint,
) -> Point<Pixels> {
    let frame = get_frame(this);
    let pos_typed: objc2_foundation::NSPoint = unsafe { std::mem::transmute(position) };
    let window_x = (pos_typed.x as f64) - (frame.origin.x as f64);
    let window_y = (frame.size.height as f64) - ((pos_typed.y as f64) - (frame.origin.y as f64));

    point(px(window_x as f32), px(window_y as f32))
}

extern "C" fn dragging_entered(this: &Object, _: Sel, dragging_info: id) -> NSDragOperation {
    let window_state = unsafe { get_window_state(this) };
    let position = drag_event_position(&window_state, dragging_info);
    let paths = external_paths_from_event(dragging_info);
    if let Some(event) =
        paths.map(|paths| PlatformInput::FileDrop(FileDropEvent::Entered { position, paths }))
        && send_new_event(&window_state, event)
    {
        window_state.lock().external_files_dragged = true;
        return NSDragOperationCopy;
    }
    NSDragOperationNone
}

extern "C" fn dragging_updated(this: &Object, _: Sel, dragging_info: id) -> NSDragOperation {
    let window_state = unsafe { get_window_state(this) };
    let position = drag_event_position(&window_state, dragging_info);
    if send_new_event(
        &window_state,
        PlatformInput::FileDrop(FileDropEvent::Pending { position }),
    ) {
        NSDragOperationCopy
    } else {
        NSDragOperationNone
    }
}

extern "C" fn dragging_exited(this: &Object, _: Sel, _: id) {
    let window_state = unsafe { get_window_state(this) };
    send_new_event(
        &window_state,
        PlatformInput::FileDrop(FileDropEvent::Exited),
    );
    window_state.lock().external_files_dragged = false;
}

extern "C" fn perform_drag_operation(this: &Object, _: Sel, dragging_info: id) -> BOOL {
    let window_state = unsafe { get_window_state(this) };
    let position = drag_event_position(&window_state, dragging_info);
    send_new_event(
        &window_state,
        PlatformInput::FileDrop(FileDropEvent::Submit { position }),
    )
    .to_objc()
}

fn external_paths_from_event(dragging_info: *mut Object) -> Option<ExternalPaths> {
    let mut paths = SmallVec::new();

    // Treat `dragging_info` as NSDraggingInfo
    let info: &ProtocolObject<dyn ObjNSDraggingInfo> = unsafe { &*(dragging_info as *mut ProtocolObject<dyn ObjNSDraggingInfo>) };
    let pasteboard = info.draggingPasteboard();

    if let Some(items) = pasteboard.pasteboardItems() {
        let count = items.count();
        for i in 0..count {
            let item = items.objectAtIndex(i);
            if let Some(url_str) = item.stringForType(unsafe { NSPasteboardTypeFileURL }) {
                if let Some(url) = ObjNSURL::URLWithString(&url_str) {
                    if url.isFileURL() {
                        if let Some(ns_path) = url.path() {
                            let s = objc2::rc::autoreleasepool(|pool| unsafe { ns_path.to_str(pool).to_owned() });
                            paths.push(PathBuf::from(s));
                        }
                    }
                }
            }
        }
    }

    if paths.is_empty() { None } else { Some(ExternalPaths(paths)) }
}

extern "C" fn conclude_drag_operation(this: &Object, _: Sel, _: id) {
    let window_state = unsafe { get_window_state(this) };
    send_new_event(
        &window_state,
        PlatformInput::FileDrop(FileDropEvent::Exited),
    );
}

async fn synthetic_drag(
    window_state: Weak<Mutex<MacWindowState>>,
    drag_id: usize,
    event: MouseMoveEvent,
) {
    loop {
        Timer::after(Duration::from_millis(16)).await;
        if let Some(window_state) = window_state.upgrade() {
            let mut lock = window_state.lock();
            if lock.synthetic_drag_counter == drag_id {
                if let Some(mut callback) = lock.event_callback.take() {
                    drop(lock);
                    callback(PlatformInput::MouseMove(event.clone()));
                    window_state.lock().event_callback = Some(callback);
                }
            } else {
                break;
            }
        }
    }
}

fn send_new_event(window_state_lock: &Mutex<MacWindowState>, e: PlatformInput) -> bool {
    let window_state = window_state_lock.lock().event_callback.take();
    if let Some(mut callback) = window_state {
        callback(e);
        window_state_lock.lock().event_callback = Some(callback);
        true
    } else {
        false
    }
}

fn drag_event_position(window_state: &Mutex<MacWindowState>, dragging_info: id) -> Point<Pixels> {
    // Treat dragging_info as NSDraggingInfo and use typed draggingLocation
    let info: &ProtocolObject<dyn ObjNSDraggingInfo> = unsafe { &*(dragging_info as *mut ProtocolObject<dyn ObjNSDraggingInfo>) };
    let drag_location = info.draggingLocation();
    convert_mouse_position(drag_location, window_state.lock().content_size().height)
}

fn with_input_handler<F, R>(window: &Object, f: F) -> Option<R>
where
    F: FnOnce(&mut PlatformInputHandler) -> R,
{
    let window_state = unsafe { get_window_state(window) };
    let mut lock = window_state.as_ref().lock();
    if let Some(mut input_handler) = lock.input_handler.take() {
        drop(lock);
        let result = f(&mut input_handler);
        window_state.lock().input_handler = Some(input_handler);
        Some(result)
    } else {
        None
    }
}

fn display_id_for_typed_screen(screen: &objc2_app_kit::NSScreen) -> CGDirectDisplayID {
    // Use typed deviceDescription and typed key lookup
    let dict = screen.deviceDescription();
    let key = objc2_foundation::NSString::from_str("NSScreenNumber");
    if let Some(any) = unsafe { dict.objectForKey_unchecked(&key) } {
        let any_ref: &objc2::runtime::AnyObject = any;
        let screen_number: u32 = unsafe { objc2::msg_send![any_ref, unsignedIntValue] };
        screen_number as CGDirectDisplayID
    } else {
        // Fallback to primary display
        MacDisplay::primary().0
    }
}

extern "C" fn blurred_view_init_with_frame(
    this: &Object,
    _: Sel,
    frame: cocoa::foundation::NSRect,
) -> id {
    unsafe {
        let view: id = msg_send![super(this, class!(NSVisualEffectView)), initWithFrame: frame];
        let vref: &objc2_app_kit::NSVisualEffectView = &*(view as *mut objc2_app_kit::NSVisualEffectView);
        // Use a colorless semantic material. The default value `AppearanceBased`, though not manually set, is deprecated.
        vref.setMaterial(objc2_app_kit::NSVisualEffectMaterial::Selection);
        vref.setState(objc2_app_kit::NSVisualEffectState::Active);
        view
    }
}

extern "C" fn blurred_view_update_layer(this: &Object, _: Sel) {
    unsafe {
        let _: () = msg_send![super(this, class!(NSVisualEffectView)), updateLayer];
        let layer_ptr: *mut objc2::runtime::AnyObject = msg_send![this, layer];
        if !layer_ptr.is_null() {
            let layer_ref: &CALayer = &*(layer_ptr as *mut CALayer);
            remove_layer_background(layer_ref);
        }
    }
}

unsafe fn remove_layer_background(layer: &CALayer) {
    unsafe {
        let layer_objc: *mut objc::runtime::Object = layer as *const _ as *mut objc::runtime::Object;
        let _: () = msg_send![layer_objc, setBackgroundColor:nil];

        let class_name: id = msg_send![layer_objc, className];
        let class_ns: &objc2_foundation::NSString = &*(class_name as *mut objc2_foundation::NSString);
        let target = objc2_foundation::NSString::from_str("CAChameleonLayer");
        let is_equal: bool = objc2::msg_send![class_ns, isEqualToString: &*target];
        if is_equal {
            // Remove the desktop tinting effect.
            let _: () = msg_send![layer_objc, setHidden: YES];
            return;
        }

        let filters_id: id = msg_send![layer_objc, filters];
        if !filters_id.is_null() {
            // Remove the increased saturation.
            // The effect of a `CAFilter` or `CIFilter` is determined by its name, and the
            // `description` reflects its name and some parameters. Currently `NSVisualEffectView`
            // uses a `CAFilter` named "colorSaturate". If one day they switch to `CIFilter`, the
            // `description` will still contain "Saturat" ("... inputSaturation = ...").
            let test_string = objc2_foundation::NSString::from_str("Saturat");
            let filters: &objc2_foundation::NSArray<objc2::runtime::AnyObject> =
                &*(filters_id as *mut objc2_foundation::NSArray<objc2::runtime::AnyObject>);
            let count = filters.len();
            for i in 0..count {
                let obj = filters.objectAtIndex(i as objc2_foundation::NSUInteger);
                let any: &objc2::runtime::AnyObject = &*obj;
                let any_objc: *mut objc::runtime::Object = any as *const _ as *mut objc::runtime::Object;
                let description: id = msg_send![any_objc, description];
                let hit: BOOL = msg_send![description, containsString: &*test_string];
                if hit == NO {
                    continue;
                }

                let all_indices = NSRange {
                    location: 0,
                    length: count as objc2_foundation::NSUInteger,
                };
                let indices: id = msg_send![class!(NSMutableIndexSet), indexSet];
                let _: () = msg_send![indices, addIndexesInRange: all_indices];
                let _: () = msg_send![indices, removeIndex:i];
                let filtered: id = msg_send![filters_id, objectsAtIndexes: indices];
                let _: () = msg_send![layer_objc, setFilters: filtered];
                break;
            }
        }

        let sublayers_id: id = msg_send![layer_objc, sublayers];
        if !sublayers_id.is_null() {
            let sublayers: &objc2_foundation::NSArray<CALayer> =
                &*(sublayers_id as *mut objc2_foundation::NSArray<CALayer>);
            for i in 0..sublayers.len() {
                let sublayer = sublayers.objectAtIndex(i as objc2_foundation::NSUInteger);
                remove_layer_background(&sublayer);
            }
        }
    }
}

extern "C" fn add_titlebar_accessory_view_controller(this: &Object, _: Sel, view_controller: id) {
    unsafe {
        let _: () = msg_send![super(this, class!(NSWindow)), addTitlebarAccessoryViewController: view_controller];

        // Hide the native tab bar and set its height to 0, since we render our own.
        let accessory_view: id = msg_send![view_controller, view];
        let _: () = msg_send![accessory_view, setHidden: YES];
        let mut frame: objc2_foundation::NSRect = msg_send![accessory_view, frame];
        frame.size.height = 0.0;
        let _: () = msg_send![accessory_view, setFrame: frame];
    }
}

extern "C" fn move_tab_to_new_window(this: &Object, _: Sel, _: id) {
    unsafe {
        let _: () = msg_send![super(this, class!(NSWindow)), moveTabToNewWindow:nil];

        let window_state = get_window_state(this);
        let mut lock = window_state.as_ref().lock();
        if let Some(mut callback) = lock.move_tab_to_new_window_callback.take() {
            drop(lock);
            callback();
            window_state.lock().move_tab_to_new_window_callback = Some(callback);
        }
    }
}

extern "C" fn merge_all_windows(this: &Object, _: Sel, _: id) {
    unsafe {
        let _: () = msg_send![super(this, class!(NSWindow)), mergeAllWindows:nil];

        let window_state = get_window_state(this);
        let mut lock = window_state.as_ref().lock();
        if let Some(mut callback) = lock.merge_all_windows_callback.take() {
            drop(lock);
            callback();
            window_state.lock().merge_all_windows_callback = Some(callback);
        }
    }
}

extern "C" fn select_next_tab(this: &Object, _sel: Sel, _id: id) {
    let window_state = unsafe { get_window_state(this) };
    let mut lock = window_state.as_ref().lock();
    if let Some(mut callback) = lock.select_next_tab_callback.take() {
        drop(lock);
        callback();
        window_state.lock().select_next_tab_callback = Some(callback);
    }
}

extern "C" fn select_previous_tab(this: &Object, _sel: Sel, _id: id) {
    let window_state = unsafe { get_window_state(this) };
    let mut lock = window_state.as_ref().lock();
    if let Some(mut callback) = lock.select_previous_tab_callback.take() {
        drop(lock);
        callback();
        window_state.lock().select_previous_tab_callback = Some(callback);
    }
}

extern "C" fn toggle_tab_bar(this: &Object, _sel: Sel, _id: id) {
    unsafe {
        let _: () = msg_send![super(this, class!(NSWindow)), toggleTabBar:nil];

        let window_state = get_window_state(this);
        let mut lock = window_state.as_ref().lock();
        lock.move_traffic_light();

        if let Some(mut callback) = lock.toggle_tab_bar_callback.take() {
            drop(lock);
            callback();
            window_state.lock().toggle_tab_bar_callback = Some(callback);
        }
    }
}
