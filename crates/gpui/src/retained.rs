//! Minimal retained view API to support external hosts.
//!
//! This module stores a simple retained tree and can synthesize a GPUI
//! element subtree for painting. It is intended for embedding hosts that
//! want to create/update/reparent views directly without rebuilding
//! elements at the callsite.

use crate::{BoxShadow, Div, Stateful, div, prelude::*, px};
use std::collections::HashMap;
use std::sync::OnceLock;
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
    fn default() -> Self {
        NodeKind::Other(String::new())
    }
}

#[derive(Clone, Debug, Default, PartialEq)]
struct LayoutFrame {
    x: f32,
    y: f32,
    w: f32,
    h: f32,
}

#[derive(Clone, Debug, Default, PartialEq)]
struct EdgeValues<T> {
    top: Option<T>,
    right: Option<T>,
    bottom: Option<T>,
    left: Option<T>,
}

impl<T> EdgeValues<T> {
    fn is_empty(&self) -> bool {
        self.top.is_none() && self.right.is_none() && self.bottom.is_none() && self.left.is_none()
    }

    fn any(&self) -> bool {
        !self.is_empty()
    }
}

#[derive(Clone, Debug, Default, PartialEq)]
struct CornerValues<T> {
    top_left: Option<T>,
    top_right: Option<T>,
    bottom_right: Option<T>,
    bottom_left: Option<T>,
}

impl<T> CornerValues<T> {
    fn is_empty(&self) -> bool {
        self.top_left.is_none()
            && self.top_right.is_none()
            && self.bottom_right.is_none()
            && self.bottom_left.is_none()
    }

    fn any(&self) -> bool {
        !self.is_empty()
    }
}

#[derive(Clone, Debug, Default, PartialEq)]
struct BorderVisual {
    uniform_radius: Option<f32>,
    uniform_width: Option<f32>,
    uniform_color: Option<[u8; 4]>,
    uniform_style: Option<crate::scene::BorderStyle>,
    widths: EdgeValues<f32>,
    colors: EdgeValues<[u8; 4]>,
    styles: EdgeValues<crate::scene::BorderStyle>,
    corner_radii: CornerValues<f32>,
}

impl BorderVisual {
    fn is_effectively_empty(&self) -> bool {
        self.uniform_radius.is_none()
            && self.uniform_width.is_none()
            && self.uniform_color.is_none()
            && self.uniform_style.is_none()
            && !self.widths.any()
            && !self.colors.any()
            && !self.styles.any()
            && !self.corner_radii.any()
    }
}

#[derive(Clone, Debug, Default, PartialEq)]
struct ShadowStyle {
    color: [u8; 4],
    ox: f32,
    oy: f32,
    blur: f32,
}

/// Text truncation modes supported by the retained text style.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum TextEllipsizeMode {
    /// Do not append ellipsis; clip overflowing text.
    Clip,
    /// Truncate the start of the string and append an ellipsis.
    Head,
    /// Truncate the middle of the string and append an ellipsis.
    Middle,
    /// Truncate the tail of the string and append an ellipsis.
    Tail,
}

/// Publicly configurable text style attributes that can be forwarded to the retained host.
#[derive(Clone, Debug, Default)]
pub struct TextStyleProps {
    /// Preferred font size in logical pixels.
    pub font_size: Option<f32>,
    /// Foreground color as RGBA components.
    pub color: Option<[u8; 4]>,
    /// Optional font family name.
    pub font_family: Option<String>,
    /// Desired font weight.
    pub font_weight: Option<crate::FontWeight>,
    /// Horizontal text alignment.
    pub text_align: Option<crate::TextAlign>,
    /// Explicit line height in logical pixels.
    pub line_height: Option<f32>,
    /// Maximum number of visible lines before clamping.
    pub max_lines: Option<usize>,
    /// Truncation behavior when text exceeds available space.
    pub ellipsize_mode: Option<TextEllipsizeMode>,
    /// Whether the text should wrap when exceeding the width.
    pub wrap: Option<bool>,
}

