//! Version helpers, the counterpart of `version_ge` and `get_current_version`/`get_latest_version`.

use std::fs;
use std::path::Path;

/// Version meaning "Zwift is not installed".
pub const NONE: &str = "none";

/// The served version file. The launcher writes the server's own bytes here after
/// every sync, so it doubles as the record of what the local install contains.
pub const VER_CUR_FILE: &str = "Zwift_ver_cur.xml";

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

/// The manifest `version` attribute, `major.minor.build` (for example `1.0.167050`).
///
/// This is the build counter the CDN uses to decide whether an update exists; it is
/// distinct from the human-facing `sversion` (for example `1.123.0 (167050)`), which
/// stays in use for display and the smoke test's cache.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Version {
    pub major: u64,
    pub minor: u64,
    pub build: u64,
}

impl Version {
    /// The version the Windows installer seeds before any game is downloaded.
    pub const ZERO: Version = Version {
        major: 0,
        minor: 0,
        build: 0,
    };

    /// Parse `major.minor.build`; exactly three numeric components are required.
    pub fn parse(text: &str) -> Result<Version, String> {
        let text = text.trim();
        let mut parts = text.split('.');
        let mut next = || -> Result<u64, String> {
            let part = parts
                .next()
                .ok_or_else(|| format!("version {text:?} needs major.minor.build"))?;
            part.parse::<u64>()
                .map_err(|_| format!("invalid version {text:?}"))
        };
        let major = next()?;
        let minor = next()?;
        let build = next()?;
        if parts.next().is_some() {
            return Err(format!("version {text:?} has too many components"));
        }
        Ok(Version {
            major,
            minor,
            build,
        })
    }
}

impl std::fmt::Display for Version {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}.{}.{}", self.major, self.minor, self.build)
    }
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
    let mut name = VER_CUR_FILE.to_string();
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
    fn version_parse_and_order() {
        let v = Version::parse("1.0.167050").unwrap();
        assert_eq!((v.major, v.minor, v.build), (1, 0, 167050));
        assert_eq!(v.to_string(), "1.0.167050");
        assert_eq!(Version::parse(" 1.2.3 ").unwrap().to_string(), "1.2.3");
        assert_eq!(Version::parse("0.0.0").unwrap(), Version::ZERO);
        assert!(Version::parse("1.0.167051").unwrap() > v);
        assert!(Version::parse("1.1.0").unwrap() > Version::parse("1.0.167050").unwrap());
        for bad in ["1.0", "1.0.1.2", "1.0.x", "", "1..0", "-1.0.0"] {
            assert!(Version::parse(bad).is_err(), "{bad:?}");
        }
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
