use crate::wasm_host::wit::since_v1_0_0::ui as wit_ui;
use gpui::{div, img, Context as GContext, Div, IntoElement, Render, SharedString, Window};
use log::{info, warn};
use once_cell::sync::Lazy;
use parking_lot::Mutex;
use serde_json::{self, Value};
use smol::channel::{unbounded, Receiver, Sender, TrySendError};
use std::collections::{HashMap, HashSet, VecDeque};
use ui::prelude::*;

// NOTE: This module is still a handcrafted preview renderer. Once the generated Redwood host
// bindings land, this file should be replaced with the codegen-produced widget factories and
// modifier translators, leaving only the event queue plumbing in place.
//
// To ease that transition we maintain a thin façade (`GeneratedHostAdapter`) whose methods mirror
// what the generated code will eventually expose. When the FIR-based pipeline lands we can swap the
// implementation without touching the surrounding channel/queue infrastructure.

const SCHEMA_STRIDE: u32 = 1_000_000;
const BASIC_SCHEMA_INDEX: u32 = 0;
const LAYOUT_SCHEMA_INDEX: u32 = 1;

const WIDGET_TEXT_INPUT: u32 = 1;
const WIDGET_TEXT: u32 = 2;
const WIDGET_IMAGE: u32 = 3;
const WIDGET_BUTTON: u32 = 4;

const LAYOUT_ROW: u32 = LAYOUT_SCHEMA_INDEX * SCHEMA_STRIDE + 1;
const LAYOUT_COLUMN: u32 = LAYOUT_SCHEMA_INDEX * SCHEMA_STRIDE + 2;
const LAYOUT_SPACER: u32 = LAYOUT_SCHEMA_INDEX * SCHEMA_STRIDE + 3;
const LAYOUT_BOX: u32 = LAYOUT_SCHEMA_INDEX * SCHEMA_STRIDE + 4;

const CHILDREN_TAG_DEFAULT: u32 = 1;

const PROP_TEXT: u32 = 1;
const PROP_BUTTON_ENABLED: u32 = 2;
const PROP_IMAGE_URL: u32 = 1;

// Redwood UI Basic event tags.
const EVENT_TEXT_INPUT_ON_CHANGE: u32 = 3;
const EVENT_IMAGE_ON_CLICK: u32 = 2;
const EVENT_BUTTON_ON_CLICK: u32 = 3;
const EVENT_TOGGLE_ON_CHANGE: u32 = 4;

const ROW_COL_PROP_WIDTH: u32 = 1;
const ROW_COL_PROP_HEIGHT: u32 = 2;
const ROW_COL_PROP_MARGIN: u32 = 3;
const ROW_COL_PROP_OVERFLOW: u32 = 4;
const ROW_COL_PROP_MAIN_ALIGN: u32 = 5;
const ROW_COL_PROP_CROSS_ALIGN: u32 = 6;

const SPACER_PROP_WIDTH: u32 = 1;
const SPACER_PROP_HEIGHT: u32 = 2;

const MOD_GROW: i32 = 1;
const MOD_SHRINK: i32 = 2;
const MOD_MARGIN: i32 = 3;
const MOD_HORIZONTAL_ALIGNMENT: i32 = 4;
const MOD_VERTICAL_ALIGNMENT: i32 = 5;
const MOD_WIDTH: i32 = 6;
const MOD_HEIGHT: i32 = 7;
const MOD_SIZE: i32 = 8;
const MOD_FLEX: i32 = 9;

#[derive(Clone, Debug)]
pub struct RedwoodFrameMessage {
    pub changes: Vec<RedwoodChange>,
}

#[derive(Clone, Debug)]
pub enum RedwoodChange {
    Create {
        id: u64,
        widget: u32,
    },
    Destroy {
        id: u64,
    },
    AddChild {
        parent: u64,
        slot: u32,
        child: u64,
        index: u32,
    },
    MoveChild {
        parent: u64,
        slot: u32,
        from_index: u32,
        to_index: u32,
        count: u32,
    },
    RemoveChild {
        parent: u64,
        slot: u32,
        index: u32,
        count: u32,
        detach: bool,
    },
    SetProperty {
        id: u64,
        widget: u32,
        property: u32,
        value_json: String,
    },
    SetModifiers {
        id: u64,
        elements: Vec<ModifierElement>,
    },
}

