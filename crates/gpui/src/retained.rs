//! Minimal retained view API to support external hosts.
//!
//! This module stores a simple retained tree and can synthesize a GPUI
//! element subtree for painting. It is intended for embedding hosts that
//! want to create/update/reparent views directly without rebuilding
//! elements at the callsite.

use crate::{div, prelude::*, px, BoxShadow, Div, Stateful};
use std::sync::OnceLock;
use std::collections::HashMap;
use std::sync::RwLock;

#[derive(Clone, Debug, PartialEq)]
enum NodeKind {
    RootView,
    View,
    Paragraph,
    Text,
    RawText,
    Image,
    ScrollView,
    Pressable,
    SafeAreaView,
    Switch,
    TextInput,
    Other(String),
}

impl Default for NodeKind {
    fn default() -> Self { NodeKind::Other(String::new()) }
}

#[derive(Clone, Debug, Default, PartialEq)]
struct LayoutFrame {
    x: f32,
    y: f32,
    w: f32,
    h: f32,
}

#[derive(Clone, Debug, Default, PartialEq)]
struct BorderStyle {
    width: f32,
    color: Option<[u8; 4]>,
    radius: f32,
}

#[derive(Clone, Debug, Default, PartialEq)]
struct ShadowStyle {
    color: [u8; 4],
    ox: f32,
    oy: f32,
    blur: f32,
}

#[derive(Clone, Debug, Default, PartialEq)]
struct TextStyle {
    font_size: Option<f32>,
    color: Option<[u8; 4]>,
}

#[derive(Clone, Debug, Default, PartialEq)]
struct TransformStyle {
    tx: f32,
    ty: f32,
    sx: f32,
    sy: f32,
    rot: f32,
}

#[derive(Clone, Debug, Default, PartialEq)]
struct ScrollState {
    offset_x: f32,
    offset_y: f32,
    content_width: f32,
    content_height: f32,
}

#[derive(Clone, Debug, Default)]
struct NodeView {
    id: u64,
    kind: NodeKind,
    parent: Option<u64>,
    children: Vec<u64>,
    layout: Option<LayoutFrame>,
    bg: Option<[u8; 4]>,
    opacity: Option<f32>,
    border: Option<BorderStyle>,
    shadow: Option<ShadowStyle>,
    transform: Option<TransformStyle>,
    text: Option<String>,
    text_style: Option<TextStyle>,
    scroll: Option<ScrollState>,
    image_uri: Option<String>,
}

#[derive(Default)]
pub(crate) struct RetainedHost {
    nodes: HashMap<u64, NodeView>,
    root: Option<u64>,
}

pub(crate) static HOST: OnceLock<RwLock<RetainedHost>> = OnceLock::new();

fn rgba([r, g, b, a]: [u8; 4]) -> crate::Rgba {
    let hex: u32 = ((r as u32) << 24) | ((g as u32) << 16) | ((b as u32) << 8) | (a as u32);
    crate::rgba(hex)
}

/// Begin a retained update batch.
pub fn begin_batch() {}
/// End a retained update batch.
pub fn commit() {}

fn parse_kind(ty: Option<&str>) -> NodeKind {
    match ty {
        Some("RootView") => NodeKind::RootView,
        Some("View") => NodeKind::View,
        Some("Paragraph") => NodeKind::Paragraph,
        Some("Text") => NodeKind::Text,
        Some("RawText") => NodeKind::RawText,
        Some("ScrollView") => NodeKind::ScrollView,
        Some("Image") => NodeKind::Image,
        Some("Pressable") => NodeKind::Pressable,
        Some("SafeAreaView") => NodeKind::SafeAreaView,
        Some("Switch") => NodeKind::Switch,
        Some("TextInput") => NodeKind::TextInput,
        Some(other) => NodeKind::Other(other.to_string()),
        None => NodeKind::Other(String::new()),
    }
}

/// Create a retained view with a given id and type name.
pub fn create_view(id: u64, ty: Option<&str>) {
    let host_lock = HOST.get_or_init(|| RwLock::new(RetainedHost::default()));
    let mut host = host_lock.write().unwrap();
    let mut n = NodeView::default();
    n.id = id;
    n.kind = parse_kind(ty);
    if matches!(n.kind, NodeKind::RootView) {
        host.root = Some(id);
    }
    host.nodes.insert(id, n);
}

/// Delete a retained view.
pub fn delete_view(id: u64) {
    let host_lock = HOST.get_or_init(|| RwLock::new(RetainedHost::default()));
    let mut host = host_lock.write().unwrap();
    if let Some(parent) = host.nodes.get(&id).and_then(|n| n.parent) {
        if let Some(p) = host.nodes.get_mut(&parent) {
            p.children.retain(|c| *c != id);
        }
    }
    host.nodes.remove(&id);
    if host.root == Some(id) { host.root = None; }
}

