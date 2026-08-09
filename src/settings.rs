//! Persistent settings (XML under `OMT_STORAGE_PATH`).

use std::env;
use std::fs;
use std::path::PathBuf;

use crate::error::OmtError;

/// Environment variable overriding the settings storage directory.
pub const OMT_STORAGE_PATH: &str = "OMT_STORAGE_PATH";

/// Key/value settings store backed by a simple XML file.
#[derive(Debug, Default)]
pub struct Settings {
    preview: bool,
    path: Option<PathBuf>,
}

impl Settings {
    /// Create default settings.
    pub fn new() -> Self {
        Self {
            preview: false,
            path: storage_dir().map(|d| d.join("omt_settings.xml")),
        }
    }

    /// Preview mode enabled.
    pub fn preview(&self) -> bool {
        self.preview
    }

    /// Set preview mode.
    pub fn set_preview(&mut self, enabled: bool) {
        self.preview = enabled;
    }

    /// Load settings from disk.
    pub fn load(&mut self) -> Result<(), OmtError> {
        let Some(path) = &self.path else {
            return Ok(());
        };
        if !path.exists() {
            return Ok(());
        }
        let xml = fs::read_to_string(path)?;
        self.preview = xml.contains(r#"Preview="true""#) || xml.contains("<Preview>true</Preview>");
        Ok(())
    }

    /// Save settings to disk as minimal XML.
    pub fn save(&self) -> Result<(), OmtError> {
        let Some(path) = &self.path else {
            return Err(OmtError::InvalidArgument(
                "no storage path (set OMT_STORAGE_PATH)".into(),
            ));
        };
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        let preview = if self.preview { "true" } else { "false" };
        let xml = format!("<OMTSettings Preview=\"{preview}\" />\n");
        fs::write(path, xml)?;
        Ok(())
    }
}

fn storage_dir() -> Option<PathBuf> {
    if let Ok(p) = env::var(OMT_STORAGE_PATH) {
        return Some(PathBuf::from(p));
    }
    dirs_fallback()
}

fn dirs_fallback() -> Option<PathBuf> {
    // Avoid extra deps: use HOME / USERPROFILE.
    if let Ok(h) = env::var("HOME") {
        return Some(PathBuf::from(h).join(".omt"));
    }
    if let Ok(h) = env::var("USERPROFILE") {
        return Some(PathBuf::from(h).join(".omt"));
    }
    None
}
