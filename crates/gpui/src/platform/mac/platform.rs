use super::{
    MacKeyboardLayout, MacKeyboardMapper,
    events::key_to_native,
    renderer,
};
use crate::{
    Action, AnyWindowHandle, BackgroundExecutor, ClipboardEntry, ClipboardItem, ClipboardString,
    CursorStyle, ForegroundExecutor, Image, ImageFormat, KeyContext, Keymap, MacDispatcher,
    MacDisplay, MacWindow, Menu, MenuItem, OsMenu, OwnedMenu, PathPromptOptions, Platform,
    PlatformDisplay, PlatformKeyboardLayout, PlatformKeyboardMapper, PlatformTextSystem,
    PlatformWindow, Result, SemanticVersion, SystemMenuType, Task, WindowAppearance, WindowParams,
    hash,
};
use anyhow::{Context as _, anyhow};
use block::ConcreteBlock;
use cocoa::{
    appkit::NSWindow,
    base::{BOOL, id, nil},
    foundation::NSInteger,
};
use objc2::rc::Retained;
use objc2::AnyThread;
use objc2_foundation::{ns_string, NSCopying};
use objc2_app_kit::{
    NSMenu as Objc2NSMenu, NSMenuItem as Objc2NSMenuItem, NSEventModifierFlags as Objc2NSEventModifierFlags,
    NSPasteboard as Objc2NSPasteboard,
    NSPasteboardTypeString as Objc2NSPasteboardTypeString,
    NSPasteboardTypePNG as Objc2NSPasteboardTypePNG,
    NSPasteboardTypeTIFF as Objc2NSPasteboardTypeTIFF,
    NSPasteboardTypeRTF as Objc2NSPasteboardTypeRTF,
    NSPasteboardTypeRTFD as Objc2NSPasteboardTypeRTFD,
    NSWorkspace, NSDocumentController,
};
use objc2::{MainThreadMarker, MainThreadOnly};
// Keep Cocoa's NSApplication trait/type in scope for existing calls elsewhere.
use cocoa::appkit::NSApplication;
use core_foundation::{
    base::{CFType, CFTypeRef, OSStatus, TCFType},
    boolean::CFBoolean,
    data::CFData,
    dictionary::{CFDictionary, CFDictionaryRef, CFMutableDictionary},
    runloop::CFRunLoopRun,
    string::{CFString, CFStringRef},
};
use ctor::ctor;
use futures::channel::oneshot;
use itertools::Itertools;
use objc::{
    class,
    msg_send,
    runtime::{Class, Object, Sel},
    sel, sel_impl,
};
use objc2::runtime::{AnyClass as Objc2AnyClass, AnyObject as Objc2AnyObject, ClassBuilder as Objc2ClassBuilder, Sel as Objc2Sel};
use parking_lot::Mutex;
use ptr::null_mut;
use std::{
    cell::{Cell, RefCell},
    convert::TryInto,
    ffi::{CStr, OsStr, c_void},
    os::{raw::c_char, unix::ffi::OsStrExt},
    path::{Path, PathBuf},
    process::Command,
    ptr,
    rc::Rc,
    str,
    sync::{Arc, OnceLock},
};
use strum::IntoEnumIterator;
use util::ResultExt;

// Removed: no longer needed after switching to typed NSString conversions

const MAC_PLATFORM_IVAR: &str = "platform";

#[ctor]
unsafe fn build_classes() {
    // Register GPUIApplicationDelegate using objc2
    let mut decl = Objc2ClassBuilder::new(
        CStr::from_bytes_with_nul(b"GPUIApplicationDelegate\0").unwrap(),
        objc2::class!(NSResponder),
    )
    .expect("failed to allocate GPUIApplicationDelegate class");
    decl.add_ivar::<*mut c_void>(CStr::from_bytes_with_nul(b"platform\0").unwrap());
    unsafe {
        decl.add_method(
            objc2::sel!(applicationWillFinishLaunching:),
            will_finish_launching as extern "C" fn(_, _, _),
        );
        decl.add_method(
            objc2::sel!(applicationDidFinishLaunching:),
            did_finish_launching as extern "C" fn(_, _, _),
        );
        decl.add_method(
            objc2::sel!(applicationShouldHandleReopen:hasVisibleWindows:),
            should_handle_reopen as extern "C" fn(_, _, _, _),
        );
        decl.add_method(
            objc2::sel!(applicationWillTerminate:),
            will_terminate as extern "C" fn(_, _, _),
        );
        decl.add_method(
            objc2::sel!(handleGPUIMenuItem:),
            handle_menu_item as extern "C" fn(_, _, _),
        );
        decl.add_method(
            objc2::sel!(cut:),
            handle_menu_item as extern "C" fn(_, _, _),
        );
        decl.add_method(
            objc2::sel!(copy:),
            handle_menu_item as extern "C" fn(_, _, _),
        );
        decl.add_method(
            objc2::sel!(paste:),
            handle_menu_item as extern "C" fn(_, _, _),
        );
        decl.add_method(
            objc2::sel!(selectAll:),
            handle_menu_item as extern "C" fn(_, _, _),
        );
        decl.add_method(
            objc2::sel!(undo:),
            handle_menu_item as extern "C" fn(_, _, _),
        );
        decl.add_method(
            objc2::sel!(redo:),
            handle_menu_item as extern "C" fn(_, _, _),
        );
        decl.add_method(
            objc2::sel!(validateMenuItem:),
            validate_menu_item as extern "C" fn(_, _, _) -> _,
        );
        decl.add_method(
            objc2::sel!(menuWillOpen:),
            menu_will_open as extern "C" fn(_, _, _),
        );
        decl.add_method(
            objc2::sel!(applicationDockMenu:),
            handle_dock_menu as extern "C" fn(_, _, _) -> _,
        );
        decl.add_method(
            objc2::sel!(application:openURLs:),
            open_urls as extern "C" fn(_, _, _, _),
        );
        decl.add_method(
            objc2::sel!(onKeyboardLayoutChange:),
            on_keyboard_layout_change as extern "C" fn(_, _, _),
        );
    }
    let _ = decl.register();
}

pub(crate) struct MacPlatform(Mutex<MacPlatformState>);

pub(crate) struct MacPlatformState {
    background_executor: BackgroundExecutor,
    foreground_executor: ForegroundExecutor,
    text_system: Arc<dyn PlatformTextSystem>,
    renderer_context: renderer::Context,
    headless: bool,
    pasteboard: Retained<Objc2NSPasteboard>,
    text_hash_pasteboard_type: Retained<objc2_foundation::NSString>,
    metadata_pasteboard_type: Retained<objc2_foundation::NSString>,
    reopen: Option<Box<dyn FnMut()>>,
    on_keyboard_layout_change: Option<Box<dyn FnMut()>>,
    quit: Option<Box<dyn FnMut()>>,
    menu_command: Option<Box<dyn FnMut(&dyn Action)>>,
    validate_menu_command: Option<Box<dyn FnMut(&dyn Action) -> bool>>,
    will_open_menu: Option<Box<dyn FnMut()>>,
    menu_actions: Vec<Box<dyn Action>>,
    open_urls: Option<Box<dyn FnMut(Vec<String>)>>,
    finish_launching: Option<Box<dyn FnOnce()>>,
    dock_menu: Option<Retained<Objc2NSMenu>>,
    menus: Option<Vec<OwnedMenu>>,
    keyboard_mapper: Rc<MacKeyboardMapper>,
}

impl Default for MacPlatform {
    fn default() -> Self {
        Self::new(false)
    }
}

impl MacPlatform {
    pub(crate) fn new(headless: bool) -> Self {
        let dispatcher = Arc::new(MacDispatcher::new());

        #[cfg(feature = "font-kit")]
        let text_system = Arc::new(crate::MacTextSystem::new());

        #[cfg(not(feature = "font-kit"))]
        let text_system = Arc::new(crate::NoopTextSystem::new());

        let keyboard_layout = MacKeyboardLayout::new();
        let keyboard_mapper = Rc::new(MacKeyboardMapper::new(keyboard_layout.id()));

        Self(Mutex::new(MacPlatformState {
            headless,
            text_system,
            background_executor: BackgroundExecutor::new(dispatcher.clone()),
            foreground_executor: ForegroundExecutor::new(dispatcher),
            renderer_context: renderer::Context::default(),
            pasteboard: Objc2NSPasteboard::generalPasteboard(),
            text_hash_pasteboard_type: objc2_foundation::NSString::from_str("zed-text-hash"),
            metadata_pasteboard_type: objc2_foundation::NSString::from_str("zed-metadata"),
            reopen: None,
            quit: None,
            menu_command: None,
            validate_menu_command: None,
            will_open_menu: None,
            menu_actions: Default::default(),
            open_urls: None,
            finish_launching: None,
            dock_menu: None,
            on_keyboard_layout_change: None,
            menus: None,
            keyboard_mapper,
        }))
    }