/// Insert a child into a parent at the given index.
pub fn insert_child(parent: u64, child: u64, index: usize) {
    let host_lock = HOST.get_or_init(|| RwLock::new(RetainedHost::default()));
    let mut host = host_lock.write().unwrap();
    if let Some(p) = host.nodes.get_mut(&parent) {
        let idx = index.min(p.children.len());
        if !p.children.contains(&child) {
            p.children.insert(idx, child);
        }
        if let Some(c) = host.nodes.get_mut(&child) { c.parent = Some(parent); }
    }
}

/// Remove a child from its parent.
pub fn remove_child(parent: u64, child: u64) {
    let host_lock = HOST.get_or_init(|| RwLock::new(RetainedHost::default()));
    let mut host = host_lock.write().unwrap();
    if let Some(p) = host.nodes.get_mut(&parent) { p.children.retain(|c| *c != child); }
    if let Some(c) = host.nodes.get_mut(&child) { c.parent = None; }
}

/// Set layout frame for a retained view.
pub fn set_layout(id: u64, x: f32, y: f32, w: f32, h: f32) {
    let host_lock = HOST.get_or_init(|| RwLock::new(RetainedHost::default()));
    let mut host = host_lock.write().unwrap();
    if let Some(n) = host.nodes.get_mut(&id) { n.layout = Some(LayoutFrame { x, y, w, h }); }
}

/// Set background color for a retained view.
pub fn set_background(id: u64, rgba_val: Option<[u8; 4]>) {
    let host_lock = HOST.get_or_init(|| RwLock::new(RetainedHost::default()));
    let mut host = host_lock.write().unwrap();
    if let Some(n) = host.nodes.get_mut(&id) { n.bg = rgba_val; }
}

/// Set opacity for a retained view.
pub fn set_opacity(id: u64, opacity: Option<f32>) {
    let host_lock = HOST.get_or_init(|| RwLock::new(RetainedHost::default()));
    let mut host = host_lock.write().unwrap();
    if let Some(n) = host.nodes.get_mut(&id) { n.opacity = opacity; }
}

/// Set border style for a retained view.
pub fn set_border(id: u64, width: f32, color: Option<[u8; 4]>, radius: f32) {
    let host_lock = HOST.get_or_init(|| RwLock::new(RetainedHost::default()));
    let mut host = host_lock.write().unwrap();
    if let Some(n) = host.nodes.get_mut(&id) { n.border = Some(BorderStyle { width, color, radius }); }
}

/// Set shadow for a retained view.
pub fn set_shadow(id: u64, color: [u8; 4], ox: f32, oy: f32, blur: f32) {
    let host_lock = HOST.get_or_init(|| RwLock::new(RetainedHost::default()));
    let mut host = host_lock.write().unwrap();
    if let Some(n) = host.nodes.get_mut(&id) { n.shadow = Some(ShadowStyle { color, ox, oy, blur }); }
}

/// Set transform for a retained view.
pub fn set_transform(id: u64, tx: f32, ty: f32, sx: f32, sy: f32, rot: f32) {
    let host_lock = HOST.get_or_init(|| RwLock::new(RetainedHost::default()));
    let mut host = host_lock.write().unwrap();
    if let Some(n) = host.nodes.get_mut(&id) { n.transform = Some(TransformStyle { tx, ty, sx, sy, rot }); }
}

/// Set text and basic text style for a retained view.
pub fn set_text(id: u64, text: Option<String>, color: Option<[u8; 4]>, font_size: Option<f32>) {
    let host_lock = HOST.get_or_init(|| RwLock::new(RetainedHost::default()));
    let mut host = host_lock.write().unwrap();
    if let Some(n) = host.nodes.get_mut(&id) {
        n.text = text;
        n.text_style = Some(TextStyle { font_size, color });
    }
}

/// Set scroll state for a retained scrollable view.
pub fn set_scroll(id: u64, offset_x: f32, offset_y: f32, content_w: f32, content_h: f32) {
    let host_lock = HOST.get_or_init(|| RwLock::new(RetainedHost::default()));
    let mut host = host_lock.write().unwrap();
    if let Some(n) = host.nodes.get_mut(&id) {
        n.scroll = Some(ScrollState { offset_x, offset_y, content_width: content_w, content_height: content_h });
    }
}

/// Render the retained view tree as a GPUI element subtree.
pub fn render_root() -> Stateful<Div> {
    let host_lock = HOST.get_or_init(|| RwLock::new(RetainedHost::default()));
    let host = host_lock.read().unwrap();
    match host.root.and_then(|r| host.nodes.get(&r)) {
        Some(root) => render_node(&host, root),
        None => div().id(("rn", 0u64)).child("(no root)")
    }
}

fn apply_layout<E: Styled>(mut e: E, l: &LayoutFrame) -> E {
    e.absolute().left(px(l.x)).top(px(l.y)).w(px(l.w)).h(px(l.h))
}

fn apply_background<E: Styled>(mut e: E, bg: Option<[u8; 4]>) -> E {
    if let Some(c) = bg { e = e.bg(rgba(c)); }
    e
}