#[derive(Clone, Debug, Default, PartialEq)]
struct TextStyle {
    font_size: Option<f32>,
    color: Option<[u8; 4]>,
    font_family: Option<String>,
    font_weight: Option<crate::FontWeight>,
    text_align: Option<crate::TextAlign>,
    line_height: Option<f32>,
    max_lines: Option<usize>,
    ellipsize_mode: Option<TextEllipsizeMode>,
    wrap: Option<bool>,
}

impl From<TextStyleProps> for TextStyle {
    fn from(props: TextStyleProps) -> Self {
        Self {
            font_size: props.font_size,
            color: props.color,
            font_family: props.font_family,
            font_weight: props.font_weight,
            text_align: props.text_align,
            line_height: props.line_height,
            max_lines: props.max_lines,
            ellipsize_mode: props.ellipsize_mode,
            wrap: props.wrap,
        }
    }
}

impl TextStyle {
    fn is_empty(&self) -> bool {
        self.font_size.is_none()
            && self.color.is_none()
            && self.font_family.is_none()
            && self.font_weight.is_none()
            && self.text_align.is_none()
            && self.line_height.is_none()
            && self.max_lines.is_none()
            && self.ellipsize_mode.is_none()
            && self.wrap.is_none()
    }
}

#[derive(Clone, Debug, Default, PartialEq)]
struct TransformStyle {
    tx: f32,
    ty: f32,
    sx: f32,
    sy: f32,
    rot: f32,
    ox: f32,
    oy: f32,
}

#[derive(Clone, Debug, Default, PartialEq)]
struct ScrollState {
    offset_x: f32,
    offset_y: f32,
    content_width: f32,
    content_height: f32,
}

#[derive(Clone, Debug, Default, PartialEq)]
struct ScrollContentStyle {
    padding: EdgeValues<f32>,
    align_items: Option<crate::AlignItems>,
    justify_content: Option<crate::JustifyContent>,
}

impl ScrollContentStyle {
    fn is_empty(&self) -> bool {
        !self.padding.any() && self.align_items.is_none() && self.justify_content.is_none()
    }
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
    border: Option<BorderVisual>,
    shadow: Option<ShadowStyle>,
    transform: Option<TransformStyle>,
    text: Option<String>,
    text_style: Option<TextStyle>,
    scroll: Option<ScrollState>,
    image_uri: Option<String>,
    clip: bool,
    z_index: Option<i32>,
    content_style: Option<ScrollContentStyle>,
    // Switch component state
    switch_checked: Option<bool>,
    switch_disabled: Option<bool>,
    // TextInput component state
    input_placeholder: Option<String>,
    input_editable: Option<bool>,
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
    if host.root == Some(id) {
        host.root = None;
    }
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
        if let Some(c) = host.nodes.get_mut(&child) {
            c.parent = Some(parent);
        }
    }
}

/// Remove a child from its parent.
pub fn remove_child(parent: u64, child: u64) {
    let host_lock = HOST.get_or_init(|| RwLock::new(RetainedHost::default()));
    let mut host = host_lock.write().unwrap();
    if let Some(p) = host.nodes.get_mut(&parent) {
        p.children.retain(|c| *c != child);
    }
    if let Some(c) = host.nodes.get_mut(&child) {
        c.parent = None;
    }
}

/// Set layout frame for a retained view.
pub fn set_layout(id: u64, x: f32, y: f32, w: f32, h: f32) {
    let host_lock = HOST.get_or_init(|| RwLock::new(RetainedHost::default()));
    let mut host = host_lock.write().unwrap();
    if let Some(n) = host.nodes.get_mut(&id) {
        n.layout = Some(LayoutFrame { x, y, w, h });
    }
}

/// Set background color for a retained view.
pub fn set_background(id: u64, rgba_val: Option<[u8; 4]>) {
    let host_lock = HOST.get_or_init(|| RwLock::new(RetainedHost::default()));
    let mut host = host_lock.write().unwrap();
    if let Some(n) = host.nodes.get_mut(&id) {
        n.bg = rgba_val;
    }
}

