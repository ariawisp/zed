use crate::{Bounds, LayoutId, Pixels, WindowId, Global, App};
use collections::FxHashMap;
use once_cell::sync::Lazy;
use parking_lot::RwLock;
use smallvec::SmallVec;

const MAX_SCROLL_LINEAGE: usize = 8;

/// Identifier for a scroll container used when tracking snapshot lineage.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Hash)]
pub struct ScrollContainerId(u64);

impl ScrollContainerId {
    pub(crate) fn new(raw: u64) -> Self {
        ScrollContainerId(raw)
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

#[derive(Default)]
pub(crate) struct NodeGeometryStore {
    snapshots: FxHashMap<LayoutId, NodeSnapshot>,
    version_counter: u64,
}

impl NodeGeometryStore {
    pub fn new() -> Self {
        Self { snapshots: FxHashMap::default(), version_counter: 0 }
    }

    pub fn clear(&mut self) {
        self.snapshots.clear();
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
        self.snapshots.insert(layout_id, snapshot.clone());
        snapshot
    }

    pub fn snapshot(&self, layout_id: LayoutId) -> Option<NodeSnapshot> {
        self.snapshots.get(&layout_id).cloned()
    }
}

#[derive(Default)]
struct GlobalNodeGeometry {
    snapshots: FxHashMap<(WindowId, LayoutId), NodeSnapshot>,
}

impl GlobalNodeGeometry {
    fn record(&mut self, window_id: WindowId, layout_id: LayoutId, snapshot: &NodeSnapshot) {
        self.snapshots.insert((window_id, layout_id), snapshot.clone());
    }

    fn snapshot(&self, window_id: WindowId, layout_id: LayoutId) -> Option<NodeSnapshot> {
        self.snapshots.get(&(window_id, layout_id)).cloned()
    }

    fn clear_window(&mut self, window_id: WindowId) {
        self.snapshots.retain(|(stored_id, _), _| *stored_id != window_id);
    }
}

static GLOBAL_NODE_GEOMETRY: Lazy<RwLock<GlobalNodeGeometry>> =
    Lazy::new(|| RwLock::new(GlobalNodeGeometry::default()));

pub(crate) fn record_global_snapshot(
    window_id: WindowId,
    layout_id: LayoutId,
    snapshot: &NodeSnapshot,
) {
    GLOBAL_NODE_GEOMETRY.write().record(window_id, layout_id, snapshot);
}

pub(crate) fn clear_global_snapshots(window_id: WindowId) {
    GLOBAL_NODE_GEOMETRY.write().clear_window(window_id);
}

/// Retrieve the last committed snapshot for a layout node in a specific window.
pub fn global_node_snapshot(window_id: WindowId, layout_id: LayoutId) -> Option<NodeSnapshot> {
    GLOBAL_NODE_GEOMETRY.read().snapshot(window_id, layout_id)
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
}

#[derive(Default)]
struct NodeGeometryServiceImpl;

impl NodeGeometryService for NodeGeometryServiceImpl {
    fn snapshot(&self, window_id: WindowId, layout_id: LayoutId) -> Option<NodeSnapshot> {
        global_node_snapshot(window_id, layout_id)
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
        Self { service: NodeGeometryServiceImpl::default() }
    }

    /// Access the underlying service implementation.
    pub fn service(&self) -> &dyn NodeGeometryService {
        &self.service
    }
}