#[derive(Clone, Debug)]
pub struct ModifierElement {
    pub tag: i32,
    pub value_json: Option<String>,
}

impl From<wit_ui::RedwoodChange> for RedwoodChange {
    fn from(change: wit_ui::RedwoodChange) -> Self {
        match change {
            wit_ui::RedwoodChange::Create(payload) => RedwoodChange::Create {
                id: payload.id,
                widget: payload.widget,
            },
            wit_ui::RedwoodChange::Destroy(payload) => RedwoodChange::Destroy { id: payload.id },
            wit_ui::RedwoodChange::AddChild(payload) => RedwoodChange::AddChild {
                parent: payload.parent,
                slot: payload.slot,
                child: payload.child,
                index: payload.index,
            },
            wit_ui::RedwoodChange::MoveChild(payload) => RedwoodChange::MoveChild {
                parent: payload.parent,
                slot: payload.slot,
                from_index: payload.from_index,
                to_index: payload.to_index,
                count: payload.count,
            },
            wit_ui::RedwoodChange::RemoveChild(payload) => RedwoodChange::RemoveChild {
                parent: payload.parent,
                slot: payload.slot,
                index: payload.index,
                count: payload.count,
                detach: payload.detach,
            },
            wit_ui::RedwoodChange::SetProperty(payload) => RedwoodChange::SetProperty {
                id: payload.id,
                widget: payload.widget,
                property: payload.property,
                value_json: payload.value_json,
            },
            wit_ui::RedwoodChange::SetModifiers(payload) => RedwoodChange::SetModifiers {
                id: payload.id,
                elements: payload
                    .elements
                    .into_iter()
                    .map(|element| ModifierElement {
                        tag: element.tag,
                        value_json: element.value_json,
                    })
                    .collect(),
            },
        }
    }
}

impl From<wit_ui::RedwoodFrame> for RedwoodFrameMessage {
    fn from(frame: wit_ui::RedwoodFrame) -> Self {
        RedwoodFrameMessage {
            changes: frame.changes.into_iter().map(Into::into).collect(),
        }
    }
}

static PANEL_SENDERS: Lazy<Mutex<HashMap<u64, Sender<RedwoodFrameMessage>>>> =
    Lazy::new(|| Mutex::new(HashMap::new()));
static PENDING_FRAMES: Lazy<Mutex<HashMap<u64, VecDeque<RedwoodFrameMessage>>>> =
    Lazy::new(|| Mutex::new(HashMap::new()));
static EVENT_QUEUES: Lazy<Mutex<HashMap<u64, Vec<wit_ui::RedwoodEvent>>>> =
    Lazy::new(|| Mutex::new(HashMap::new()));

pub fn dispatch_frame(panel_id: u64, frame: impl Into<RedwoodFrameMessage>) {
    let frame = frame.into();
    if let Some(sender) = PANEL_SENDERS.lock().get(&panel_id).cloned() {
        if let Err(err) = sender.try_send(frame) {
            match err {
                TrySendError::Full(frame) | TrySendError::Closed(frame) => {
                    warn!(
                        "redwood-panel: queueing frame for panel {} (receiver not ready)",
                        panel_id
                    );
                    PENDING_FRAMES
                        .lock()
                        .entry(panel_id)
                        .or_default()
                        .push_back(frame);
                }
            }
        }
    } else {
        PENDING_FRAMES
            .lock()
            .entry(panel_id)
            .or_default()
            .push_back(frame);
    }
}