    fn read_from_pasteboard_typed(
        &self,
        pasteboard: &Objc2NSPasteboard,
        kind: &objc2_foundation::NSString,
    ) -> Option<Vec<u8>> {
        if let Some(data) = pasteboard.dataForType(kind) {
            let len = data.length();
            if len == 0 {
                return Some(Vec::new());
            }
            let mut buf = vec![0u8; len as usize];
            // SAFETY: `buf` is uniquely owned and non-null
            unsafe {
                objc2_foundation::NSData::getBytes_length(
                    &data,
                    std::ptr::NonNull::new_unchecked(buf.as_mut_ptr() as *mut _),
                    len,
                );
            }
            Some(buf)
        } else {
            None
        }
    }

    // Removed legacy Cocoa menu builders in favor of typed objc2 menu APIs

    unsafe fn create_menu_bar_typed(
        &self,
        menus: &Vec<Menu>,
        delegate: *mut Objc2AnyObject,
        actions: &mut Vec<Box<dyn Action>>,
        keymap: &Keymap,
    ) -> Retained<Objc2NSMenu> {
        let mtm = MainThreadMarker::new().expect("building menus must be on main thread");

        // Use the app delegate object as the NSMenuDelegate target
        let delegate_any: &Objc2AnyObject = unsafe { &*delegate };

        let application_menu = Objc2NSMenu::initWithTitle(Objc2NSMenu::alloc(mtm), ns_string!(""));
        unsafe { let _: () = objc2::msg_send![&*application_menu, setDelegate: delegate_any]; }

        // NSApplication (typed) for setting system menus
        let app = objc2_app_kit::NSApplication::sharedApplication(mtm);

        for menu_config in menus {
            let menu = Objc2NSMenu::initWithTitle(Objc2NSMenu::alloc(mtm), ns_string!(""));
            menu.setTitle(&objc2_foundation::NSString::from_str(&menu_config.name));
            unsafe { let _: () = objc2::msg_send![&*menu, setDelegate: delegate_any]; }

            for item_config in &menu_config.items {
                let item = Self::create_menu_item_typed(item_config, actions, keymap, mtm);
                menu.addItem(&item);
            }

            // Top-level item wrapping the submenu
            let item = unsafe {
                Objc2NSMenuItem::initWithTitle_action_keyEquivalent(
                    Objc2NSMenuItem::alloc(mtm),
                    &objc2_foundation::NSString::from_str(&menu_config.name),
                    None,
                    ns_string!(""),
                )
            };
            item.setSubmenu(Some(&menu));
            application_menu.addItem(&item);

            if menu_config.name == "Window" {
                app.setWindowsMenu(Some(&menu));
            }
        }

        application_menu
    }

    fn create_dock_menu_typed(
        &self,
        menu_items: Vec<MenuItem>,
        actions: &mut Vec<Box<dyn Action>>,
        keymap: &Keymap,
        mtm: MainThreadMarker,
    ) -> Retained<Objc2NSMenu> {
        let dock_menu = Objc2NSMenu::initWithTitle(Objc2NSMenu::alloc(mtm), ns_string!(""));
        for item_config in menu_items {
            let item = Self::create_menu_item_typed(&item_config, actions, keymap, mtm);
            dock_menu.addItem(&item);
        }
        dock_menu
    }

    // Removed legacy Cocoa menu item builder in favor of typed objc2 menu APIs

    fn create_menu_item_typed(
        item: &MenuItem,
        actions: &mut Vec<Box<dyn Action>>,
        keymap: &Keymap,
        mtm: MainThreadMarker,
    ) -> Retained<Objc2NSMenuItem> {
        match item {
            MenuItem::Separator => Objc2NSMenuItem::separatorItem(mtm),
            MenuItem::Action { name, action, os_action } => {
                // Find keystrokes as before
                let keystrokes = keymap
                    .bindings_for_action(action.as_ref())
                    .find_or_first(|binding| {
                        binding.predicate().is_none_or(|predicate| {
                            static DEFAULT_CONTEXT: OnceLock<Vec<KeyContext>> = OnceLock::new();
                            predicate.eval(DEFAULT_CONTEXT.get_or_init(|| {
                                let mut workspace_context = KeyContext::new_with_defaults();
                                workspace_context.add("Workspace");
                                let mut pane_context = KeyContext::new_with_defaults();
                                pane_context.add("Pane");
                                let mut editor_context = KeyContext::new_with_defaults();
                                editor_context.add("Editor");
                                pane_context.extend(&editor_context);
                                workspace_context.extend(&pane_context);
                                vec![workspace_context]
                            }))
                        })
                    })
                    .map(|binding| binding.keystrokes());

                let sel = match os_action {
                    Some(crate::OsAction::Cut) => Some(objc2::sel!(cut:)),
                    Some(crate::OsAction::Copy) => Some(objc2::sel!(copy:)),
                    Some(crate::OsAction::Paste) => Some(objc2::sel!(paste:)),
                    Some(crate::OsAction::SelectAll) => Some(objc2::sel!(selectAll:)),
                    Some(crate::OsAction::Undo) => Some(objc2::sel!(handleGPUIMenuItem:)),
                    Some(crate::OsAction::Redo) => Some(objc2::sel!(handleGPUIMenuItem:)),
                    None => Some(objc2::sel!(handleGPUIMenuItem:)),
                };

                let item = if let Some(keystrokes) = keystrokes {
                    if keystrokes.len() == 1 {
                        let keystroke = &keystrokes[0];
                        // Build modifier mask using typed flags
                        let mut mask = Objc2NSEventModifierFlags::empty();
                        if keystroke.modifiers().platform {
                            mask.insert(Objc2NSEventModifierFlags::Command);
                        }
                        if keystroke.modifiers().control {
                            mask.insert(Objc2NSEventModifierFlags::Control);
                        }
                        if keystroke.modifiers().alt {
                            mask.insert(Objc2NSEventModifierFlags::Option);
                        }
                        if keystroke.modifiers().shift {
                            mask.insert(Objc2NSEventModifierFlags::Shift);
                        }

                        let item = unsafe {
                            Objc2NSMenuItem::initWithTitle_action_keyEquivalent(
                                Objc2NSMenuItem::alloc(mtm),
                                &objc2_foundation::NSString::from_str(name),
                                sel,
                                &objc2_foundation::NSString::from_str(key_to_native(keystroke.key()).as_ref()),
                            )
                        };
                        if Self::os_version() >= SemanticVersion::new(12, 0, 0) {
                            item.setAllowsAutomaticKeyEquivalentLocalization(false);
                        }
                        item.setKeyEquivalentModifierMask(mask);
                        item
                    } else {
                        unsafe {
                            Objc2NSMenuItem::initWithTitle_action_keyEquivalent(
                                Objc2NSMenuItem::alloc(mtm),
                                &objc2_foundation::NSString::from_str(name),
                                sel,
                                ns_string!(""),
                            )
                        }
                    }
                } else {
                    unsafe {
                        Objc2NSMenuItem::initWithTitle_action_keyEquivalent(
                            Objc2NSMenuItem::alloc(mtm),
                            &objc2_foundation::NSString::from_str(name),
                            sel,
                            ns_string!(""),
                        )
                    }
                };

                let tag = actions.len() as usize as objc2_foundation::NSInteger;
                item.setTag(tag);
                actions.push(action.boxed_clone());
                item
            }
            MenuItem::Submenu(Menu { name, items }) => {
                let submenu = Objc2NSMenu::initWithTitle(
                    Objc2NSMenu::alloc(mtm),
                    &objc2_foundation::NSString::from_str(name),
                );
                for subitem in items {
                    let item = Self::create_menu_item_typed(subitem, actions, keymap, mtm);
                    submenu.addItem(&item);
                }
                let item = unsafe {
                    Objc2NSMenuItem::initWithTitle_action_keyEquivalent(
                        Objc2NSMenuItem::alloc(mtm),
                        &objc2_foundation::NSString::from_str(name),
                        None,
                        ns_string!(""),
                    )
                };
                item.setSubmenu(Some(&submenu));
                item
            }
            MenuItem::SystemMenu(OsMenu { name, menu_type }) => {
                let submenu = Objc2NSMenu::initWithTitle(
                    Objc2NSMenu::alloc(mtm),
                    &objc2_foundation::NSString::from_str(name),
                );
                let item = unsafe {
                    Objc2NSMenuItem::initWithTitle_action_keyEquivalent(
                        Objc2NSMenuItem::alloc(mtm),
                        &objc2_foundation::NSString::from_str(name),
                        None,
                        ns_string!(""),
                    )
                };
                item.setSubmenu(Some(&submenu));

                // Set services menu on NSApplication when requested
                if matches!(menu_type, SystemMenuType::Services) {
                    let app = objc2_app_kit::NSApplication::sharedApplication(mtm);
                    app.setServicesMenu(Some(&submenu));
                }

                item
            }
        }
    }

