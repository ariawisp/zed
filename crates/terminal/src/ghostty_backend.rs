#![cfg(feature = "ghostty-backend")]

use libghostty::{input, pty::Pty, vt::Session};
#[cfg(target_os = "macos")]
use libghostty::renderer;
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

    pub fn encode_key_event(&self, ev: &input::KeyEvent) -> Vec<u8> { input::encode_key(&self.vt, ev) }

    pub fn link_uri_grid(&self, row: u16, col: u16) -> Option<String> { self.vt.link_uri_grid(row, col) }

    pub fn link_span_grid_row(&self, row: u16, col: u16) -> Option<(u16, u16)> { self.vt.link_span_grid_row(row, col) }

    pub fn read_selection_text_grid(&self, row0: u16, col0: u16, row1: u16, col1: u16) -> Option<String> {
        self.vt.read_selection_text_grid(row0, col0, row1, col1)
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
