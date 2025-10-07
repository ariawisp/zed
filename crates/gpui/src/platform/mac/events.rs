use crate::{
    Capslock, KeyDownEvent, KeyUpEvent, Keystroke, Modifiers, ModifiersChangedEvent, MouseButton,
    MouseDownEvent, MouseExitEvent, MouseMoveEvent, MouseUpEvent, NavigationDirection, Pixels,
    PlatformInput, ScrollDelta, ScrollWheelEvent, TouchPhase,
    platform::mac::{
        LMGetKbdType, TISCopyCurrentKeyboardLayoutInputSource,
        TISGetInputSourceProperty, UCKeyTranslate, kTISPropertyUnicodeKeyLayoutData,
    },
    point, px,
};
use objc2_app_kit::{NSEvent, NSEventModifierFlags, NSEventPhase, NSEventType};
use objc2::rc::autoreleasepool;
use core_foundation::data::{CFDataGetBytePtr, CFDataRef};
use core_graphics::event::CGKeyCode;
// Prefer objc2 messaging; only use objc macros elsewhere for dynamic classes
use std::{borrow::Cow, ffi::c_void};

const BACKSPACE_KEY: u16 = 0x7f;
const SPACE_KEY: u16 = b' ' as u16;
const ENTER_KEY: u16 = 0x0d;
const NUMPAD_ENTER_KEY: u16 = 0x03;
pub(crate) const ESCAPE_KEY: u16 = 0x1b;
const TAB_KEY: u16 = 0x09;
const SHIFT_TAB_KEY: u16 = 0x19;

// Function-key Unicode values reserved by AppKit (0xF700–0xF8FF).
// Copied from AppKit/NSEvent.h to avoid importing Cocoa's deprecated constants.
#[allow(non_upper_case_globals)]
mod function_keys {
    pub const NSUpArrowFunctionKey: u16 = 0xF700;
    pub const NSDownArrowFunctionKey: u16 = 0xF701;
    pub const NSLeftArrowFunctionKey: u16 = 0xF702;
    pub const NSRightArrowFunctionKey: u16 = 0xF703;
    pub const NSF1FunctionKey: u16 = 0xF704;
    pub const NSF2FunctionKey: u16 = 0xF705;
    pub const NSF3FunctionKey: u16 = 0xF706;
    pub const NSF4FunctionKey: u16 = 0xF707;
    pub const NSF5FunctionKey: u16 = 0xF708;
    pub const NSF6FunctionKey: u16 = 0xF709;
    pub const NSF7FunctionKey: u16 = 0xF70A;
    pub const NSF8FunctionKey: u16 = 0xF70B;
    pub const NSF9FunctionKey: u16 = 0xF70C;
    pub const NSF10FunctionKey: u16 = 0xF70D;
    pub const NSF11FunctionKey: u16 = 0xF70E;
    pub const NSF12FunctionKey: u16 = 0xF70F;
    pub const NSF13FunctionKey: u16 = 0xF710;
    pub const NSF14FunctionKey: u16 = 0xF711;
    pub const NSF15FunctionKey: u16 = 0xF712;
    pub const NSF16FunctionKey: u16 = 0xF713;
    pub const NSF17FunctionKey: u16 = 0xF714;
    pub const NSF18FunctionKey: u16 = 0xF715;
    pub const NSF19FunctionKey: u16 = 0xF716;
    pub const NSF20FunctionKey: u16 = 0xF717;
    pub const NSF21FunctionKey: u16 = 0xF718;
    pub const NSF22FunctionKey: u16 = 0xF719;
    pub const NSF23FunctionKey: u16 = 0xF71A;
    pub const NSF24FunctionKey: u16 = 0xF71B;
    pub const NSF25FunctionKey: u16 = 0xF71C;
    pub const NSF26FunctionKey: u16 = 0xF71D;
    pub const NSF27FunctionKey: u16 = 0xF71E;
    pub const NSF28FunctionKey: u16 = 0xF71F;
    pub const NSF29FunctionKey: u16 = 0xF720;
    pub const NSF30FunctionKey: u16 = 0xF721;
    pub const NSF31FunctionKey: u16 = 0xF722;
    pub const NSF32FunctionKey: u16 = 0xF723;
    pub const NSF33FunctionKey: u16 = 0xF724;
    pub const NSF34FunctionKey: u16 = 0xF725;
    pub const NSF35FunctionKey: u16 = 0xF726;
    pub const NSDeleteFunctionKey: u16 = 0xF728;
    pub const NSHomeFunctionKey: u16 = 0xF729;
    pub const NSEndFunctionKey: u16 = 0xF72B;
    pub const NSPageUpFunctionKey: u16 = 0xF72C;
    pub const NSPageDownFunctionKey: u16 = 0xF72D;
    pub const NSHelpFunctionKey: u16 = 0xF746;
    pub const NSModeSwitchFunctionKey: u16 = 0xF747;
}

