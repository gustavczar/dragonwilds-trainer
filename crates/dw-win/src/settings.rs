//! Feature switches and panel language, kept in `dw-trainer/settings.json`.

use dw_core::features::Lang;
use dw_core::Toggles;
use serde::{Deserialize, Serialize};
use std::sync::Mutex;

#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    pub toggles: Toggles,
    pub lang: Lang,
}

static CURRENT: Mutex<Option<Settings>> = Mutex::new(None);

fn path() -> std::path::PathBuf {
    crate::data_dir().join("settings.json")
}

pub fn get() -> Settings {
    let mut cur = CURRENT.lock().unwrap_or_else(|e| e.into_inner());
    *cur.get_or_insert_with(|| {
        std::fs::read(path()).ok().and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or_default()
    })
}

pub fn toggles() -> Toggles {
    get().toggles
}

/// Stores new settings and writes them to disk.
pub fn set(s: Settings) {
    *CURRENT.lock().unwrap_or_else(|e| e.into_inner()) = Some(s);
    if let Ok(json) = serde_json::to_vec_pretty(&s) {
        let _ = std::fs::write(path(), json);
    }
}