    fn os_version() -> SemanticVersion {
        let pi = objc2_foundation::NSProcessInfo::processInfo();
        let version: objc2_foundation::NSOperatingSystemVersion = unsafe { objc2::msg_send![&*pi, operatingSystemVersion] };
        SemanticVersion::new(
            version.majorVersion as usize,
            version.minorVersion as usize,
            version.patchVersion as usize,
        )
    }
}

impl Platform for MacPlatform {
    fn background_executor(&self) -> BackgroundExecutor {
        self.0.lock().background_executor.clone()
    }

    fn foreground_executor(&self) -> crate::ForegroundExecutor {
        self.0.lock().foreground_executor.clone()
    }

    fn text_system(&self) -> Arc<dyn PlatformTextSystem> {
        self.0.lock().text_system.clone()
    }

    fn run(&self, on_finish_launching: Box<dyn FnOnce()>) {
        let mut state = self.0.lock();
        if state.headless {
            drop(state);
            on_finish_launching();
            unsafe { CFRunLoopRun() };
        } else {
            state.finish_launching = Some(on_finish_launching);
            drop(state);
        }

        unsafe {
            let mtm = objc2::MainThreadMarker::new().expect("must run on main thread");
            let app = objc2_app_kit::NSApplication::sharedApplication(mtm);

            // Allocate delegate from registered class
            let delegate_cls: &Objc2AnyClass = Objc2AnyClass::get(CStr::from_bytes_with_nul(b"GPUIApplicationDelegate\0").unwrap())
                .expect("delegate class not registered");
            let app_delegate: *mut Objc2AnyObject = objc2::msg_send![delegate_cls, new];
            // Set delegate using untyped messaging
            let delegate_any: &Objc2AnyObject = unsafe { &*app_delegate };
            let _: () = objc2::msg_send![&*app, setDelegate: delegate_any];

            // Store platform pointer in delegate ivar
            let self_ptr = self as *const Self as *const c_void;
            let ivar_name = CStr::from_bytes_with_nul(b"platform\0").unwrap();
            let ivar = delegate_any.class().instance_variable(ivar_name).expect("platform ivar not found");
            let delegate_ref: &mut Objc2AnyObject = &mut *app_delegate;
            unsafe { *ivar.load_mut::<*const c_void>(delegate_ref) = self_ptr; }

            objc2::rc::autoreleasepool(|_| {
                app.run();
            });

            // Clear the ivar on the delegate if present
            let current_delegate: *mut Objc2AnyObject = objc2::msg_send![&*app, delegate];
            if !current_delegate.is_null() {
                let ivar_name = CStr::from_bytes_with_nul(b"platform\0").unwrap();
                let ivar = (&*current_delegate).class().instance_variable(ivar_name).expect("platform ivar not found");
                unsafe { *ivar.load_mut::<*const c_void>(&mut *current_delegate) = ptr::null() };
            }
        }
    }

    fn quit(&self) {
        // Quitting the app causes us to close windows, which invokes `Window::on_close` callbacks
        // synchronously before this method terminates. If we call `Platform::quit` while holding a
        // borrow of the app state (which most of the time we will do), we will end up
        // double-borrowing the app state in the `on_close` callbacks for our open windows. To solve
        // this, we make quitting the application asynchronous so that we aren't holding borrows to
        // the app state on the stack when we actually terminate the app.

        use super::dispatcher::{dispatch_get_main_queue, dispatch_sys::dispatch_async_f};

        unsafe {
            dispatch_async_f(dispatch_get_main_queue(), ptr::null_mut(), Some(quit));
        }

        unsafe extern "C" fn quit(_: *mut c_void) {
            unsafe {
                let mtm = objc2::MainThreadMarker::new()
                    .expect("terminate must be called on main thread");
                let app = objc2_app_kit::NSApplication::sharedApplication(mtm);
                let none: Option<&objc2::runtime::AnyObject> = None;
                let _: () = objc2::msg_send![&*app, terminate: none];
            }
        }
    }