fn register_panel_channel(panel_id: u64, sender: Sender<RedwoodFrameMessage>) {
    PANEL_SENDERS.lock().insert(panel_id, sender.clone());

    if let Some(mut pending) = PENDING_FRAMES.lock().remove(&panel_id) {
        while let Some(frame) = pending.pop_front() {
            if sender.try_send(frame).is_err() {
                PENDING_FRAMES
                    .lock()
                    .entry(panel_id)
                    .or_default()
                    .extend(pending);
                break;
            }
        }
    }
}

fn unregister_panel_channel(panel_id: u64) {
    PANEL_SENDERS.lock().remove(&panel_id);
    PENDING_FRAMES.lock().remove(&panel_id);
    EVENT_QUEUES.lock().remove(&panel_id);
}

#[derive(Clone, Debug)]
struct Modifier {
    tag: i32,
    value: Option<Value>,
}

#[derive(Clone, Debug)]
struct RedwoodNode {
    widget_tag: u32,
    properties: HashMap<u32, Value>,
    modifiers: Vec<Modifier>,
}

impl RedwoodNode {
    fn new(widget_tag: u32) -> Self {
        Self {
            widget_tag,
            properties: HashMap::new(),
            modifiers: Vec::new(),
        }
    }
}

pub struct RedwoodPanel {
    panel_id: u64,
    nodes: HashMap<u64, RedwoodNode>,
    children: HashMap<u64, Vec<u64>>,
    roots: Vec<u64>,
    rx: Receiver<RedwoodFrameMessage>,
}

impl RedwoodPanel {
    pub fn new(panel_id: u64, window: &mut Window, _cx: &mut GContext<Self>) -> Self {
        let (tx, rx) = unbounded::<RedwoodFrameMessage>();
        register_panel_channel(panel_id, tx);
        super::register_panel_window(panel_id, window.window_handle());
        Self {
            panel_id,
            nodes: HashMap::new(),
            children: HashMap::new(),
            roots: Vec::new(),
            rx,
        }
    }

    fn apply_frame(&mut self, frame: RedwoodFrameMessage) {
        for change in frame.changes {
            self.apply_change(change);
        }
        self.roots.clear();
    }

    fn apply_change(&mut self, change: RedwoodChange) {
        match change {
            RedwoodChange::Create { id, widget } => {
                self.nodes.insert(id, RedwoodNode::new(widget));
                self.children.entry(id).or_default();
            }
            RedwoodChange::Destroy { id } => {
                self.nodes.remove(&id);
                self.children.remove(&id);
                for children in self.children.values_mut() {
                    children.retain(|child| *child != id);
                }
            }
            RedwoodChange::AddChild {
                parent,
                slot,
                child,
                index,
            } => {
                if slot != CHILDREN_TAG_DEFAULT {
                    warn!(
                        "redwood-panel: unsupported children slot {} on {}",
                        slot, parent
                    );
                    return;
                }
                let children = self.children.entry(parent).or_default();
                let index = (index as usize).min(children.len());
                if let Some(position) = self.roots.iter().position(|&root| root == child) {
                    self.roots.remove(position);
                }
                if !children.contains(&child) {
                    children.insert(index, child);
                }
            }
            RedwoodChange::MoveChild {
                parent,
                slot,
                from_index,
                to_index,
                count,
            } => {
                if slot != CHILDREN_TAG_DEFAULT {
                    warn!(
                        "redwood-panel: unsupported move slot {} on {}",
                        slot, parent
                    );
                    return;
                }
                if let Some(children) = self.children.get_mut(&parent) {
                    let len = children.len();
                    if len == 0 || count == 0 {
                        return;
                    }
                    let from = (from_index as usize).min(len - 1);
                    let count = (count as usize).min(len - from);
                    let to = (to_index as usize).min(len - count);
                    let mut segment: Vec<u64> = children
                        .splice(from..from + count, std::iter::empty())
                        .collect();
                    for (offset, child) in segment.drain(..).enumerate() {
                        children.insert(to + offset, child);
                    }
                }
            }
            RedwoodChange::RemoveChild {
                parent,
                slot,
                index,
                count,
                detach: _,
            } => {
                if slot != CHILDREN_TAG_DEFAULT {
                    warn!(
                        "redwood-panel: unsupported remove slot {} on {}",
                        slot, parent
                    );
                    return;
                }
                if let Some(children) = self.children.get_mut(&parent) {
                    let len = children.len();
                    if len == 0 {
                        return;
                    }
                    let start = (index as usize).min(len - 1);
                    let count = (count as usize).min(len - start);
                    children.drain(start..start + count);
                }
            }
            RedwoodChange::SetProperty {
                id,
                property,
                value_json,
                ..
            } => {
                if let Some(node) = self.nodes.get_mut(&id) {
                    match serde_json::from_str::<Value>(&value_json) {
                        Ok(value) => {
                            node.properties.insert(property, value);
                        }
                        Err(error) => {
                            warn!(
                                "redwood-panel: failed to parse property {} for widget {}: {error}",
                                property, id
                            );
                        }
                    }
                }
            }
            RedwoodChange::SetModifiers { id, elements } => {
                if let Some(node) = self.nodes.get_mut(&id) {
                    let mut modifiers = Vec::with_capacity(elements.len());
                    for element in elements {
                        let value = match element.value_json {
                            Some(json) => match serde_json::from_str::<Value>(&json) {
                                Ok(value) => Some(value),
                                Err(error) => {
                                    warn!(
                                        "redwood-panel: failed to parse modifier {} for widget {}: {error}",
                                        element.tag, id
                                    );
                                    None
                                }
                            },
                            None => None,
                        };
                        modifiers.push(Modifier {
                            tag: element.tag,
                            value,
                        });
                    }
                    node.modifiers = modifiers;
                }
            }
        }
    }
}