pub fn key_to_native(key: &str) -> Cow<'_, str> {
    let code = match key {
        "space" => SPACE_KEY,
        "backspace" => BACKSPACE_KEY,
        "escape" => ESCAPE_KEY,
        "up" => function_keys::NSUpArrowFunctionKey,
        "down" => function_keys::NSDownArrowFunctionKey,
        "left" => function_keys::NSLeftArrowFunctionKey,
        "right" => function_keys::NSRightArrowFunctionKey,
        "pageup" => function_keys::NSPageUpFunctionKey,
        "pagedown" => function_keys::NSPageDownFunctionKey,
        "home" => function_keys::NSHomeFunctionKey,
        "end" => function_keys::NSEndFunctionKey,
        "delete" => function_keys::NSDeleteFunctionKey,
        "insert" => function_keys::NSHelpFunctionKey,
        "f1" => function_keys::NSF1FunctionKey,
        "f2" => function_keys::NSF2FunctionKey,
        "f3" => function_keys::NSF3FunctionKey,
        "f4" => function_keys::NSF4FunctionKey,
        "f5" => function_keys::NSF5FunctionKey,
        "f6" => function_keys::NSF6FunctionKey,
        "f7" => function_keys::NSF7FunctionKey,
        "f8" => function_keys::NSF8FunctionKey,
        "f9" => function_keys::NSF9FunctionKey,
        "f10" => function_keys::NSF10FunctionKey,
        "f11" => function_keys::NSF11FunctionKey,
        "f12" => function_keys::NSF12FunctionKey,
        "f13" => function_keys::NSF13FunctionKey,
        "f14" => function_keys::NSF14FunctionKey,
        "f15" => function_keys::NSF15FunctionKey,
        "f16" => function_keys::NSF16FunctionKey,
        "f17" => function_keys::NSF17FunctionKey,
        "f18" => function_keys::NSF18FunctionKey,
        "f19" => function_keys::NSF19FunctionKey,
        "f20" => function_keys::NSF20FunctionKey,
        "f21" => function_keys::NSF21FunctionKey,
        "f22" => function_keys::NSF22FunctionKey,
        "f23" => function_keys::NSF23FunctionKey,
        "f24" => function_keys::NSF24FunctionKey,
        "f25" => function_keys::NSF25FunctionKey,
        "f26" => function_keys::NSF26FunctionKey,
        "f27" => function_keys::NSF27FunctionKey,
        "f28" => function_keys::NSF28FunctionKey,
        "f29" => function_keys::NSF29FunctionKey,
        "f30" => function_keys::NSF30FunctionKey,
        "f31" => function_keys::NSF31FunctionKey,
        "f32" => function_keys::NSF32FunctionKey,
        "f33" => function_keys::NSF33FunctionKey,
        "f34" => function_keys::NSF34FunctionKey,
        "f35" => function_keys::NSF35FunctionKey,
        _ => return Cow::Borrowed(key),
    };
    Cow::Owned(String::from_utf16(&[code]).unwrap())
}

