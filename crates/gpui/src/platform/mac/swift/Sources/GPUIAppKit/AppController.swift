@preconcurrency import AppKit
import QuartzCore
import GPUIFFI
import ObjectiveC

// Store callbacks copied from Rust so we can call back into it.
@MainActor fileprivate var gCallbacks: GPUI_Callbacks? = nil
@MainActor fileprivate var gUserData: UnsafeMutableRawPointer? = nil

@MainActor private var kDelegateKey: UInt8 = 0

@MainActor final class GPUIWindowDelegate: NSObject, NSWindowDelegate {
    let handle: GPUI_WindowHandle
    init(handle: GPUI_WindowHandle) { self.handle = handle }

    func windowDidResize(_ notification: Notification) {
        guard let win = notification.object as? NSWindow else { return }
        let size = win.contentLayoutRect.size
        let scale = Float(win.backingScaleFactor)
        if let cb = gCallbacks?.on_window_resized {
            cb(handle, UInt32(size.width), UInt32(size.height), scale)
        }
    }

    func windowDidBecomeKey(_ notification: Notification) {
        gCallbacks?.on_window_active_changed?(gUserData, handle, true)
    }

    func windowDidResignKey(_ notification: Notification) {
        gCallbacks?.on_window_active_changed?(gUserData, handle, false)
    }

    func windowDidMove(_ notification: Notification) {
        gCallbacks?.on_window_moved?(gUserData, handle)
    }
    func windowDidChangeOcclusionState(_ notification: Notification) {
        guard let win = notification.object as? NSWindow else { return }
        let visible = win.occlusionState.contains(.visible)
        gCallbacks?.on_window_visibility_changed?(gUserData, handle, visible)
    }

    func windowShouldClose(_ sender: NSWindow) -> Bool {
        if let cb = gCallbacks?.on_window_should_close {
            return cb(gUserData, handle)
        }
        return true
    }

    func windowWillClose(_ notification: Notification) {
        gCallbacks?.on_window_will_close?(gUserData, handle)
    }
}

@MainActor final class GPUIView: NSView, NSTextInputClient {
    override var acceptsFirstResponder: Bool { true }

    private func modifiersMask(_ flags: NSEvent.ModifierFlags) -> UInt32 {
        var m: UInt32 = 0
        if flags.contains(.shift) { m |= 1 << 0 }
        if flags.contains(.command) { m |= 1 << 1 }
        if flags.contains(.control) { m |= 1 << 2 }
        if flags.contains(.option) { m |= 1 << 3 }
        if flags.contains(.function) { m |= 1 << 4 }
        if flags.contains(.capsLock) { m |= 1 << 5 }
        return m
    }

    private func sendMouse(_ type: GPUI_MouseType, event: NSEvent, dx: CGFloat = 0, dy: CGFloat = 0) {
        guard let cb = gCallbacks?.on_mouse_event else { return }
        let loc = self.convert(event.locationInWindow, from: nil)
        let button: GPUI_MouseButton
        switch event.buttonNumber {
        case 1: button = GPUI_MouseButtonRight
        case 2: button = GPUI_MouseButtonMiddle
        default: button = GPUI_MouseButtonLeft
        }
        var ev = GPUI_MouseEvent(
            window: Unmanaged.passUnretained(self.window!).toOpaque(),
            type: type,
            button: button,
            x: Float(loc.x), y: Float(loc.y),
            dx: Float(dx), dy: Float(dy),
            click_count: UInt32(event.clickCount),
            modifiers: modifiersMask(event.modifierFlags)
        )
        withUnsafePointer(to: &ev) { cb($0) }
    }

