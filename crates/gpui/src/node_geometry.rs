use crate::{App, Bounds, Global, LayoutId, Pixels, WindowId};
use collections::{FxHashMap, FxHashSet};
use once_cell::sync::Lazy;
use parking_lot::RwLock;
use smallvec::SmallVec;
use std::sync::Arc;

const MAX_SCROLL_LINEAGE: usize = 8;

/// Identifier for a scroll container used when tracking snapshot lineage.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Hash)]
pub struct ScrollContainerId(u64);

impl ScrollContainerId {
    pub(crate) fn new(raw: u64) -> Self {
        ScrollContainerId(raw)
    }

    /// Expose the raw identifier for interop layers.
    pub fn as_u64(self) -> u64 {
        self.0
    }
}

/// Snapshot of a node's geometry captured during the last frame.
#[derive(Clone, Debug, PartialEq)]
pub struct NodeSnapshot {
    /// Bounds in the node's local coordinate space (typically layout output).
    pub local: Bounds<Pixels>,
    /// Bounds in window coordinates after applying any active element offsets.
    pub window: Bounds<Pixels>,
    /// Ordered list of scroll container ancestors (closest first).
    pub scroll_lineage: SmallVec<[ScrollContainerId; MAX_SCROLL_LINEAGE]>,
    /// Monotonically increasing version useful for cache invalidation.
    pub version: u64,
}

/// Change notification emitted whenever a node snapshot is updated or invalidated.
#[derive(Clone, Debug)]
pub enum NodeGeometryChange {
    /// The node produced a fresh snapshot for the current frame.
    Updated(NodeSnapshot),
    /// The node snapshot was dropped (e.g., due to invalidation or removal).
    Invalidated,
}

/// Callback invoked when a subscribed node snapshot changes.
pub type NodeGeometryCallback = Arc<dyn Fn(NodeGeometryChange) + Send + Sync>;

/// Handle that keeps a node geometry subscription alive.
#[must_use]
pub struct NodeGeometrySubscription {
    window_id: WindowId,
    layout_id: LayoutId,
    subscriber_id: u64,
    active: bool,
}

impl NodeGeometrySubscription {
    /// Returns whether this subscription is still registered with the service.
    pub fn is_active(&self) -> bool {
        self.active
    }

    /// Cancel the subscription immediately instead of waiting for `Drop`.
    pub fn unsubscribe(mut self) {
        self.teardown();
    }

    fn teardown(&mut self) {
        if self.active {
            remove_global_subscription(self.window_id, self.layout_id, self.subscriber_id);
            self.active = false;
        }
    }
}

impl Drop for NodeGeometrySubscription {
    fn drop(&mut self) {
        self.teardown();
    }
}

#[derive(Default)]
pub(crate) struct NodeGeometryStore {
    snapshots: FxHashMap<LayoutId, NodeSnapshot>,
    scroll_index: FxHashMap<ScrollContainerId, FxHashSet<LayoutId>>,
    version_counter: u64,
}

impl NodeGeometryStore {
    pub fn new() -> Self {
        Self {
            snapshots: FxHashMap::default(),
            scroll_index: FxHashMap::default(),
            version_counter: 0,
        }
    }

    pub fn clear(&mut self) {
        self.snapshots.clear();
        self.scroll_index.clear();
        self.version_counter = 0;
    }

    pub fn record(
        &mut self,
        layout_id: LayoutId,
        local: Bounds<Pixels>,
        window: Bounds<Pixels>,
        lineage: &[ScrollContainerId],
    ) -> NodeSnapshot {
        self.version_counter = self.version_counter.wrapping_add(1);
        let version = self.version_counter;
        let snapshot = NodeSnapshot {
            local,
            window,
            scroll_lineage: SmallVec::from_slice(lineage),
            version,
        };
        if let Some(previous) = self.snapshots.insert(layout_id, snapshot.clone()) {
            self.remove_scroll_membership(layout_id, &previous.scroll_lineage);
        }
        self.add_scroll_membership(layout_id, &snapshot.scroll_lineage);
        snapshot
    }

    pub fn snapshot(&self, layout_id: LayoutId) -> Option<NodeSnapshot> {
        self.snapshots.get(&layout_id).cloned()
    }

    pub fn invalidate(&mut self, window_id: WindowId, layout_id: LayoutId) -> bool {
        if let Some(snapshot) = self.snapshots.remove(&layout_id) {
            self.remove_scroll_membership(layout_id, &snapshot.scroll_lineage);
            invalidate_global_snapshot(window_id, layout_id);
            true
        } else {
            invalidate_global_snapshot(window_id, layout_id);
            false
        }
    }