    fn restart(&self, _binary_path: Option<PathBuf>) {
        use std::os::unix::process::CommandExt as _;

        let app_pid = std::process::id().to_string();
        let app_path = self
            .app_path()
            .ok()
            // When the app is not bundled, `app_path` returns the
            // directory containing the executable. Disregard this
            // and get the path to the executable itself.
            .and_then(|path| (path.extension()?.to_str()? == "app").then_some(path))
            .unwrap_or_else(|| std::env::current_exe().unwrap());

        // Wait until this process has exited and then re-open this path.
        let script = r#"
            while kill -0 $0 2> /dev/null; do
                sleep 0.1
            done
            open "$1"
        "#;

        #[allow(
            clippy::disallowed_methods,
            reason = "We are restarting ourselves, using std command thus is fine"
        )]
        let restart_process = Command::new("/bin/bash")
            .arg("-c")
            .arg(script)
            .arg(app_pid)
            .arg(app_path)
            .process_group(0)
            .spawn();

        match restart_process {
            Ok(_) => self.quit(),
            Err(e) => log::error!("failed to spawn restart script: {:?}", e),
        }
    }

    fn activate(&self, ignoring_other_apps: bool) {
        let mtm = MainThreadMarker::new().expect("activate must be on main thread");
        let app = objc2_app_kit::NSApplication::sharedApplication(mtm);
        #[allow(deprecated)]
        app.activateIgnoringOtherApps(ignoring_other_apps);
    }

    fn hide(&self) {
        unsafe {
            let mtm = MainThreadMarker::new().expect("hide must be on main thread");
            let app = objc2_app_kit::NSApplication::sharedApplication(mtm);
            let none: Option<&objc2::runtime::AnyObject> = None;
            let _: () = objc2::msg_send![&*app, hide: none];
        }
    }

    fn hide_other_apps(&self) {
        unsafe {
            let mtm = MainThreadMarker::new().expect("hideOtherApplications must be on main thread");
            let app = objc2_app_kit::NSApplication::sharedApplication(mtm);
            let none: Option<&objc2::runtime::AnyObject> = None;
            let _: () = objc2::msg_send![&*app, hideOtherApplications: none];
        }
    }

    fn unhide_other_apps(&self) {
        unsafe {
            let mtm = MainThreadMarker::new().expect("unhideAllApplications must be on main thread");
            let app = objc2_app_kit::NSApplication::sharedApplication(mtm);
            let none: Option<&objc2::runtime::AnyObject> = None;
            let _: () = objc2::msg_send![&*app, unhideAllApplications: none];
        }
    }

    fn primary_display(&self) -> Option<Rc<dyn PlatformDisplay>> {
        Some(Rc::new(MacDisplay::primary()))
    }

    fn displays(&self) -> Vec<Rc<dyn PlatformDisplay>> {
        MacDisplay::all()
            .map(|screen| Rc::new(screen) as Rc<_>)
            .collect()
    }

    #[cfg(feature = "screen-capture")]
    fn is_screen_capture_supported(&self) -> bool {
        let min_version = cocoa::foundation::NSOperatingSystemVersion::new(12, 3, 0);
        super::is_macos_version_at_least(min_version)
    }

    #[cfg(feature = "screen-capture")]
    fn screen_capture_sources(
        &self,
    ) -> oneshot::Receiver<Result<Vec<Rc<dyn crate::ScreenCaptureSource>>>> {
        super::screen_capture::get_sources()
    }

    fn active_window(&self) -> Option<AnyWindowHandle> {
        MacWindow::active_window()
    }

    // Returns the windows ordered front-to-back, meaning that the active
    // window is the first one in the returned vec.
    fn window_stack(&self) -> Option<Vec<AnyWindowHandle>> {
        Some(MacWindow::ordered_windows())
    }

    fn open_window(
        &self,
        handle: AnyWindowHandle,
        options: WindowParams,
    ) -> Result<Box<dyn PlatformWindow>> {
        let renderer_context = self.0.lock().renderer_context.clone();
        Ok(Box::new(MacWindow::open(
            handle,
            options,
            self.foreground_executor(),
            renderer_context,
        )))
    }

    fn window_appearance(&self) -> WindowAppearance {
        unsafe {
            let mtm = MainThreadMarker::new().expect("NSApplication access must be on main thread");
            let app = objc2_app_kit::NSApplication::sharedApplication(mtm);
            let appearance: *mut Objc2AnyObject = objc2::msg_send![&*app, effectiveAppearance];
            WindowAppearance::from_native(appearance)
        }
    }

    fn open_url(&self, url: &str) {
        let ws = NSWorkspace::sharedWorkspace();
        let s = objc2_foundation::NSString::from_str(url);
        if let Some(nsurl) = unsafe { objc2_foundation::NSURL::URLWithString(&s) } {
            let _: bool = unsafe { objc2::msg_send![&*ws, openURL: &*nsurl] };
        }
    }

    fn register_url_scheme(&self, scheme: &str) -> Task<anyhow::Result<()>> {
        // API only available post Monterey
        // https://developer.apple.com/documentation/appkit/nsworkspace/3753004-setdefaultapplicationaturl
        let (done_tx, done_rx) = oneshot::channel();
        if Self::os_version() < SemanticVersion::new(12, 0, 0) {
            return Task::ready(Err(anyhow!(
                "macOS 12.0 or later is required to register URL schemes"
            )));
        }

        let bundle_id = unsafe {
            let bundle = objc2_foundation::NSBundle::mainBundle();
            let bundle_id: *mut objc2::runtime::AnyObject = objc2::msg_send![&*bundle, bundleIdentifier];
            if bundle_id.is_null() {
                return Task::ready(Err(anyhow!("Can only register URL scheme in bundled apps")));
            }
            bundle_id
        };

        unsafe {
            let ws: id = msg_send![class!(NSWorkspace), sharedWorkspace];
            let scheme = objc2_foundation::NSString::from_str(scheme);
            let app: id = msg_send![ws, URLForApplicationWithBundleIdentifier: bundle_id];
            if app == nil {
                return Task::ready(Err(anyhow!(
                    "Cannot register URL scheme until app is installed"
                )));
            }
            let done_tx = Cell::new(Some(done_tx));
            let block = ConcreteBlock::new(move |error: id| {
                let result = if error == nil {
                    Ok(())
                } else {
                    let msg: id = msg_send![error, localizedDescription];
                    Err(anyhow!("Failed to register: {msg:?}"))
                };

                if let Some(done_tx) = done_tx.take() {
                    let _ = done_tx.send(result);
                }
            });
            let block = block.copy();
            let scheme_ref: &objc2_foundation::NSString = &*scheme;
            let _: () = msg_send![ws, setDefaultApplicationAtURL: app toOpenURLsWithScheme: scheme_ref completionHandler: block];
        }

        self.background_executor()
            .spawn(async { crate::Flatten::flatten(done_rx.await.map_err(|e| anyhow!(e))) })
    }

    fn on_open_urls(&self, callback: Box<dyn FnMut(Vec<String>)>) {
        self.0.lock().open_urls = Some(callback);
    }

    fn prompt_for_paths(
        &self,
        options: PathPromptOptions,
    ) -> oneshot::Receiver<Result<Option<Vec<PathBuf>>>> {
        let (done_tx, done_rx) = oneshot::channel();
        self.foreground_executor()
            .spawn(async move {
                let mtm = MainThreadMarker::new().expect("NSOpenPanel on main thread");
                let panel = objc2_app_kit::NSOpenPanel::openPanel(mtm);
                // Configure panel
                panel.setCanChooseDirectories(options.directories);
                panel.setCanChooseFiles(options.files);
                panel.setAllowsMultipleSelection(options.multiple);
                unsafe {
                    let _: () = objc2::msg_send![&*panel, setCanCreateDirectories: true];
                    let _: () = objc2::msg_send![&*panel, setResolvesAliases: false];
                }

                let done_tx = Rc::new(RefCell::new(Some(done_tx)));
                let panel_for_block = panel.clone();
                let block = block2::StackBlock::new(move |response: objc2_app_kit::NSModalResponse| {
                    let result = if response == objc2_app_kit::NSModalResponseOK {
                        let mut out = Vec::new();
                        let urls = panel_for_block.URLs();
                        for i in 0..urls.len() {
                            let url = urls.objectAtIndex(i as objc2_foundation::NSUInteger);
                            if url.isFileURL() {
                                if let Ok(path) = objc_url_to_path(&url) {
                                    out.push(path);
                                }
                            }
                        }
                        Some(out)
                    } else {
                        None
                    };

                    if let Some(done_tx) = done_tx.borrow_mut().take() {
                        let _ = done_tx.send(Ok(result));
                    }
                })
                .copy();

                if let Some(prompt) = options.prompt {
                    let s = objc2_foundation::NSString::from_str(&prompt);
                    panel.setPrompt(Some(&s));
                }

                panel.beginWithCompletionHandler(&block);
            })
            .detach();
        done_rx
    }

    fn prompt_for_new_path(
        &self,
        directory: &Path,
        suggested_name: Option<&str>,
    ) -> oneshot::Receiver<Result<Option<PathBuf>>> {
        let directory = directory.to_owned();
        let suggested_name = suggested_name.map(|s| s.to_owned());
        let (done_tx, done_rx) = oneshot::channel();
        self.foreground_executor()
            .spawn(async move {
                let mtm = MainThreadMarker::new().expect("NSSavePanel on main thread");
                let panel = objc2_app_kit::NSSavePanel::savePanel(mtm);
                let url = objc2_foundation::NSURL::fileURLWithPath_isDirectory(
                    &objc2_foundation::NSString::from_str(directory.to_string_lossy().as_ref()),
                    true,
                );
                panel.setDirectoryURL(Some(&url));

                if let Some(suggested_name) = suggested_name {
                    panel.setNameFieldStringValue(&objc2_foundation::NSString::from_str(&suggested_name));
                }

                let done_tx = Rc::new(RefCell::new(Some(done_tx)));
                let panel_for_block = panel.clone();
                let block = block2::StackBlock::new(move |response: objc2_app_kit::NSModalResponse| {
                    let mut result = None;
                    if response == objc2_app_kit::NSModalResponseOK {
                        if let Some(url) = panel_for_block.URL() {
                            if url.isFileURL() {
                                result = objc_url_to_path(&url).ok().map(|mut result| {
                                    let Some(filename) = result.file_name() else {
                                        return result;
                                    };
                                    let chunks = filename
                                        .as_bytes()
                                        .split(|&b| b == b'.')
                                        .collect::<Vec<_>>();

                                    // https://github.com/zed-industries/zed/issues/16969
                                    // Workaround a bug in macOS Sequoia that adds an extra file-extension
                                    // sometimes. e.g. `a.sql` becomes `a.sql.s` or `a.txtx` becomes `a.txtx.txt`
                                    //
                                    // This is conditional on OS version because I'd like to get rid of it, so that
                                    // you can manually create a file called `a.sql.s`. That said it seems better
                                    // to break that use-case than breaking `a.sql`.
                                    if chunks.len() == 3
                                        && chunks[1].starts_with(chunks[2])
                                        && Self::os_version() >= SemanticVersion::new(15, 0, 0)
                                    {
                                        let new_filename = OsStr::from_bytes(
                                            &filename.as_bytes()
                                                [..chunks[0].len() + 1 + chunks[1].len()],
                                        )
                                        .to_owned();
                                        result.set_file_name(&new_filename);
                                    }
                                    result
                                })
                            }
                        }
                    }

                    if let Some(done_tx) = done_tx.borrow_mut().take() {
                        let _ = done_tx.send(Ok(result));
                    }
                })
                .copy();
                panel.beginWithCompletionHandler(&block);
            })
            .detach();

        done_rx
    }

    fn can_select_mixed_files_and_dirs(&self) -> bool {
        true
    }

    fn reveal_path(&self, path: &Path) {
        let path = path.to_path_buf();
        self.0
            .lock()
            .background_executor
            .spawn(async move {
                let ws = NSWorkspace::sharedWorkspace();
                let full_path = objc2_foundation::NSString::from_str(path.to_str().unwrap_or(""));
                let root = objc2_foundation::NSString::from_str("");
                let full_ref: &objc2_foundation::NSString = &*full_path;
                let root_ref: &objc2_foundation::NSString = &*root;
                let _: BOOL = unsafe { objc2::msg_send![&*ws, selectFile: full_ref, inFileViewerRootedAtPath: root_ref] };
            })
            .detach();
    }

    fn open_with_system(&self, path: &Path) {
        let path = path.to_owned();
        self.0
            .lock()
            .background_executor
            .spawn(async move {
                if let Some(mut child) = smol::process::Command::new("open")
                    .arg(path)
                    .spawn()
                    .context("invoking open command")
                    .log_err()
                {
                    child.status().await.log_err();
                }
            })
            .detach();
    }

    fn on_quit(&self, callback: Box<dyn FnMut()>) {
        self.0.lock().quit = Some(callback);
    }

    fn on_reopen(&self, callback: Box<dyn FnMut()>) {
        self.0.lock().reopen = Some(callback);
    }

    fn on_keyboard_layout_change(&self, callback: Box<dyn FnMut()>) {
        self.0.lock().on_keyboard_layout_change = Some(callback);
    }

    fn on_app_menu_action(&self, callback: Box<dyn FnMut(&dyn Action)>) {
        self.0.lock().menu_command = Some(callback);
    }

    fn on_will_open_app_menu(&self, callback: Box<dyn FnMut()>) {
        self.0.lock().will_open_menu = Some(callback);
    }

    fn on_validate_app_menu_command(&self, callback: Box<dyn FnMut(&dyn Action) -> bool>) {
        self.0.lock().validate_menu_command = Some(callback);
    }

    fn keyboard_layout(&self) -> Box<dyn PlatformKeyboardLayout> {
        Box::new(MacKeyboardLayout::new())
    }

    fn keyboard_mapper(&self) -> Rc<dyn PlatformKeyboardMapper> {
        self.0.lock().keyboard_mapper.clone()
    }

    fn app_path(&self) -> Result<PathBuf> {
        unsafe {
            let bundle = objc2_foundation::NSBundle::mainBundle();
            let bobj: &objc::runtime::Object =
                &*((&*bundle as *const _) as *mut objc::runtime::Object);
            Ok(path_from_objc(msg_send![bobj, bundlePath]))
        }
    }

    fn set_menus(&self, menus: Vec<Menu>, keymap: &Keymap) {
        let mtm = MainThreadMarker::new().expect("menus must be set on main thread");
        // Get app delegate id via Cocoa to reuse as NSMenuDelegate
        let delegate_id: *mut Objc2AnyObject = unsafe {
            let mtm = MainThreadMarker::new().expect("menus must be set on main thread");
            let app = objc2_app_kit::NSApplication::sharedApplication(mtm);
            objc2::msg_send![&*app, delegate]
        };
        let mut state = self.0.lock();
        let actions = &mut state.menu_actions;
        let application_menu = unsafe { self.create_menu_bar_typed(&menus, delegate_id, actions, keymap) };
        drop(state);

        let app = objc2_app_kit::NSApplication::sharedApplication(mtm);
        app.setMainMenu(Some(&application_menu));

        self.0.lock().menus = Some(menus.into_iter().map(|menu| menu.owned()).collect());
    }

    fn get_menus(&self) -> Option<Vec<OwnedMenu>> {
        self.0.lock().menus.clone()
    }

    fn set_dock_menu(&self, menu: Vec<MenuItem>, keymap: &Keymap) {
        let mtm = MainThreadMarker::new().expect("dock menu must be set on main thread");
        let mut state = self.0.lock();
        let actions = &mut state.menu_actions;
        let new = self.create_dock_menu_typed(menu, actions, keymap, mtm);
        state.dock_menu = Some(new);
    }

    fn add_recent_document(&self, path: &Path) {
        if let Some(path_str) = path.to_str() {
            let path = std::path::Path::new(path_str);
            if let Some(url) = objc2_foundation::NSURL::from_file_path(path) {
                let mtm = MainThreadMarker::new().expect("NSDocumentController on main thread");
                let dc = NSDocumentController::sharedDocumentController(mtm);
                unsafe { let _: () = objc2::msg_send![&*dc, noteNewRecentDocumentURL: &*url]; }
            }
        }
    }

    fn path_for_auxiliary_executable(&self, name: &str) -> Result<PathBuf> {
        unsafe {
            let bundle = objc2_foundation::NSBundle::mainBundle();
            let name = objc2_foundation::NSString::from_str(name);
            let bobj: &objc::runtime::Object =
                &*((&*bundle as *const _) as *mut objc::runtime::Object);
            let url: id = msg_send![bobj, URLForAuxiliaryExecutable: &*name];
            anyhow::ensure!(!url.is_null(), "resource not found");
            ns_url_to_path(url)
        }
    }

    /// Match cursor style to one of the styles available
    /// in macOS's [NSCursor](https://developer.apple.com/documentation/appkit/nscursor).
    fn set_cursor_style(&self, style: CursorStyle) {
        unsafe {
            if style == CursorStyle::None {
                let _: () = objc2::msg_send![objc2::class!(NSCursor), setHiddenUntilMouseMoves: true];
                return;
            }

            let new_cursor: *mut objc2::runtime::AnyObject = match style {
                CursorStyle::Arrow => objc2::msg_send![objc2::class!(NSCursor), arrowCursor],
                CursorStyle::IBeam => objc2::msg_send![objc2::class!(NSCursor), IBeamCursor],
                CursorStyle::Crosshair => objc2::msg_send![objc2::class!(NSCursor), crosshairCursor],
                CursorStyle::ClosedHand => objc2::msg_send![objc2::class!(NSCursor), closedHandCursor],
                CursorStyle::OpenHand => objc2::msg_send![objc2::class!(NSCursor), openHandCursor],
                CursorStyle::PointingHand => objc2::msg_send![objc2::class!(NSCursor), pointingHandCursor],
                CursorStyle::ResizeLeftRight => objc2::msg_send![objc2::class!(NSCursor), resizeLeftRightCursor],
                CursorStyle::ResizeUpDown => objc2::msg_send![objc2::class!(NSCursor), resizeUpDownCursor],
                CursorStyle::ResizeLeft => objc2::msg_send![objc2::class!(NSCursor), resizeLeftCursor],
                CursorStyle::ResizeRight => objc2::msg_send![objc2::class!(NSCursor), resizeRightCursor],
                CursorStyle::ResizeColumn => objc2::msg_send![objc2::class!(NSCursor), resizeLeftRightCursor],
                CursorStyle::ResizeRow => objc2::msg_send![objc2::class!(NSCursor), resizeUpDownCursor],
                CursorStyle::ResizeUp => objc2::msg_send![objc2::class!(NSCursor), resizeUpCursor],
                CursorStyle::ResizeDown => objc2::msg_send![objc2::class!(NSCursor), resizeDownCursor],

                // Undocumented, private class methods:
                // https://stackoverflow.com/questions/27242353/cocoa-predefined-resize-mouse-cursor
                CursorStyle::ResizeUpLeftDownRight => {
                    objc2::msg_send![objc2::class!(NSCursor), _windowResizeNorthWestSouthEastCursor]
                }
                CursorStyle::ResizeUpRightDownLeft => {
                    objc2::msg_send![objc2::class!(NSCursor), _windowResizeNorthEastSouthWestCursor]
                }

                CursorStyle::IBeamCursorForVerticalLayout => {
                    objc2::msg_send![objc2::class!(NSCursor), IBeamCursorForVerticalLayout]
                }
                CursorStyle::OperationNotAllowed => {
                    objc2::msg_send![objc2::class!(NSCursor), operationNotAllowedCursor]
                }
                CursorStyle::DragLink => objc2::msg_send![objc2::class!(NSCursor), dragLinkCursor],
                CursorStyle::DragCopy => objc2::msg_send![objc2::class!(NSCursor), dragCopyCursor],
                CursorStyle::ContextualMenu => objc2::msg_send![objc2::class!(NSCursor), contextualMenuCursor],
                CursorStyle::None => unreachable!(),
            };

            // Set cursor using typed NSCursor API
            let cursor_ref: &objc2_app_kit::NSCursor = &*(new_cursor as *mut objc2_app_kit::NSCursor);
            cursor_ref.set();
        }
    }

    fn should_auto_hide_scrollbars(&self) -> bool {
        #[allow(non_upper_case_globals)]
        const NSScrollerStyleOverlay: NSInteger = 1;

        unsafe {
            let style: NSInteger = objc2::msg_send![objc2::class!(NSScroller), preferredScrollerStyle];
            style == NSScrollerStyleOverlay
        }
    }

    fn write_to_clipboard(&self, item: ClipboardItem) {
        use crate::ClipboardEntry;

        unsafe {
            // We only want to use NSAttributedString if there are multiple entries to write.
            if item.entries.len() <= 1 {
                match item.entries.first() {
                    Some(entry) => match entry {
                        ClipboardEntry::String(string) => {
                            self.write_plaintext_to_clipboard(string);
                        }
                        ClipboardEntry::Image(image) => {
                            self.write_image_to_clipboard(image);
                        }
                    },
                    None => {
                        // Writing an empty list of entries just clears the clipboard.
                        let state = self.0.lock();
                        state.pasteboard.clearContents();
                    }
                }
            } else {
                let mut any_images = false;
                let attributed_string = {
                    let mut buf = objc2_foundation::NSMutableAttributedString::new();
                    for entry in item.entries {
                        if let ClipboardEntry::String(ClipboardString { text, metadata: _ }) = entry {
                            let ns = objc2_foundation::NSString::from_str(&text);
                            let to_append = objc2_foundation::NSAttributedString::initWithString(
                                objc2_foundation::NSAttributedString::alloc(),
                                &ns,
                            );
                            buf.appendAttributedString(&to_append);
                        }
                    }
                    // Return immutable copy for further operations
                    buf.copy()
                };

                let state = self.0.lock();
                state.pasteboard.clearContents();

                // Only set rich text clipboard types if we actually have 1+ images to include.
                if any_images {
                    let dict_empty: objc2::rc::Retained<
                        objc2_foundation::NSDictionary<
                            objc2::runtime::AnyObject,
                            objc2::runtime::AnyObject,
                        >,
                    > = objc2_foundation::NSDictionary::init(
                        objc2_foundation::NSDictionary::alloc(),
                    );
                    let _dict: &objc2_foundation::NSDictionary<
                        objc2_app_kit::NSAttributedStringDocumentAttributeKey,
                        objc2::runtime::AnyObject,
                    > = unsafe { dict_empty.cast_unchecked() };
                    let range = objc2_foundation::NSRange::new(0, attributed_string.length());

                    if let Some(rtfd_data) = unsafe {
                        let data: Option<objc2::rc::Retained<objc2_foundation::NSData>> = objc2::msg_send![
                            &*attributed_string,
                            RTFDFromRange: range,
                            documentAttributes: &*dict_empty
                        ];
                        data
                    } {
                        state
                            .pasteboard
                            .setData_forType(Some(&rtfd_data), Objc2NSPasteboardTypeRTFD);
                    }

                    if let Some(rtf_data) = unsafe {
                        let data: Option<objc2::rc::Retained<objc2_foundation::NSData>> = objc2::msg_send![
                            &*attributed_string,
                            RTFFromRange: range,
                            documentAttributes: &*dict_empty
                        ];
                        data
                    } {
                        state
                            .pasteboard
                            .setData_forType(Some(&rtf_data), Objc2NSPasteboardTypeRTF);
                    }
                }

                let s_ref = attributed_string.string();
                state
                    .pasteboard
                    .setString_forType(&s_ref, Objc2NSPasteboardTypeString);
            }
        }
    }

    fn read_from_clipboard(&self) -> Option<ClipboardItem> {
        let state = self.0.lock();
        let pasteboard = &*state.pasteboard;

        // First, see if it's a string.
        unsafe {
            if let Some(types) = pasteboard.types() {
                if types.containsObject(Objc2NSPasteboardTypeString) {
                    if let Some(data) = pasteboard.dataForType(Objc2NSPasteboardTypeString) {
                        let len = data.length();
                        if len == 0 {
                            return Some(self.read_string_from_clipboard(&state, &[]));
                        }
                        let mut buf = vec![0u8; len as usize];
                        objc2_foundation::NSData::getBytes_length(
                            &data,
                            std::ptr::NonNull::new_unchecked(buf.as_mut_ptr() as *mut _),
                            len,
                        );
                        return Some(self.read_string_from_clipboard(&state, &buf));
                    } else {
                        return None;
                    }
                }
            }

            // If it wasn't a string, try the various supported image types.
            for format in ImageFormat::iter() {
                if let Some(item) = try_clipboard_image(pasteboard, format) {
                    return Some(item);
                }
            }
        }

        // If it wasn't a string or a supported image type, give up.
        None
    }

    fn write_credentials(&self, url: &str, username: &str, password: &[u8]) -> Task<Result<()>> {
        let url = url.to_string();
        let username = username.to_string();
        let password = password.to_vec();
        self.background_executor().spawn(async move {
            unsafe {
                use security::*;

                let url = CFString::from(url.as_str());
                let username = CFString::from(username.as_str());
                let password = CFData::from_buffer(&password);

                // First, check if there are already credentials for the given server. If so, then
                // update the username and password.
                let mut verb = "updating";
                let mut query_attrs = CFMutableDictionary::with_capacity(2);
                query_attrs.set(kSecClass as *const _, kSecClassInternetPassword as *const _);
                query_attrs.set(kSecAttrServer as *const _, url.as_CFTypeRef());

                let mut attrs = CFMutableDictionary::with_capacity(4);
                attrs.set(kSecClass as *const _, kSecClassInternetPassword as *const _);
                attrs.set(kSecAttrServer as *const _, url.as_CFTypeRef());
                attrs.set(kSecAttrAccount as *const _, username.as_CFTypeRef());
                attrs.set(kSecValueData as *const _, password.as_CFTypeRef());

                let mut status = SecItemUpdate(
                    query_attrs.as_concrete_TypeRef(),
                    attrs.as_concrete_TypeRef(),
                );

                // If there were no existing credentials for the given server, then create them.
                if status == errSecItemNotFound {
                    verb = "creating";
                    status = SecItemAdd(attrs.as_concrete_TypeRef(), ptr::null_mut());
                }
                anyhow::ensure!(status == errSecSuccess, "{verb} password failed: {status}");
            }
            Ok(())
        })
    }

    fn read_credentials(&self, url: &str) -> Task<Result<Option<(String, Vec<u8>)>>> {
        let url = url.to_string();
        self.background_executor().spawn(async move {
            let url = CFString::from(url.as_str());
            let cf_true = CFBoolean::true_value().as_CFTypeRef();

            unsafe {
                use security::*;

                // Find any credentials for the given server URL.
                let mut attrs = CFMutableDictionary::with_capacity(5);
                attrs.set(kSecClass as *const _, kSecClassInternetPassword as *const _);
                attrs.set(kSecAttrServer as *const _, url.as_CFTypeRef());
                attrs.set(kSecReturnAttributes as *const _, cf_true);
                attrs.set(kSecReturnData as *const _, cf_true);

                let mut result = CFTypeRef::from(ptr::null());
                let status = SecItemCopyMatching(attrs.as_concrete_TypeRef(), &mut result);
                match status {
                    security::errSecSuccess => {}
                    security::errSecItemNotFound | security::errSecUserCanceled => return Ok(None),
                    _ => anyhow::bail!("reading password failed: {status}"),
                }

                let result = CFType::wrap_under_create_rule(result)
                    .downcast::<CFDictionary>()
                    .context("keychain item was not a dictionary")?;
                let username = result
                    .find(kSecAttrAccount as *const _)
                    .context("account was missing from keychain item")?;
                let username = CFType::wrap_under_get_rule(*username)
                    .downcast::<CFString>()
                    .context("account was not a string")?;
                let password = result
                    .find(kSecValueData as *const _)
                    .context("password was missing from keychain item")?;
                let password = CFType::wrap_under_get_rule(*password)
                    .downcast::<CFData>()
                    .context("password was not a string")?;

                Ok(Some((username.to_string(), password.bytes().to_vec())))
            }
        })
    }

    fn delete_credentials(&self, url: &str) -> Task<Result<()>> {
        let url = url.to_string();

        self.background_executor().spawn(async move {
            unsafe {
                use security::*;

                let url = CFString::from(url.as_str());
                let mut query_attrs = CFMutableDictionary::with_capacity(2);
                query_attrs.set(kSecClass as *const _, kSecClassInternetPassword as *const _);
                query_attrs.set(kSecAttrServer as *const _, url.as_CFTypeRef());

                let status = SecItemDelete(query_attrs.as_concrete_TypeRef());
                anyhow::ensure!(status == errSecSuccess, "delete password failed: {status}");
            }
            Ok(())
        })
    }
}