    private func sendKey(_ phase: GPUI_KeyPhase, event: NSEvent) {
        guard let cb = gCallbacks?.on_key_event else { return }
        let chars = event.charactersIgnoringModifiers ?? ""
        let typed = event.characters
        let keyLower = chars.lowercased()
        let key_c = keyLower.cString(using: .utf8)
        let key_ptr = key_c?.withUnsafeBufferPointer { $0.baseAddress }
        var key_char_ptr: UnsafePointer<CChar>? = nil
        if let typed, !typed.isEmpty {
            let typedLower = typed
            let c = typedLower.cString(using: .utf8)
            key_char_ptr = c?.withUnsafeBufferPointer { $0.baseAddress }
        }
        var kev = GPUI_KeyEvent(
            window: Unmanaged.passUnretained(self.window!).toOpaque(),
            phase: phase,
            key_code: UInt16(event.keyCode),
            unicode: 0,
            modifiers: modifiersMask(event.modifierFlags),
            is_repeat: event.isARepeat,
            key: key_ptr,
            key_char: key_char_ptr
        )
        withUnsafePointer(to: &kev) { cb($0) }
    }

    override func mouseDown(with event: NSEvent) { sendMouse(GPUI_MouseDown, event: event) }
    override func mouseUp(with event: NSEvent) { sendMouse(GPUI_MouseUp, event: event) }
    override func rightMouseDown(with event: NSEvent) { sendMouse(GPUI_MouseDown, event: event) }
    override func rightMouseUp(with event: NSEvent) { sendMouse(GPUI_MouseUp, event: event) }
    override func otherMouseDown(with event: NSEvent) { sendMouse(GPUI_MouseDown, event: event) }
    override func otherMouseUp(with event: NSEvent) { sendMouse(GPUI_MouseUp, event: event) }
    override func mouseMoved(with event: NSEvent) { sendMouse(GPUI_MouseMove, event: event) }
    override func mouseDragged(with event: NSEvent) { sendMouse(GPUI_MouseDrag, event: event) }
    override func rightMouseDragged(with event: NSEvent) { sendMouse(GPUI_MouseDrag, event: event) }
    override func otherMouseDragged(with event: NSEvent) { sendMouse(GPUI_MouseDrag, event: event) }
    override func scrollWheel(with event: NSEvent) { sendMouse(GPUI_MouseScroll, event: event, dx: event.scrollingDeltaX, dy: event.scrollingDeltaY) }

    override func keyDown(with event: NSEvent) { sendKey(GPUI_KeyDown, event: event) }
    override func keyUp(with event: NSEvent) { sendKey(GPUI_KeyUp, event: event) }
    override func flagsChanged(with event: NSEvent) { sendKey(GPUI_FlagsChanged, event: event) }

    // MARK: - NSTextInputClient

    func hasMarkedText() -> Bool {
        guard let cb = gCallbacks?.ime_marked_range else { return false }
        var loc: UInt32 = 0, len: UInt32 = 0
        return cb(gUserData, Unmanaged.passUnretained(self.window!).toOpaque(), &loc, &len)
    }

    func markedRange() -> NSRange {
        guard let cb = gCallbacks?.ime_marked_range else { return NSRange(location: NSNotFound, length: 0) }
        var loc: UInt32 = 0, len: UInt32 = 0
        let ok = cb(gUserData, Unmanaged.passUnretained(self.window!).toOpaque(), &loc, &len)
        return ok ? NSRange(location: Int(loc), length: Int(len)) : NSRange(location: NSNotFound, length: 0)
    }

    func selectedRange() -> NSRange {
        guard let cb = gCallbacks?.ime_selected_range else { return NSRange(location: NSNotFound, length: 0) }
        var loc: UInt32 = 0, len: UInt32 = 0
        var rev: Bool = false
        let ok = cb(gUserData, Unmanaged.passUnretained(self.window!).toOpaque(), &loc, &len, &rev)
        return ok ? NSRange(location: Int(loc), length: Int(len)) : NSRange(location: NSNotFound, length: 0)
    }

    func attributedSubstring(forProposedRange range: NSRange, actualRange: NSRangePointer?) -> NSAttributedString? {
        guard let cb = gCallbacks?.ime_text_for_range, let freeFn = gCallbacks?.ime_free_text else { return nil }
        var ptr: UnsafePointer<UInt8>? = nil
        var len: Int = 0
        var adjLoc: UInt32 = 0, adjLen: UInt32 = 0
        let ok = cb(gUserData, Unmanaged.passUnretained(self.window!).toOpaque(), UInt32(range.location), UInt32(range.length), &ptr, &len, &adjLoc, &adjLen)
        guard ok, let base = ptr, len > 0 else { return nil }
        let data = Data(bytes: base, count: len)
        // Free the allocated bytes on Rust side
        freeFn(base, len)
        guard let s = String(data: data, encoding: .utf8) else { return nil }
        if let actual = actualRange, adjLen > 0 {
            actual.pointee = NSRange(location: Int(adjLoc), length: Int(adjLen))
        }
        return NSAttributedString(string: s)
    }

