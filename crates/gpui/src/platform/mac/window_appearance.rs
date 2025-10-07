use crate::WindowAppearance;
use objc2::{msg_send, rc::Retained, runtime::AnyObject};
use objc2_foundation::{ns_string, NSString};

impl WindowAppearance {
    // Accept an Objective-C object pointer and use objc2 APIs to inspect it.
    pub(crate) unsafe fn from_native(appearance: *mut AnyObject) -> Self {
        // SAFETY: Caller guarantees `appearance` is an NSAppearance instance.
        let receiver: &AnyObject = unsafe { &*appearance };
        let name: Retained<NSString> = msg_send![receiver, name];

        // Compare against the canonical AppKit appearance name strings.
        // Using string equality avoids relying on deprecated cocoa constants.
        if &*name == ns_string!("NSAppearanceNameVibrantLight") {
            Self::VibrantLight
        } else if &*name == ns_string!("NSAppearanceNameVibrantDark") {
            Self::VibrantDark
        } else if &*name == ns_string!("NSAppearanceNameAqua") {
            Self::Light
        } else if &*name == ns_string!("NSAppearanceNameDarkAqua") {
            Self::Dark
        } else {
            // Fall back to light and log the unknown value for diagnostics.
            // Note: `to_string()` allocates; keep this on unknown path only.
            println!("unknown appearance: {:?}", name.to_string());
            Self::Light
        }
    }
}
