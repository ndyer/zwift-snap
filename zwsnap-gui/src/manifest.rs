//! The Zwift update XML boundary: bytes in, domain types out.
//!
//! Parsers for `Zwift_ver_cur.xml` and the update manifest, ported from
//! [bpauli/zwift-bootstrap](https://github.com/bpauli/zwift-bootstrap) `src/wire.rs`
//! (MIT OR Apache-2.0, the same license as this repository). Nothing outside this
//! module sees `roxmltree`, element names or attribute names.
//!
//! The same parsers read the server copies and the local copies kept in the install
//! directory: a local file is no more trusted than a remote one, because it may come
//! from an older run or another installer.

use std::collections::BTreeMap;
use std::time::{Duration, SystemTime};

use roxmltree::{Document, Node};

use crate::version::Version;

/// A manifest path, stored with `/` separators regardless of the `\` the manifest uses.
///
/// Constructed only through [`RelPath::parse`], which rejects anything that could
/// escape the install directory or collide with the files the launcher owns.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct RelPath(String);

impl RelPath {
    /// Parse a manifest path (`\`- or `/`-separated) into a safe relative path.
    pub fn parse(raw: &str) -> Result<RelPath, String> {
        let mut components: Vec<&str> = Vec::new();
        for component in raw.split(['\\', '/']) {
            match component {
                "" => return Err("empty path component".to_string()),
                "." | ".." => return Err(format!("path component {component:?} is not allowed")),
                c if c.contains(':') => return Err(format!("path component {c:?} is not allowed")),
                c => components.push(c),
            }
        }
        if components.is_empty() {
            return Err("empty path".to_string());
        }
        // `Zwift_ver_cur.xml` is written by the launcher from the server's own bytes,
        // never taken from a game file, so the manifest must not claim it.
        if components.len() == 1 && components[0] == crate::version::VER_CUR_FILE {
            return Err(format!("path {raw:?} is reserved for the launcher"));
        }
        Ok(RelPath(components.join("/")))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// The path components, split on `/`.
    pub fn components(&self) -> impl Iterator<Item = &str> {
        self.0.split('/')
    }
}

impl std::fmt::Display for RelPath {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

/// One file's expected content. `checksum` is `None` when the manifest says `-1`
/// ("absent"), which is never proof that a local copy is correct.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FileEntry {
    pub length: u64,
    pub checksum: Option<u32>,
    pub modified: Option<SystemTime>,
}

/// A parsed update manifest: the file set for one server version.
#[derive(Clone, Debug)]
pub struct Manifest {
    version: Version,
    folder: String,
    files: BTreeMap<RelPath, FileEntry>,
}

/// Why a manifest's file set was rejected.
#[derive(Debug, PartialEq, Eq)]
pub enum ManifestRejection {
    Duplicate(RelPath),
    CaseCollision(String, String),
    FileIsAlsoDir(RelPath),
}

impl std::fmt::Display for ManifestRejection {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ManifestRejection::Duplicate(p) => write!(f, "duplicate path {p}"),
            ManifestRejection::CaseCollision(a, b) => {
                write!(f, "paths {a} and {b} differ only in case")
            }
            ManifestRejection::FileIsAlsoDir(p) => write!(f, "{p} is both a file and a directory"),
        }
    }
}

impl Manifest {
    /// A manifest with no files: the state of a prefix with no game installed, so a
    /// diff against it fetches everything.
    pub fn empty() -> Manifest {
        Manifest {
            version: Version::ZERO,
            folder: String::new(),
            files: BTreeMap::new(),
        }
    }