    func insertText(_ string: Any, replacementRange: NSRange) {
        guard let cb = gCallbacks?.ime_replace_text_in_range else { return }
        let str: String
        if let s = string as? String { str = s }
        else if let a = string as? NSAttributedString { str = a.string }
        else { return }
        let utf8 = Array(str.utf8)
        utf8.withUnsafeBufferPointer { buf in
            let hasRange = replacementRange.location != NSNotFound
            cb(gUserData,
               Unmanaged.passUnretained(self.window!).toOpaque(),
               hasRange,
               UInt32(replacementRange.location),
               UInt32(replacementRange.length),
               buf.baseAddress,
               buf.count)
        }
    }

    func setMarkedText(_ string: Any, selectedRange: NSRange, replacementRange: NSRange) {
        guard let cb = gCallbacks?.ime_replace_and_mark_text_in_range else { return }
        let str: String
        if let s = string as? String { str = s }
        else if let a = string as? NSAttributedString { str = a.string }
        else { return }
        let utf8 = Array(str.utf8)
        utf8.withUnsafeBufferPointer { buf in
            let hasRange = replacementRange.location != NSNotFound
            let hasSel = selectedRange.location != NSNotFound
            cb(gUserData,
               Unmanaged.passUnretained(self.window!).toOpaque(),
               hasRange,
               UInt32(replacementRange.location),
               UInt32(replacementRange.length),
               buf.baseAddress,
               buf.count,
               hasSel,
               UInt32(selectedRange.location),
               UInt32(selectedRange.length))
        }
    }

    func unmarkText() {
        gCallbacks?.ime_unmark_text?(gUserData, Unmanaged.passUnretained(self.window!).toOpaque())
    }

    func firstRect(forCharacterRange range: NSRange, actualRange: NSRangePointer?) -> NSRect {
        guard let cb = gCallbacks?.ime_bounds_for_range, let win = self.window else { return .zero }
        var x: Float = 0, y: Float = 0, w: Float = 0, h: Float = 0
        let ok = cb(gUserData, Unmanaged.passUnretained(win).toOpaque(), UInt32(range.location), UInt32(range.length), &x, &y, &w, &h)
        guard ok else { return .zero }
        let frame = win.frame
        let content = win.contentLayoutRect
        let hasFullSize = win.styleMask.contains(.fullSizeContentView)
        let titlebarOffset = hasFullSize ? 0.0 : (frame.size.height - content.size.height)
        let rect = NSRect(
            x: frame.origin.x + CGFloat(x),
            y: frame.origin.y + frame.size.height - CGFloat(y) - CGFloat(h) - titlebarOffset,
            width: CGFloat(w), height: CGFloat(h)
        )
        return rect
    }

    func validAttributesForMarkedText() -> [NSAttributedString.Key] { [] }

    func characterIndex(for point: NSPoint) -> Int { NSNotFound }

    // MARK: - NSDraggingDestination (file URLs)
    override func draggingEntered(_ sender: NSDraggingInfo) -> NSDragOperation {
        guard let window = self.window else { return [] }
        let loc = self.convert(sender.draggingLocation, from: nil)
        var paths: [String] = []
        if let items = sender.draggingPasteboard.readObjects(forClasses: [NSURL.self], options: [ .urlReadingFileURLsOnly: true]) as? [NSURL] {
            for url in items {
                if let path = url.path { paths.append(path) }
            }
        }
        if !paths.isEmpty, let cb = gCallbacks?.on_file_drop_event {
            let data = try? JSONSerialization.data(withJSONObject: paths)
            if let bytes = data {
                bytes.withUnsafeBytes { raw in
                    let ptr = raw.bindMemory(to: UInt8.self).baseAddress
                    cb(gUserData, Unmanaged.passUnretained(window).toOpaque(), 0, Float(loc.x), Float(loc.y), ptr, bytes.count)
                }
            }
            return .copy
        }
        return []
    }