    pub fn scroll_container_updated(&mut self, window_id: WindowId, scroll_id: ScrollContainerId) {
        if let Some(layout_ids) = self.scroll_index.remove(&scroll_id) {
            for layout_id in layout_ids {
                if let Some(snapshot) = self.snapshots.remove(&layout_id) {
                    self.remove_scroll_membership(layout_id, &snapshot.scroll_lineage);
                }
                invalidate_global_snapshot(window_id, layout_id);
            }
        }
    }

    fn add_scroll_membership(&mut self, layout_id: LayoutId, lineage: &[ScrollContainerId]) {
        for scroll_id in lineage {
            self.scroll_index
                .entry(*scroll_id)
                .or_default()
                .insert(layout_id);
        }
    }

    fn remove_scroll_membership(&mut self, layout_id: LayoutId, lineage: &[ScrollContainerId]) {
        for scroll_id in lineage {
            if let Some(entries) = self.scroll_index.get_mut(scroll_id) {
                entries.remove(&layout_id);
                if entries.is_empty() {
                    self.scroll_index.remove(scroll_id);
                }
            }
        }
    }
}

struct GlobalNodeGeometry {
    snapshots: FxHashMap<(WindowId, LayoutId), NodeSnapshot>,
    subscriptions: FxHashMap<(WindowId, LayoutId), Vec<SubscriptionEntry>>,
    next_subscription_id: u64,
}

#[derive(Clone)]
struct SubscriptionEntry {
    id: u64,
    callback: NodeGeometryCallback,
}

impl Default for GlobalNodeGeometry {
    fn default() -> Self {
        Self {
            snapshots: FxHashMap::default(),
            subscriptions: FxHashMap::default(),
            next_subscription_id: 1,
        }
    }
}

impl GlobalNodeGeometry {
    fn record(
        &mut self,
        window_id: WindowId,
        layout_id: LayoutId,
        snapshot: &NodeSnapshot,
    ) -> Vec<NodeGeometryCallback> {
        self.snapshots
            .insert((window_id, layout_id), snapshot.clone());
        self.collect_callbacks(window_id, layout_id)
    }

    fn snapshot(&self, window_id: WindowId, layout_id: LayoutId) -> Option<NodeSnapshot> {
        self.snapshots.get(&(window_id, layout_id)).cloned()
    }

    fn invalidate(
        &mut self,
        window_id: WindowId,
        layout_id: LayoutId,
    ) -> Vec<NodeGeometryCallback> {
        let key = (window_id, layout_id);
        let had_snapshot = self.snapshots.remove(&key).is_some();
        if had_snapshot || self.subscriptions.contains_key(&key) {
            self.collect_callbacks(window_id, layout_id)
        } else {
            Vec::new()
        }
    }

    fn clear_window(&mut self, window_id: WindowId) -> Vec<Vec<NodeGeometryCallback>> {
        let targets: Vec<(WindowId, LayoutId)> = self
            .snapshots
            .keys()
            .copied()
            .filter(|(stored_id, _)| *stored_id == window_id)
            .collect();

        for key in &targets {
            self.snapshots.remove(key);
        }

        let mut callback_sets = Vec::new();
        for (_, layout_id) in &targets {
            let callbacks = self.collect_callbacks(window_id, *layout_id);
            if !callbacks.is_empty() {
                callback_sets.push(callbacks);
            }
        }

        self.subscriptions
            .retain(|(stored_id, _), _| *stored_id != window_id);
        callback_sets
    }

    fn subscribe(
        &mut self,
        window_id: WindowId,
        layout_id: LayoutId,
        callback: NodeGeometryCallback,
    ) -> NodeGeometrySubscription {
        let id = self.next_subscription_id;
        self.next_subscription_id = match self.next_subscription_id.wrapping_add(1) {
            0 => 1,
            next => next,
        };

        self.subscriptions
            .entry((window_id, layout_id))
            .or_default()
            .push(SubscriptionEntry { id, callback });

        NodeGeometrySubscription {
            window_id,
            layout_id,
            subscriber_id: id,
            active: true,
        }
    }

    fn remove_subscription(
        &mut self,
        window_id: WindowId,
        layout_id: LayoutId,
        subscriber_id: u64,
    ) {
        if let Some(entries) = self.subscriptions.get_mut(&(window_id, layout_id)) {
            entries.retain(|entry| entry.id != subscriber_id);
            if entries.is_empty() {
                self.subscriptions.remove(&(window_id, layout_id));
            }
        }
    }

