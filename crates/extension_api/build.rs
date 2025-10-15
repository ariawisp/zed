use std::error::Error;
use std::fs;
use std::path::{Path, PathBuf};

const VERSIONS: &[&str] = &[
    "since_v0.0.1",
    "since_v0.0.4",
    "since_v0.0.6",
    "since_v0.1.0",
    "since_v0.2.0",
    "since_v0.3.0",
    "since_v0.4.0",
    "since_v0.5.0",
    "since_v0.6.0",
    "since_v1.0.0",
];

const BASE_MODULES: &[(&str, &str)] = &[
    ("common.wit", "zed/common/common.wit"),
    ("context-server.wit", "zed/context-server/context-server.wit"),
    ("dap.wit", "zed/dap/dap.wit"),
    ("github.wit", "zed/github/github.wit"),
    ("http-client.wit", "zed/http-client/http-client.wit"),
    ("lsp.wit", "zed/lsp/lsp.wit"),
    ("nodejs.wit", "zed/nodejs/nodejs.wit"),
    ("platform.wit", "zed/platform/platform.wit"),
    ("process.wit", "zed/process/process.wit"),
    ("slash-command.wit", "zed/slash-command/slash-command.wit"),
    ("ui.wit", "zed/ui/ui.wit"),
    ("version.wit", "zed/version/version.wit"),
    ("protocol.wit", "redwood/protocol/protocol.wit"),
];

fn main() -> Result<(), Box<dyn Error>> {
    stage_wit_definitions()?;
    write_version_bytes()?;
    Ok(())
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

    let staged_root = manifest_dir.join("wit");
    if staged_root.exists() {
        fs::remove_dir_all(&staged_root)?;
    }
    fs::create_dir_all(&staged_root)?;

    for version in VERSIONS {
        generate_version(&wit_root, &staged_root, version)?;
    }

    Ok(())
}

fn generate_version(wit_root: &Path, staged_root: &Path, version: &str) -> Result<(), Box<dyn Error>> {
    let dest_dir = staged_root.join(version);
    fs::create_dir_all(&dest_dir)?;

    let extension_src = wit_root
        .join("zed/extension")
        .join(version)
        .join("extension.wit");
    convert_wit_file(&extension_src, &dest_dir.join("extension.wit"), true)?;

    let settings_src = wit_root
        .join("zed/extension")
        .join(version)
        .join("settings.rs");
    if settings_src.exists() {
        fs::copy(&settings_src, dest_dir.join("settings.rs"))?;
    }

    for (dest, src) in BASE_MODULES {
        let src_path = wit_root.join(src);
        if src_path.exists() {
            convert_wit_file(&src_path, &dest_dir.join(dest), false)?;
        }
    }

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

fn convert_wit_file(src: &Path, dest: &Path, is_extension: bool) -> Result<(), Box<dyn Error>> {
    let contents = fs::read_to_string(src)?;
    let converted = convert_wit(&contents, is_extension);
    fs::write(dest, converted)?;
    Ok(())
}

fn convert_wit(source: &str, is_extension: bool) -> String {
    let mut output = String::new();

    for line in source.lines() {
        if let Some(converted) = convert_line(line, is_extension) {
            output.push_str(&converted);
            output.push('\n');
        }
    }

    output
}

fn convert_line(line: &str, is_extension: bool) -> Option<String> {
    let trimmed = line.trim_start();
    let indent = &line[..line.len() - trimmed.len()];

    if trimmed.starts_with("package ") {
        if is_extension {
            Some(format!("{}{}", indent, rewrite_package(trimmed)))
        } else {
            None
        }
    } else if trimmed.starts_with("import ") && trimmed.contains(':') {
        Some(format!("{}{}", indent, convert_path_segment(trimmed, "import ")))
    } else if trimmed.starts_with("use ") && trimmed.contains(':') {
        Some(format!("{}{}", indent, convert_path_segment(trimmed, "use ")))
    } else {
        Some(line.to_string())
    }
}

fn rewrite_package(line: &str) -> String {
    if let Some(rest) = line.strip_prefix("package ") {
        if let Some((name, _)) = rest.split_once('@') {
            let name = name.trim_end_matches(';');
            return format!("package {};", name);
        }
    }
    line.to_string()
}

fn convert_path_segment(line: &str, prefix: &str) -> String {
    let rest = &line[prefix.len()..];
    if let Some((_, after_colon)) = rest.split_once(':') {
        if rest.starts_with("zed:") || rest.starts_with("redwood:") {
            let mut cleaned = remove_version_suffix(after_colon);
            if let Some(slash_index) = cleaned.find('/') {
                cleaned = cleaned[slash_index + 1..].to_string();
            }
            return format!("{}{}", prefix, cleaned);
        }
    }
    line.to_string()
}

fn remove_version_suffix(segment: &str) -> String {
    if let Some(at_idx) = segment.find('@') {
        let bytes = segment.as_bytes();
        let mut end = at_idx + 1;
        while end < bytes.len() {
            let c = bytes[end] as char;
            if c.is_ascii_digit() {
                end += 1;
            } else if c == '.' && end + 1 < bytes.len() && (bytes[end + 1] as char).is_ascii_digit() {
                end += 1;
            } else {
                break;
            }
        }
        let mut result = String::from(&segment[..at_idx]);
        result.push_str(&segment[end..]);
        result
    } else {
        segment.to_string()
    }
}

fn write_version_bytes() -> Result<(), Box<dyn Error>> {
    let version = std::env::var("CARGO_PKG_VERSION")?;
    let out_dir = PathBuf::from(std::env::var("OUT_DIR")?);

    let mut parts = version.split(|c: char| !c.is_ascii_digit());
    let major = parts.next().unwrap().parse::<u16>().unwrap().to_be_bytes();
    let minor = parts.next().unwrap().parse::<u16>().unwrap().to_be_bytes();
    let patch = parts.next().unwrap().parse::<u16>().unwrap().to_be_bytes();

    fs::write(
        out_dir.join("version_bytes"),
        [major[0], major[1], minor[0], minor[1], patch[0], patch[1]],
    )?;

    Ok(())
}