impl Drop for RedwoodPanel {
    fn drop(&mut self) {
        unregister_panel_channel(self.panel_id);
    }
}

impl Render for RedwoodPanel {
    fn render(&mut self, _window: &mut Window, cx: &mut GContext<Self>) -> impl IntoElement {
        while let Ok(frame) = self.rx.try_recv() {
            GeneratedHostAdapter::apply_frame(self, frame);
        }

        let mut root = v_flex().size_full().overflow_hidden();
        if let Some(children) = self.children.get(&0) {
            for &child in children {
                root = root.child(self.render_node(child, cx));
            }
        } else {
            if self.roots.is_empty() {
                let mut has_parent = HashSet::new();
                for children in self.children.values() {
                    for &child in children {
                        has_parent.insert(child);
                    }
                }
                self.roots = self
                    .nodes
                    .keys()
                    .copied()
                    .filter(|id| !has_parent.contains(id))
                    .collect();
            }
            for &child in &self.roots {
                root = root.child(self.render_node(child, cx));
            }
        }
        root
    }
}

pub fn queue_event(panel_id: u64, event: wit_ui::RedwoodEvent) {
    EVENT_QUEUES
        .lock()
        .entry(panel_id)
        .or_default()
        .push(event);
}

pub fn drain_events(panel_id: u64) -> Vec<wit_ui::RedwoodEvent> {
    EVENT_QUEUES.lock().remove(&panel_id).unwrap_or_default()
}

fn enqueue_event(
    panel_id: u64,
    node_id: u64,
    widget_tag: u32,
    event_tag: u32,
    args_json: Vec<String>,
) {
    queue_event(
        panel_id,
        wit_ui::RedwoodEvent {
            id: node_id,
            widget: widget_tag,
            event: event_tag,
            args_json,
        },
    );
}

pub fn emit_button_click(panel_id: u64, node_id: u64) {
    enqueue_event(panel_id, node_id, WIDGET_BUTTON, EVENT_BUTTON_ON_CLICK, Vec::new());
}

pub fn emit_toggle_change(panel_id: u64, node_id: u64, checked: bool) {
    enqueue_event(
        panel_id,
        node_id,
        WIDGET_BUTTON, // Placeholder; replace with toggle widget tag once mapped.
        EVENT_TOGGLE_ON_CHANGE,
        vec![checked.to_string()],
    );
}