/// Set opacity for a retained view.
pub fn set_opacity(id: u64, opacity: Option<f32>) {
    let host_lock = HOST.get_or_init(|| RwLock::new(RetainedHost::default()));
    let mut host = host_lock.write().unwrap();
    if let Some(n) = host.nodes.get_mut(&id) {
        n.opacity = opacity;
    }
}

/// Set border style for a retained view (uniform width/color/radius).
pub fn set_border(id: u64, width: f32, color: Option<[u8; 4]>, radius: f32) {
    let host_lock = HOST.get_or_init(|| RwLock::new(RetainedHost::default()));
    let mut host = host_lock.write().unwrap();
    if let Some(n) = host.nodes.get_mut(&id) {
        let mut border = n.border.take().unwrap_or_default();
        border.uniform_radius = if radius > 0.0 { Some(radius) } else { None };
        border.uniform_color = color;
        border.uniform_width = if width > 0.0 { Some(width) } else { None };
        if border.is_effectively_empty() {
            n.border = None;
        } else {
            n.border = Some(border);
        }
    }
}

/// Set uniform border line style.
pub fn set_border_style(id: u64, style: Option<crate::scene::BorderStyle>) {
    let host_lock = HOST.get_or_init(|| RwLock::new(RetainedHost::default()));
    let mut host = host_lock.write().unwrap();
    if let Some(n) = host.nodes.get_mut(&id) {
        let mut border = n.border.take().unwrap_or_default();
        border.uniform_style = style;
        if border.is_effectively_empty() {
            n.border = None;
        } else {
            n.border = Some(border);
        }
    }
}

/// Set per-edge border widths (top, right, bottom, left).
pub fn set_border_edge_widths(id: u64, widths: [Option<f32>; 4]) {
    let host_lock = HOST.get_or_init(|| RwLock::new(RetainedHost::default()));
    let mut host = host_lock.write().unwrap();
    if let Some(n) = host.nodes.get_mut(&id) {
        let mut border = n.border.take().unwrap_or_default();
        border.widths.top = widths[0];
        border.widths.right = widths[1];
        border.widths.bottom = widths[2];
        border.widths.left = widths[3];
        if border.is_effectively_empty() {
            n.border = None;
        } else {
            n.border = Some(border);
        }
    }
}

/// Set per-edge border colors (top, right, bottom, left).
pub fn set_border_edge_colors(id: u64, colors: [Option<[u8; 4]>; 4]) {
    let host_lock = HOST.get_or_init(|| RwLock::new(RetainedHost::default()));
    let mut host = host_lock.write().unwrap();
    if let Some(n) = host.nodes.get_mut(&id) {
        let mut border = n.border.take().unwrap_or_default();
        border.colors.top = colors[0];
        border.colors.right = colors[1];
        border.colors.bottom = colors[2];
        border.colors.left = colors[3];
        if border.is_effectively_empty() {
            n.border = None;
        } else {
            n.border = Some(border);
        }
    }
}

/// Set per-edge border styles (top, right, bottom, left).
pub fn set_border_edge_styles(id: u64, styles: [Option<crate::scene::BorderStyle>; 4]) {
    let host_lock = HOST.get_or_init(|| RwLock::new(RetainedHost::default()));
    let mut host = host_lock.write().unwrap();
    if let Some(n) = host.nodes.get_mut(&id) {
        let mut border = n.border.take().unwrap_or_default();
        border.styles.top = styles[0];
        border.styles.right = styles[1];
        border.styles.bottom = styles[2];
        border.styles.left = styles[3];
        if border.is_effectively_empty() {
            n.border = None;
        } else {
            n.border = Some(border);
        }
    }
}

/// Set per-corner border radii (top-left, top-right, bottom-right, bottom-left).
pub fn set_border_corner_radii(id: u64, radii: [Option<f32>; 4]) {
    let host_lock = HOST.get_or_init(|| RwLock::new(RetainedHost::default()));
    let mut host = host_lock.write().unwrap();
    if let Some(n) = host.nodes.get_mut(&id) {
        let mut border = n.border.take().unwrap_or_default();
        border.corner_radii.top_left = radii[0];
        border.corner_radii.top_right = radii[1];
        border.corner_radii.bottom_right = radii[2];
        border.corner_radii.bottom_left = radii[3];
        if border.is_effectively_empty() {
            n.border = None;
        } else {
            n.border = Some(border);
        }
    }
}

