//! The Zwift CDN boundary: URLs in, verified bytes out.
//!
//! Everything here is the port of the transfer layer of
//! [bpauli/zwift-bootstrap](https://github.com/bpauli/zwift-bootstrap) `src/net.rs`
//! (MIT OR Apache-2.0). The launcher downloads the game itself instead of running
//! `ZwiftSetup.exe`, because the official installer is an NSIS wrapper around the same
//! CDN: it seeds a prefix, writes `Zwift_ver_cur.xml`, and then `ZwiftLauncher.exe`
//! diffs the manifest. Skipping it removes the .NET launcher from the update path.
//!
//! Two rules are load-bearing and are repeated at each call site:
//!
//! * Every byte is verified against the manifest (length always, CRC-32 when the
//!   manifest carries one) before it is placed in the install directory.
//! * A `file://` base is a first-class source, so the whole sync path can be tested
//!   offline against a directory of fixture files.

use std::fs;
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};
use std::time::Duration;

use percent_encoding::{utf8_percent_encode, AsciiSet, CONTROLS};

use crate::manifest::{parse_manifest, parse_ver_cur, Manifest, RelPath, VerCur};
use crate::version::VER_CUR_FILE;

/// Characters escaped inside a single URL path component.
///
/// `%` must be escaped or a literal `%` in a file name would be read as an escape.
/// `/` and `\` are escaped too, but a manifest path never contains them: [`RelPath`]
/// splits on both, so the CDN never sees the `%5C` it answers with HTTP 403.
const PATH_SEGMENT: &AsciiSet = &CONTROLS
    .add(b' ')
    .add(b'"')
    .add(b'#')
    .add(b'%')
    .add(b'/')
    .add(b'<')
    .add(b'>')
    .add(b'?')
    .add(b'\\')
    .add(b'`')
    .add(b'{')
    .add(b'}');

/// Largest `Zwift_ver_cur.xml` accepted; the real one is a few hundred bytes.
const MAX_VER_CUR_BYTES: u64 = 64 * 1024;

/// Largest manifest accepted; the real one is around 6 MiB.
const MAX_MANIFEST_BYTES: u64 = 256 * 1024 * 1024;

/// Read chunk for downloads, and the throughput the per-file deadline is derived from.
const CHUNK_BYTES: u64 = 64 * 1024;

/// Lower bound for a file's deadline, so many small files still get a fair chance.
const MIN_FILE_TIMEOUT: Duration = Duration::from_secs(60);

/// Attempts per transfer, including the first.
const ATTEMPTS: u32 = 4;

/// The first backoff step; doubled for each further attempt.
const BACKOFF: Duration = Duration::from_secs(1);

/// What a downloaded file must look like. `checksum` is `None` when the manifest says
/// `-1`, which only pins the length.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Expect {
    pub length: u64,
    pub checksum: Option<u32>,
}

impl Expect {
    /// Whether a file of this size and checksum satisfies the expectation.
    pub fn matches(&self, length: u64, checksum: u32) -> bool {
        self.length == length && self.checksum.is_none_or(|want| want == checksum)
    }
}

/// Where the update files come from: the CDN, or a local directory for tests.
enum Source {
    Http(ureq::Agent),
    Local(PathBuf),
}

/// A CDN base URL plus the client used to reach it.
pub struct Remote {
    base: String,
    source: Source,
    backoff: Duration,
}

/// Where the game is fetched from when nothing overrides it. The game's own
/// patcher uses the same root.
pub const DEFAULT_BASE: &str = "https://cdn.zwift.com/gameassets/Zwift_Updates_Root/";

impl Remote {
    /// Build a client for `base`, for example
    /// `https://cdn.zwift.com/gameassets/Zwift_Updates_Root`. A `file://` base reads
    /// from the local filesystem instead, which is how the offline tests run the
    /// whole sync path without a network.
    pub fn new(base: &str) -> Remote {
        let base = base.trim_end_matches('/').to_string();
        let source = match base.strip_prefix("file://") {
            Some(root) => Source::Local(PathBuf::from(root)),
            None => {
                let agent = ureq::AgentBuilder::new()
                    .user_agent(concat!("zwift-snap/", env!("CARGO_PKG_VERSION")))
                    // Per-call deadlines are set per request; these bound the
                    // individual phases of a call that never reaches a body.
                    .timeout_connect(Duration::from_secs(30))
                    .timeout_read(MIN_FILE_TIMEOUT)
                    .build();
                Source::Http(agent)
            }
        };
        Remote {
            base,
            source,
            backoff: BACKOFF,
        }
    }