impl MacPlatform {
    unsafe fn read_string_from_clipboard(
        &self,
        state: &MacPlatformState,
        text_bytes: &[u8],
    ) -> ClipboardItem {
        let text = String::from_utf8_lossy(text_bytes).to_string();
        let metadata = self
            .read_from_pasteboard_typed(&state.pasteboard, &state.text_hash_pasteboard_type)
            .and_then(|hash_bytes| {
                let hash_bytes = hash_bytes.as_slice().try_into().ok()?;
                let hash = u64::from_be_bytes(hash_bytes);
                let metadata = self
                    .read_from_pasteboard_typed(&state.pasteboard, &state.metadata_pasteboard_type)?;

                if hash == ClipboardString::text_hash(&text) {
                    String::from_utf8(metadata).ok()
                } else {
                    None
                }
            });

        ClipboardItem {
            entries: vec![ClipboardEntry::String(ClipboardString { text, metadata })],
        }
    }

    unsafe fn write_plaintext_to_clipboard(&self, string: &ClipboardString) {
        let state = self.0.lock();
        state.pasteboard.clearContents();

        // Create typed NSData from Rust bytes
        let text_len = string.text.len();
        let text_ptr = string.text.as_ptr() as *const c_void;
        let text_bytes = unsafe { objc2_foundation::NSData::dataWithBytes_length(text_ptr, text_len) };
        unsafe {
            state
                .pasteboard
                .setData_forType(Some(&text_bytes), Objc2NSPasteboardTypeString);
        }

        if let Some(metadata) = string.metadata.as_ref() {
            let hash_bytes_arr = ClipboardString::text_hash(&string.text).to_be_bytes();
            let hash_ptr = hash_bytes_arr.as_ptr() as *const c_void;
            let hash_bytes = unsafe {
                objc2_foundation::NSData::dataWithBytes_length(hash_ptr, hash_bytes_arr.len())
            };
            state
                .pasteboard
                .setData_forType(Some(&hash_bytes), &state.text_hash_pasteboard_type);

            let meta_ptr = metadata.as_ptr() as *const c_void;
            let meta_bytes =
                unsafe { objc2_foundation::NSData::dataWithBytes_length(meta_ptr, metadata.len()) };
            state
                .pasteboard
                .setData_forType(Some(&meta_bytes), &state.metadata_pasteboard_type);
        }
    }