    /// Build a manifest, rejecting sets the filesystem cannot represent faithfully.
    pub fn new(
        version: Version,
        folder: String,
        entries: Vec<(RelPath, FileEntry)>,
    ) -> Result<Manifest, ManifestRejection> {
        let mut files: BTreeMap<RelPath, FileEntry> = BTreeMap::new();
        let mut folded: BTreeMap<String, RelPath> = BTreeMap::new();
        for (path, entry) in entries {
            if files.contains_key(&path) {
                return Err(ManifestRejection::Duplicate(path));
            }
            let lower = path.as_str().to_lowercase();
            if let Some(other) = folded.get(&lower) {
                if other != &path {
                    return Err(ManifestRejection::CaseCollision(
                        other.to_string(),
                        path.to_string(),
                    ));
                }
            }
            folded.insert(lower, path.clone());
            files.insert(path, entry);
        }
        // A path that is also a directory prefix of another path cannot exist on disk.
        for path in files.keys() {
            let mut prefix = String::new();
            let components: Vec<&str> = path.components().collect();
            for component in &components[..components.len().saturating_sub(1)] {
                if !prefix.is_empty() {
                    prefix.push('/');
                }
                prefix.push_str(component);
                if files.contains_key(&RelPath(prefix.clone())) {
                    return Err(ManifestRejection::FileIsAlsoDir(RelPath(prefix)));
                }
            }
        }
        Ok(Manifest {
            version,
            folder,
            files,
        })
    }

    pub fn version(&self) -> Version {
        self.version
    }

    pub fn folder(&self) -> &str {
        &self.folder
    }

    pub fn files(&self) -> &BTreeMap<RelPath, FileEntry> {
        &self.files
    }

    pub fn get(&self, path: &RelPath) -> Option<&FileEntry> {
        self.files.get(path)
    }
}

/// The parsed `Zwift_ver_cur.xml`.
#[derive(Clone, Debug)]
pub struct VerCur {
    pub version: Version,
    /// Manifest file name, relative to the CDN base.
    pub manifest: String,
    /// CRC-32 of the manifest, or `None` when the ver_cur has no (or a `-1`) checksum.
    pub manifest_checksum: Option<u32>,
    pub mandatory_version: Option<Version>,
    raw: Box<[u8]>,
}

impl VerCur {
    /// The served bytes, written verbatim into the install directory.
    pub fn raw(&self) -> &[u8] {
        &self.raw
    }
}

/// Parse `Zwift_ver_cur.xml`.
///
/// Root `Zwift`. Required: `version`, `manifest`. Optional: `manifest_checksum`
/// (missing = absent) and `mandatory_version`. Ignored: `sversion`, `gbranch`,
/// `gcommit`, `GAME_URL`, `ver_cur_checksum`.
pub fn parse_ver_cur(bytes: &[u8]) -> Result<VerCur, String> {
    let text = std::str::from_utf8(bytes).map_err(|e| e.to_string())?;
    let doc = Document::parse(text).map_err(|e| e.to_string())?;
    let root = zwift_root(&doc)?;
    let version = Version::parse(attr(root, "version")?)?;
    let manifest = manifest_name(attr(root, "manifest")?)?;
    let manifest_checksum = match root.attribute("manifest_checksum") {
        Some(text) => parse_checksum(text)?,
        None => None,
    };
    let mandatory_version = root
        .attribute("mandatory_version")
        .map(Version::parse)
        .transpose()?;
    Ok(VerCur {
        version,
        manifest,
        manifest_checksum,
        mandatory_version,
        raw: bytes.into(),
    })
}