/// Set shadow for a retained view.
pub fn set_shadow(id: u64, color: [u8; 4], ox: f32, oy: f32, blur: f32) {
    let host_lock = HOST.get_or_init(|| RwLock::new(RetainedHost::default()));
    let mut host = host_lock.write().unwrap();
    if let Some(n) = host.nodes.get_mut(&id) {
        n.shadow = Some(ShadowStyle {
            color,
            ox,
            oy,
            blur,
        });
    }
}

/// Enable or disable overflow clipping for a view.
pub fn set_clip(id: u64, clip: bool) {
    let host_lock = HOST.get_or_init(|| RwLock::new(RetainedHost::default()));
    let mut host = host_lock.write().unwrap();
    if let Some(n) = host.nodes.get_mut(&id) {
        n.clip = clip;
    }
}

/// Set scroll content container style (padding/alignment) for a scroll view.
pub fn set_scroll_content_style(
    id: u64,
    padding: [Option<f32>; 4],
    align_items: Option<crate::AlignItems>,
    justify_content: Option<crate::JustifyContent>,
) {
    let host_lock = HOST.get_or_init(|| RwLock::new(RetainedHost::default()));
    let mut host = host_lock.write().unwrap();
    if let Some(n) = host.nodes.get_mut(&id) {
        let mut style = n.content_style.take().unwrap_or_default();
        style.padding.top = padding[0];
        style.padding.right = padding[1];
        style.padding.bottom = padding[2];
        style.padding.left = padding[3];
        style.align_items = align_items;
        style.justify_content = justify_content;
        if style.is_empty() {
            n.content_style = None;
        } else {
            n.content_style = Some(style);
        }
    }
}

/// Set transform for a retained view.
pub fn set_transform(id: u64, tx: f32, ty: f32, sx: f32, sy: f32, rot: f32, ox: f32, oy: f32) {
    let host_lock = HOST.get_or_init(|| RwLock::new(RetainedHost::default()));
    let mut host = host_lock.write().unwrap();
    if let Some(n) = host.nodes.get_mut(&id) {
        n.transform = Some(TransformStyle {
            tx,
            ty,
            sx,
            sy,
            rot,
            ox,
            oy,
        });
    }
}

/// Set z-index ordering hint for a view.
pub fn set_z_index(id: u64, z_index: Option<i32>) {
    let host_lock = HOST.get_or_init(|| RwLock::new(RetainedHost::default()));
    let mut host = host_lock.write().unwrap();
    if let Some(n) = host.nodes.get_mut(&id) {
        n.z_index = z_index;
    }
}

/// Set text content and styled attributes for a retained view.
pub fn set_text(id: u64, text: Option<String>, style: Option<TextStyleProps>) {
    let host_lock = HOST.get_or_init(|| RwLock::new(RetainedHost::default()));
    let mut host = host_lock.write().unwrap();
    if let Some(n) = host.nodes.get_mut(&id) {
        n.text = text;
        n.text_style = style.map(TextStyle::from).filter(|s| !s.is_empty());
    }
}

/// Set scroll state for a retained scrollable view.
pub fn set_scroll(id: u64, offset_x: f32, offset_y: f32, content_w: f32, content_h: f32) {
    let host_lock = HOST.get_or_init(|| RwLock::new(RetainedHost::default()));
    let mut host = host_lock.write().unwrap();
    if let Some(n) = host.nodes.get_mut(&id) {
        n.scroll = Some(ScrollState {
            offset_x,
            offset_y,
            content_width: content_w,
            content_height: content_h,
        });
    }
}

