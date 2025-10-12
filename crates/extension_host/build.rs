use std::error::Error;
use std::fs;
use std::path::{Path, PathBuf};

fn main() -> Result<(), Box<dyn Error>> {
    stage_wit_definitions()
}

fn stage_wit_definitions() -> Result<(), Box<dyn Error>> {
    let manifest_dir = PathBuf::from(std::env::var("CARGO_MANIFEST_DIR")?);
    let default_root = manifest_dir.join("../../../zed-extension-wit/wit");
    let wit_root = std::env::var("ZED_EXTENSION_WIT_ROOT")
        .map(PathBuf::from)
        .unwrap_or(default_root);

    println!("cargo:rerun-if-env-changed=ZED_EXTENSION_WIT_ROOT");

    let wit_root = wit_root.canonicalize().map_err(|_| {
        format!(
            "unable to locate WIT definitions at {}",
            wit_root.display()
        )
    })?;

    emit_rerun_if_changed(&wit_root)?;

    let staged_root = manifest_dir.join(".wit");
    if staged_root.exists() {
        fs::remove_dir_all(&staged_root)?;
    }
    copy_dir_recursive(&wit_root, &staged_root)?;

    Ok(())
}

fn emit_rerun_if_changed(path: &Path) -> Result<(), Box<dyn Error>> {
    if path.is_file() {
        println!("cargo:rerun-if-changed={}", path.display());
        return Ok(());
    }

    for entry in fs::read_dir(path)? {
        let entry = entry?;
        let entry_path = entry.path();
        if entry_path.is_dir() {
            emit_rerun_if_changed(&entry_path)?;
        } else {
            println!("cargo:rerun-if-changed={}", entry_path.display());
        }
    }

    Ok(())
}

fn copy_dir_recursive(src: &Path, dst: &Path) -> Result<(), Box<dyn Error>> {
    fs::create_dir_all(dst)?;
    for entry in fs::read_dir(src)? {
        let entry = entry?;
        let entry_path = entry.path();
        let dest_path = dst.join(entry.file_name());

        if entry_path.is_dir() {
            copy_dir_recursive(&entry_path, &dest_path)?;
        } else {
            if let Some(parent) = dest_path.parent() {
                fs::create_dir_all(parent)?;
            }
            fs::copy(&entry_path, &dest_path)?;
        }
    }

    Ok(())
}
