use crate::{
    DevicePixels, ForegroundExecutor, SharedString, SourceMetadata,
    platform::{ScreenCaptureFrame, ScreenCaptureSource, ScreenCaptureStream},
    size,
};
use anyhow::{Result, anyhow};
use block::ConcreteBlock;
// Remove Cocoa id/nil; prefer objc2 types and msg_send
use collections::HashMap;
use core_foundation::base::TCFType;
use core_graphics::display::{
    CGDirectDisplayID, CGDisplayCopyDisplayMode, CGDisplayModeGetPixelHeight,
    CGDisplayModeGetPixelWidth, CGDisplayModeRelease,
};
use ctor::ctor;
use futures::channel::oneshot;
use media::core_media::{CMSampleBuffer, CMSampleBufferRef};
use metal::NSInteger;
use objc::{
    class,
    declare::ClassDecl,
    runtime::{Class, Object, Sel},
    sel,
};
use std::{cell::RefCell, ffi::c_void, mem, ptr, rc::Rc};

use objc2_foundation::NSString as Objc2NSString;

#[derive(Clone)]
pub struct MacScreenCaptureSource {
    sc_display: *mut objc2::runtime::AnyObject,
    meta: Option<ScreenMeta>,
}

pub struct MacScreenCaptureStream {
    sc_stream: *mut objc2::runtime::AnyObject,
    sc_stream_output: *mut objc2::runtime::AnyObject,
    meta: SourceMetadata,
}

static mut DELEGATE_CLASS: *const Class = ptr::null();
static mut OUTPUT_CLASS: *const Class = ptr::null();
const FRAME_CALLBACK_IVAR: &str = "frame_callback";

#[allow(non_upper_case_globals)]
const SCStreamOutputTypeScreen: NSInteger = 0;

impl ScreenCaptureSource for MacScreenCaptureSource {
    fn metadata(&self) -> Result<SourceMetadata> {
        let (display_id, size) = unsafe {
            let display_id: CGDirectDisplayID = msg_send![self.sc_display, displayID];
            let display_mode_ref = CGDisplayCopyDisplayMode(display_id);
            let width = CGDisplayModeGetPixelWidth(display_mode_ref);
            let height = CGDisplayModeGetPixelHeight(display_mode_ref);
            CGDisplayModeRelease(display_mode_ref);

            (
                display_id,
                size(DevicePixels(width as i32), DevicePixels(height as i32)),
            )
        };
        let (label, is_main) = self
            .meta
            .clone()
            .map(|meta| (meta.label, meta.is_main))
            .unzip();

        Ok(SourceMetadata {
            id: display_id as u64,
            label,
            is_main,
            resolution: size,
        })
    }