    unsafe fn write_image_to_clipboard(&self, image: &Image) {
        let state = self.0.lock();
        state.pasteboard.clearContents();

        let len = image.bytes.len();
        let ptr = image.bytes.as_ptr() as *const c_void;
        let bytes = unsafe { objc2_foundation::NSData::dataWithBytes_length(ptr, len) };

        let ty: UTType = image.format.into();
        state
            .pasteboard
            .setData_forType(Some(&bytes), &ty.0);
    }
}

fn try_clipboard_image(pasteboard: &Objc2NSPasteboard, format: ImageFormat) -> Option<ClipboardItem> {
    let ut_type: UTType = format.into();

    if let Some(types) = pasteboard.types() {
        if types.containsObject(&ut_type.0) {
            if let Some(data) = pasteboard.dataForType(&ut_type.0) {
                let len = data.length();
                let mut bytes = vec![0u8; len as usize];
                if len > 0 {
                    unsafe {
                        objc2_foundation::NSData::getBytes_length(
                            &data,
                            std::ptr::NonNull::new_unchecked(bytes.as_mut_ptr() as *mut _),
                            len,
                        );
                    }
                }
                let id = hash(&bytes);
                return Some(ClipboardItem { entries: vec![ClipboardEntry::Image(Image { format, bytes, id })] });
            }
        }
    }
    None
}

