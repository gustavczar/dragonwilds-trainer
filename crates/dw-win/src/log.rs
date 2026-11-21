//! Append-only log at `dw-trainer/trainer.log`.

use std::io::Write;
use std::time::{SystemTime, UNIX_EPOCH};

pub fn line(msg: &str) {
    let secs = SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0);
    if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(crate::data_dir().join("trainer.log")) {
        let _ = writeln!(f, "[{secs}] {msg}");
    }
}
