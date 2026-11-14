//! The artwork shown behind the account picker.
//!
//! The image is resolved in this order:
//!
//! 1. the `backdrop` key in `settings.json`, if it points at an existing file;
//! 2. a previously downloaded copy under `$SNAP_USER_COMMON/launcher-art/`;
//! 3. a best-effort download from `ZWIFT_LAUNCHER_ART_URL` (a built-in default
//!    pointing at Zwift's CDN);
//! 4. nothing — the picker then uses the plain theme background.
//!
//! Every step is silent on failure: a missing or broken backdrop must never stop
//! somebody from launching the game, so problems are only ever reported through
//! the debug log.

use crate::logging::Logger;
use crate::profile;
use gtk::prelude::*;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;

/// Environment variable overriding the artwork URL.
pub const ART_ENV: &str = "ZWIFT_LAUNCHER_ART_URL";

/// The folder under `$SNAP_USER_COMMON` caching the downloaded artwork.
pub const ART_DIR: &str = "launcher-art";

/// Base name of every cached artwork file (the extension varies).
const ART_STEM: &str = "backdrop";

/// Where the launcher page keeps its artwork, used when `ART_ENV` is unset.
const DEFAULT_ART_URL: &str = "https://cdn.zwift.com/static/web/launcher/backdrop.jpg";

/// Resolves the backdrop image to a local file, downloading and caching it if
/// necessary. Returns `None` when no image could be found.
pub fn resolve(
    backdrop: Option<&str>,
    common: &Path,
    logger: Option<&Arc<Logger>>,
) -> Option<PathBuf> {
    if let Some(configured) = backdrop {
        let path = Path::new(configured);
        if path.is_file() {
            debug(
                logger,
                &format!("using configured backdrop {}", path.display()),
            );
            return Some(path.to_path_buf());
        }
        debug(
            logger,
            &format!("configured backdrop {} is not a file", path.display()),
        );
    }

    let dir = common.join(ART_DIR);
    if let Some(cached) = cached(&dir) {
        debug(
            logger,
            &format!("using cached backdrop {}", cached.display()),
        );
        return Some(cached);
    }

    let url = std::env::var(ART_ENV).unwrap_or_else(|_| DEFAULT_ART_URL.to_owned());
    match download(&url, &dir, logger) {
        Some(path) => Some(path),
        None => {
            debug(logger, &format!("no backdrop available from {url}"));
            None
        }
    }
}

/// A widget painting the resolved backdrop stretched to fill its parent.
///
/// The image is applied as a CSS `background-image` on a plain box rather than
/// through `GtkPicture`: `content-fit: cover` only exists from GTK 4.8 onward,
/// while CSS `background-size: cover` works on the 4.0 API this crate targets.
pub fn picture(path: &Path) -> gtk::Widget {
    let area = gtk::Box::new(gtk::Orientation::Vertical, 0);
    area.set_hexpand(true);
    area.set_vexpand(true);
    area.set_can_focus(false);
    area.add_css_class("backdrop-art");

    let provider = gtk::CssProvider::new();
    provider.load_from_data(&format!(
        ".backdrop-art {{ background-image: url(\"{}\"); background-size: cover; background-position: center; }}",
        css_url(path),
    ));
    if let Some(display) = gtk::gdk::Display::default() {
        gtk::style_context_add_provider_for_display(
            &display,
            &provider,
            gtk::STYLE_PROVIDER_PRIORITY_APPLICATION,
        );
    }
    area.upcast()
}

/// A `file://` URL for `path` safe to embed in a CSS `url()`.
fn css_url(path: &Path) -> String {
    let mut url = String::from("file://");
    for byte in path.as_os_str().as_encoded_bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'/' | b'-' | b'_' | b'.' | b'~' => {
                url.push(*byte as char)
            }
            _ => url.push_str(&format!("%{byte:02X}")),
        }
    }
    url
}

/// Returns the first `backdrop.*` file in `dir`, if any.
fn cached(dir: &Path) -> Option<PathBuf> {
    let entries = fs::read_dir(dir).ok()?;
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_file() && path.file_stem().is_some_and(|stem| stem == ART_STEM) {
            return Some(path);
        }
    }
    None
}