    override func draggingUpdated(_ sender: NSDraggingInfo) -> NSDragOperation {
        guard let window = self.window else { return [] }
        let loc = self.convert(sender.draggingLocation, from: nil)
        gCallbacks?.on_file_drop_event?(gUserData, Unmanaged.passUnretained(window).toOpaque(), 1, Float(loc.x), Float(loc.y), nil, 0)
        return .copy
    }

    override func draggingExited(_ sender: NSDraggingInfo?) {
        guard let window = self.window else { return }
        gCallbacks?.on_file_drop_event?(gUserData, Unmanaged.passUnretained(window).toOpaque(), 2, 0, 0, nil, 0)
    }

    override func performDragOperation(_ sender: NSDraggingInfo) -> Bool {
        guard let window = self.window else { return false }
        let loc = self.convert(sender.draggingLocation, from: nil)
        gCallbacks?.on_file_drop_event?(gUserData, Unmanaged.passUnretained(window).toOpaque(), 3, Float(loc.x), Float(loc.y), nil, 0)
        return true
    }

    // MARK: - Hover tracking
    private var trackingAreaRef: NSTrackingArea?
    override func updateTrackingAreas() {
        super.updateTrackingAreas()
        if let ta = trackingAreaRef { self.removeTrackingArea(ta) }
        let options: NSTrackingArea.Options = [.mouseEnteredAndExited, .activeAlways, .inVisibleRect]
        let ta = NSTrackingArea(rect: self.bounds, options: options, owner: self, userInfo: nil)
        self.addTrackingArea(ta)
        trackingAreaRef = ta
    }

    override func mouseEntered(with event: NSEvent) {
        guard let window = self.window else { return }
        gCallbacks?.on_hover_changed?(gUserData, Unmanaged.passUnretained(window).toOpaque(), true)
    }
    override func mouseExited(with event: NSEvent) {
        guard let window = self.window else { return }
        gCallbacks?.on_hover_changed?(gUserData, Unmanaged.passUnretained(window).toOpaque(), false)
    }

    override func viewDidChangeEffectiveAppearance() {
        super.viewDidChangeEffectiveAppearance()
        guard let window = self.window else { return }
        gCallbacks?.on_window_appearance_changed?(gUserData, Unmanaged.passUnretained(window).toOpaque())
    }
}

@MainActor @_cdecl("gpui_macos_init")
public func gpui_macos_init(_ userData: UnsafeMutableRawPointer?, _ callbacks: UnsafePointer<GPUI_Callbacks>?) {
    if let callbacks = callbacks?.pointee {
        gCallbacks = callbacks
    } else {
        gCallbacks = nil
    }
    gUserData = userData
}

@MainActor @_cdecl("gpui_macos_run")
public func gpui_macos_run() {
    let app = NSApplication.shared

    // Call lifecycle callbacks. For the skeleton we invoke immediately.
    if let will = gCallbacks?.on_app_will_finish_launching {
        will()
    }
    if let did = gCallbacks?.on_app_did_finish_launching {
        did()
    }
    // Own the run loop now that callbacks are wired.
    app.run()
}

@MainActor @_cdecl("gpui_macos_quit")
public func gpui_macos_quit() {
    NSApplication.shared.terminate(nil)
}