pub fn emit_text_change(panel_id: u64, node_id: u64, value: &str) {
    enqueue_event(
        panel_id,
        node_id,
        WIDGET_TEXT_INPUT,
        EVENT_TEXT_INPUT_ON_CHANGE,
        vec![serde_json::to_string(value).unwrap_or_else(|_| "\"\"".into())],
    );
}

pub fn emit_menu_select(panel_id: u64, node_id: u64, item_id: &str) {
    enqueue_event(
        panel_id,
        node_id,
        WIDGET_BUTTON, // Placeholder until menu widget tags are wired.
        EVENT_IMAGE_ON_CLICK,
        vec![serde_json::to_string(item_id).unwrap_or_else(|_| format!("\"{item_id}\""))],
    );
}

/// Temporary façade that mimics the API surface we expect from the generated Redwood GPUI host
/// adapter. The current implementation just logs the mapping and updates the handcrafted tree;
/// once codegen lands, replace this struct with the generated host factory.
pub struct GeneratedHostAdapter;

impl GeneratedHostAdapter {
    pub fn apply_frame(panel: &mut RedwoodPanel, frame: RedwoodFrameMessage) {
        info!(
            "redwood-panel: applying frame with {} changes (preview shim)",
            frame.changes.len()
        );
        panel.apply_frame(frame);
    }
}

impl RedwoodPanel {
    fn render_node(&self, node_id: u64, cx: &mut GContext<Self>) -> AnyElement {
        GeneratedHostAdapter::render_widget(self, node_id, cx)
    }

    fn render_text(&self, node: &RedwoodNode) -> Label {
        let text = node
            .properties
            .get(&PROP_TEXT)
            .and_then(Value::as_str)
            .unwrap_or_default();

        Label::new(text)
            .size(LabelSize::Small)
            .line_height_style(LineHeightStyle::UiLabel)
    }

    fn render_button(&self, node_id: u64, node: &RedwoodNode) -> Button {
        let label = node
            .properties
            .get(&PROP_TEXT)
            .and_then(Value::as_str)
            .unwrap_or("Button");
        let enabled = node
            .properties
            .get(&PROP_BUTTON_ENABLED)
            .and_then(Value::as_bool)
            .unwrap_or(true);

        let panel_id = self.panel_id;
        Button::new(ElementId::Integer(node_id), SharedString::from(label))
            .style(ButtonStyle::Filled)
            .disabled(!enabled)
            .on_click(move |_, _, _| {
                emit_button_click(panel_id, node_id);
            })
    }

    fn render_image(&self, node: &RedwoodNode) -> AnyElement {
        let src = node
            .properties
            .get(&PROP_IMAGE_URL)
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string();
        img(src).into_any_element()
    }

    fn render_text_input(&self, node: &RedwoodNode) -> AnyElement {
        let hint = node
            .properties
            .get(&PROP_TEXT)
            .and_then(Value::as_str)
            .unwrap_or("Text Input");
        div()
            .flex()
            .flex_row()
            .items_center()
            .px(px(8.0))
            .border_1()
            .rounded(px(6.0))
            .child(Label::new(hint))
            .into_any_element()
    }

    fn render_row(&self, node_id: u64, node: &RedwoodNode, cx: &mut GContext<Self>) -> AnyElement {
        let mut container = h_flex().gap(px(8.0));
        container = self.apply_container_constraints(container, node);
        container = self.apply_container_alignment(container, node, Orientation::Horizontal);
        container = self.apply_container_margin(container, node);
        container = self.apply_container_overflow(container, node, Orientation::Horizontal);

        if let Some(children) = self.children.get(&node_id) {
            for &child in children {
                let element = self.render_node(child, cx);
                container = container.child(self.apply_child_modifiers(child, element));
            }
        }

        container.into_any_element()
    }