    /// Set the first retry delay, doubled for each further attempt. Zero makes
    /// retries immediate, which the tests use so a deliberate failure is fast.
    pub fn with_backoff(mut self, backoff: Duration) -> Remote {
        self.backoff = backoff;
        self
    }

    /// The served version file.
    pub fn ver_cur_url(&self) -> String {
        self.url(&[VER_CUR_FILE])
    }

    /// The manifest named by a `Zwift_ver_cur.xml`.
    pub fn manifest_url(&self, name: &str) -> String {
        self.url(&[name])
    }

    /// One game file: `{base}/{folder}/{path}`, with each component escaped.
    pub fn file_url(&self, folder: &str, path: &RelPath) -> String {
        let mut components = vec![folder];
        components.extend(path.components());
        self.url(&components)
    }

    fn url(&self, components: &[&str]) -> String {
        let encoded: Vec<String> = components
            .iter()
            .map(|c| utf8_percent_encode(c, PATH_SEGMENT).to_string())
            .collect();
        format!("{}/{}", self.base, encoded.join("/"))
    }

    /// Fetch and parse `Zwift_ver_cur.xml`, keeping the served bytes for the copy
    /// written into the install directory.
    pub fn fetch_ver_cur(&self) -> Result<VerCur, String> {
        let url = self.ver_cur_url();
        let bytes = self.get_small(&url, MAX_VER_CUR_BYTES)?;
        parse_ver_cur(&bytes).map_err(|e| format!("{url}: {e}"))
    }

    /// Fetch and parse a manifest, keeping the served bytes.
    ///
    /// `checksum` is the `manifest_checksum` from the ver_cur: when present it is
    /// verified, and the retry loop covers a truncated or corrupted transfer.
    pub fn fetch_manifest(
        &self,
        name: &str,
        checksum: Option<u32>,
    ) -> Result<(Manifest, Box<[u8]>), String> {
        let url = self.manifest_url(name);
        let mut last = String::new();
        for attempt in 0..ATTEMPTS {
            match self.get_small(&url, MAX_MANIFEST_BYTES) {
                Ok(bytes) => {
                    if let Some(want) = checksum {
                        let got = crc32fast::hash(&bytes);
                        if got != want {
                            last = format!("{url}: manifest checksum {got:#010x} != {want:#010x}");
                            self.backoff(attempt);
                            continue;
                        }
                    }
                    return parse_manifest(&bytes)
                        .map(|m| (m, bytes))
                        .map_err(|e| format!("{url}: {e}"));
                }
                Err(error) => {
                    last = error;
                    self.backoff(attempt);
                }
            }
        }
        Err(last)
    }

    /// Download one file to `dest`, verified against `expect`.
    ///
    /// The body is hashed while it is written, so a corrupt transfer never becomes a
    /// file on disk that later looks installed. A mismatch is retried: a proxy or a
    /// flaky link is far more likely than a wrong manifest.
    pub fn fetch_file(&self, url: &str, dest: &Path, expect: Expect) -> Result<(), String> {
        let deadline = MIN_FILE_TIMEOUT.max(Duration::from_secs(expect.length / CHUNK_BYTES));
        let mut last = String::new();
        for attempt in 0..ATTEMPTS {
            match self.get_stream(url, deadline) {
                Ok(reader) => match copy_verified(reader, dest, expect) {
                    Ok(()) => return Ok(()),
                    Err(error) => {
                        last = format!("{url}: {error}");
                        let _ = fs::remove_file(dest);
                        self.backoff(attempt);
                    }
                },
                Err(error) => {
                    last = error;
                    self.backoff(attempt);
                }
            }
        }
        Err(last)
    }