/// Render the retained view tree as a GPUI element subtree.
pub fn render_root() -> Stateful<Div> {
    let host_lock = HOST.get_or_init(|| RwLock::new(RetainedHost::default()));
    let host = host_lock.read().unwrap();
    match host.root.and_then(|r| host.nodes.get(&r)) {
        Some(root) => render_node(&host, root),
        None => div().id(("rn", 0u64)).child("(no root)"),
    }
}

fn apply_layout<E: Styled>(mut e: E, l: &LayoutFrame) -> E {
    e.absolute()
        .left(px(l.x))
        .top(px(l.y))
        .w(px(l.w))
        .h(px(l.h))
}

fn apply_background<E: Styled>(mut e: E, bg: Option<[u8; 4]>) -> E {
    if let Some(c) = bg {
        e = e.bg(rgba(c));
    }
    e
}

fn apply_border<E: Styled>(mut e: E, b: &BorderVisual) -> E {
    let top_width = b.widths.top.or(b.uniform_width);
    let right_width = b.widths.right.or(b.uniform_width);
    let bottom_width = b.widths.bottom.or(b.uniform_width);
    let left_width = b.widths.left.or(b.uniform_width);

    if let Some(w) = top_width {
        e = e.border_t(px(w));
    }
    if let Some(w) = right_width {
        e = e.border_r(px(w));
    }
    if let Some(w) = bottom_width {
        e = e.border_b(px(w));
    }
    if let Some(w) = left_width {
        e = e.border_l(px(w));
    }

    let color = b
        .uniform_color
        .or(b.colors.top)
        .or(b.colors.right)
        .or(b.colors.bottom)
        .or(b.colors.left);
    if let Some(c) = color {
        e = e.border_color(rgba(c));
    }

    let style = b
        .uniform_style
        .or(b.styles.top)
        .or(b.styles.right)
        .or(b.styles.bottom)
        .or(b.styles.left);
    if let Some(s) = style {
        {
            let style_ref = e.style();
            style_ref.border_style = Some(s);
        }
    }

    if b.uniform_radius.is_some() || b.corner_radii.any() {
        {
            let style_ref = e.style();
            if let Some(r) = b.uniform_radius {
                style_ref.corner_radii.top_left = Some(px(r).into());
                style_ref.corner_radii.top_right = Some(px(r).into());
                style_ref.corner_radii.bottom_right = Some(px(r).into());
                style_ref.corner_radii.bottom_left = Some(px(r).into());
            }
            if let Some(r) = b.corner_radii.top_left {
                style_ref.corner_radii.top_left = Some(px(r).into());
            }
            if let Some(r) = b.corner_radii.top_right {
                style_ref.corner_radii.top_right = Some(px(r).into());
            }
            if let Some(r) = b.corner_radii.bottom_right {
                style_ref.corner_radii.bottom_right = Some(px(r).into());
            }
            if let Some(r) = b.corner_radii.bottom_left {
                style_ref.corner_radii.bottom_left = Some(px(r).into());
            }
        }
    }
    e
}

fn apply_shadow<E: Styled>(mut e: E, s: &ShadowStyle) -> E {
    let color = rgba(s.color).into();
    e.shadow(vec![BoxShadow {
        color,
        offset: crate::Point {
            x: px(s.ox),
            y: px(s.oy),
        },
        blur_radius: px(s.blur),
        spread_radius: px(0.0),
    }])
}

fn apply_transform<E: Styled>(mut e: E, t: &TransformStyle) -> E {
    let has_origin = t.ox != 0.0 || t.oy != 0.0;
    if has_origin {
        e = e.translate(px(-t.ox), px(-t.oy));
    }
    if t.rot != 0.0 {
        e = e.rotate(crate::Radians(t.rot));
    }
    if t.sx != 1.0 || t.sy != 1.0 {
        e = e.scale_xy(t.sx, t.sy);
    }
    if has_origin {
        e = e.translate(px(t.ox), px(t.oy));
    }
    if t.tx != 0.0 || t.ty != 0.0 {
        e = e.translate(px(t.tx), px(t.ty));
    }
    e
}