/// Parse an update manifest.
///
/// Root `Zwift` with `version` and `folder`, a child `files`, children `file` with
/// `path`, `length`, `checksum` and an optional `modified`. Unknown elements are
/// ignored so new server fields do not break parsing.
pub fn parse_manifest(bytes: &[u8]) -> Result<Manifest, String> {
    let text = std::str::from_utf8(bytes).map_err(|e| e.to_string())?;
    let doc = Document::parse(text).map_err(|e| e.to_string())?;
    let root = zwift_root(&doc)?;
    let version = Version::parse(attr(root, "version")?)?;
    let folder = server_folder(attr(root, "folder")?)?;
    let files = only_child(root, "files")?.ok_or("no <files> element")?;
    let mut entries = Vec::new();
    for file in files.children().filter(|n| n.has_tag_name("file")) {
        let raw_path = required_text(file, "path")?;
        let path = RelPath::parse(raw_path).map_err(|e| format!("path {raw_path:?}: {e}"))?;
        let length_text = required_text(file, "length")?;
        let length = length_text
            .trim()
            .parse::<u64>()
            .map_err(|_| format!("{path}: invalid length {length_text:?}"))?;
        let checksum =
            parse_checksum(required_text(file, "checksum")?).map_err(|e| format!("{path}: {e}"))?;
        let modified = only_child(file, "modified")
            .ok()
            .flatten()
            .and_then(|n| n.text())
            .and_then(|t| parse_modified(t.trim()).ok());
        entries.push((
            path,
            FileEntry {
                length,
                checksum,
                modified,
            },
        ));
    }
    Manifest::new(version, folder, entries).map_err(|rejection| rejection.to_string())
}

/// A CRC-32 field: a *signed* 32-bit decimal, so `-1563255996` is the CRC
/// `0xa2ce1e04`. `-1` means "absent".
fn parse_checksum(text: &str) -> Result<Option<u32>, String> {
    let value = text
        .trim()
        .parse::<i64>()
        .map_err(|_| format!("invalid checksum {text:?}"))?;
    if value == -1 {
        return Ok(None);
    }
    i32::try_from(value)
        .map(|value| Some(value as u32))
        .map_err(|_| format!("invalid checksum {text:?}"))
}

/// A manifest file name: a single component, no separators, no traversal, and not the
/// version file itself, which the launcher owns and writes from the server's bytes.
fn manifest_name(text: &str) -> Result<String, String> {
    if text.is_empty() || text.contains(['/', '\\']) || text.contains("..") {
        return Err(format!("invalid manifest name {text:?}"));
    }
    if text == crate::version::VER_CUR_FILE {
        return Err(format!(
            "manifest name {text:?} is reserved for the launcher"
        ));
    }
    Ok(text.to_string())
}

/// The manifest's server folder name: a single component, no separators.
fn server_folder(text: &str) -> Result<String, String> {
    if text.is_empty() || text.contains(['/', '\\']) || text.contains("..") {
        return Err(format!("invalid server folder {text:?}"));
    }
    Ok(text.to_string())
}

fn zwift_root<'a>(doc: &'a Document) -> Result<Node<'a, 'a>, String> {
    let root = doc.root_element();
    if !root.has_tag_name("Zwift") {
        return Err(format!(
            "root element is <{}>, not <Zwift>",
            root.tag_name().name()
        ));
    }
    Ok(root)
}

fn attr<'a>(node: Node<'a, '_>, name: &str) -> Result<&'a str, String> {
    node.attribute(name)
        .ok_or_else(|| format!("<{}> has no {name} attribute", node.tag_name().name()))
}

/// At most one child element with this name.
fn only_child<'a, 'i>(node: Node<'a, 'i>, name: &str) -> Result<Option<Node<'a, 'i>>, String> {
    let mut matches = node.children().filter(|n| n.has_tag_name(name));
    let first = matches.next();
    if matches.next().is_some() {
        return Err(format!("more than one <{name}> element"));
    }
    Ok(first)
}

fn required_text<'a>(node: Node<'a, '_>, name: &str) -> Result<&'a str, String> {
    only_child(node, name)?
        .ok_or_else(|| format!("<file> has no <{name}>"))
        .map(|n| n.text().unwrap_or_default())
}