    fn collect_callbacks(
        &self,
        window_id: WindowId,
        layout_id: LayoutId,
    ) -> Vec<NodeGeometryCallback> {
        self.subscriptions
            .get(&(window_id, layout_id))
            .map(|entries| entries.iter().map(|entry| entry.callback.clone()).collect())
            .unwrap_or_default()
    }
}

static GLOBAL_NODE_GEOMETRY: Lazy<RwLock<GlobalNodeGeometry>> =
    Lazy::new(|| RwLock::new(GlobalNodeGeometry::default()));

pub(crate) fn record_global_snapshot(
    window_id: WindowId,
    layout_id: LayoutId,
    snapshot: &NodeSnapshot,
) {
    let callbacks = {
        let mut registry = GLOBAL_NODE_GEOMETRY.write();
        registry.record(window_id, layout_id, snapshot)
    };
    notify_callbacks(callbacks, NodeGeometryChange::Updated(snapshot.clone()));
}

pub(crate) fn clear_global_snapshots(window_id: WindowId) {
    let callback_sets = {
        let mut registry = GLOBAL_NODE_GEOMETRY.write();
        registry.clear_window(window_id)
    };
    for callbacks in callback_sets {
        notify_callbacks(callbacks, NodeGeometryChange::Invalidated);
    }
}

pub(crate) fn invalidate_global_snapshot(window_id: WindowId, layout_id: LayoutId) {
    let callbacks = {
        let mut registry = GLOBAL_NODE_GEOMETRY.write();
        registry.invalidate(window_id, layout_id)
    };
    notify_callbacks(callbacks, NodeGeometryChange::Invalidated);
}

/// Retrieve the last committed snapshot for a layout node in a specific window.
pub fn global_node_snapshot(window_id: WindowId, layout_id: LayoutId) -> Option<NodeSnapshot> {
    GLOBAL_NODE_GEOMETRY.read().snapshot(window_id, layout_id)
}

fn subscribe_global_node_geometry(
    window_id: WindowId,
    layout_id: LayoutId,
    callback: NodeGeometryCallback,
) -> NodeGeometrySubscription {
    GLOBAL_NODE_GEOMETRY
        .write()
        .subscribe(window_id, layout_id, callback)
}

fn remove_global_subscription(window_id: WindowId, layout_id: LayoutId, subscriber_id: u64) {
    GLOBAL_NODE_GEOMETRY
        .write()
        .remove_subscription(window_id, layout_id, subscriber_id);
}

fn notify_callbacks(callbacks: Vec<NodeGeometryCallback>, change: NodeGeometryChange) {
    if callbacks.is_empty() {
        return;
    }

    for callback in callbacks {
        callback(change.clone());
    }
}

/// Ensure the shared node geometry service global has been registered.
pub fn ensure_node_geometry_service(cx: &mut App) {
    if cx.try_global::<NodeGeometryServiceGlobal>().is_none() {
        cx.set_global(NodeGeometryServiceGlobal::new());
    }
}

/// Public service interface for querying node geometry snapshots.
pub trait NodeGeometryService: Send + Sync {
    /// Retrieve the most recent snapshot for `layout_id` within `window_id`.
    fn snapshot(&self, window_id: WindowId, layout_id: LayoutId) -> Option<NodeSnapshot>;
    /// Subscribe to future snapshot changes for the given layout node.
    fn subscribe(
        &self,
        window_id: WindowId,
        layout_id: LayoutId,
        callback: NodeGeometryCallback,
    ) -> NodeGeometrySubscription;
}

#[derive(Default)]
struct NodeGeometryServiceImpl;

impl NodeGeometryService for NodeGeometryServiceImpl {
    fn snapshot(&self, window_id: WindowId, layout_id: LayoutId) -> Option<NodeSnapshot> {
        global_node_snapshot(window_id, layout_id)
    }

    fn subscribe(
        &self,
        window_id: WindowId,
        layout_id: LayoutId,
        callback: NodeGeometryCallback,
    ) -> NodeGeometrySubscription {
        subscribe_global_node_geometry(window_id, layout_id, callback)
    }
}

/// Global wrapper that exposes the node geometry service to GPUI callers.
#[derive(Default)]
pub struct NodeGeometryServiceGlobal {
    service: NodeGeometryServiceImpl,
}

impl Global for NodeGeometryServiceGlobal {}

impl NodeGeometryServiceGlobal {
    /// Construct a new service wrapper.
    pub fn new() -> Self {
        Self {
            service: NodeGeometryServiceImpl::default(),
        }
    }

    /// Access the underlying service implementation.
    pub fn service(&self) -> &dyn NodeGeometryService {
        &self.service
    }
}
