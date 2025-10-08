use std::env;
use std::path::PathBuf;

fn main() {
    // Optional static link of the Kotlin/Native host. Set REDWOOD_HOST_LIB_DIR to the directory
    // containing libredwood_host_gpui.a to enable linking and the redwood_host cfg.
    if let Ok(dir) = env::var("REDWOOD_HOST_LIB_DIR") {
        println!("cargo:rustc-link-search=native={}", dir);
        println!("cargo:rustc-link-lib=static=redwood_host_gpui");
        // NOTE(stopgap): If Kotlin/Native requires additional system libs/frameworks on macOS,
        // add them here (e.g., -lobjc, -framework Foundation). We'll determine the minimal set
        // once we switch off the demo.
        #[cfg(target_os = "macos")] {
            // println!("cargo:rustc-link-lib=framework=Foundation");
            // println!("cargo:rustc-link-lib=objc");
        }
        // Enable conditional compilation for redwood_host integration.
        println!("cargo:rustc-cfg=redwood_host");
    }
}

