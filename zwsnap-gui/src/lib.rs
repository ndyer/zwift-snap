//! Zwift snap launcher: GTK frontend, Wine lifecycle, and Rust-owned logging.

pub mod application;
pub mod cancel;
pub mod cdn;
pub mod display;
pub mod download;
pub mod gpu_test;
pub mod graphics;
pub mod install;
pub mod launch;
pub mod logging;
pub mod manifest;
pub mod oauth;
pub mod orchestration;
pub mod process;
pub mod runtime;
pub mod sync;
pub mod update;
pub mod version;
pub mod wine_tools;

#[cfg(test)]
pub mod test_support;

#[cfg(feature = "gtk-ui")]
pub mod gui;

#[cfg(test)]
// Avoid exec races while other tests are writing temporary executable stubs.
pub(crate) static SCRIPT_TEST_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());
