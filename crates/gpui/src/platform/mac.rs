//! Macos screen have a y axis that goings up from the bottom of the screen and
//! an origin at the bottom left of the main display.
mod dispatcher;
mod display;
mod display_link;
mod events;
mod keyboard;

#[cfg(feature = "screen-capture")]
mod screen_capture;

#[cfg(all(not(feature = "macos-blade"), not(feature = "macos-metal4")))]
mod metal_atlas;
#[cfg(all(not(feature = "macos-blade"), feature = "macos-metal4"))]
pub mod metal4_renderer;
#[cfg(all(not(feature = "macos-blade"), not(feature = "macos-metal4")))]
pub mod metal_renderer;

use core_video::image_buffer::CVImageBuffer;
#[cfg(all(not(feature = "macos-blade"), feature = "macos-metal4"))]
use metal4_renderer as renderer;
#[cfg(all(not(feature = "macos-blade"), not(feature = "macos-metal4")))]
use metal_renderer as renderer;

#[cfg(feature = "macos-blade")]
use crate::platform::blade as renderer;


#[cfg(feature = "font-kit")]
mod open_type;

#[cfg(feature = "font-kit")]
mod text_system;

mod platform;
mod window;
#[cfg(feature = "macos-swift")]
mod swift_window;
mod window_appearance;
mod shims;
#[cfg(feature = "macos-swift")]
mod swift_ffi;

use crate::{DevicePixels, Pixels, Size, px, size};
use objc2_foundation::{NSRect, NSSize, NSNotFound, NSUInteger};
use objc2::runtime::Bool;
use std::ops::Range;

pub(crate) use dispatcher::*;
pub(crate) use display::*;
pub(crate) use display_link::*;
pub(crate) use keyboard::*;
pub(crate) use platform::*;
#[cfg(not(feature = "macos-swift"))]
pub(crate) use window::*;
#[cfg(feature = "macos-swift")]
pub(crate) use swift_window::*;
#[cfg(feature = "macos-swift")]
pub(crate) use swift_ffi::*;

#[cfg(feature = "font-kit")]
pub(crate) use text_system::*;

/// A frame of video captured from a screen.
pub(crate) type PlatformScreenCaptureFrame = CVImageBuffer;

trait BoolExt {
    fn to_objc(self) -> Bool;
}

impl BoolExt for bool {
    fn to_objc(self) -> Bool {
        if self { Bool::YES } else { Bool::NO }
    }
}

// Deprecated NSString::UTF8String shim removed; use typed NSString + autoreleasepool

// Use objc2_foundation's NSRange directly and extend its helpers
pub(crate) type NSRange = objc2_foundation::NSRange;

trait NSRangeExt {
    fn invalid() -> Self;
    fn is_valid(&self) -> bool;
    fn to_range(self) -> Option<Range<usize>>;
}

impl NSRangeExt for NSRange {
    fn invalid() -> Self {
        Self::new(NSNotFound as NSUInteger, 0)
    }
    fn is_valid(&self) -> bool {
        self.location != NSNotFound as NSUInteger
    }
    fn to_range(self) -> Option<Range<usize>> {
        if self.is_valid() {
            let start = self.location as usize;
            let end = start + self.length as usize;
            Some(start..end)
        } else {
            None
        }
    }
}

impl From<NSSize> for Size<Pixels> {
    fn from(value: NSSize) -> Self {
        Size {
            width: px(value.width as f32),
            height: px(value.height as f32),
        }
    }
}

impl From<NSRect> for Size<Pixels> {
    fn from(rect: NSRect) -> Self {
        let NSSize { width, height } = rect.size;
        size(width.into(), height.into())
    }
}

impl From<NSRect> for Size<DevicePixels> {
    fn from(rect: NSRect) -> Self {
        let NSSize { width, height } = rect.size;
        size(DevicePixels(width as i32), DevicePixels(height as i32))
    }
}

// Removed Cocoa NSRect/NSSize conversions; use typed objc2_foundation NSRect/NSSize instead.