@MainActor @_cdecl("gpui_macos_create_window")
public func gpui_macos_create_window(
    _ params: UnsafePointer<GPUI_WindowParams>?,
    _ outHandle: UnsafeMutablePointer<GPUI_WindowHandle>?,
    _ outMetalLayer: UnsafeMutablePointer<UnsafeMutableRawPointer?>?
) {
    let width = Double(params?.pointee.width ?? 800)
    let height = Double(params?.pointee.height ?? 600)
    let frame = NSRect(x: 0, y: 0, width: width, height: height)

    let style: NSWindow.StyleMask = [.titled, .closable, .resizable, .miniaturizable]
    let win = NSWindow(contentRect: frame, styleMask: style, backing: .buffered, defer: false)

    if let cString = params?.pointee.title {
        win.title = String(cString: cString)
    }

    let view = GPUIView(frame: frame)
    view.wantsLayer = true
    let metalLayer = CAMetalLayer()
    view.layer = metalLayer
    win.contentView = view
    win.initialFirstResponder = view
    win.makeKeyAndOrderFront(nil)
    // Register for file URL drags
    view.registerForDraggedTypes([.fileURL])

    // Set delegate to forward resize events
    let handle = Unmanaged.passRetained(win).toOpaque()
    let delegate = GPUIWindowDelegate(handle: handle)
    win.delegate = delegate
    // Retain delegate strongly using associated object to ensure lifetime
    objc_setAssociatedObject(win, &kDelegateKey, delegate, .OBJC_ASSOCIATION_RETAIN_NONATOMIC)

    if let outHandle = outHandle {
        outHandle.pointee = handle
    }
    if let outMetalLayer = outMetalLayer {
        outMetalLayer.pointee = Unmanaged.passUnretained(metalLayer).toOpaque()
    }

    // Send initial resize callback so Rust can size its renderer
    let size = win.contentLayoutRect.size
    let scale = Float(win.backingScaleFactor)
    if let cb = gCallbacks?.on_window_resized {
        cb(handle, UInt32(size.width), UInt32(size.height), scale)
    }
}

// MARK: - Menus

private struct MenuItemDesc: Decodable {
    let kind: String
    let title: String?
    let tag: Int?
    let key: String?
    let mods: UInt32?
    let items: [MenuItemDesc]?
}

private struct MenuDesc: Decodable {
    let title: String
    let items: [MenuItemDesc]
}

@MainActor @_cdecl("gpui_macos_set_menus")
public func gpui_macos_set_menus(_ json: UnsafePointer<UInt8>?, _ len: Int) {
    guard let json, len > 0 else { return }
    let data = Data(bytes: json, count: len)
    guard let menus = try? JSONDecoder().decode([MenuDesc].self, from: data) else { return }
    let mtm = Thread.isMainThread
    let build: () -> Void = {
        let mainMenu = NSMenu(title: "")
        for m in menus {
            let sub = NSMenu(title: m.title)
            populateMenu(sub, items: m.items)
            let topItem = NSMenuItem(title: m.title, action: nil, keyEquivalent: "")
            topItem.submenu = sub
            mainMenu.addItem(topItem)
        }
        NSApplication.shared.mainMenu = mainMenu
    }
    if mtm { build() } else { DispatchQueue.main.async { build() } }
}

fileprivate var gDockMenu: NSMenu? = nil

@MainActor @_cdecl("gpui_macos_set_dock_menu")
public func gpui_macos_set_dock_menu(_ json: UnsafePointer<UInt8>?, _ len: Int) {
    guard let json, len > 0 else { return }
    let data = Data(bytes: json, count: len)
    guard let items = try? JSONDecoder().decode([MenuItemDesc].self, from: data) else { return }
    let build: () -> Void = {
        let menu = NSMenu(title: "Dock")
        populateMenu(menu, items: items)
        gDockMenu = menu
        if NSApplication.shared.delegate == nil {
            NSApplication.shared.delegate = AppDelegateProxy.shared
        }
    }
    if Thread.isMainThread { build() } else { DispatchQueue.main.async { build() } }
}