fn read_modifiers(ev: &NSEvent) -> Modifiers {
    let modifiers: objc2_app_kit::NSEventModifierFlags = ev.modifierFlags();
    let control = modifiers.contains(NSEventModifierFlags::Control);
    let alt = modifiers.contains(NSEventModifierFlags::Option);
    let shift = modifiers.contains(NSEventModifierFlags::Shift);
    let command = modifiers.contains(NSEventModifierFlags::Command);
    let function = modifiers.contains(NSEventModifierFlags::Function);

    Modifiers { control, alt, shift, platform: command, function }
}

impl PlatformInput {
    pub(crate) fn from_native(ev: &NSEvent, window_height: Option<Pixels>) -> Option<Self> {
        let event_type: NSEventType = ev.r#type();

            // Filter out event types that aren't in the NSEventType enum.
            // See https://github.com/servo/cocoa-rs/issues/155#issuecomment-323482792 for details.
            match event_type.0 as u64 {
                0 | 21 | 32 | 33 | 35 | 36 | 37 => {
                    return None;
                }
                _ => {}
            }

            match event_type {
                NSEventType::FlagsChanged => {
                    let m = ev.modifierFlags();
                    Some(Self::ModifiersChanged(ModifiersChangedEvent {
                        modifiers: read_modifiers(ev),
                        capslock: Capslock { on: m.contains(NSEventModifierFlags::CapsLock) },
                    }))
                }
                NSEventType::KeyDown => Some(Self::KeyDown(KeyDownEvent {
                    keystroke: parse_keystroke(ev),
                    is_held: ev.isARepeat(),
                })),
                NSEventType::KeyUp => Some(Self::KeyUp(KeyUpEvent {
                    keystroke: parse_keystroke(ev),
                })),
                NSEventType::LeftMouseDown
                | NSEventType::RightMouseDown
                | NSEventType::OtherMouseDown => {
                    let button = match ev.buttonNumber() as i64 {
                        0 => MouseButton::Left,
                        1 => MouseButton::Right,
                        2 => MouseButton::Middle,
                        3 => MouseButton::Navigate(NavigationDirection::Back),
                        4 => MouseButton::Navigate(NavigationDirection::Forward),
                        // Other mouse buttons aren't tracked currently
                        _ => return None,
                    };
                    window_height.map(|window_height| {
                        let p = ev.locationInWindow();
                        Self::MouseDown(MouseDownEvent {
                            button,
                            position: point(
                                px(p.x as f32),
                                // MacOS screen coordinates are relative to bottom left
                                window_height - px(p.y as f32),
                            ),
                            modifiers: read_modifiers(ev),
                            click_count: ev.clickCount() as usize,
                            first_mouse: false,
                        })
                    })
                }
                NSEventType::LeftMouseUp
                | NSEventType::RightMouseUp
                | NSEventType::OtherMouseUp => {
                    let button = match ev.buttonNumber() as i64 {
                        0 => MouseButton::Left,
                        1 => MouseButton::Right,
                        2 => MouseButton::Middle,
                        3 => MouseButton::Navigate(NavigationDirection::Back),
                        4 => MouseButton::Navigate(NavigationDirection::Forward),
                        // Other mouse buttons aren't tracked currently
                        _ => return None,
                    };

                    window_height.map(|window_height| {
                        let p = ev.locationInWindow();
                        Self::MouseUp(MouseUpEvent {
                            button,
                            position: point(
                                px(p.x as f32),
                                window_height - px(p.y as f32),
                            ),
                            modifiers: read_modifiers(ev),
                            click_count: ev.clickCount() as usize,
                        })
                    })
                }
                // Some mice (like Logitech MX Master) send navigation buttons as swipe events
                NSEventType::Swipe => {
                    let phase: NSEventPhase = ev.phase();
                    let navigation_direction = if phase.contains(NSEventPhase::Ended) {
                        match unsafe { let dx: f64 = objc2::msg_send![ev, deltaX]; dx } {
                            x if x > 0.0 => Some(NavigationDirection::Back),
                            x if x < 0.0 => Some(NavigationDirection::Forward),
                            _ => return None,
                        }
                    } else {
                        None
                    };

                    match navigation_direction {
                        Some(direction) => window_height.map(|window_height| {
                            let p = ev.locationInWindow();
                            Self::MouseDown(MouseDownEvent {
                                button: MouseButton::Navigate(direction),
                                position: point(
                                    px(p.x as f32),
                                    window_height - px(p.y as f32),
                                ),
                                modifiers: read_modifiers(ev),
                                click_count: 1,
                                first_mouse: false,
                            })
                        }),
                        _ => None,
                    }
                }
                NSEventType::ScrollWheel => window_height.map(|window_height| {
                    let phase_bits: NSEventPhase = ev.phase();
                    let phase = if phase_bits.contains(NSEventPhase::MayBegin) || phase_bits.contains(NSEventPhase::Began) {
                        TouchPhase::Started
                    } else if phase_bits.contains(NSEventPhase::Ended) {
                        TouchPhase::Ended
                    } else {
                        TouchPhase::Moved
                    };

                    let dx: f64 = unsafe { objc2::msg_send![ev, scrollingDeltaX] };
                    let dy: f64 = unsafe { objc2::msg_send![ev, scrollingDeltaY] };
                    let raw_data = point(dx as f32, dy as f32);

                    let delta = if ev.hasPreciseScrollingDeltas() {
                        ScrollDelta::Pixels(raw_data.map(px))
                    } else {
                        ScrollDelta::Lines(raw_data)
                    };

                    Self::ScrollWheel(ScrollWheelEvent {
                        position: {
                            let p = ev.locationInWindow();
                            point(px(p.x as f32), window_height - px(p.y as f32))
                        },
                        delta,
                        touch_phase: phase,
                        modifiers: read_modifiers(ev),
                    })
                }),
                NSEventType::LeftMouseDragged
                | NSEventType::RightMouseDragged
                | NSEventType::OtherMouseDragged => {
                    let pressed_button = match ev.buttonNumber() as i64 {
                        0 => MouseButton::Left,
                        1 => MouseButton::Right,
                        2 => MouseButton::Middle,
                        3 => MouseButton::Navigate(NavigationDirection::Back),
                        4 => MouseButton::Navigate(NavigationDirection::Forward),
                        // Other mouse buttons aren't tracked currently
                        _ => return None,
                    };

                    window_height.map(|window_height| {
                        let p = ev.locationInWindow();
                        Self::MouseMove(MouseMoveEvent {
                            pressed_button: Some(pressed_button),
                            position: point(
                                px(p.x as f32),
                                window_height - px(p.y as f32),
                            ),
                            modifiers: read_modifiers(ev),
                        })
                    })
                }
                NSEventType::MouseMoved => window_height.map(|window_height| {
                    let p = ev.locationInWindow();
                    Self::MouseMove(MouseMoveEvent {
                        position: point(px(p.x as f32), window_height - px(p.y as f32)),
                        pressed_button: None,
                        modifiers: read_modifiers(ev),
                    })
                }),
                NSEventType::MouseExited => window_height.map(|window_height| {
                    let p = ev.locationInWindow();
                    Self::MouseExited(MouseExitEvent {
                        position: point(px(p.x as f32), window_height - px(p.y as f32)),

                        pressed_button: None,
                        modifiers: read_modifiers(ev),
                    })
                }),
                _ => None,
            }
        }
}