    fn stream(
        &self,
        _foreground_executor: &ForegroundExecutor,
        frame_callback: Box<dyn Fn(ScreenCaptureFrame) + Send>,
    ) -> oneshot::Receiver<Result<Box<dyn ScreenCaptureStream>>> {
        unsafe {
            let stream: *mut objc2::runtime::AnyObject = objc2::msg_send![objc2::class!(SCStream), alloc];
            let filter: *mut objc2::runtime::AnyObject = objc2::msg_send![objc2::class!(SCContentFilter), alloc];
            let configuration: *mut objc2::runtime::AnyObject = objc2::msg_send![objc2::class!(SCStreamConfiguration), alloc];
            let delegate: *mut objc2::runtime::AnyObject = objc2::msg_send![DELEGATE_CLASS, alloc];
            let output: *mut objc2::runtime::AnyObject = objc2::msg_send![OUTPUT_CLASS, alloc];

            let excluded_windows: *mut objc2::runtime::AnyObject = objc2::msg_send![objc2::class!(NSArray), array];
            let filter: *mut objc2::runtime::AnyObject = objc2::msg_send![filter, initWithDisplay: self.sc_display, excludingWindows: excluded_windows];
            let configuration: *mut objc2::runtime::AnyObject = objc2::msg_send![configuration, init];
            let _: *mut objc2::runtime::AnyObject = objc2::msg_send![configuration, setScalesToFit: true];
            let _: *mut objc2::runtime::AnyObject = objc2::msg_send![configuration, setPixelFormat: 0x42475241];
            // let _: id = msg_send![configuration, setShowsCursor: false];
            // let _: id = msg_send![configuration, setCaptureResolution: 3];
            let delegate: *mut objc2::runtime::AnyObject = objc2::msg_send![delegate, init];
            let output: *mut objc2::runtime::AnyObject = objc2::msg_send![output, init];

            output.as_mut().unwrap().set_ivar(
                FRAME_CALLBACK_IVAR,
                Box::into_raw(Box::new(frame_callback)) as *mut c_void,
            );

            let meta = self.metadata().unwrap();
            let _: *mut objc2::runtime::AnyObject = objc2::msg_send![configuration, setWidth: (meta.resolution.width.0 as i64)];
            let _: *mut objc2::runtime::AnyObject = objc2::msg_send![configuration, setHeight: (meta.resolution.height.0 as i64)];
            let stream: *mut objc2::runtime::AnyObject = objc2::msg_send![stream, initWithFilter: filter, configuration: configuration, delegate: delegate];

            let (mut tx, rx) = oneshot::channel();

            let mut error: *mut objc2::runtime::AnyObject = std::ptr::null_mut();
            let _: () = objc2::msg_send![stream, addStreamOutput: output, type: SCStreamOutputTypeScreen, sampleHandlerQueue: 0, error: &mut error as *mut _];
            if !error.is_null() {
                let message: *mut objc2::runtime::AnyObject = objc2::msg_send![error, localizedDescription];
                tx.send(Err(anyhow!("failed to add stream  output {message:?}")))
                    .ok();
                return rx;
            }

            let tx = Rc::new(RefCell::new(Some(tx)));
            let handler = ConcreteBlock::new({
                move |error: *mut objc2::runtime::AnyObject| {
                    let result = if error.is_null() {
                        let stream = MacScreenCaptureStream {
                            meta: meta.clone(),
                            sc_stream: stream,
                            sc_stream_output: output,
                        };
                        Ok(Box::new(stream) as Box<dyn ScreenCaptureStream>)
                    } else {
                        let message: *mut objc2::runtime::AnyObject = objc2::msg_send![error, localizedDescription];
                        Err(anyhow!("failed to stop screen capture stream {message:?}"))
                    };
                    if let Some(tx) = tx.borrow_mut().take() {
                        tx.send(result).ok();
                    }
                }
            });
            let handler = handler.copy();
            let _: () = objc2::msg_send![stream, startCaptureWithCompletionHandler: handler];
            rx
        }
    }
}

impl Drop for MacScreenCaptureSource {
    fn drop(&mut self) {
        unsafe { let _: () = objc2::msg_send![self.sc_display, release]; }
    }
}

impl ScreenCaptureStream for MacScreenCaptureStream {
    fn metadata(&self) -> Result<SourceMetadata> {
        Ok(self.meta.clone())
    }
}

impl Drop for MacScreenCaptureStream {
    fn drop(&mut self) {
        unsafe {
            let mut error: *mut objc2::runtime::AnyObject = std::ptr::null_mut();
            let _: () = objc2::msg_send![self.sc_stream, removeStreamOutput: self.sc_stream_output, type: SCStreamOutputTypeScreen, error: &mut error as *mut _];
            if !error.is_null() {
                let message: *mut objc2::runtime::AnyObject = objc2::msg_send![error, localizedDescription];
                log::error!("failed to add stream  output {message:?}");
            }

            let handler = ConcreteBlock::new(move |error: *mut objc2::runtime::AnyObject| {
                if !error.is_null() {
                    let message: *mut objc2::runtime::AnyObject = objc2::msg_send![error, localizedDescription];
                    log::error!("failed to stop screen capture stream {message:?}");
                }
            });
            let block = handler.copy();
            let _: () = objc2::msg_send![self.sc_stream, stopCaptureWithCompletionHandler: block];
            let _: () = objc2::msg_send![self.sc_stream, release];
            let _: () = objc2::msg_send![self.sc_stream_output, release];
        }
    }
}

#[derive(Clone)]
struct ScreenMeta {
    label: SharedString,
    // Is this the screen with menu bar?
    is_main: bool,
}

unsafe fn screen_id_to_human_label() -> HashMap<CGDirectDisplayID, ScreenMeta> {
    let screens_id: *mut objc2::runtime::AnyObject = objc2::msg_send![objc2::class!(NSScreen), screens];
    let screens: &objc2_foundation::NSArray<objc2_app_kit::NSScreen> =
        unsafe { &*(screens_id as *mut objc2_foundation::NSArray<objc2_app_kit::NSScreen>) };
    let mut map = HashMap::default();
    let screen_number_key = objc2_foundation::NSString::from_str("NSScreenNumber");
    for i in 0..screens.len() {
        let sref = screens.objectAtIndex(i);
        let dict = sref.deviceDescription();
        let val = dict.objectForKey_unchecked(&screen_number_key);
        let Some(any) = val else { continue; };
        let any_ref: &objc2::runtime::AnyObject = any;
        let screen_id: u32 = objc2::msg_send![any_ref, unsignedIntValue];

        if let Some(name_ref) = sref.localizedName() {
            let rust_str = objc2::rc::autoreleasepool(|pool| unsafe { name_ref.to_str(pool).to_owned() });
            map.insert(
                screen_id,
                ScreenMeta {
                    label: rust_str.into(),
                    is_main: i == 0,
                },
            );
        }
    }
    map
}