    /// Sleep between attempts, skipping the sleep after the last one.
    fn backoff(&self, attempt: u32) {
        if attempt + 1 < ATTEMPTS && !self.backoff.is_zero() {
            std::thread::sleep(self.backoff * 2u32.pow(attempt));
        }
    }

    /// Read a whole small object, refusing anything larger than `max`.
    fn get_small(&self, url: &str, max: u64) -> Result<Box<[u8]>, String> {
        let mut reader = self.get_stream(url, MIN_FILE_TIMEOUT)?;
        let mut bytes = Vec::new();
        // One byte over the limit distinguishes "exactly at the limit" from "too big".
        reader
            .by_ref()
            .take(max + 1)
            .read_to_end(&mut bytes)
            .map_err(|e| format!("{url}: {e}"))?;
        if bytes.len() as u64 > max {
            return Err(format!("{url}: larger than {max} bytes"));
        }
        Ok(bytes.into_boxed_slice())
    }

    /// Open a response body, mapping the transport's errors to text.
    fn get_stream(&self, url: &str, timeout: Duration) -> Result<Box<dyn Read>, String> {
        match &self.source {
            Source::Http(agent) => {
                let response = agent
                    .get(url)
                    .timeout(timeout)
                    .call()
                    .map_err(|error| describe(url, error))?;
                Ok(Box::new(response.into_reader()))
            }
            Source::Local(root) => {
                let path = local_path(root, url)?;
                let file = fs::File::open(&path).map_err(|e| format!("{}: {e}", path.display()))?;
                Ok(Box::new(file))
            }
        }
    }
}

/// The CDN boundary the sync works against.
///
/// This exists so the whole sync path can run against an in-memory source in unit
/// tests, and against a `file://` directory in integration tests, without a server.
/// [`Remote`] is the production implementation; every byte it returns has already
/// been verified (length always, CRC-32 when the manifest carries one).
pub trait Fetcher: Sync {
    /// Fetch and parse `Zwift_ver_cur.xml`, keeping the served bytes.
    fn fetch_ver_cur(&self) -> Result<VerCur, String>;

    /// Fetch and parse a manifest, keeping the served bytes.
    fn fetch_manifest(
        &self,
        name: &str,
        checksum: Option<u32>,
    ) -> Result<(Manifest, Box<[u8]>), String>;

    /// The URL of one game file.
    fn file_url(&self, folder: &str, path: &RelPath) -> String;

    /// Download one file to `dest`, verified against `expect`.
    fn fetch_file(&self, url: &str, dest: &Path, expect: Expect) -> Result<(), String>;
}

impl Fetcher for Remote {
    fn fetch_ver_cur(&self) -> Result<VerCur, String> {
        Remote::fetch_ver_cur(self)
    }

    fn fetch_manifest(
        &self,
        name: &str,
        checksum: Option<u32>,
    ) -> Result<(Manifest, Box<[u8]>), String> {
        Remote::fetch_manifest(self, name, checksum)
    }

    fn file_url(&self, folder: &str, path: &RelPath) -> String {
        Remote::file_url(self, folder, path)
    }

    fn fetch_file(&self, url: &str, dest: &Path, expect: Expect) -> Result<(), String> {
        Remote::fetch_file(self, url, dest, expect)
    }
}

/// Render a `ureq` failure. The status is kept because 404 and 503 mean different
/// things to the caller (a wrong manifest versus a transient outage).
fn describe(url: &str, error: ureq::Error) -> String {
    match error {
        ureq::Error::Status(code, _) => format!("{url}: HTTP {code}"),
        other => format!("{url}: {other}"),
    }
}

/// Map a URL back to a file under a `file://` root.
fn local_path(root: &Path, url: &str) -> Result<PathBuf, String> {
    let url_path = url.strip_prefix("file://").unwrap_or(url);
    let relative = url_path
        .strip_prefix(root.to_str().unwrap_or_default())
        .ok_or_else(|| format!("{url}: not under {root:?}"))?;
    let mut path = root.to_path_buf();
    for component in relative.trim_start_matches('/').split('/') {
        let decoded = percent_decode(component).ok_or_else(|| format!("{url}: bad escape"))?;
        if decoded.is_empty() || decoded == "." || decoded == ".." || decoded.contains('/') {
            return Err(format!("{url}: unsafe path component"));
        }
        path.push(decoded);
    }
    Ok(path)
}

