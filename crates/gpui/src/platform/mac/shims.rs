use objc2::{msg_send, class};
use objc2::runtime::AnyObject;
use objc2_foundation::{NSRect, NSString};
use objc2_app_kit::{NSWindow, NSWindowButton, NSTrackingAreaOptions, NSStatusItem};

pub(crate) unsafe fn nswindow_standard_button(win: &NSWindow, button: NSWindowButton) -> *mut AnyObject {
    msg_send![win, standardWindowButton: button]
}

pub(crate) unsafe fn nswindow_content_view(win: &NSWindow) -> *mut AnyObject {
    msg_send![win, contentView]
}

pub(crate) unsafe fn nswindow_set_tabbing_identifier(win: &NSWindow, ident: Option<&NSString>) {
    let _: () = msg_send![win, setTabbingIdentifier: ident];
}

pub(crate) unsafe fn nsscreen_screens() -> *mut AnyObject {
    msg_send![class!(NSScreen), screens]
}

pub(crate) unsafe fn nsscreen_main() -> *mut AnyObject {
    msg_send![class!(NSScreen), mainScreen]
}

pub(crate) unsafe fn nstrackingarea_init(rect: NSRect, options: NSTrackingAreaOptions, owner: *mut AnyObject, user_info: *mut AnyObject) -> *mut AnyObject {
    let tracking_area: *mut AnyObject = msg_send![class!(NSTrackingArea), alloc];
    msg_send![tracking_area, initWithRect: rect, options: options, owner: owner, userInfo: user_info]
}

pub(crate) unsafe fn nsobject_autorelease(obj: *mut AnyObject) {
    let _: () = msg_send![obj, autorelease];
}

pub(crate) unsafe fn nswindow_add_tabbed_window(window: *mut AnyObject, new_window: *mut AnyObject, ordered: i64) {
    let _: () = msg_send![window, addTabbedWindow: new_window, ordered: ordered];
}

pub(crate) unsafe fn status_item_button_ptr(item: &NSStatusItem) -> *mut AnyObject {
    msg_send![item, button]
}