pub(crate) fn get_sources() -> oneshot::Receiver<Result<Vec<Rc<dyn ScreenCaptureSource>>>> {
    unsafe {
        let (mut tx, rx) = oneshot::channel();
        let tx = Rc::new(RefCell::new(Some(tx)));
        let screen_id_to_label = screen_id_to_human_label();
        let block = ConcreteBlock::new(move |shareable_content: *mut objc2::runtime::AnyObject, error: *mut objc2::runtime::AnyObject| {
            let Some(mut tx) = tx.borrow_mut().take() else {
                return;
            };

            let result = if error.is_null() {
                let displays: *mut objc2::runtime::AnyObject = objc2::msg_send![shareable_content, displays];
                let mut result = Vec::new();
                for i in 0..displays.count() {
                    let display = displays.objectAtIndex(i);
                    let id: CGDirectDisplayID = objc2::msg_send![display, displayID];
                    let meta = screen_id_to_label.get(&id).cloned();
                    let source = MacScreenCaptureSource {
                        sc_display: objc2::msg_send![display, retain],
                        meta,
                    };
                    result.push(Rc::new(source) as Rc<dyn ScreenCaptureSource>);
                }
                Ok(result)
            } else {
                let msg: *mut objc2::runtime::AnyObject = objc2::msg_send![error, localizedDescription];
                let sref: &Objc2NSString = &*(msg as *mut Objc2NSString);
                let s = objc2::rc::autoreleasepool(|pool| unsafe { sref.to_str(pool).to_owned() });
                Err(anyhow!("Screen share failed: {}", s))
            };
            tx.send(result).ok();
        });
        let block = block.copy();

        let _: () = objc2::msg_send![
            objc2::class!(SCShareableContent),
            getShareableContentExcludingDesktopWindows: true,
                                   onScreenWindowsOnly: true,
                                     completionHandler: block
        ];
        rx
    }
}

#[ctor]
unsafe fn build_classes() {
    let mut decl = ClassDecl::new("GPUIStreamDelegate", class!(NSObject)).unwrap();
    unsafe {
        decl.add_method(
            sel!(outputVideoEffectDidStartForStream:),
            output_video_effect_did_start_for_stream as extern "C" fn(&Object, Sel, id),
        );
        decl.add_method(
            sel!(outputVideoEffectDidStopForStream:),
            output_video_effect_did_stop_for_stream as extern "C" fn(&Object, Sel, id),
        );
        decl.add_method(
            sel!(stream:didStopWithError:),
            stream_did_stop_with_error as extern "C" fn(&Object, Sel, id, id),
        );
        DELEGATE_CLASS = decl.register();

        let mut decl = ClassDecl::new("GPUIStreamOutput", class!(NSObject)).unwrap();
        decl.add_method(
            sel!(stream:didOutputSampleBuffer:ofType:),
            stream_did_output_sample_buffer_of_type
                as extern "C" fn(&Object, Sel, id, id, NSInteger),
        );
        decl.add_ivar::<*mut c_void>(FRAME_CALLBACK_IVAR);

        OUTPUT_CLASS = decl.register();
    }
}

extern "C" fn output_video_effect_did_start_for_stream(_this: &Object, _: Sel, _stream: id) {}

extern "C" fn output_video_effect_did_stop_for_stream(_this: &Object, _: Sel, _stream: id) {}

extern "C" fn stream_did_stop_with_error(_this: &Object, _: Sel, _stream: id, _error: id) {}

extern "C" fn stream_did_output_sample_buffer_of_type(
    this: &Object,
    _: Sel,
    _stream: id,
    sample_buffer: id,
    buffer_type: NSInteger,
) {
    if buffer_type != SCStreamOutputTypeScreen {
        return;
    }

    unsafe {
        let sample_buffer = sample_buffer as CMSampleBufferRef;
        let sample_buffer = CMSampleBuffer::wrap_under_get_rule(sample_buffer);
        if let Some(buffer) = sample_buffer.image_buffer() {
            let callback: Box<Box<dyn Fn(ScreenCaptureFrame)>> =
                Box::from_raw(*this.get_ivar::<*mut c_void>(FRAME_CALLBACK_IVAR) as *mut _);
            callback(ScreenCaptureFrame(buffer));
            mem::forget(callback);
        }
    }
}
