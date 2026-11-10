//! The Wine binaries the launcher runs.
//!
//! The paths are plain names resolved from `PATH` (or `$SNAP/bin` under the snap)
//! rather than hard-coded locations, so the tests can point them at stubs. Nothing
//! here runs `wget`: the game is downloaded by the launcher itself
//! ([`crate::cdn`] and [`crate::sync`]), not by a helper process.

use std::path::PathBuf;

/// The Wine tools the launcher invokes.
pub struct Tools {
    pub wine: PathBuf,
    pub winepath: PathBuf,
    pub wineserver: PathBuf,
}

impl Default for Tools {
    fn default() -> Self {
        Self {
            wine: "wine".into(),
            winepath: "winepath".into(),
            wineserver: "wineserver".into(),
        }
    }
}