unsafe fn path_from_objc(path: id) -> PathBuf {
    let sref: &objc2_foundation::NSString = unsafe { &*(path as *mut objc2_foundation::NSString) };
    let s = objc2::rc::autoreleasepool(|pool| unsafe { sref.to_str(pool).to_owned() });
    PathBuf::from(s)
}

unsafe fn get_mac_platform(object: &mut Objc2AnyObject) -> &MacPlatform {
    let ivar_name = CStr::from_bytes_with_nul(b"platform\0").unwrap();
    let ivar = object.class().instance_variable(ivar_name).expect("platform ivar not found");
    let platform_ptr: *mut c_void = unsafe { *ivar.load_mut::<*mut c_void>(object) };
    assert!(!platform_ptr.is_null());
    unsafe { &*(platform_ptr as *const MacPlatform) }
}

extern "C" fn will_finish_launching(_this: &mut Objc2AnyObject, _: Objc2Sel, _: *mut Objc2AnyObject) {
    // Prefer typed NSUserDefaults; use msg_send for specific selector calls
    let defaults = objc2_foundation::NSUserDefaults::standardUserDefaults();
    let key = objc2_foundation::NSString::from_str("NSAutoFillHeuristicControllerEnabled");
    let key_ref: &objc2_foundation::NSString = &*key;
    let existing: *mut objc2::runtime::AnyObject = unsafe { objc2::msg_send![&*defaults, objectForKey: key_ref] };
    if existing.is_null() {
        let _: () = unsafe { objc2::msg_send![&*defaults, setBool: false, forKey: key_ref] };
    }
}

extern "C" fn did_finish_launching(this: &mut Objc2AnyObject, _: Objc2Sel, _: *mut Objc2AnyObject) {
    unsafe {
        // Set activation policy using objc2-app-kit
        let mtm = MainThreadMarker::new().expect("activation policy must be set on main thread");
        let app = objc2_app_kit::NSApplication::sharedApplication(mtm);
        use objc2_app_kit::NSApplicationActivationPolicy;
        app.setActivationPolicy(NSApplicationActivationPolicy::Regular);

        let notification_center: *mut Objc2AnyObject =
            objc2::msg_send![objc2::class!(NSNotificationCenter), defaultCenter];
        let name = objc2_foundation::NSString::from_str("NSTextInputContextKeyboardSelectionDidChangeNotification");
        let none: Option<&Objc2AnyObject> = None;
        let this_ref: &Objc2AnyObject = this;
        let _: () = objc2::msg_send![
            notification_center,
            addObserver: this_ref,
            selector: objc2::sel!(onKeyboardLayoutChange:),
            name: &*name,
            object: none
        ];

        let platform = get_mac_platform(this);
        let callback = platform.0.lock().finish_launching.take();
        if let Some(callback) = callback {
            callback();
        }
    }
}

extern "C" fn should_handle_reopen(this: &mut Objc2AnyObject, _: Objc2Sel, _: *mut Objc2AnyObject, has_open_windows: objc2::runtime::Bool) {
    if !has_open_windows.as_bool() {
        let platform = unsafe { get_mac_platform(this) };
        let mut lock = platform.0.lock();
        if let Some(mut callback) = lock.reopen.take() {
            drop(lock);
            callback();
            platform.0.lock().reopen.get_or_insert(callback);
        }
    }
}

extern "C" fn will_terminate(this: &mut Objc2AnyObject, _: Objc2Sel, _: *mut Objc2AnyObject) {
    let platform = unsafe { get_mac_platform(this) };
    let mut lock = platform.0.lock();
    if let Some(mut callback) = lock.quit.take() {
        drop(lock);
        callback();
        platform.0.lock().quit.get_or_insert(callback);
    }
}

extern "C" fn on_keyboard_layout_change(this: &mut Objc2AnyObject, _: Objc2Sel, _: *mut Objc2AnyObject) {
    let platform = unsafe { get_mac_platform(this) };
    let mut lock = platform.0.lock();
    let keyboard_layout = MacKeyboardLayout::new();
    lock.keyboard_mapper = Rc::new(MacKeyboardMapper::new(keyboard_layout.id()));
    if let Some(mut callback) = lock.on_keyboard_layout_change.take() {
        drop(lock);
        callback();
        platform
            .0
            .lock()
            .on_keyboard_layout_change
            .get_or_insert(callback);
    }
}

extern "C" fn open_urls(this: &mut Objc2AnyObject, _: Objc2Sel, _: *mut Objc2AnyObject, urls: *mut Objc2AnyObject) {
    let urls = unsafe {
        let arr: &objc2_foundation::NSArray<objc2_foundation::NSURL> =
            &*(urls as *mut objc2_foundation::NSArray<objc2_foundation::NSURL>);
        (0..arr.len())
            .filter_map(|i| {
                let url = arr.objectAtIndex(i as objc2_foundation::NSUInteger);
                if let Some(abs) = url.absoluteString() {
                    let s = objc2::rc::autoreleasepool(|pool| unsafe { abs.to_str(pool).to_owned() });
                    Some(s)
                } else {
                    None
                }
            })
            .collect::<Vec<_>>()
    };
    let platform = unsafe { get_mac_platform(this) };
    let mut lock = platform.0.lock();
    if let Some(mut callback) = lock.open_urls.take() {
        drop(lock);
        callback(urls);
        platform.0.lock().open_urls.get_or_insert(callback);
    }
}

extern "C" fn handle_menu_item(this: &mut Objc2AnyObject, _: Objc2Sel, item: *mut Objc2AnyObject) {
    unsafe {
        let platform = get_mac_platform(this);
        let mut lock = platform.0.lock();
        if let Some(mut callback) = lock.menu_command.take() {
            let item_obj: &objc::runtime::Object = unsafe { &*(item as *mut objc::runtime::Object) };
            let tag: NSInteger = msg_send![item_obj, tag];
            let index = tag as usize;
            if let Some(action) = lock.menu_actions.get(index) {
                let action = action.boxed_clone();
                drop(lock);
                callback(&*action);
            }
            platform.0.lock().menu_command.get_or_insert(callback);
        }
    }
}