fn apply_border<E: Styled>(mut e: E, b: &BorderStyle) -> E {
    if b.width > 0.0 { e = e.border_t(px(b.width)).border_b(px(b.width)).border_l(px(b.width)).border_r(px(b.width)); }
    if let Some(c) = b.color { e = e.border_color(rgba(c)); }
    if b.radius > 0.0 { e = e.rounded(px(b.radius)); }
    e
}

fn apply_shadow<E: Styled>(mut e: E, s: &ShadowStyle) -> E {
    let color = rgba(s.color).into();
    e.shadow(vec![BoxShadow { color, offset: crate::Point { x: px(s.ox), y: px(s.oy) }, blur_radius: px(s.blur), spread_radius: px(0.0) }])
}

fn apply_transform<E: Styled>(mut e: E, t: &TransformStyle) -> E {
    if t.tx != 0.0 || t.ty != 0.0 { e = e.translate(px(t.tx), px(t.ty)); }
    if t.sx != 1.0 || t.sy != 1.0 { e = e.scale_xy(t.sx, t.sy); }
    if t.rot != 0.0 { e = e.rotate(crate::Radians(t.rot)); }
    e
}

fn apply_layout_and_style<E: Styled>(mut e: E, n: &NodeView) -> E {
    if let Some(l) = &n.layout {
        if !matches!(n.kind, NodeKind::RootView) {
            e = apply_layout(e, l);
            if let Some(tr) = n.transform.as_ref() { e = apply_transform(e, tr); }
        }
    }
    e = apply_background(e, n.bg);
    if let Some(op) = n.opacity { e = e.opacity(op); }
    if let Some(b) = n.border.as_ref() { e = apply_border(e, b); }
    if let Some(s) = n.shadow.as_ref() { e = apply_shadow(e, s); }
    e
}

fn apply_scroll<E: Styled>(mut e: E, s: &ScrollState) -> E {
    e = e.w(px(s.content_width));
    e = e.h(px(s.content_height));
    if s.offset_x != 0.0 { e = e.left(px(-s.offset_x)); }
    if s.offset_y != 0.0 { e = e.top(px(-s.offset_y)); }
    e
}

fn render_node(host: &RetainedHost, node: &NodeView) -> Stateful<Div> {
    match node.kind {
        NodeKind::Paragraph | NodeKind::Text | NodeKind::RawText => render_text(node),
        NodeKind::Image => render_image(node),
        NodeKind::ScrollView => render_scroll(host, node),
        NodeKind::Pressable { .. } => finalize_children(host, div().cursor_pointer().id(("rn", node.id)), node),
        _ => {
            let base = if matches!(node.kind, NodeKind::RootView) {
                div().relative().size_full().bg(rgba([0xFF, 0xFF, 0xFF, 0xFF])).id(("rn", node.id))
            } else { div().id(("rn", node.id)) };
            finalize_children(host, base, node)
        }
    }
}

fn render_text(node: &NodeView) -> Stateful<Div> {
    let mut e = div().child(node.text.clone().unwrap_or_default()).id(("rn", node.id));
    e = apply_layout_and_style(e, node);
    if let Some(ts) = node.text_style.as_ref() {
        if let Some(c) = ts.color { e = e.text_color(rgba(c)); }
        if let Some(sz) = ts.font_size { e = e.text_size(px(sz)); }
    }
    e
}

fn render_image(node: &NodeView) -> Stateful<Div> {
    let source = node.image_uri.clone().unwrap_or_default();
    let mut container = div().id(("rn", node.id));
    container = apply_layout_and_style(container, node);
    container.child(crate::img(source))
}

fn render_scroll(host: &RetainedHost, node: &NodeView) -> Stateful<Div> {
    let mut viewport = div().overflow_hidden().id(("rn", node.id));
    viewport = apply_layout_and_style(viewport, node).relative();
    let mut content = div().absolute();
    if let Some(s) = node.scroll.as_ref() { content = apply_scroll(content, s); }
    for child in &node.children {
        if let Some(ch) = host.nodes.get(child) {
            content = content.child(render_node(host, ch));
        }
    }
    viewport.child(content)
}

fn finalize_children<E>(host: &RetainedHost, base: E, node: &NodeView) -> E
where
    E: Styled + ParentElement,
{
    let mut e = apply_layout_and_style(base, node);
    for child in &node.children {
        if let Some(ch) = host.nodes.get(child) {
            e = e.child(render_node(host, ch));
        }
    }
    e
}
/// Set image uri for an image view.
pub fn set_image_uri(id: u64, uri: Option<String>) {
    let host_lock = HOST.get_or_init(|| RwLock::new(RetainedHost::default()));
    let mut host = host_lock.write().unwrap();
    if let Some(n) = host.nodes.get_mut(&id) { n.image_uri = uri; }
}

/// Whether a retained root exists.
pub fn has_root() -> bool {
    HOST.get()
        .and_then(|h| h.read().ok())
        .and_then(|r| r.root)
        .is_some()
}