private func populateMenu(_ menu: NSMenu, items: [MenuItemDesc]) {
    menu.delegate = nil
    for item in items {
        switch item.kind {
        case "separator":
            menu.addItem(NSMenuItem.separator())
        case "submenu":
            let title = item.title ?? ""
            let sub = NSMenu(title: title)
            if let children = item.items { populateMenu(sub, items: children) }
            let it = NSMenuItem(title: title, action: nil, keyEquivalent: "")
            it.submenu = sub
            menu.addItem(it)
        case "action":
            let title = item.title ?? ""
            let tag = item.tag ?? -1
            let keyEq = item.key ?? ""
            let it = NSMenuItem(title: title, action: #selector(MenuTarget.onMenuItem(_:)), keyEquivalent: keyEq)
            it.target = MenuTarget.shared
            it.tag = tag
            if let mods = item.mods {
                var flags: NSEvent.ModifierFlags = []
                if (mods & (1 << 1)) != 0 { flags.insert(.command) }
                if (mods & (1 << 2)) != 0 { flags.insert(.control) }
                if (mods & (1 << 3)) != 0 { flags.insert(.option) }
                if (mods & (1 << 0)) != 0 { flags.insert(.shift) }
                if (mods & (1 << 4)) != 0 { flags.insert(.function) }
                it.keyEquivalentModifierMask = flags
            }
            menu.addItem(it)
        default:
            continue
        }
    }
}

@MainActor private final class MenuTarget: NSObject {
    static let shared = MenuTarget()
    @objc func onMenuItem(_ sender: NSMenuItem) {
        if let cb = gCallbacks?.on_key_event { _ = cb } // silence unused in case
        if let cb = gCallbacks?.on_mouse_event { _ = cb }
        if let action = gCallbacks?.on_menu_action {
            action(gUserData, Int32(sender.tag))
        }
    }
    override func validateMenuItem(_ menuItem: NSMenuItem) -> Bool {
        if let validate = gCallbacks?.on_validate_menu {
            return validate(gUserData, Int32(menuItem.tag))
        }
        return true
    }
}

final class AppDelegateProxy: NSObject, NSApplicationDelegate {
    static let shared = AppDelegateProxy()
    func applicationDockMenu(_ sender: NSApplication) -> NSMenu? { gDockMenu }
}

// MARK: - Panels

private struct OpenPanelOptions: Decodable {
    let files: Bool
    let directories: Bool
    let multiple: Bool
    let prompt: String?
}

private struct SavePanelOptions: Decodable {
    let directory: String
    let suggested_name: String?
    let prompt: String?
}

@MainActor @_cdecl("gpui_macos_open_panel")
public func gpui_macos_open_panel(_ json: UnsafePointer<UInt8>?, _ len: Int, _ requestId: UInt64) {
    guard let json, len > 0 else { return }
    let data = Data(bytes: json, count: len)
    guard let opts = try? JSONDecoder().decode(OpenPanelOptions.self, from: data) else { return }
    let work = {
        let p = NSOpenPanel()
        p.canChooseFiles = opts.files
        p.canChooseDirectories = opts.directories
        p.allowsMultipleSelection = opts.multiple
        if let prompt = opts.prompt { p.prompt = prompt }
        p.begin { resp in
            var payload: [String: Any]? = nil
            if resp == .OK {
                let urls = p.urls
                let paths = urls.compactMap { $0.path(percentEncoded: false) }
                payload = ["paths": paths]
            }
            let jsonData = try? JSONSerialization.data(withJSONObject: payload ?? NSNull())
            if let bytes = jsonData {
                bytes.withUnsafeBytes { raw in
                    let ptr = raw.bindMemory(to: UInt8.self).baseAddress
                    gCallbacks?.on_open_panel_result?(gUserData, requestId, ptr, bytes.count)
                }
            } else {
                gCallbacks?.on_open_panel_result?(gUserData, requestId, nil, 0)
            }
        }
    }
    if Thread.isMainThread { work() } else { DispatchQueue.main.async { work() } }
}

@MainActor @_cdecl("gpui_macos_save_panel")
public func gpui_macos_save_panel(_ json: UnsafePointer<UInt8>?, _ len: Int, _ requestId: UInt64) {
    guard let json, len > 0 else { return }
    let data = Data(bytes: json, count: len)
    guard let opts = try? JSONDecoder().decode(SavePanelOptions.self, from: data) else { return }
    let work = {
        let p = NSSavePanel()
        let dirURL = URL(fileURLWithPath: opts.directory, isDirectory: true)
        p.directoryURL = dirURL
        if let name = opts.suggested_name { p.nameFieldStringValue = name }
        if let prompt = opts.prompt { p.prompt = prompt }
        p.begin { resp in
            var payload: [String: Any]? = nil
            if resp == .OK, let url = p.url { payload = ["path": url.path(percentEncoded: false)] }
            let jsonData = try? JSONSerialization.data(withJSONObject: payload ?? NSNull())
            if let bytes = jsonData {
                bytes.withUnsafeBytes { raw in
                    let ptr = raw.bindMemory(to: UInt8.self).baseAddress
                    gCallbacks?.on_save_panel_result?(gUserData, requestId, ptr, bytes.count)
                }
            } else {
                gCallbacks?.on_save_panel_result?(gUserData, requestId, nil, 0)
            }
        }
    }
    if Thread.isMainThread { work() } else { DispatchQueue.main.async { work() } }
}

// MARK: - Cursor

@MainActor @_cdecl("gpui_macos_set_cursor")
public func gpui_macos_set_cursor(_ style: Int32, _ hideUntilMouseMoves: Bool) {
    if hideUntilMouseMoves || style < 0 {
        NSCursor.setHiddenUntilMouseMoves(true)
        return
    }
    let cursor: NSCursor? = {
        switch style {
        case 0: return .arrow
        case 1: return .iBeam
        case 2: return .crosshair
        case 3: return .closedHand
        case 4: return .openHand
        case 5: return .pointingHand
        case 6: return .resizeLeftRight
        case 7: return .resizeUpDown
        // The following are best-effort fallbacks to public cursors
        case 8: return .resizeLeftRight
        case 9: return .resizeLeftRight
        case 10: return .resizeLeftRight
        case 11: return .resizeUpDown
        case 12: return .resizeUpDown
        case 13: return .resizeUpDown
        case 14: return .resizeLeftRight
        case 15: return .resizeLeftRight
        case 16:
            // Vertical layout I-beam is not public; fallback to I-beam
            return .iBeam
        case 17: return .operationNotAllowed
        case 18:
            if #available(macOS 10.13, *) { return .dragLink }
            else { return .pointingHand }
        case 19:
            if #available(macOS 10.13, *) { return .dragCopy }
            else { return .pointingHand }
        case 20:
            if #available(macOS 10.13, *) { return .contextualMenu }
            else { return .arrow }
        default: return nil
        }
    }()
    cursor?.set()
}

// MARK: - Window Commands

@MainActor @_cdecl("gpui_macos_window_set_title")
public func gpui_macos_window_set_title(_ window: UnsafeMutableRawPointer?, _ utf8: UnsafePointer<UInt8>?, _ len: Int) {
    guard let window else { return }
    let win = Unmanaged<NSWindow>.fromOpaque(window).takeUnretainedValue()
    if let utf8, len > 0 {
        let data = Data(bytes: utf8, count: len)
        if let s = String(data: data, encoding: .utf8) {
            win.title = s
        }
    }
}

@MainActor @_cdecl("gpui_macos_window_minimize")
public func gpui_macos_window_minimize(_ window: UnsafeMutableRawPointer?) {
    guard let window else { return }
    let win = Unmanaged<NSWindow>.fromOpaque(window).takeUnretainedValue()
    win.miniaturize(nil)
}

@MainActor @_cdecl("gpui_macos_window_zoom")
public func gpui_macos_window_zoom(_ window: UnsafeMutableRawPointer?) {
    guard let window else { return }
    let win = Unmanaged<NSWindow>.fromOpaque(window).takeUnretainedValue()
    win.performZoom(nil)
}

@MainActor @_cdecl("gpui_macos_window_toggle_fullscreen")
public func gpui_macos_window_toggle_fullscreen(_ window: UnsafeMutableRawPointer?) {
    guard let window else { return }
    let win = Unmanaged<NSWindow>.fromOpaque(window).takeUnretainedValue()
    win.toggleFullScreen(nil)
}

@MainActor @_cdecl("gpui_macos_window_is_fullscreen")
public func gpui_macos_window_is_fullscreen(_ window: UnsafeMutableRawPointer?) -> Bool {
    guard let window else { return false }
    let win = Unmanaged<NSWindow>.fromOpaque(window).takeUnretainedValue()
    return win.styleMask.contains(.fullScreen)
}