    fn render_column(
        &self,
        node_id: u64,
        node: &RedwoodNode,
        cx: &mut GContext<Self>,
    ) -> AnyElement {
        let mut container = v_flex().gap(px(8.0));
        container = self.apply_container_constraints(container, node);
        container = self.apply_container_alignment(container, node, Orientation::Vertical);
        container = self.apply_container_margin(container, node);
        container = self.apply_container_overflow(container, node, Orientation::Vertical);

        if let Some(children) = self.children.get(&node_id) {
            for &child in children {
                let element = self.render_node(child, cx);
                container = container.child(self.apply_child_modifiers(child, element));
            }
        }

        container.into_any_element()
    }

    fn render_box(&self, node_id: u64, node: &RedwoodNode, cx: &mut GContext<Self>) -> AnyElement {
        let mut container = div().relative().flex().flex_col();
        container = self.apply_container_constraints(container, node);
        container = self.apply_container_margin(container, node);

        if let Some(children) = self.children.get(&node_id) {
            for &child in children {
                let element = self.render_node(child, cx);
                container = container.child(self.apply_child_modifiers(child, element));
            }
        }

        container.into_any_element()
    }

    fn render_spacer(&self, node: &RedwoodNode) -> Div {
        let width = node
            .properties
            .get(&SPACER_PROP_WIDTH)
            .and_then(Value::as_f64)
            .map(|value| px(value as f32));
        let height = node
            .properties
            .get(&SPACER_PROP_HEIGHT)
            .and_then(Value::as_f64)
            .map(|value| px(value as f32));

        let mut spacer = div().flex_none();
        if let Some(width) = width {
            spacer = spacer.w(width);
        }
        if let Some(height) = height {
            spacer = spacer.h(height);
        }
        spacer
    }

    fn apply_container_constraints(&self, mut element: Div, node: &RedwoodNode) -> Div {
        if let Some(width) = node
            .properties
            .get(&ROW_COL_PROP_WIDTH)
            .and_then(Value::as_i64)
        {
            if width == 1 {
                element = element.w_full();
            }
        }
        if let Some(height) = node
            .properties
            .get(&ROW_COL_PROP_HEIGHT)
            .and_then(Value::as_i64)
        {
            if height == 1 {
                element = element.h_full();
            }
        }
        element
    }

    fn apply_container_alignment(
        &self,
        mut element: Div,
        node: &RedwoodNode,
        orientation: Orientation,
    ) -> Div {
        if let Some(main) = node
            .properties
            .get(&ROW_COL_PROP_MAIN_ALIGN)
            .and_then(Value::as_i64)
        {
            element = match (orientation, main) {
                (_, 1) => element.justify_center(),
                (_, 2) => element.justify_end(),
                (_, 3) => element.justify_between(),
                (_, 4) => element.justify_between(),
                (_, 5) => element.justify_between(),
                _ => element.justify_start(),
            };
        }
        if let Some(cross) = node
            .properties
            .get(&ROW_COL_PROP_CROSS_ALIGN)
            .and_then(Value::as_i64)
        {
            element = match cross {
                1 => element.items_center(),
                2 => element.items_end(),
                _ => element.items_start(),
            };
        }
        element
    }

    fn apply_container_margin(&self, mut element: Div, node: &RedwoodNode) -> Div {
        if let Some(margin) = node.properties.get(&ROW_COL_PROP_MARGIN) {
            if let Some(edge) = parse_margin(margin) {
                element = element
                    .ml(px(edge.start))
                    .mr(px(edge.end))
                    .mt(px(edge.top))
                    .mb(px(edge.bottom));
            }
        }
        element
    }

    fn apply_container_overflow(
        &self,
        mut element: Div,
        node: &RedwoodNode,
        orientation: Orientation,
    ) -> Div {
        if let Some(overflow) = node
            .properties
            .get(&ROW_COL_PROP_OVERFLOW)
            .and_then(Value::as_i64)
        {
            if overflow == 1 {
                element = match orientation {
                    Orientation::Horizontal => element.overflow_x_scroll(),
                    Orientation::Vertical => element.overflow_y_scroll(),
                };
            }
        }
        element
    }