/// Downloads `url` into `dir`, named `backdrop.<ext>`. Returns the cache path on
/// success and `None` on any failure.
fn download(url: &str, dir: &Path, logger: Option<&Arc<Logger>>) -> Option<PathBuf> {
    let bytes = match profile::fetch_image(url) {
        Ok(bytes) => bytes,
        Err(error) => {
            debug(
                logger,
                &format!("cannot download backdrop from {url}: {error}"),
            );
            return None;
        }
    };
    let path = dir.join(format!("{ART_STEM}.{}", extension(url)));
    if let Err(error) = write_private(&path, &bytes) {
        debug(
            logger,
            &format!("cannot cache backdrop at {}: {error}", path.display()),
        );
        return None;
    }
    debug(
        logger,
        &format!("cached backdrop from {url} at {}", path.display()),
    );
    Some(path)
}

/// The file extension implied by a URL's path, defaulting to `jpg`.
fn extension(url: &str) -> &str {
    let without_query = url.split(['?', '#']).next().unwrap_or(url);
    let name = without_query.rsplit('/').next().unwrap_or("");
    match name.rsplit_once('.') {
        Some((_, ext)) if ext.len() <= 4 && ext.chars().all(|c| c.is_ascii_alphanumeric()) => ext,
        _ => "jpg",
    }
}

/// Writes `data` to `path` atomically with owner-only permissions, creating the
/// parent directory (0700) as needed.
fn write_private(path: &Path, data: &[u8]) -> std::io::Result<()> {
    use std::io::Write;
    use std::os::unix::ffi::OsStrExt;
    use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};

    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
        fs::set_permissions(parent, fs::Permissions::from_mode(0o700))?;
    }
    let mut tmp = path.as_os_str().as_bytes().to_vec();
    tmp.extend_from_slice(b".tmp");
    let tmp_path = PathBuf::from(std::ffi::OsStr::from_bytes(&tmp));
    {
        let mut file = fs::OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(true)
            .mode(0o600)
            .open(&tmp_path)?;
        file.write_all(data)?;
        file.sync_all()?;
    }
    fs::rename(&tmp_path, path)
}

fn debug(logger: Option<&Arc<Logger>>, message: &str) {
    if let Some(logger) = logger {
        logger.debug(message);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extension_from_url_path() {
        assert_eq!(extension("https://cdn.zwift.com/a/b/backdrop.jpg"), "jpg");
        assert_eq!(
            extension("https://cdn.zwift.com/a/b/backdrop.png?v=2"),
            "png"
        );
        assert_eq!(extension("https://cdn.zwift.com/a/b/backdrop#frag"), "jpg");
        assert_eq!(extension("https://cdn.zwift.com/a/b/noext"), "jpg");
    }

    #[test]
    fn configured_backdrop_wins_and_must_exist() {
        let dir = std::env::temp_dir().join(format!("backdrop-test-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        let image = dir.join("mine.png");
        fs::write(&image, b"x").unwrap();

        let resolved = resolve(Some(image.to_str().unwrap()), &dir, None);
        assert_eq!(resolved.as_deref(), Some(image.as_path()));

        let missing = dir.join("nope.png");
        // Falls through to the cache/download path; the empty dir has no cache
        // entry, so the download attempt fails and we get `None`.
        let resolved = resolve(Some(missing.to_str().unwrap()), &dir, None);
        assert!(resolved.is_none() || resolved.as_deref() != Some(missing.as_path()));

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn cached_backdrop_is_preferred_over_download() {
        let dir = std::env::temp_dir().join(format!("backdrop-cache-{}", std::process::id()));
        let art = dir.join(ART_DIR);
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&art).unwrap();
        let image = art.join("backdrop.webp");
        fs::write(&image, b"x").unwrap();

        let resolved = resolve(None, &dir, None);
        assert_eq!(resolved.as_deref(), Some(image.as_path()));

        let _ = fs::remove_dir_all(&dir);
    }
}
