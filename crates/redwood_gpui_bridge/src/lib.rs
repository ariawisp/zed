use once_cell::sync::Lazy;
use parking_lot::Mutex;
use smol::channel::Sender;
use std::collections::HashMap;

// =============== Core command model used by the preview ===============

#[derive(Clone, Copy, Debug, uniffi::Enum)]
pub enum RedwoodWidget { Text, Button, Image, Row, Column }

#[derive(Clone, Copy, Debug)]
pub enum NodeKind { Text, Button, Image, Row, Column }

impl From<RedwoodWidget> for NodeKind {
    fn from(w: RedwoodWidget) -> Self {
        match w { RedwoodWidget::Text => Self::Text, RedwoodWidget::Button => Self::Button, RedwoodWidget::Image => Self::Image, RedwoodWidget::Row => Self::Row, RedwoodWidget::Column => Self::Column }
    }
}

pub type Handle = i64;

#[derive(Debug)]
pub enum Cmd {
    Create { handle: Handle, kind: NodeKind },
    Destroy { handle: Handle },
    AppendChild { parent: Handle, child: Handle },
    InsertChild { parent: Handle, index: i32, child: Handle },
    RemoveChild { parent: Handle, child: Handle },
    SetText { handle: Handle, text: String },
    SetButtonText { handle: Handle, text: String },
    SetButtonEnabled { handle: Handle, enabled: bool },
    SetImageUrl { handle: Handle, url: String },
    SetImageFit { handle: Handle, fit: i32 },
    SetImageRadius { handle: Handle, radius: f32 },
}

static UI_SENDER: Lazy<Mutex<Option<Sender<Cmd>>>> = Lazy::new(|| Mutex::new(None));
static PANEL_SENDERS: Lazy<Mutex<HashMap<u64, Sender<Cmd>>>> = Lazy::new(|| Mutex::new(HashMap::new()));

pub fn register_ui_sender(tx: Sender<Cmd>) { *UI_SENDER.lock() = Some(tx); }
pub fn register_panel_sender(panel_id: u64, tx: Sender<Cmd>) { PANEL_SENDERS.lock().insert(panel_id, tx); }
pub fn unregister_panel_sender(panel_id: u64) { PANEL_SENDERS.lock().remove(&panel_id); }

pub fn emit(cmd: Cmd) {
    if let Some(tx) = UI_SENDER.lock().as_ref() { let _ = tx.try_send(cmd); }
}

pub fn emit_to(panel_id: u64, cmd: Cmd) {
    if let Some(tx) = PANEL_SENDERS.lock().get(&panel_id) { let _ = tx.try_send(cmd.clone()); return; }
    emit(cmd);
}

// =============== UniFFI-exposed typed frame API ===============

#[derive(uniffi::Record, Clone, Copy)]
pub struct RedwoodChangeCreate { pub id: u64, pub widget: RedwoodWidget }
#[derive(uniffi::Record, Clone, Copy)]
pub struct RedwoodChangeDestroy { pub id: u64 }
#[derive(uniffi::Record, Clone, Copy)]
pub struct RedwoodChangeAppendChild { pub parent: u64, pub child: u64 }
#[derive(uniffi::Record, Clone, Copy)]
pub struct RedwoodChangeInsertChild { pub parent: u64, pub index: u32, pub child: u64 }
#[derive(uniffi::Record, Clone, Copy)]
pub struct RedwoodChangeRemoveChild { pub parent: u64, pub child: u64 }
#[derive(uniffi::Record, Clone, Copy)]
pub struct RedwoodChangeSetText { pub id: u64, pub text: u32 }
#[derive(uniffi::Record, Clone, Copy)]
pub struct RedwoodChangeSetEnabled { pub id: u64, pub enabled: bool }
#[derive(uniffi::Record, Clone, Copy)]
pub struct RedwoodChangeSetImageUrl { pub id: u64, pub url: u32 }

#[derive(uniffi::Enum, Copy, Clone)]
pub enum RedwoodChangeKind { Create, Destroy, AppendChild, InsertChild, RemoveChild, SetText, SetEnabled, SetImageUrl }

#[derive(uniffi::Record, Clone)]
pub struct RedwoodChangeRec {
    pub kind: RedwoodChangeKind,
    pub create: Option<RedwoodChangeCreate>,
    pub destroy: Option<RedwoodChangeDestroy>,
    pub append_child: Option<RedwoodChangeAppendChild>,
    pub insert_child: Option<RedwoodChangeInsertChild>,
    pub remove_child: Option<RedwoodChangeRemoveChild>,
    pub set_text: Option<RedwoodChangeSetText>,
    pub set_enabled: Option<RedwoodChangeSetEnabled>,
    pub set_image_url: Option<RedwoodChangeSetImageUrl>,
}