fn apply_layout_and_style<E: Styled>(mut e: E, n: &NodeView) -> E {
    if let Some(l) = &n.layout {
        if !matches!(n.kind, NodeKind::RootView) {
            e = apply_layout(e, l);
            if let Some(tr) = n.transform.as_ref() {
                e = apply_transform(e, tr);
            }
        }
    }
    e = apply_background(e, n.bg);
    if let Some(op) = n.opacity {
        e = e.opacity(op);
    }
    if let Some(b) = n.border.as_ref() {
        e = apply_border(e, b);
    }
    if let Some(s) = n.shadow.as_ref() {
        e = apply_shadow(e, s);
    }
    e
}

fn apply_scroll<E: Styled>(mut e: E, s: &ScrollState) -> E {
    e = e.w(px(s.content_width));
    e = e.h(px(s.content_height));
    if s.offset_x != 0.0 {
        e = e.left(px(-s.offset_x));
    }
    if s.offset_y != 0.0 {
        e = e.top(px(-s.offset_y));
    }
    e
}

fn apply_scroll_content_style<E: Styled>(mut e: E, style: &ScrollContentStyle) -> E {
    if let Some(p) = style.padding.top {
        e = e.pt(px(p));
    }
    if let Some(p) = style.padding.right {
        e = e.pr(px(p));
    }
    if let Some(p) = style.padding.bottom {
        e = e.pb(px(p));
    }
    if let Some(p) = style.padding.left {
        e = e.pl(px(p));
    }
    if style.align_items.is_some() || style.justify_content.is_some() {
        let style_ref = e.style();
        if let Some(align) = style.align_items {
            style_ref.align_items = Some(align);
        }
        if let Some(justify) = style.justify_content {
            style_ref.justify_content = Some(justify);
        }
    }
    e
}

fn render_node(host: &RetainedHost, node: &NodeView) -> Stateful<Div> {
    match node.kind {
        NodeKind::Paragraph | NodeKind::Text | NodeKind::RawText => render_text(node),
        NodeKind::Image => render_image(node),
        NodeKind::ScrollView => render_scroll(host, node),
        NodeKind::Switch => render_switch(node),
        NodeKind::TextInput => render_textinput(node),
        NodeKind::Pressable => {
            let base = div().cursor_pointer().id(("rn", node.id));
            let base = if node.clip {
                base.overflow_hidden()
            } else {
                base
            };
            finalize_children(host, base, node)
        }
        _ => {
            let mut base = if matches!(node.kind, NodeKind::RootView) {
                div()
                    .relative()
                    .size_full()
                    .bg(rgba([0xFF, 0xFF, 0xFF, 0xFF]))
                    .id(("rn", node.id))
            } else {
                div().id(("rn", node.id))
            };
            if node.clip {
                base = base.overflow_hidden();
            }
            finalize_children(host, base, node)
        }
    }
}

fn render_text(node: &NodeView) -> Stateful<Div> {
    let mut e = div()
        .child(node.text.clone().unwrap_or_default())
        .id(("rn", node.id));
    if node.clip {
        e = e.overflow_hidden();
    }
    e = apply_layout_and_style(e, node);
    if let Some(ts) = node.text_style.as_ref() {
        if let Some(c) = ts.color {
            e = e.text_color(rgba(c));
        }
        if let Some(sz) = ts.font_size {
            e = e.text_size(px(sz));
        }
        if let Some(family) = ts.font_family.as_ref() {
            e = e.font_family(family.clone());
        }
        if let Some(weight) = ts.font_weight {
            e = e.font_weight(weight);
        }
        if let Some(align) = ts.text_align {
            e = e.text_align(align);
        }
        if let Some(line_height) = ts.line_height {
            e = e.line_height(px(line_height));
        }
        if let Some(max_lines) = ts.max_lines {
            e = e.line_clamp(max_lines);
        }
        if let Some(wrap) = ts.wrap {
            if wrap {
                e = e.whitespace_normal();
            } else {
                e = e.whitespace_nowrap();
            }
        }
        match ts.ellipsize_mode {
            Some(TextEllipsizeMode::Tail) => {
                e = e.text_ellipsis();
            }
            Some(TextEllipsizeMode::Head) | Some(TextEllipsizeMode::Middle) => {
                e = e.text_ellipsis();
            }
            Some(TextEllipsizeMode::Clip) | None => {}
        }
    }
    e
}