fn parse_keystroke(ev: &NSEvent) -> Keystroke {
    let mut characters = if let Some(ns) = ev.charactersIgnoringModifiers() {
        autoreleasepool(|pool| unsafe { ns.to_str(pool).to_owned() })
    } else {
        String::new()
    };
        let mut key_char = None;
        let first_char = characters.chars().next().map(|ch| ch as u16);
        let modifiers: objc2_app_kit::NSEventModifierFlags = ev.modifierFlags();
        let key_code: u16 = ev.keyCode() as u16;

        let control = modifiers.contains(objc2_app_kit::NSEventModifierFlags::Control);
        let alt = modifiers.contains(objc2_app_kit::NSEventModifierFlags::Option);
        let mut shift = modifiers.contains(objc2_app_kit::NSEventModifierFlags::Shift);
        let command = modifiers.contains(objc2_app_kit::NSEventModifierFlags::Command);
        let function = modifiers.contains(objc2_app_kit::NSEventModifierFlags::Function)
            && first_char
                .is_none_or(|ch| !(function_keys::NSUpArrowFunctionKey..=function_keys::NSModeSwitchFunctionKey).contains(&ch));

        #[allow(non_upper_case_globals)]
        let key = match first_char {
            Some(SPACE_KEY) => {
                key_char = Some(" ".to_string());
                "space".to_string()
            }
            Some(TAB_KEY) => {
                key_char = Some("\t".to_string());
                "tab".to_string()
            }
            Some(ENTER_KEY) | Some(NUMPAD_ENTER_KEY) => {
                key_char = Some("\n".to_string());
                "enter".to_string()
            }
            Some(BACKSPACE_KEY) => "backspace".to_string(),
            Some(ESCAPE_KEY) => "escape".to_string(),
            Some(SHIFT_TAB_KEY) => "tab".to_string(),
            Some(function_keys::NSUpArrowFunctionKey) => "up".to_string(),
            Some(function_keys::NSDownArrowFunctionKey) => "down".to_string(),
            Some(function_keys::NSLeftArrowFunctionKey) => "left".to_string(),
            Some(function_keys::NSRightArrowFunctionKey) => "right".to_string(),
            Some(function_keys::NSPageUpFunctionKey) => "pageup".to_string(),
            Some(function_keys::NSPageDownFunctionKey) => "pagedown".to_string(),
            Some(function_keys::NSHomeFunctionKey) => "home".to_string(),
            Some(function_keys::NSEndFunctionKey) => "end".to_string(),
            Some(function_keys::NSDeleteFunctionKey) => "delete".to_string(),
            // Observed Insert==NSHelpFunctionKey not NSInsertFunctionKey.
            Some(function_keys::NSHelpFunctionKey) => "insert".to_string(),
            Some(function_keys::NSF1FunctionKey) => "f1".to_string(),
            Some(function_keys::NSF2FunctionKey) => "f2".to_string(),
            Some(function_keys::NSF3FunctionKey) => "f3".to_string(),
            Some(function_keys::NSF4FunctionKey) => "f4".to_string(),
            Some(function_keys::NSF5FunctionKey) => "f5".to_string(),
            Some(function_keys::NSF6FunctionKey) => "f6".to_string(),
            Some(function_keys::NSF7FunctionKey) => "f7".to_string(),
            Some(function_keys::NSF8FunctionKey) => "f8".to_string(),
            Some(function_keys::NSF9FunctionKey) => "f9".to_string(),
            Some(function_keys::NSF10FunctionKey) => "f10".to_string(),
            Some(function_keys::NSF11FunctionKey) => "f11".to_string(),
            Some(function_keys::NSF12FunctionKey) => "f12".to_string(),
            Some(function_keys::NSF13FunctionKey) => "f13".to_string(),
            Some(function_keys::NSF14FunctionKey) => "f14".to_string(),
            Some(function_keys::NSF15FunctionKey) => "f15".to_string(),
            Some(function_keys::NSF16FunctionKey) => "f16".to_string(),
            Some(function_keys::NSF17FunctionKey) => "f17".to_string(),
            Some(function_keys::NSF18FunctionKey) => "f18".to_string(),
            Some(function_keys::NSF19FunctionKey) => "f19".to_string(),
            Some(function_keys::NSF20FunctionKey) => "f20".to_string(),
            Some(function_keys::NSF21FunctionKey) => "f21".to_string(),
            Some(function_keys::NSF22FunctionKey) => "f22".to_string(),
            Some(function_keys::NSF23FunctionKey) => "f23".to_string(),
            Some(function_keys::NSF24FunctionKey) => "f24".to_string(),
            Some(function_keys::NSF25FunctionKey) => "f25".to_string(),
            Some(function_keys::NSF26FunctionKey) => "f26".to_string(),
            Some(function_keys::NSF27FunctionKey) => "f27".to_string(),
            Some(function_keys::NSF28FunctionKey) => "f28".to_string(),
            Some(function_keys::NSF29FunctionKey) => "f29".to_string(),
            Some(function_keys::NSF30FunctionKey) => "f30".to_string(),
            Some(function_keys::NSF31FunctionKey) => "f31".to_string(),
            Some(function_keys::NSF32FunctionKey) => "f32".to_string(),
            Some(function_keys::NSF33FunctionKey) => "f33".to_string(),
            Some(function_keys::NSF34FunctionKey) => "f34".to_string(),
            Some(function_keys::NSF35FunctionKey) => "f35".to_string(),
            _ => {
                // Cases to test when modifying this:
                //
                //           qwerty key | none | cmd   | cmd-shift
                // * Armenian         s | ս    | cmd-s | cmd-shift-s  (layout is non-ASCII, so we use cmd layout)
                // * Dvorak+QWERTY    s | o    | cmd-s | cmd-shift-s  (layout switches on cmd)
                // * Ukrainian+QWERTY s | с    | cmd-s | cmd-shift-s  (macOS reports cmd-s instead of cmd-S)
                // * Czech            7 | ý    | cmd-ý | cmd-7        (layout has shifted numbers)
                // * Norwegian        7 | 7    | cmd-7 | cmd-/        (macOS reports cmd-shift-7 instead of cmd-/)
                // * Russian          7 | 7    | cmd-7 | cmd-&        (shift-7 is . but when cmd is down, should use cmd layout)
                // * German QWERTZ    ; | ö    | cmd-ö | cmd-Ö        (Zed's shift special case only applies to a-z)
                //
                let mut chars_ignoring_modifiers =
                    chars_for_modified_key(key_code as CGKeyCode, NO_MOD);
                let mut chars_with_shift =
                    chars_for_modified_key(key_code as CGKeyCode, SHIFT_MOD);
                let always_use_cmd_layout = always_use_command_layout();

                // Handle Dvorak+QWERTY / Russian / Armenian
                if command || always_use_cmd_layout {
                    let chars_with_cmd = chars_for_modified_key(key_code as CGKeyCode, CMD_MOD);
                    let chars_with_both =
                        chars_for_modified_key(key_code as CGKeyCode, CMD_MOD | SHIFT_MOD);

                    // We don't do this in the case that the shifted command key generates
                    // the same character as the unshifted command key (Norwegian, e.g.)
                    if chars_with_both != chars_with_cmd {
                        chars_with_shift = chars_with_both;

                    // Handle edge-case where cmd-shift-s reports cmd-s instead of
                    // cmd-shift-s (Ukrainian, etc.)
                    } else if chars_with_cmd.to_ascii_uppercase() != chars_with_cmd {
                        chars_with_shift = chars_with_cmd.to_ascii_uppercase();
                    }
                    chars_ignoring_modifiers = chars_with_cmd;
                }

                if !control && !command && !function {
                    let mut mods = NO_MOD;
                    if shift {
                        mods |= SHIFT_MOD;
                    }
                    if alt {
                        mods |= OPTION_MOD;
                    }

                    key_char = Some(chars_for_modified_key(key_code as CGKeyCode, mods));
                }

                if shift
                    && chars_ignoring_modifiers
                        .chars()
                        .all(|c| c.is_ascii_lowercase())
                {
                    chars_ignoring_modifiers
                } else if shift {
                    shift = false;
                    chars_with_shift
                } else {
                    chars_ignoring_modifiers
                }
            }
        };

        Keystroke {
            modifiers: Modifiers {
                control,
                alt,
                shift,
                platform: command,
                function,
            },
            key,
            key_char,
        }
}

