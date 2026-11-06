//! Version helpers, the counterpart of `version_ge` and `get_current_version`/`get_latest_version`.

use std::fs;
use std::path::Path;

/// Version meaning "Zwift is not installed".
pub const NONE: &str = "none";

fn missing(v: &str) -> bool {
    v.is_empty() || v == NONE
}

fn parts(v: &str) -> Vec<u64> {
    v.split('.').map(|p| p.parse().unwrap_or(0)).collect()
}

/// True when `a` is the same as or newer than `b`. A missing version is older than any version.
pub fn version_ge(a: &str, b: &str) -> bool {
    if missing(a) {
        return false;
    }
    if missing(b) {
        return true;
    }
    let (mut pa, mut pb) = (parts(a), parts(b));
    let len = pa.len().max(pb.len());
    pa.resize(len, 0);
    pb.resize(len, 0);
    pa >= pb
}

/// First `sversion="..."` value of a Zwift version XML (terminated by a quote or whitespace).
pub fn parse_sversion(xml: &str) -> Option<String> {
    let rest = &xml[xml.find("sversion=\"")? + "sversion=\"".len()..];
    let end = rest.find(|c: char| c == '"' || c.is_whitespace())?;
    let v = &rest[..end];
    (!v.is_empty()).then(|| v.to_string())
}

/// Installed version, or `none`. `Zwift_ver_cur_filename.txt` (NUL padded) names the real
/// version file, else `Zwift_ver_cur.xml` is used. The installer's 0.0.0 placeholder has
/// no sversion and gives `none`.
pub fn current_version(install_dir: &Path) -> String {
    let mut name = "Zwift_ver_cur.xml".to_string();
    if let Ok(raw) = fs::read(install_dir.join("Zwift_ver_cur_filename.txt")) {
        let text = String::from_utf8_lossy(&raw).replace('\0', "\n");
        if let Some(line) = text.lines().next().filter(|l| !l.is_empty()) {
            name = line.to_string();
        }
    }
    fs::read_to_string(install_dir.join(name))
        .ok()
        .and_then(|xml| parse_sversion(&xml))
        .unwrap_or_else(|| NONE.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ge() {
        assert!(version_ge("1.125.0", "1.124.0"));
        assert!(version_ge("1.124.0", "1.124.0"));
        assert!(!version_ge("1.123.9", "1.124.0"));
        assert!(!version_ge("1.9.0", "1.10.0"));
        assert!(!version_ge("none", "1.124.0"));
        assert!(!version_ge("", "1.124.0"));
        assert!(version_ge("1.124.0", "none"));
    }

    #[test]
    fn sversion() {
        let xml = r#"<Zwift version="1.1" sversion="1.125.0 (abc)" manifest="x"/>"#;
        assert_eq!(parse_sversion(xml).as_deref(), Some("1.125.0"));
        assert_eq!(parse_sversion(r#"<Zwift version="0.0.0"/>"#), None);
    }

    #[test]
    fn current_placeholder_is_none() {
        let dir = std::env::temp_dir().join(format!("zwift-gui-ver-{}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        fs::write(
            dir.join("Zwift_ver_cur.xml"),
            r#"<Zwift version="0.0.0" GAME_URL="x"/>"#,
        )
        .unwrap();
        assert_eq!(current_version(&dir), "none");
        fs::write(dir.join("Zwift_ver_cur_filename.txt"), b"Other.xml\0\0").unwrap();
        fs::write(dir.join("Other.xml"), r#"<Zwift sversion="1.2.3 x"/>"#).unwrap();
        assert_eq!(current_version(&dir), "1.2.3");
    }
}