fn render_image(node: &NodeView) -> Stateful<Div> {
    let source = node.image_uri.clone().unwrap_or_default();
    let mut container = div().id(("rn", node.id));
    if node.clip {
        container = container.overflow_hidden();
    }
    container = apply_layout_and_style(container, node);
    container.child(crate::img(source))
}

fn render_scroll(host: &RetainedHost, node: &NodeView) -> Stateful<Div> {
    let mut viewport = div().overflow_hidden().id(("rn", node.id));
    viewport = apply_layout_and_style(viewport, node).relative();
    let mut content = div().absolute();
    if let Some(s) = node.scroll.as_ref() {
        content = apply_scroll(content, s);
    }
    if let Some(style) = node.content_style.as_ref() {
        content = apply_scroll_content_style(content, style);
    }
    for child in sorted_children(node, host) {
        if let Some(ch) = host.nodes.get(&child) {
            content = content.child(render_node(host, ch));
        }
    }
    viewport.child(content)
}

fn render_switch(node: &NodeView) -> Stateful<Div> {
    let checked = node.switch_checked.unwrap_or(false);
    let disabled = node.switch_disabled.unwrap_or(false);

    // Colors based on state
    let bg_color = if checked {
        // Primary blue when checked
        [37, 99, 235, 255] // rgb(37, 99, 235) - Tailwind blue-600
    } else {
        // Gray when unchecked
        [209, 213, 219, 255] // rgb(209, 213, 219) - Tailwind gray-300
    };

    let toggle_color = if disabled {
        // Dimmed white when disabled
        [255, 255, 255, 89] // 35% opacity = 89/255
    } else {
        // Full white when enabled
        [255, 255, 255, 255]
    };

    let bg_color = if disabled {
        // Dim background when disabled
        [bg_color[0], bg_color[1], bg_color[2], (bg_color[3] as f32 * 0.5) as u8]
    } else {
        bg_color
    };

    // Sizes
    let bg_width = px(36.);
    let bg_height = px(20.);
    let bar_width = px(16.);
    let inset = px(2.);

    // Calculate toggle position
    let max_x = bg_width - bar_width - inset * 2.;
    let toggle_x = if checked { max_x } else { px(0.) };

    let mut container = div().id(("rn", node.id));
    container = apply_layout_and_style(container, node);

    container.child(
        div()
            .flex()
            .flex_row()
            .items_center()
            .child(
                // Switch background bar
                div()
                    .w(bg_width)
                    .h(bg_height)
                    .rounded(bg_height) // Fully rounded
                    .flex()
                    .items_center()
                    .border(inset)
                    .border_color(crate::transparent_black())
                    .bg(rgba(bg_color))
                    .child(
                        // Switch toggle circle
                        div()
                            .rounded(bg_height)
                            .bg(rgba(toggle_color))
                            .size(bar_width)
                            .left(toggle_x),
                    ),
            ),
    )
}

