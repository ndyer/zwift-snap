//! Fixtures shared by the unit tests of the direct-CDN download path.
//!
//! Only compiled for tests: it writes a `file://` CDN tree that [`crate::cdn::Remote`]
//! reads as if it were the real one, which lets the whole sync run without a network.

use std::fs;
use std::path::Path;

/// The version the fixture CDN serves.
pub const SERVER_VERSION: &str = "1.0.2";

/// Write a CDN tree under `root`: the served version file, one manifest and the game
/// files it lists, under the manifest's `folder` (`v1`).
///
/// `files` are `(relative path, content)` pairs; the checksums are computed the way the
/// real CDN records them.
pub fn write_cdn(root: &Path, version: &str, files: &[(&str, &str)]) {
    let mut entries = String::new();
    for (path, content) in files {
        let file = root.join("v1").join(path);
        fs::create_dir_all(file.parent().unwrap()).unwrap();
        fs::write(&file, content).unwrap();
        entries.push_str(&format!(
            "<file><path>{}</path><length>{}</length>\
             <modified>2024-01-02 03:04:05.000</modified><checksum>{}</checksum></file>",
            path.replace('/', "\\"),
            content.len(),
            // The CDN records CRCs as signed decimals, so a CRC above 2^31 is
            // negative in the manifest.
            crc32fast::hash(content.as_bytes()) as i32
        ));
    }
    fs::write(
        root.join("manifest.xml"),
        format!("<Zwift version=\"{version}\" folder=\"v1\"><files>{entries}</files></Zwift>"),
    )
    .unwrap();
    fs::write(
        root.join("Zwift_ver_cur.xml"),
        format!(
            "<Zwift version=\"{version}\" sversion=\"{version} (168455)\" \
             manifest=\"manifest.xml\"/>"
        ),
    )
    .unwrap();
}