extern "C" fn validate_menu_item(this: &mut Objc2AnyObject, _: Objc2Sel, item: *mut Objc2AnyObject) -> objc2::runtime::Bool {
    unsafe {
        let mut result = false;
        let platform = get_mac_platform(this);
        let mut lock = platform.0.lock();
        if let Some(mut callback) = lock.validate_menu_command.take() {
            let item_obj: &objc::runtime::Object = unsafe { &*(item as *mut objc::runtime::Object) };
            let tag: NSInteger = msg_send![item_obj, tag];
            let index = tag as usize;
            if let Some(action) = lock.menu_actions.get(index) {
                let action = action.boxed_clone();
                drop(lock);
                result = callback(action.as_ref());
            }
            platform
                .0
                .lock()
                .validate_menu_command
                .get_or_insert(callback);
        }
        objc2::runtime::Bool::new(result)
    }
}

extern "C" fn menu_will_open(this: &mut Objc2AnyObject, _: Objc2Sel, _: *mut Objc2AnyObject) {
    unsafe {
        let platform = get_mac_platform(this);
        let mut lock = platform.0.lock();
        if let Some(mut callback) = lock.will_open_menu.take() {
            drop(lock);
            callback();
            platform.0.lock().will_open_menu.get_or_insert(callback);
        }
    }
}

extern "C" fn handle_dock_menu(this: &mut Objc2AnyObject, _: Objc2Sel, _: *mut Objc2AnyObject) -> *mut Objc2AnyObject {
    unsafe {
        let platform = get_mac_platform(this);
        let mut state = platform.0.lock();
        if let Some(ref menu) = state.dock_menu {
            // Return the raw Objective-C pointer; ownership stays with our Retained
            Retained::as_ptr(menu) as *mut Objc2AnyObject
        } else {
            std::ptr::null_mut()
        }
    }
}

// Removed legacy ns_string helper; prefer objc2_foundation::NSString::from_str instead.

unsafe fn ns_url_to_path(url: id) -> Result<PathBuf> {
    let path: *mut c_char = msg_send![url, fileSystemRepresentation];
    anyhow::ensure!(!path.is_null(), "url is not a file path: {}", {
        let abs: id = msg_send![url, absoluteString];
        if abs.is_null() { String::new() } else {
            let sref: &objc2_foundation::NSString = unsafe { &*(abs as *mut objc2_foundation::NSString) };
            objc2::rc::autoreleasepool(|pool| unsafe { sref.to_str(pool).to_owned() })
        }
    });
    Ok(PathBuf::from(OsStr::from_bytes(unsafe {
        CStr::from_ptr(path).to_bytes()
    })))
}

fn objc_url_to_path(url: &objc2_foundation::NSURL) -> Result<PathBuf> {
    // SAFETY: `fileSystemRepresentation` returns a stable pointer valid while `url` is alive.
    let path_ptr = url.fileSystemRepresentation();
    // Ensure not null; convert to PathBuf
    let cstr = unsafe { CStr::from_ptr(path_ptr.as_ptr()) };
    Ok(PathBuf::from(OsStr::from_bytes(cstr.to_bytes())))
}

#[link(name = "Carbon", kind = "framework")]
unsafe extern "C" {
    pub(super) fn TISCopyCurrentKeyboardLayoutInputSource() -> *mut Object;
    pub(super) fn TISGetInputSourceProperty(
        inputSource: *mut Object,
        propertyKey: *const c_void,
    ) -> *mut Object;

    pub(super) fn UCKeyTranslate(
        keyLayoutPtr: *const ::std::os::raw::c_void,
        virtualKeyCode: u16,
        keyAction: u16,
        modifierKeyState: u32,
        keyboardType: u32,
        keyTranslateOptions: u32,
        deadKeyState: *mut u32,
        maxStringLength: usize,
        actualStringLength: *mut usize,
        unicodeString: *mut u16,
    ) -> u32;
    pub(super) fn LMGetKbdType() -> u16;
    pub(super) static kTISPropertyUnicodeKeyLayoutData: CFStringRef;
    pub(super) static kTISPropertyInputSourceID: CFStringRef;
    pub(super) static kTISPropertyLocalizedName: CFStringRef;
}

mod security {
    #![allow(non_upper_case_globals)]
    use super::*;

    #[link(name = "Security", kind = "framework")]
    unsafe extern "C" {
        pub static kSecClass: CFStringRef;
        pub static kSecClassInternetPassword: CFStringRef;
        pub static kSecAttrServer: CFStringRef;
        pub static kSecAttrAccount: CFStringRef;
        pub static kSecValueData: CFStringRef;
        pub static kSecReturnAttributes: CFStringRef;
        pub static kSecReturnData: CFStringRef;

        pub fn SecItemAdd(attributes: CFDictionaryRef, result: *mut CFTypeRef) -> OSStatus;
        pub fn SecItemUpdate(query: CFDictionaryRef, attributes: CFDictionaryRef) -> OSStatus;
        pub fn SecItemDelete(query: CFDictionaryRef) -> OSStatus;
        pub fn SecItemCopyMatching(query: CFDictionaryRef, result: *mut CFTypeRef) -> OSStatus;
    }

    pub const errSecSuccess: OSStatus = 0;
    pub const errSecUserCanceled: OSStatus = -128;
    pub const errSecItemNotFound: OSStatus = -25300;
}

impl From<ImageFormat> for UTType {
    fn from(value: ImageFormat) -> Self {
        match value {
            ImageFormat::Png => Self::png(),
            ImageFormat::Jpeg => Self::jpeg(),
            ImageFormat::Tiff => Self::tiff(),
            ImageFormat::Webp => Self::webp(),
            ImageFormat::Gif => Self::gif(),
            ImageFormat::Bmp => Self::bmp(),
            ImageFormat::Svg => Self::svg(),
        }
    }
}

// See https://developer.apple.com/documentation/uniformtypeidentifiers/uttype-swift.struct/
struct UTType(Retained<objc2_foundation::NSString>);

impl UTType {
    pub fn png() -> Self {
        // built-in NSPasteboardType
        Self(unsafe { Retained::retain(Objc2NSPasteboardTypePNG as *const _ as *mut _) }.unwrap())
    }

    pub fn jpeg() -> Self {
        Self(objc2_foundation::NSString::from_str("public.jpeg"))
    }

    pub fn gif() -> Self {
        Self(objc2_foundation::NSString::from_str("com.compuserve.gif"))
    }

    pub fn webp() -> Self {
        Self(objc2_foundation::NSString::from_str("org.webmproject.webp"))
    }

    pub fn bmp() -> Self {
        Self(objc2_foundation::NSString::from_str("com.microsoft.bmp"))
    }

    pub fn svg() -> Self {
        Self(objc2_foundation::NSString::from_str("public.svg-image"))
    }

    pub fn tiff() -> Self {
        // built-in NSPasteboardType
        Self(unsafe { Retained::retain(Objc2NSPasteboardTypeTIFF as *const _ as *mut _) }.unwrap())
    }
}

#[cfg(test)]
mod tests {
    use crate::ClipboardItem;

    use super::*;

    #[test]
    fn test_clipboard() {
        let platform = build_platform();
        assert_eq!(platform.read_from_clipboard(), None);

        let item = ClipboardItem::new_string("1".to_string());
        platform.write_to_clipboard(item.clone());
        assert_eq!(platform.read_from_clipboard(), Some(item));

        let item = ClipboardItem {
            entries: vec![ClipboardEntry::String(
                ClipboardString::new("2".to_string()).with_json_metadata(vec![3, 4]),
            )],
        };
        platform.write_to_clipboard(item.clone());
        assert_eq!(platform.read_from_clipboard(), Some(item));

        let text_from_other_app = "text from other app";
        unsafe {
            let bytes = NSData::dataWithBytes_length_(
                nil,
                text_from_other_app.as_ptr() as *const c_void,
                text_from_other_app.len() as u64,
            );
            platform
                .0
                .lock()
                .pasteboard
                .setData_forType(bytes, NSPasteboardTypeString);
        }
        assert_eq!(
            platform.read_from_clipboard(),
            Some(ClipboardItem::new_string(text_from_other_app.to_string()))
        );
    }

    fn build_platform() -> MacPlatform {
        let platform = MacPlatform::new(false);
        platform.0.lock().pasteboard = unsafe { NSPasteboard::pasteboardWithUniqueName(nil) };
        platform
    }
}
