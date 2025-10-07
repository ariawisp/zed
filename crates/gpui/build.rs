use std::{env, path::PathBuf, process::Command};

fn main() {
    // Only relevant on macOS with the Swift path enabled
    let target_os = env::var("CARGO_CFG_TARGET_OS").unwrap_or_default();
    if target_os != "macos" { return; }
    if env::var_os("CARGO_FEATURE_MACOS_SWIFT").is_none() { return; }

    let manifest_dir = PathBuf::from(env::var("CARGO_MANIFEST_DIR").unwrap());
    let swift_dir = manifest_dir.join("src/platform/mac/swift");

    // Rebuild when Swift sources or the canonical header change
    println!("cargo:rerun-if-changed={}", swift_dir.join("Package.swift").display());
    println!(
        "cargo:rerun-if-changed={}",
        manifest_dir.join("src/platform/mac/swift/Sources/GPUIAppKit").display()
    );
    println!(
        "cargo:rerun-if-changed={}",
        manifest_dir.join("src/platform/mac/gpui_macos_ffi.h").display()
    );

    // If the Swift toolchain isn't available in this environment, skip building.
    let swift_ok = Command::new("swift").arg("--version").output().is_ok();
    if !swift_ok {
        // In dev environments without Swift, we still allow `cargo check` to succeed.
        return;
    }

    // Build the Swift package in release mode
    let status = Command::new("swift")
        .args(["build", "-c", "release"]).current_dir(&swift_dir)
        .status()
        .expect("failed to invoke swift build");
    if !status.success() {
        panic!("swift build failed");
    }

    // Link the built dynamic library for dev runs
    let out_dir = swift_dir.join(".build/release");
    println!("cargo:rustc-link-search=native={}", out_dir.display());
    println!("cargo:rustc-link-lib=dylib=GPUIAppKit");
}

