use std::sync::{Mutex, OnceLock};
use std::time::Instant;

type FrameClockCallback = extern "C" fn(ts_ms: f64);

static FRAME_CB: OnceLock<Mutex<Option<FrameClockCallback>>> = OnceLock::new();
static START: OnceLock<Instant> = OnceLock::new();

fn frame_cb_lock() -> &'static Mutex<Option<FrameClockCallback>> {
    FRAME_CB.get_or_init(|| Mutex::new(None))
}

fn monotonic_millis() -> f64 {
    let start = START.get_or_init(Instant::now);
    let elapsed = start.elapsed();
    (elapsed.as_secs() as f64) * 1000.0 + (elapsed.subsec_nanos() as f64) / 1_000_000.0
}

/// Internal: notify the host (if registered) that a frame completed, with timestamp in ms.
pub(crate) fn notify_frame_clock_now() {
    let ts = monotonic_millis();
    let cb = { *frame_cb_lock().lock().unwrap() };
    if let Some(f) = cb {
        // Safety: calling foreign function pointer provided by host
        f(ts);
    }
}

/// Set a host callback to be invoked after each completed frame.
/// The callback receives a monotonic timestamp in milliseconds.
#[unsafe(no_mangle)]
pub extern "C" fn gpui_set_frame_clock_callback(cb: Option<FrameClockCallback>) {
    *frame_cb_lock().lock().unwrap() = cb;
}
