use gpui::{IntoElement, Render, Window, Context as GContext, SharedString};
use gpui::{div, img};
use smol::channel::{unbounded, Receiver};
use std::collections::{HashMap, HashSet};

use redwood_gpui_bridge::{Cmd, NodeKind, register_panel_sender};

#[derive(Default, Clone)]
struct TextNode { text: String }
#[derive(Default, Clone)]
struct ButtonNode { text: String, enabled: bool }
#[derive(Default, Clone)]
struct ImageNode { url: String }

#[derive(Clone)]
enum Node {
    Text(TextNode),
    Button(ButtonNode),
    Image(ImageNode),
    Row,
    Column,
}

pub struct RedwoodPanel {
    nodes: HashMap<i64, Node>,
    children: HashMap<i64, Vec<i64>>, // parent -> ordered children
    roots: Vec<i64>,
    rx: Receiver<Cmd>,
}

impl RedwoodPanel {
    pub fn new(panel_id: u64, window: &mut Window, _cx: &mut GContext<Self>) -> Self {
        let (tx, rx) = unbounded::<Cmd>();
        register_panel_sender(panel_id, tx);
        super::register_panel_window(panel_id, window.window_handle());
        Self { nodes: HashMap::new(), children: HashMap::new(), roots: Vec::new(), rx }
    }

    fn apply_cmd(&mut self, cmd: Cmd) {
        match cmd {
            Cmd::Create{handle,kind} => {
                let n = match kind {
                    NodeKind::Text => Node::Text(TextNode::default()),
                    NodeKind::Button => Node::Button(ButtonNode::default()),
                    NodeKind::Image => Node::Image(ImageNode::default()),
                    NodeKind::Row => Node::Row,
                    NodeKind::Column => Node::Column,
                };
                self.nodes.insert(handle, n);
                if !self.children.contains_key(&handle) { self.children.insert(handle, Vec::new()); }
            }
            Cmd::Destroy{handle} => {
                self.nodes.remove(&handle);
                self.children.remove(&handle);
                for ch in self.children.values_mut() { ch.retain(|&h| h != handle); }
                self.roots.retain(|&h| h != handle);
            }
            Cmd::AppendChild{parent,child} => {
                self.children.entry(parent).or_default().push(child);
                if let Some(pos) = self.roots.iter().position(|&h| h==child) { self.roots.remove(pos); }
                if !self.children.contains_key(&parent) { self.children.insert(parent, Vec::new()); }
            }
            Cmd::InsertChild{parent,index,child} => {
                let e = self.children.entry(parent).or_default();
                let idx = index.max(0) as usize;
                if idx >= e.len() { e.push(child); } else { e.insert(idx, child); }
                if let Some(pos) = self.roots.iter().position(|&h| h==child) { self.roots.remove(pos); }
            }
            Cmd::RemoveChild{parent,child} => {
                self.children.entry(parent).or_default().retain(|&h| h!=child);
            }
            Cmd::SetText{handle,text} => { if let Some(Node::Text(n)) = self.nodes.get_mut(&handle) { n.text = text; } }
            Cmd::SetButtonText{handle,text} => { if let Some(Node::Button(n)) = self.nodes.get_mut(&handle) { n.text = text; } }
            Cmd::SetButtonEnabled{handle,enabled} => { if let Some(Node::Button(n)) = self.nodes.get_mut(&handle) { n.enabled = enabled; } }
            Cmd::SetImageUrl{handle,url} => { if let Some(Node::Image(n)) = self.nodes.get_mut(&handle) { n.url = url; } }
            Cmd::SetImageFit{..} => {}
            Cmd::SetImageRadius{..} => {}
        }
    }
}

impl Render for RedwoodPanel {
    fn render(&mut self, _window: &mut Window, cx: &mut GContext<Self>) -> impl IntoElement {
        while let Ok(cmd) = self.rx.try_recv() { self.apply_cmd(cmd); }

        let mut root = div().w_full().h_full().scroll_y();
        if let Some(children) = self.children.get(&0) {
            for &h in children { root = root.child(render_node(h, &self.nodes, &self.children, cx)); }
        } else {
            if self.roots.is_empty() {
                let mut has_parent = HashSet::new();
                for (_p, ch) in &self.children { for &h in ch { has_parent.insert(h); } }
                for (&h, _) in &self.nodes { if !has_parent.contains(&h) { self.roots.push(h); } }
            }
            for &h in &self.roots { root = root.child(render_node(h, &self.nodes, &self.children, cx)); }
        }
        root
    }
}

fn render_node(handle: i64, nodes: &HashMap<i64, Node>, children: &HashMap<i64, Vec<i64>>, _cx: &mut GContext<RedwoodPanel>) -> impl IntoElement {
    match nodes.get(&handle) {
        Some(Node::Text(n)) => { div().child(gpui::StyledText::new(SharedString::from(n.text.clone()))) }
        Some(Node::Button(n)) => {
            let mut d = div().p_2().border_1();
            if !n.enabled { d = d.opacity(0.5); }
            let label = gpui::StyledText::new(SharedString::from(n.text.clone()));
            d.child(label)
        }
        Some(Node::Image(n)) => { img(n.url.clone()) }
        Some(Node::Row) => {
            let mut row = div().flex_row().gap_2();
            for &ch in children.get(&handle).into_iter().flatten() { row = row.child(render_node(ch, nodes, children, _cx)); }
            row
        }
        Some(Node::Column) => {
            let mut col = div().flex_col().gap_2();
            for &ch in children.get(&handle).into_iter().flatten() { col = col.child(render_node(ch, nodes, children, _cx)); }
            col
        }
        None => div()
    }
}