    fn apply_child_modifiers(&self, node_id: u64, element: AnyElement) -> AnyElement {
        let node = match self.nodes.get(&node_id) {
            Some(node) => node,
            None => return element,
        };

        let mut wrapper = div().child(element);

        for modifier in &node.modifiers {
            match modifier.tag {
                MOD_GROW | MOD_FLEX => {
                    wrapper = wrapper.flex_grow();
                }
                MOD_SHRINK => {
                    wrapper = wrapper.flex_shrink();
                }
                MOD_MARGIN => {
                    if let Some(value) = modifier.value.as_ref() {
                        if let Some(edge) = parse_margin(value) {
                            wrapper = wrapper
                                .ml(px(edge.start))
                                .mr(px(edge.end))
                                .mt(px(edge.top))
                                .mb(px(edge.bottom));
                        }
                    }
                }
                MOD_WIDTH => {
                    if let Some(width) = modifier
                        .value
                        .as_ref()
                        .and_then(|value| extract_field_dp(value, "width"))
                    {
                        wrapper = wrapper.w(px(width));
                    }
                }
                MOD_HEIGHT => {
                    if let Some(height) = modifier
                        .value
                        .as_ref()
                        .and_then(|value| extract_field_dp(value, "height"))
                    {
                        wrapper = wrapper.h(px(height));
                    }
                }
                MOD_SIZE => {
                    if let Some(Value::Object(map)) = modifier.value.as_ref() {
                        if let Some(width) = map
                            .get("width")
                            .and_then(|value| extract_field_dp(value, "width"))
                        {
                            wrapper = wrapper.w(px(width));
                        }
                        if let Some(height) = map
                            .get("height")
                            .and_then(|value| extract_field_dp(value, "height"))
                        {
                            wrapper = wrapper.h(px(height));
                        }
                    }
                }
                MOD_HORIZONTAL_ALIGNMENT | MOD_VERTICAL_ALIGNMENT => {
                    // TODO: map align-self semantics.
                }
                other => {
                    warn!("redwood-panel: unsupported modifier {other} on {}", node_id);
                }
            }
        }

        wrapper.into_any_element()
    }
}

fn extract_field_dp(value: &Value, field: &str) -> Option<f32> {
    match value {
        Value::Object(map) => {
            if let Some(inner) = map.get(field) {
                dp_from_value(inner)
            } else {
                dp_from_value(value)
            }
        }
        _ => dp_from_value(value),
    }
}

fn dp_from_value(value: &Value) -> Option<f32> {
    match value {
        Value::Number(number) => number.as_f64().map(|f| f as f32),
        Value::Object(map) => {
            if let Some(Value::Number(number)) = map.get("value") {
                number.as_f64().map(|f| f as f32)
            } else if map.len() == 1 {
                map.values().next().and_then(dp_from_value)
            } else {
                None
            }
        }
        _ => None,
    }
}

#[derive(Clone, Copy)]
enum Orientation {
    Horizontal,
    Vertical,
}

struct EdgeInsets {
    start: f32,
    end: f32,
    top: f32,
    bottom: f32,
}

fn parse_margin(value: &Value) -> Option<EdgeInsets> {
    fn parse_inner(object: &serde_json::Map<String, Value>) -> EdgeInsets {
        let start = object
            .get("start")
            .and_then(Value::as_f64)
            .unwrap_or_default() as f32;
        let end = object
            .get("end")
            .and_then(Value::as_f64)
            .unwrap_or(start as f64) as f32;
        let top = object
            .get("top")
            .and_then(Value::as_f64)
            .unwrap_or_default() as f32;
        let bottom = object
            .get("bottom")
            .and_then(Value::as_f64)
            .unwrap_or(top as f64) as f32;
        EdgeInsets {
            start,
            end,
            top,
            bottom,
        }
    }

    match value {
        Value::Object(map) => {
            if let Some(Value::Object(inner)) = map.get("margin") {
                Some(parse_inner(inner))
            } else {
                Some(parse_inner(map))
            }
        }
        _ => None,
    }
}