/// `YYYY-MM-DD HH:MM:SS.mmm`, read as UTC. Days-from-civil arithmetic; no chrono.
fn parse_modified(text: &str) -> Result<SystemTime, String> {
    let bad = || format!("invalid time {text:?}");
    let (date, time) = text.split_once(' ').ok_or_else(bad)?;
    let (hms, millis) = time.split_once('.').unwrap_or((time, "0"));
    let numbers = |s: &str, sep: char| -> Result<Vec<u32>, String> {
        s.split(sep)
            .map(|n| n.parse::<u32>().map_err(|_| bad()))
            .collect()
    };
    let date = numbers(date, '-')?;
    let hms = numbers(hms, ':')?;
    let [year, month, day] = date[..] else {
        return Err(bad());
    };
    let [hour, minute, second] = hms[..] else {
        return Err(bad());
    };
    let millis: u32 = millis.parse().map_err(|_| bad())?;
    if !(1970..=9999).contains(&year)
        || !(1..=12).contains(&month)
        || !(1..=31).contains(&day)
        || hour > 23
        || minute > 59
        || second > 60
        || millis > 999
    {
        return Err(bad());
    }
    let days = days_from_civil(i64::from(year), i64::from(month), i64::from(day));
    let secs = days * 86_400 + i64::from(hour * 3600 + minute * 60 + second);
    let secs = u64::try_from(secs).map_err(|_| bad())?;
    Ok(SystemTime::UNIX_EPOCH + Duration::from_secs(secs) + Duration::from_millis(millis.into()))
}

/// Days since 1970-01-01 in the proleptic Gregorian calendar (Howard Hinnant's algorithm).
fn days_from_civil(year: i64, month: i64, day: i64) -> i64 {
    let year = if month <= 2 { year - 1 } else { year };
    let era = year.div_euclid(400);
    let year_of_era = year - era * 400;
    let month_index = (month + 9) % 12;
    let day_of_year = (153 * month_index + 2) / 5 + day - 1;
    let day_of_era = year_of_era * 365 + year_of_era / 4 - year_of_era / 100 + day_of_year;
    era * 146_097 + day_of_era - 719_468
}

#[cfg(test)]
mod tests {
    use super::*;

    const VER_CUR: &str = "<Zwift version=\"1.0.167050\" sversion=\"1.123.0 (167050)\" gbranch=\"rc/1.123.0\" gcommit=\"bd4bbe7a\" GAME_URL=\"https://us-or-rly101.zwift.com\" manifest=\"Zwift_1.0.167050_ce7420cb_manifest.xml\" manifest_checksum=\"667523848\" ver_cur_checksum=\"1134193414\"/>\r\n";

    const MANIFEST: &str = concat!(
        "<Zwift version=\"1.0.167050\" sversion=\"1.123.0 (167050)\" folder=\"Zwift_1.0.167050_ce7420cb\">\r\n",
        "    <files>\r\n",
        "        <file>\r\n",
        "            <path>data\\Worlds\\world9\\map04.ztx</path>\r\n",
        "            <length>60</length>\r\n",
        "            <modified>2026-10-01 17:32:38.235</modified>\r\n",
        "            <checksum>351459945</checksum>\r\n",
        "        </file>\r\n",
        "        <file>\r\n",
        "            <path>ZwiftApp.exe</path>\r\n",
        "            <length>61</length>\r\n",
        "            <modified>yesterday</modified>\r\n",
        "            <checksum>-1</checksum>\r\n",
        "            <future>ignored</future>\r\n",
        "        </file>\r\n",
        "    </files>\r\n",
        "</Zwift>\r\n",
    );

    fn manifest_with_paths(paths: &[&str]) -> String {
        let files: String = paths
            .iter()
            .map(|p| {
                format!("<file><path>{p}</path><length>1</length><checksum>5</checksum></file>")
            })
            .collect();
        format!("<Zwift version=\"1.0.1\" folder=\"F\"><files>{files}</files></Zwift>")
    }

    #[test]
    fn ver_cur_keeps_served_bytes_verbatim_including_crlf() {
        let v = parse_ver_cur(VER_CUR.as_bytes()).unwrap();
        assert_eq!(v.raw(), VER_CUR.as_bytes());
        assert_eq!(v.version.to_string(), "1.0.167050");
        assert_eq!(v.manifest, "Zwift_1.0.167050_ce7420cb_manifest.xml");
        assert_eq!(v.manifest_checksum, Some(667523848));
        assert_eq!(v.mandatory_version, None);
    }

