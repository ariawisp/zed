use serde::Deserialize;
use std::fs;
use std::path::Path;

#[derive(Debug, Deserialize)]
pub struct ZedlineRuntime {
    pub r#type: String,
    pub entry: String,
    #[serde(default)]
    pub manifest: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct ZedlineUiPanel {
    pub id: String,
    pub title: String,
    pub entry: String,
}

#[derive(Debug, Deserialize)]
pub struct ZedlineUi {
    #[serde(default)]
    pub panels: Vec<ZedlineUiPanel>,
    #[serde(default)]
    pub modals: Vec<ZedlineUiPanel>,
}

#[derive(Debug, Deserialize)]
pub struct ZedlineManifest {
    pub format: String,
    pub id: String,
    pub name: String,
    pub version: String,
    #[serde(default)]
    pub publisher: Option<String>,
    pub runtime: ZedlineRuntime,
    #[serde(default)]
    pub ui: Option<ZedlineUi>,
}

pub fn try_load_from_env() {
    if let Ok(path) = std::env::var("ZEDLINE_MANIFEST_PATH") {
        let p = Path::new(&path);
        match fs::read(p) {
            Ok(bytes) => match serde_json::from_slice::<ZedlineManifest>(&bytes) {
                Ok(m) => {
                    if m.format.starts_with("zedline@") {
                        log::info!(
                            "Detected Zedline manifest id={} name={} version={} runtime.type={} panels={}",
                            m.id,
                            m.name,
                            m.version,
                            m.runtime.r#type,
                            m.ui.as_ref().map(|u| u.panels.len()).unwrap_or(0),
                        );
                    } else {
                        log::warn!("ZEDLINE_MANIFEST_PATH set but format != zedline@*");
                    }
                }
                Err(e) => log::warn!("Failed to parse Zedline manifest {}: {}", path, e),
            },
            Err(e) => log::warn!("Failed to read Zedline manifest {}: {}", path, e),
        }
    }
}