/// Minimal percent-decoding for the local source, so a `file://` fixture can hold
/// files whose names the URL escaping would otherwise hide.
fn percent_decode(text: &str) -> Option<String> {
    let bytes = text.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] == b'%' {
            let hex = bytes.get(index + 1..index + 3)?;
            let text = std::str::from_utf8(hex).ok()?;
            out.push(u8::from_str_radix(text, 16).ok()?);
            index += 3;
        } else {
            out.push(bytes[index]);
            index += 1;
        }
    }
    String::from_utf8(out).ok()
}

/// Stream a body into `dest`, hashing as it goes, and verify it afterwards.
fn copy_verified(mut reader: Box<dyn Read>, dest: &Path, expect: Expect) -> Result<(), String> {
    if let Some(parent) = dest.parent() {
        fs::create_dir_all(parent).map_err(|e| format!("{}: {e}", parent.display()))?;
    }
    let file = fs::File::create(dest).map_err(|e| format!("{}: {e}", dest.display()))?;
    let mut writer = io::BufWriter::new(file);
    let mut hasher = crc32fast::Hasher::new();
    let mut buffer = vec![0u8; CHUNK_BYTES as usize];
    let mut total = 0u64;
    // One byte over the expected length catches a server that ignores the range.
    let mut limited = reader.by_ref().take(expect.length + 1);
    loop {
        let read = limited
            .read(&mut buffer)
            .map_err(|e| format!("read failed after {total} bytes: {e}"))?;
        if read == 0 {
            break;
        }
        hasher.update(&buffer[..read]);
        writer
            .write_all(&buffer[..read])
            .map_err(|e| format!("{}: {e}", dest.display()))?;
        total += read as u64;
    }
    writer
        .flush()
        .map_err(|e| format!("{}: {e}", dest.display()))?;
    drop(writer);
    let checksum = hasher.finalize();
    if !expect.matches(total, checksum) {
        return Err(format!(
            "expected {} bytes / {:#010x}, got {total} bytes / {checksum:#010x}",
            expect.length,
            expect.checksum.unwrap_or(0)
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture_root(name: &str) -> PathBuf {
        let root = std::env::temp_dir().join(format!(
            "zwift-cdn-{name}-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).unwrap();
        root
    }

    #[test]
    fn urls_escape_components_but_not_the_separators() {
        let remote = Remote::new("https://cdn.zwift.com/gameassets/Zwift_Updates_Root/");
        assert_eq!(
            remote.ver_cur_url(),
            "https://cdn.zwift.com/gameassets/Zwift_Updates_Root/Zwift_ver_cur.xml"
        );
        assert_eq!(
            remote.manifest_url("Zwift_1.0.1_ab_manifest.xml"),
            "https://cdn.zwift.com/gameassets/Zwift_Updates_Root/Zwift_1.0.1_ab_manifest.xml"
        );
        let path = RelPath::parse(r"data\Worlds\Map 04.ztx").unwrap();
        assert_eq!(
            remote.file_url("Zwift_1.0.1_ab", &path),
            "https://cdn.zwift.com/gameassets/Zwift_Updates_Root/Zwift_1.0.1_ab/data/Worlds/Map%2004.ztx"
        );
    }

    #[test]
    fn urls_escape_a_literal_percent_so_it_is_not_read_as_an_escape() {
        let remote = Remote::new("https://example.invalid/base");
        let path = RelPath::parse("100%25.txt").unwrap();
        assert_eq!(
            remote.file_url("F", &path),
            "https://example.invalid/base/F/100%2525.txt"
        );
    }

    #[test]
    fn local_base_reads_ver_cur_and_manifest_and_files() {
        let root = fixture_root("local");
        let base = format!("file://{}", root.display());
        let remote = Remote::new(&base).with_backoff(Duration::ZERO);
        let ver_cur = br#"<Zwift version="1.0.2" manifest="M_manifest.xml"/>"#;
        fs::write(root.join(VER_CUR_FILE), ver_cur).unwrap();
        fs::write(
            root.join("M_manifest.xml"),
            r#"<Zwift version="1.0.2" folder="F"><files><file><path>a.txt</path><length>5</length><checksum>-1</checksum></file></files></Zwift>"#,
        )
        .unwrap();
        fs::create_dir_all(root.join("F")).unwrap();
        fs::write(root.join("F").join("a.txt"), b"hello").unwrap();

        let parsed = remote.fetch_ver_cur().unwrap();
        assert_eq!(parsed.raw(), ver_cur);
        assert_eq!(parsed.manifest, "M_manifest.xml");
        let (manifest, raw) = remote.fetch_manifest("M_manifest.xml", None).unwrap();
        assert!(String::from_utf8_lossy(&raw).contains(r#"folder="F""#));
        let path = RelPath::parse("a.txt").unwrap();
        let entry = manifest.get(&path).unwrap();
        let url = remote.file_url(manifest.folder(), &path);
        let dest = root.join("out.txt");
        remote
            .fetch_file(
                &url,
                &dest,
                Expect {
                    length: entry.length,
                    checksum: entry.checksum,
                },
            )
            .unwrap();
        assert_eq!(fs::read(&dest).unwrap(), b"hello");
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn local_base_verifies_length_and_checksum_and_cleans_up_failures() {
        let root = fixture_root("verify");
        let remote =
            Remote::new(&format!("file://{}", root.display())).with_backoff(Duration::ZERO);
        fs::write(root.join("short.bin"), b"abc").unwrap();
        let url = format!("{}/short.bin", remote.base);

        let dest = root.join("out.bin");
        // Correct content, wrong length in the manifest.
        let error = remote
            .fetch_file(
                &url,
                &dest,
                Expect {
                    length: 10,
                    checksum: None,
                },
            )
            .unwrap_err();
        assert!(error.contains("expected 10 bytes"), "{error}");
        assert!(!dest.exists(), "a failed transfer must not leave a file");

        // Right length, wrong checksum.
        let error = remote
            .fetch_file(
                &url,
                &dest,
                Expect {
                    length: 3,
                    checksum: Some(0),
                },
            )
            .unwrap_err();
        assert!(error.contains("expected 3 bytes"), "{error}");
        assert!(!dest.exists());

        // A missing file is an error, not an empty file.
        let error = remote
            .fetch_file(
                &format!("{}/absent.bin", remote.base),
                &dest,
                Expect {
                    length: 1,
                    checksum: None,
                },
            )
            .unwrap_err();
        assert!(error.contains("absent.bin"), "{error}");
        assert!(!dest.exists());
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn local_base_refuses_to_escape_its_root() {
        let root = fixture_root("escape");
        let remote = Remote::new(&format!("file://{}", root.display()));
        let error = remote.get_small("file:///etc/passwd", 4096).unwrap_err();
        assert!(error.contains("not under"), "{error}");
        let error = remote
            .get_small(&format!("{}/../escape", remote.base), 4096)
            .unwrap_err();
        assert!(error.contains("unsafe path component"), "{error}");
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn oversized_small_objects_are_rejected() {
        let root = fixture_root("oversize");
        let remote = Remote::new(&format!("file://{}", root.display()));
        fs::write(root.join(VER_CUR_FILE), vec![b'x'; 32]).unwrap();
        assert!(remote.get_small(&remote.ver_cur_url(), 32).is_ok());
        let error = remote.get_small(&remote.ver_cur_url(), 31).unwrap_err();
        assert!(error.contains("larger than 31 bytes"), "{error}");
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn expect_matches_uses_length_always_and_checksum_when_present() {
        let with = Expect {
            length: 4,
            checksum: Some(7),
        };
        assert!(with.matches(4, 7));
        assert!(!with.matches(4, 8));
        assert!(!with.matches(5, 7));
        let without = Expect {
            length: 4,
            checksum: None,
        };
        assert!(without.matches(4, 0));
        assert!(without.matches(4, 999));
        assert!(!without.matches(5, 0));
    }
}