    #[test]
    fn windows_seed_ver_cur_parses_with_absent_checksum() {
        let seed = br#"<Zwift version="0.0.0" GAME_URL="https://us-or-rly101.zwift.com" manifest="Zwift_0.0.0_manifest.xml"/>"#;
        let v = parse_ver_cur(seed).unwrap();
        assert_eq!(v.version, Version::ZERO);
        assert_eq!(v.manifest_checksum, None);
    }

    #[test]
    fn ver_cur_with_mandatory_version() {
        let xml =
            br#"<Zwift version="1.0.5" manifest="a_manifest.xml" mandatory_version="1.0.3"/>"#;
        let v = parse_ver_cur(xml).unwrap();
        assert_eq!(v.mandatory_version, Some(Version::parse("1.0.3").unwrap()));
    }

    #[test]
    fn ver_cur_rejects_bad_input() {
        for bad in [
            &br#"<Other version="1.0.0" manifest="a_manifest.xml"/>"#[..],
            br#"<Zwift manifest="a_manifest.xml"/>"#,
            br#"<Zwift version="1.0.0"/>"#,
            br#"<Zwift version="1.0.0" manifest="Zwift_ver_cur.xml"/>"#,
            br#"<Zwift version="1.0.0" manifest="../a_manifest.xml"/>"#,
            br#"<Zwift version="1.0.0" manifest="a_manifest.xml" manifest_checksum="x"/>"#,
            br#"<Zwift version="1.0.0" manifest="a_manifest.xml" mandatory_version="soon"/>"#,
            b"\xff",
            b"<Zwift",
        ] {
            assert!(
                parse_ver_cur(bad).is_err(),
                "{}",
                String::from_utf8_lossy(bad)
            );
        }
    }

    #[test]
    fn manifest_parses_entries_minus_one_checksum_and_modified_time() {
        let m = parse_manifest(MANIFEST.as_bytes()).unwrap();
        assert_eq!(m.version().to_string(), "1.0.167050");
        assert_eq!(m.folder(), "Zwift_1.0.167050_ce7420cb");
        assert_eq!(m.files().len(), 2);
        let map04 = m
            .get(&RelPath::parse(r"data\Worlds\world9\map04.ztx").unwrap())
            .unwrap();
        assert_eq!(map04.length, 60);
        assert_eq!(map04.checksum, Some(351459945));
        let expected = SystemTime::UNIX_EPOCH + Duration::from_millis(1_790_875_958_235);
        assert_eq!(map04.modified, Some(expected));
        let app = m.get(&RelPath::parse("ZwiftApp.exe").unwrap()).unwrap();
        assert_eq!(app.checksum, None);
        assert_eq!(app.modified, None);
    }

    #[test]
    fn checksum_is_a_signed_32_bit_crc() {
        // The CDN prints the CRC as a signed decimal, as the real manifest does for
        // roughly half of its files.
        let xml = "<Zwift version=\"1.0.1\" folder=\"F\"><files>\
            <file><path>a</path><length>1</length><checksum>-1563255996</checksum></file>\
            </files></Zwift>";
        let m = parse_manifest(xml.as_bytes()).unwrap();
        // 4294967296 - 1563255996.
        assert_eq!(
            m.get(&RelPath::parse("a").unwrap()).unwrap().checksum,
            Some(2_731_711_300)
        );
        // 0xffffffff is the signed -1, which means "absent", not the CRC 0xffffffff.
        let absent = "<Zwift version=\"1.0.1\" folder=\"F\"><files>\
            <file><path>a</path><length>1</length><checksum>-1</checksum></file>\
            </files></Zwift>";
        let m = parse_manifest(absent.as_bytes()).unwrap();
        assert_eq!(m.get(&RelPath::parse("a").unwrap()).unwrap().checksum, None);
        // Outside i32 is not a CRC at all.
        let bad = "<Zwift version=\"1.0.1\" folder=\"F\"><files>\
            <file><path>a</path><length>1</length><checksum>4294967295</checksum></file>\
            </files></Zwift>";
        assert!(parse_manifest(bad.as_bytes()).is_err());
    }