fn always_use_command_layout() -> bool {
    if chars_for_modified_key(0, NO_MOD).is_ascii() {
        return false;
    }

    chars_for_modified_key(0, CMD_MOD).is_ascii()
}

const NO_MOD: u32 = 0;
const CMD_MOD: u32 = 1;
const SHIFT_MOD: u32 = 2;
const OPTION_MOD: u32 = 8;

fn chars_for_modified_key(code: CGKeyCode, modifiers: u32) -> String {
    // Values from: https://github.com/phracker/MacOSX-SDKs/blob/master/MacOSX10.6.sdk/System/Library/Frameworks/Carbon.framework/Versions/A/Frameworks/HIToolbox.framework/Versions/A/Headers/Events.h#L126
    // shifted >> 8 for UCKeyTranslate
    const CG_SPACE_KEY: u16 = 49;
    // https://github.com/phracker/MacOSX-SDKs/blob/master/MacOSX10.6.sdk/System/Library/Frameworks/CoreServices.framework/Versions/A/Frameworks/CarbonCore.framework/Versions/A/Headers/UnicodeUtilities.h#L278
    #[allow(non_upper_case_globals)]
    const kUCKeyActionDown: u16 = 0;
    #[allow(non_upper_case_globals)]
    const kUCKeyTranslateNoDeadKeysMask: u32 = 0;

    let keyboard_type = unsafe { LMGetKbdType() as u32 };
    const BUFFER_SIZE: usize = 4;
    let mut dead_key_state = 0;
    let mut buffer: [u16; BUFFER_SIZE] = [0; BUFFER_SIZE];
    let mut buffer_size: usize = 0;

    let keyboard = unsafe { TISCopyCurrentKeyboardLayoutInputSource() };
    if keyboard.is_null() {
        return "".to_string();
    }
    let layout_data = unsafe {
        TISGetInputSourceProperty(keyboard, kTISPropertyUnicodeKeyLayoutData as *const c_void)
            as CFDataRef
    };
    if layout_data.is_null() {
        unsafe { let _: () = objc2::msg_send![keyboard as *mut objc2::runtime::AnyObject, release]; }
        return "".to_string();
    }
    let keyboard_layout = unsafe { CFDataGetBytePtr(layout_data) };

    unsafe {
        UCKeyTranslate(
            keyboard_layout as *const c_void,
            code,
            kUCKeyActionDown,
            modifiers,
            keyboard_type,
            kUCKeyTranslateNoDeadKeysMask,
            &mut dead_key_state,
            BUFFER_SIZE,
            &mut buffer_size as *mut usize,
            &mut buffer as *mut u16,
        );
        if dead_key_state != 0 {
            UCKeyTranslate(
                keyboard_layout as *const c_void,
                CG_SPACE_KEY,
                kUCKeyActionDown,
                modifiers,
                keyboard_type,
                kUCKeyTranslateNoDeadKeysMask,
                &mut dead_key_state,
                BUFFER_SIZE,
                &mut buffer_size as *mut usize,
                &mut buffer as *mut u16,
            );
        }
        let _: () = objc2::msg_send![keyboard as *mut objc2::runtime::AnyObject, release];
    }
    String::from_utf16(&buffer[..buffer_size]).unwrap_or_default()
}
