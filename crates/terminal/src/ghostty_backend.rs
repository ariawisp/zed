#![cfg(feature = "ghostty-backend")]

use libghostty::{input, pty::Pty, renderer, vt::Session};
use std::{ffi::CString, io, os::raw::c_void};

pub struct GhosttyBackend {
    vt: Session,
    pty: Pty,
    #[cfg(target_os = "macos")]
    renderer: Option<renderer::Renderer>,
}

impl GhosttyBackend {
    pub fn new(rows: u16, cols: u16) -> io::Result<Self> {
        let vt = Session::new(cols, rows, 8 * 1024 * 1024).map_err(to_io)?;
        let pty = Pty::open(rows, cols).map_err(to_io)?;
        Ok(Self { vt, pty, #[cfg(target_os = "macos")] renderer: None })
    }

    #[cfg(target_os = "macos")]
    pub fn attach_renderer(&mut self, nsview: *mut c_void, content_scale: f64) -> io::Result<()> {
        let mut r = renderer::Renderer::new_macos(nsview, content_scale).map_err(to_io)?;
        r.attach_vt(&self.vt).map_err(to_io)?;
        r.set_visible(true);
        r.set_focus(true);
        self.renderer = Some(r);
        Ok(())
    }

    pub fn spawn_shell(&self, argv: &[&str], cwd: Option<&str>) -> io::Result<()> {
        let c_argv: Vec<_> = argv.iter().map(|s| CString::new(*s).unwrap()).collect();
        let c_argv_ref: Vec<&std::ffi::CStr> = c_argv.iter().map(|s| s.as_c_str()).collect();
        let c_cwd = cwd.map(|s| CString::new(s).unwrap());
        self.pty
            .spawn(&c_argv_ref, None, c_cwd.as_ref().map(|s| s.as_c_str()))
            .map_err(to_io)
    }

    pub fn pty_fd(&self) -> std::os::fd::RawFd { self.pty.master_fd() }

    pub fn feed(&self, bytes: &[u8]) { self.vt.feed(bytes) }

    pub fn encode_key(&self, ev: &input::KeyEvent) -> Vec<u8> { input::encode_key(&self.vt, ev) }

    pub fn encode_mouse_move(&self, row: u16, col: u16, mods: i32) -> Vec<u8> {
        input::encode_mouse_move(&self.vt, row, col, mods)
    }

    pub fn encode_mouse_button(&self, button: i32, pressed: bool, row: u16, col: u16, mods: i32) -> Vec<u8> {
        input::encode_mouse_button(&self.vt, button, pressed, row, col, mods)
    }

    pub fn encode_scroll(&self, dx: f64, dy: f64, row: u16, col: u16, mods: i32) -> Vec<u8> {
        input::encode_scroll(&self.vt, dx, dy, row, col, mods)
    }

    pub fn encode_key_event(&self, ev: &libghostty_sys::ghostty_input_key_s) -> Vec<u8> {
        input::encode_key(&self.vt, ev)
    }

    pub fn link_uri_grid(&self, row: u16, col: u16) -> Option<String> {
        use libghostty_sys as sys;
        let mut len: usize = 0;
        // allocate 2KB buffer for typical URIs
        let mut buf = vec![0i8; 2048];
        let ok = unsafe {
            sys::ghostty_vt_link_uri_grid(
                self.vt.as_raw(),
                row,
                col,
                buf.as_mut_ptr(),
                buf.len(),
                &mut len as *mut usize,
            )
        };
        if !ok || len == 0 { return None; }
        let bytes = unsafe { std::slice::from_raw_parts(buf.as_ptr() as *const u8, len) };
        Some(String::from_utf8_lossy(bytes).to_string())
    }

    pub fn link_span_grid_row(&self, row: u16, col: u16) -> Option<(u16, u16)> {
        use libghostty_sys as sys;
        let mut c0: u16 = 0;
        let mut c1: u16 = 0;
        let ok = unsafe {
            sys::ghostty_vt_link_span_grid_row(
                self.vt.as_raw(),
                row,
                col,
                &mut c0 as *mut u16,
                &mut c1 as *mut u16,
            )
        };
        if ok { Some((c0, c1)) } else { None }
    }

    pub fn read_selection_text_grid(&self, row0: u16, col0: u16, row1: u16, col1: u16) -> Option<String> {
        use libghostty_sys as sys;
        let cols = self.vt.cols() as usize;
        if cols == 0 { return None; }
        let (r0, r1) = if row0 <= row1 { (row0, row1) } else { (row1, row0) };
        let (c0, c1) = if col0 <= col1 { (col0, col1) } else { (col1, col0) };
        let mut out = String::new();
        for row in r0..=r1 {
            let mut cells: Vec<sys::ghostty_vt_cell_s> = vec![unsafe { std::mem::zeroed() }; cols];
            let mut arena = vec![0u8; cols * 8 + 64];
            let mut used: usize = 0;
            unsafe {
                sys::ghostty_vt_viewport_row_cells_into(
                    self.vt.as_raw(),
                    row,
                    cells.as_mut_ptr(),
                    cells.len(),
                    arena.as_mut_ptr() as *mut i8,
                    arena.len(),
                    &mut used as *mut usize,
                );
            }
            let sc0 = if row == r0 { c0 as usize } else { 0usize };
            let sc1 = if row == r1 { c1 as usize } else { cols.saturating_sub(1) };
            for i in sc0..=sc1 {
                let cell = unsafe { cells.get_unchecked(i) };
                if cell.width == 0 || cell.text.is_null() { continue; }
                let len = cell.text_len as usize;
                if len == 0 { continue; }
                let bytes = unsafe { std::slice::from_raw_parts(cell.text as *const u8, len) };
                out.push_str(&String::from_utf8_lossy(bytes));
            }
            if row != r1 { out.push('\n'); }
        }
        Some(out)
    }

    pub fn refresh(&mut self) {
        #[cfg(target_os = "macos")]
        if let Some(r) = &mut self.renderer { r.refresh(); }
    }

    pub fn resize(&self, rows: u16, cols: u16) -> io::Result<()> {
        self.vt.resize(cols, rows);
        self.pty.set_size(rows, cols).map_err(to_io)
    }

    pub fn write(&self, bytes: &[u8]) -> io::Result<usize> {
        let fd = self.pty.master_fd();
        if fd < 0 { return Err(io::Error::from_raw_os_error(fd)); }
        let ret = unsafe { libc::write(fd, bytes.as_ptr() as *const libc::c_void, bytes.len()) };
        if ret < 0 { Err(io::Error::last_os_error()) } else { Ok(ret as usize) }
    }

    #[cfg(target_os = "macos")]
    pub fn set_renderer_focus(&mut self, focused: bool) {
        if let Some(r) = &mut self.renderer { r.set_focus(focused); }
    }

    #[cfg(target_os = "macos")]
    pub fn renderer_cell_size(&self) -> Option<(u32, u32)> {
        self.renderer.as_ref().and_then(|r| r.cell_size())
    }

    #[cfg(target_os = "macos")]
    pub fn set_renderer_visible(&mut self, visible: bool) {
        if let Some(r) = &mut self.renderer { r.set_visible(visible); }
    }

    #[cfg(target_os = "macos")]
    pub fn renderer_preedit(&mut self, text: Option<&str>) {
        if let Some(r) = &mut self.renderer { r.preedit(text); }
    }

    #[cfg(target_os = "macos")]
    pub fn renderer_set_selection(&mut self, rectangle: bool, row0: u16, col0: u16, row1: u16, col1: u16) {
        if let Some(r) = &mut self.renderer { r.set_selection(rectangle, row0, col0, row1, col1); }
    }
    #[cfg(target_os = "macos")]
    pub fn renderer_clear_selection(&mut self) {
        if let Some(r) = &mut self.renderer { r.clear_selection(); }
    }
}

fn to_io(code: i32) -> io::Error { io::Error::from_raw_os_error(if code == 0 { -1 } else { code }) }