    #[test]
    fn manifest_rejects_hostile_paths_and_sets() {
        for paths in [
            &[r"..\x"][..],
            &[r"a\..\b"],
            &[r"\abs"],
            &[r"C:\x"],
            &[r"a\\b"],
            &["a/../b"],
            &["Zwift_ver_cur.xml"],
            &["A", "a"],
            &["a", r"a\b"],
        ] {
            let xml = manifest_with_paths(paths);
            assert!(parse_manifest(xml.as_bytes()).is_err(), "{paths:?}");
        }
    }

    #[test]
    fn manifest_rejects_malformed_entries() {
        let file = |inner: &str| {
            format!(
                "<Zwift version=\"1.0.1\" folder=\"F\"><files><file>{inner}</file></files></Zwift>"
            )
        };
        for bad in [
            "<Zwift version=\"1.0.1\" folder=\"F\"></Zwift>".to_owned(),
            "<Zwift version=\"1.0.1\" folder=\"a/b\"><files/></Zwift>".to_owned(),
            file("<path>a</path><checksum>1</checksum>"),
            file("<path>a</path><length>-1</length><checksum>1</checksum>"),
            file("<path>a</path><path>b</path><length>1</length><checksum>1</checksum>"),
            "<!DOCTYPE z [<!ENTITY a \"aaaa\">]><Zwift version=\"1.0.1\" folder=\"F\"><files/></Zwift>"
                .to_owned(),
        ] {
            assert!(parse_manifest(bad.as_bytes()).is_err(), "{bad}");
        }
    }

    #[test]
    fn modified_time_is_utc_and_malformed_values_are_errors() {
        assert_eq!(
            parse_modified("2000-02-29 00:00:00.000"),
            Ok(SystemTime::UNIX_EPOCH + Duration::from_secs(951_782_400))
        );
        for bad in [
            "2026-13-01 00:00:00.000",
            "2026-10-01",
            "2026-10-01 25:00:00.0",
            "1969-12-31 23:59:59.0",
        ] {
            assert!(parse_modified(bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn rel_path_normalises_separators_and_components() {
        assert_eq!(
            RelPath::parse(r"data\Worlds\a.ztx").unwrap().as_str(),
            "data/Worlds/a.ztx"
        );
        assert_eq!(RelPath::parse("a/b").unwrap().as_str(), "a/b");
        assert_eq!(
            RelPath::parse("ZwiftApp.exe").unwrap().as_str(),
            "ZwiftApp.exe"
        );
        for bad in ["", "/", "a//b", "..", "a/./b", "C:/x", "a:b/c"] {
            assert!(RelPath::parse(bad).is_err(), "{bad:?}");
        }
    }

    /// Parse a manifest the CDN actually served, to keep the parser honest about the
    /// real format (backslash paths, `-1` checksums, CRLF, element order).
    ///
    /// ```text
    /// ZWIFT_TEST_MANIFEST=~/zwift-cache/game/Zwift_1.0.168455_e1746cce_manifest.xml \
    ///     cargo test --lib -- --ignored parses_a_real_manifest
    /// ```
    #[test]
    #[ignore = "needs a real manifest in ZWIFT_TEST_MANIFEST"]
    fn parses_a_real_manifest() {
        let path = std::env::var("ZWIFT_TEST_MANIFEST").expect("set ZWIFT_TEST_MANIFEST");
        let bytes = std::fs::read(&path).unwrap();
        let manifest = parse_manifest(&bytes).unwrap();
        assert!(
            manifest.files().len() > 1000,
            "{} files",
            manifest.files().len()
        );
        assert!(manifest.version() > Version::ZERO);
        assert!(!manifest.folder().is_empty());
    }
}