fn render_textinput(node: &NodeView) -> Stateful<Div> {
    let editable = node.input_editable.unwrap_or(true);
    let text = node.text.as_ref();
    let placeholder = node.input_placeholder.as_ref();

    // Determine what to display (owned String to avoid lifetime issues)
    let display_text: String = if let Some(t) = text {
        if t.is_empty() {
            placeholder.map(|p| p.clone()).unwrap_or_default()
        } else {
            t.clone()
        }
    } else {
        placeholder.map(|p| p.clone()).unwrap_or_default()
    };

    let is_placeholder = text.map_or(true, |t| t.is_empty());

    // Placeholder text color (light gray)
    let placeholder_color = [156, 163, 175, 255]; // rgb(156, 163, 175) - Tailwind gray-400

    let mut container = div()
        .id(("rn", node.id))
        .cursor_text()
        .child(display_text);

    // Apply text styling
    if let Some(ts) = node.text_style.as_ref() {
        // Use placeholder color if showing placeholder, otherwise use specified color
        if let Some(c) = ts.color {
            if is_placeholder {
                container = container.text_color(rgba(placeholder_color));
            } else {
                container = container.text_color(rgba(c));
            }
        } else if is_placeholder {
            container = container.text_color(rgba(placeholder_color));
        }

        if let Some(sz) = ts.font_size {
            container = container.text_size(px(sz));
        }
        if let Some(family) = ts.font_family.as_ref() {
            container = container.font_family(family.clone());
        }
        if let Some(weight) = ts.font_weight {
            container = container.font_weight(weight);
        }
        if let Some(align) = ts.text_align {
            container = container.text_align(align);
        }
        if let Some(line_height) = ts.line_height {
            container = container.line_height(px(line_height));
        }
    } else if is_placeholder {
        // Default placeholder color if no text style
        container = container.text_color(rgba(placeholder_color));
    }

    // Apply layout and other styles
    container = apply_layout_and_style(container, node);

    // Dim appearance when not editable
    if !editable {
        container = container.opacity(0.6);
    }

    if node.clip {
        container = container.overflow_hidden();
    }

    container
}

fn sorted_children(node: &NodeView, host: &RetainedHost) -> Vec<u64> {
    let mut ids = node.children.clone();
    ids.sort_by(|a, b| {
        let za = host.nodes.get(a).and_then(|n| n.z_index).unwrap_or(0);
        let zb = host.nodes.get(b).and_then(|n| n.z_index).unwrap_or(0);
        zb.cmp(&za)
    });
    ids
}

fn finalize_children<E>(host: &RetainedHost, base: E, node: &NodeView) -> E
where
    E: Styled + ParentElement,
{
    let mut e = apply_layout_and_style(base, node);
    for child in sorted_children(node, host) {
        if let Some(ch) = host.nodes.get(&child) {
            e = e.child(render_node(host, ch));
        }
    }
    e
}
/// Set image uri for an image view.
pub fn set_image_uri(id: u64, uri: Option<String>) {
    let host_lock = HOST.get_or_init(|| RwLock::new(RetainedHost::default()));
    let mut host = host_lock.write().unwrap();
    if let Some(n) = host.nodes.get_mut(&id) {
        n.image_uri = uri;
    }
}

/// Set the checked state of a switch component.
pub fn set_switch_checked(id: u64, checked: bool) {
    let host_lock = HOST.get_or_init(|| RwLock::new(RetainedHost::default()));
    let mut host = host_lock.write().unwrap();
    if let Some(n) = host.nodes.get_mut(&id) {
        n.switch_checked = Some(checked);
    }
}

/// Set the disabled state of a switch component.
pub fn set_switch_disabled(id: u64, disabled: bool) {
    let host_lock = HOST.get_or_init(|| RwLock::new(RetainedHost::default()));
    let mut host = host_lock.write().unwrap();
    if let Some(n) = host.nodes.get_mut(&id) {
        n.switch_disabled = Some(disabled);
    }
}

/// Set placeholder text for a text input component.
pub fn set_input_placeholder(id: u64, placeholder: Option<String>) {
    let host_lock = HOST.get_or_init(|| RwLock::new(RetainedHost::default()));
    let mut host = host_lock.write().unwrap();
    if let Some(n) = host.nodes.get_mut(&id) {
        n.input_placeholder = placeholder;
    }
}

/// Set whether a text input is editable.
pub fn set_input_editable(id: u64, editable: bool) {
    let host_lock = HOST.get_or_init(|| RwLock::new(RetainedHost::default()));
    let mut host = host_lock.write().unwrap();
    if let Some(n) = host.nodes.get_mut(&id) {
        n.input_editable = Some(editable);
    }
}

/// Whether a retained root exists.
pub fn has_root() -> bool {
    HOST.get()
        .and_then(|h| h.read().ok())
        .and_then(|r| r.root)
        .is_some()
}