#[derive(uniffi::Record, Clone)]
pub struct RedwoodFrameRec {
    pub strings: Vec<String>,
    pub changes: Vec<RedwoodChangeRec>,
}

#[uniffi::export]
pub fn redwood_create_view(_view_id: u64) { /* no-op in preview bridge */ }

#[uniffi::export]
pub fn redwood_apply(_view_id: u64, frame: RedwoodFrameRec) {
    let strings = frame.strings;
    let mut str_of = |id: u32| -> String { strings.get(id as usize).cloned().unwrap_or_default() };
    for ch in frame.changes.into_iter() {
        match ch.kind {
            RedwoodChangeKind::Create => {
                if let Some(r) = ch.create { emit(Cmd::Create { handle: r.id as i64, kind: r.widget.into() }); }
            }
            RedwoodChangeKind::Destroy => {
                if let Some(r) = ch.destroy { emit(Cmd::Destroy { handle: r.id as i64 }); }
            }
            RedwoodChangeKind::AppendChild => {
                if let Some(r) = ch.append_child { emit(Cmd::AppendChild { parent: r.parent as i64, child: r.child as i64 }); }
            }
            RedwoodChangeKind::InsertChild => {
                if let Some(r) = ch.insert_child { emit(Cmd::InsertChild { parent: r.parent as i64, index: r.index as i32, child: r.child as i64 }); }
            }
            RedwoodChangeKind::RemoveChild => {
                if let Some(r) = ch.remove_child { emit(Cmd::RemoveChild { parent: r.parent as i64, child: r.child as i64 }); }
            }
            RedwoodChangeKind::SetText => {
                if let Some(r) = ch.set_text { emit(Cmd::SetText { handle: r.id as i64, text: str_of(r.text) }); }
            }
            RedwoodChangeKind::SetEnabled => {
                if let Some(r) = ch.set_enabled { emit(Cmd::SetButtonEnabled { handle: r.id as i64, enabled: r.enabled }); }
            }
            RedwoodChangeKind::SetImageUrl => {
                if let Some(r) = ch.set_image_url { emit(Cmd::SetImageUrl { handle: r.id as i64, url: str_of(r.url) }); }
            }
        }
    }
}

#[uniffi::export]
pub fn redwood_click(_view_id: u64, _handle: u64) { /* no-op: input not wired here */ }

/// Apply to a specific panel if registered; otherwise fallback to global sender.
#[uniffi::export]
pub fn redwood_apply_to(panel_id: u64, frame: RedwoodFrameRec) {
    let strings = frame.strings;
    let mut str_of = |id: u32| -> String { strings.get(id as usize).cloned().unwrap_or_default() };
    for ch in frame.changes.into_iter() {
        match ch.kind {
            RedwoodChangeKind::Create => if let Some(r) = ch.create { emit_to(panel_id, Cmd::Create { handle: r.id as i64, kind: r.widget.into() }); },
            RedwoodChangeKind::Destroy => if let Some(r) = ch.destroy { emit_to(panel_id, Cmd::Destroy { handle: r.id as i64 }); },
            RedwoodChangeKind::AppendChild => if let Some(r) = ch.append_child { emit_to(panel_id, Cmd::AppendChild { parent: r.parent as i64, child: r.child as i64 }); },
            RedwoodChangeKind::InsertChild => if let Some(r) = ch.insert_child { emit_to(panel_id, Cmd::InsertChild { parent: r.parent as i64, index: r.index as i32, child: r.child as i64 }); },
            RedwoodChangeKind::RemoveChild => if let Some(r) = ch.remove_child { emit_to(panel_id, Cmd::RemoveChild { parent: r.parent as i64, child: r.child as i64 }); },
            RedwoodChangeKind::SetText => if let Some(r) = ch.set_text { emit_to(panel_id, Cmd::SetText { handle: r.id as i64, text: str_of(r.text) }); },
            RedwoodChangeKind::SetEnabled => if let Some(r) = ch.set_enabled { emit_to(panel_id, Cmd::SetButtonEnabled { handle: r.id as i64, enabled: r.enabled }); },
            RedwoodChangeKind::SetImageUrl => if let Some(r) = ch.set_image_url { emit_to(panel_id, Cmd::SetImageUrl { handle: r.id as i64, url: str_of(r.url) }); },
        }
    }
}

uniffi::setup_scaffolding!();
