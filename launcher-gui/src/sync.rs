//! The update decision, the diff between the installed file set and the served one,
//! and the sync that carries it out.
//!
//! The plan half is the port of [bpauli/zwift-bootstrap](https://github.com/bpauli/zwift-bootstrap)
//! `src/plan.rs` (MIT OR Apache-2.0); the sync is a simplification of its `src/sync.rs`.
//! The simplification the project chose is to drop the pending-marker typestate machine
//! that makes a sync crash-safe in favour of a rule that needs no extra state: the
//! version files (`Zwift_ver_cur.xml` and the manifest copy) are written **last**, so a
//! run that dies partway leaves the previous version recorded and the next run simply
//! diffs again and finishes the job.
//!
//! The version key is the `version` attribute (for example `1.0.168455`), not the
//! human-facing `sversion`; the latter is kept for display and the smoke test's cache.

use std::fs;
use std::io::ErrorKind;
use std::path::Path;

use crate::cdn::Fetcher;
use crate::download::{download, DownloadConfig};
use crate::logging::Logger;
use crate::manifest::{parse_manifest, parse_ver_cur, FileEntry, Manifest, RelPath, VerCur};
use crate::version::{Version, VER_CUR_FILE};

/// The pointer file the Windows installer wrote to name the real version file.
const VER_CUR_FILENAME_FILE: &str = "Zwift_ver_cur_filename.txt";

/// Where a sync gets its files and where it puts them.
pub struct SyncConfig<'a> {
    /// The Zwift installation directory (`.../drive_c/Program Files (x86)/Zwift`).
    pub install_dir: &'a Path,
    /// Scratch space for in-flight downloads; ideally on the install directory's
    /// filesystem, so placing a file is a rename.
    pub staging_dir: &'a Path,
    /// Parallel transfers.
    pub jobs: usize,
}

/// What the served version means for the local install.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Assessment {
    /// The local install already carries this version.
    Current,
    /// The served version differs. `mandatory` marks an update the server insists on
    /// (its `mandatory_version` is above what is installed).
    Update { mandatory: bool },
}

/// What a completed sync did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Outcome {
    /// Nothing to do: the install already matches the served version.
    Current { version: Version },
    /// The install was brought up to the served version.
    Updated {
        version: Version,
        fetched: usize,
        deleted: usize,
    },
}

/// Decide whether `local` must be updated to reach the served `ver_cur`.
///
/// This is deliberately version-only, like the reference implementation: a run that
/// completed recorded its version, so a matching version means the file set is the one
/// that version shipped. A run interrupted before the version files were written
/// reports the older version and is re-diffed.
pub fn assess(local: Version, ver_cur: &VerCur) -> Assessment {
    if local == ver_cur.version {
        return Assessment::Current;
    }
    Assessment::Update {
        mandatory: ver_cur
            .mandatory_version
            .is_some_and(|wanted| wanted > local),
    }
}

/// The work needed to bring the installed file set up to the served one.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Plan {
    /// Files to download, with what each must look like once fetched.
    pub fetch: Vec<(RelPath, FileEntry)>,
    /// Installed files the served manifest no longer lists.
    pub delete: Vec<RelPath>,
}

/// Compare the installed file set with the served one.
///
/// A file counts as unchanged only when its length matches *and* its checksum matches.
/// A `-1` checksum (`None`) pins the length alone, which is not proof that the bytes
/// are right, so such a file is always re-fetched rather than trusted.
pub fn diff(local: &Manifest, server: &Manifest) -> Plan {
    let mut fetch = Vec::new();
    for (path, want) in server.files() {
        let unchanged = match (want.checksum, local.get(path)) {
            (Some(checksum), Some(have)) => {
                have.length == want.length && have.checksum == Some(checksum)
            }
            _ => false,
        };
        if !unchanged {
            fetch.push((path.clone(), *want));
        }
    }
    let delete = local
        .files()
        .keys()
        .filter(|path| server.get(path).is_none())
        .cloned()
        .collect();
    Plan { fetch, delete }
}

/// The version file the install points at: `Zwift_ver_cur_filename.txt` (NUL padded)
/// names it, else `Zwift_ver_cur.xml`. Mirrors [`crate::version::current_version`].
fn local_ver_cur_file(install_dir: &Path) -> String {
    if let Ok(raw) = fs::read(install_dir.join(VER_CUR_FILENAME_FILE)) {
        let text = String::from_utf8_lossy(&raw).replace('\0', "\n");
        if let Some(line) = text.lines().next().filter(|line| !line.is_empty()) {
            return line.to_string();
        }
    }
    VER_CUR_FILE.to_string()
}

/// The served-version record the install carries, if it parses.
fn local_ver_cur(install_dir: &Path) -> Option<VerCur> {
    let bytes = fs::read(install_dir.join(local_ver_cur_file(install_dir))).ok()?;
    parse_ver_cur(&bytes).ok()
}

/// The `version` attribute of the installed game, or [`Version::ZERO`] when unknown.
///
/// A prefix with no game, or one carrying only the installer's `0.0.0` seed, gives
/// `ZERO`, which no served version equals, so a sync fetches everything.
pub fn local_version(install_dir: &Path) -> Version {
    local_ver_cur(install_dir)
        .map(|ver_cur| ver_cur.version)
        .unwrap_or(Version::ZERO)
}

/// The manifest recorded by the installed game, or an empty one when unknown.
///
/// The recorded manifest is trusted as the description of what is on disk; the sync
/// never re-hashes the tree to rebuild it. An unreadable or missing manifest gives an
/// empty set, so every file is fetched.
pub fn local_manifest(install_dir: &Path) -> Manifest {
    local_ver_cur(install_dir)
        .and_then(|ver_cur| {
            // `manifest_name` already rejected separators and `..`, so joining is safe.
            let bytes = fs::read(install_dir.join(&ver_cur.manifest)).ok()?;
            parse_manifest(&bytes).ok()
        })
        .unwrap_or_else(Manifest::empty)
}

/// Bring the install directory up to the version the CDN serves.
///
/// `progress` receives bytes completed and the total to complete while files are
/// transferred. On success the install directory holds the served file set and the
/// version files that record it.
pub fn sync(
    fetcher: &dyn Fetcher,
    config: &SyncConfig<'_>,
    logger: &Logger,
    progress: &(dyn Fn(u64, u64) + Sync),
) -> Result<Outcome, String> {
    let ver_cur = fetcher.fetch_ver_cur()?;
    sync_version(fetcher, &ver_cur, config, logger, progress)
}

/// [`sync`] for a caller that has already read the served version file.
///
/// The update check reads `Zwift_ver_cur.xml` itself so it can tell "the CDN is
/// unreachable" (a warning, because the installed game can still be started) from "the
/// transfer failed" (an error), and passes the record it read in here instead of
/// fetching it twice.
pub fn sync_version(
    fetcher: &dyn Fetcher,
    ver_cur: &VerCur,
    config: &SyncConfig<'_>,
    logger: &Logger,
    progress: &(dyn Fn(u64, u64) + Sync),
) -> Result<Outcome, String> {
    let installed = local_version(config.install_dir);
    match assess(installed, ver_cur) {
        Assessment::Current => {
            logger.info(&format!("Zwift {} is already installed", ver_cur.version));
            return Ok(Outcome::Current {
                version: ver_cur.version,
            });
        }
        Assessment::Update { mandatory } => logger.info(&format!(
            "Updating Zwift from {installed} to {}{}",
            ver_cur.version,
            if mandatory { " (mandatory)" } else { "" }
        )),
    }

    let local = local_manifest(config.install_dir);
    let (server, manifest_bytes) =
        fetcher.fetch_manifest(&ver_cur.manifest, ver_cur.manifest_checksum)?;
    let plan = diff(&local, &server);
    logger.info(&format!(
        "Zwift {} has {} files: {} to download, {} to delete",
        ver_cur.version,
        server.files().len(),
        plan.fetch.len(),
        plan.delete.len()
    ));

    fs::create_dir_all(config.install_dir)
        .map_err(|e| format!("{}: {e}", config.install_dir.display()))?;
    fs::create_dir_all(config.staging_dir)
        .map_err(|e| format!("{}: {e}", config.staging_dir.display()))?;
    download(
        fetcher,
        &DownloadConfig {
            folder: server.folder(),
            install_dir: config.install_dir,
            staging_dir: config.staging_dir,
            jobs: config.jobs,
        },
        &plan.fetch,
        logger,
        progress,
    )?;

    for path in &plan.delete {
        remove_file(&config.install_dir.join(path.as_str()))?;
    }
    prune_empty_directories(config.install_dir, &plan.delete);

    // Written last: they are the record that this version is complete.
    write_file(&config.install_dir.join(&ver_cur.manifest), &manifest_bytes)?;
    write_file(&config.install_dir.join(VER_CUR_FILE), ver_cur.raw())?;
    remove_stale_version_files(config.install_dir)?;

    logger.info(&format!(
        "Zwift {} is installed ({} files fetched, {} removed)",
        ver_cur.version,
        plan.fetch.len(),
        plan.delete.len()
    ));
    Ok(Outcome::Updated {
        version: ver_cur.version,
        fetched: plan.fetch.len(),
        deleted: plan.delete.len(),
    })
}

fn write_file(path: &Path, bytes: &[u8]) -> Result<(), String> {
    fs::write(path, bytes).map_err(|e| format!("{}: {e}", path.display()))
}

fn remove_file(path: &Path) -> Result<(), String> {
    match fs::remove_file(path) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == ErrorKind::NotFound => Ok(()),
        Err(error) => Err(format!("{}: {error}", path.display())),
    }
}

/// Remove directories a deletion left empty, from the leaves upwards.
///
/// `remove_dir` refuses a non-empty directory, which is exactly the stop condition.
fn prune_empty_directories(install_dir: &Path, deleted: &[RelPath]) {
    for path in deleted {
        let mut components: Vec<&str> = path.components().collect();
        components.pop();
        while !components.is_empty() {
            if fs::remove_dir(install_dir.join(components.join("/"))).is_err() {
                break;
            }
            components.pop();
        }
    }
}

/// Whether `name` is the Windows installer's versioned copy of the version file, as
/// `Zwift_ver_cur_filename.txt` names it (`Zwift_ver_cur.<build>.xml`).
fn is_stale_version_file(name: &str) -> bool {
    name.strip_prefix("Zwift_ver_cur.")
        .and_then(|rest| rest.strip_suffix(".xml"))
        .is_some_and(|build| !build.is_empty() && build.bytes().all(|b| b.is_ascii_digit()))
}

/// Drop the version-file indirection the Windows installer left behind.
///
/// `version::current_version` prefers the file `Zwift_ver_cur_filename.txt` names, so
/// while that pointer and the versioned copy exist the launcher would keep reading the
/// pre-sync `sversion` and the smoke test's cache would record a stale version.
fn remove_stale_version_files(install_dir: &Path) -> Result<(), String> {
    remove_file(&install_dir.join(VER_CUR_FILENAME_FILE))?;
    let entries = match fs::read_dir(install_dir) {
        Ok(entries) => entries,
        Err(error) if error.kind() == ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(format!("{}: {error}", install_dir.display())),
    };
    for entry in entries {
        let entry = entry.map_err(|error| format!("{}: {error}", install_dir.display()))?;
        let name = entry.file_name();
        if is_stale_version_file(&name.to_string_lossy()) {
            remove_file(&entry.path())?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Mutex;

    use crate::cdn::Expect;
    use crate::logging::Logger;

    fn ver_cur_xml(version: &str, sversion: &str, manifest: &str) -> String {
        format!(
            "<Zwift version=\"{version}\" sversion=\"{sversion}\" gbranch=\"rc\" \
             GAME_URL=\"https://us-or-rly101.zwift.com\" manifest=\"{manifest}\"/>"
        )
    }

    fn manifest_xml(version: &str, folder: &str, files: &[(&str, &[u8], Option<u32>)]) -> String {
        let mut xml = format!("<Zwift version=\"{version}\" folder=\"{folder}\"><files>");
        for (path, bytes, checksum) in files {
            let checksum = match checksum {
                // Real manifests carry the CRC as a *signed* 32-bit decimal.
                Some(value) => (*value as i32).to_string(),
                None => "-1".to_string(),
            };
            xml.push_str(&format!(
                "<file><path>{path}</path><length>{}</length><checksum>{checksum}</checksum></file>",
                bytes.len()
            ));
        }
        xml.push_str("</files></Zwift>");
        xml
    }

    fn crc(bytes: &[u8]) -> u32 {
        crc32fast::hash(bytes)
    }

    /// An in-memory CDN: a ver_cur, one manifest, and the file bytes.
    struct Memory {
        ver_cur: Vec<u8>,
        manifest_name: String,
        manifest: Vec<u8>,
        files: BTreeMap<String, Vec<u8>>,
        fetches: AtomicUsize,
    }

    impl Memory {
        fn new(
            ver_cur: String,
            manifest_name: &str,
            manifest: String,
            files: &[(&str, &[u8])],
        ) -> Memory {
            Memory {
                ver_cur: ver_cur.into_bytes(),
                manifest_name: manifest_name.to_string(),
                manifest: manifest.into_bytes(),
                files: files
                    .iter()
                    .map(|(path, bytes)| (path.to_string(), bytes.to_vec()))
                    .collect(),
                fetches: AtomicUsize::new(0),
            }
        }

        fn key(url: &str) -> String {
            url.splitn(3, '/').nth(2).unwrap_or_default().to_string()
        }
    }

    impl Fetcher for Memory {
        fn fetch_ver_cur(&self) -> Result<VerCur, String> {
            parse_ver_cur(&self.ver_cur)
        }

        fn fetch_manifest(
            &self,
            name: &str,
            _checksum: Option<u32>,
        ) -> Result<(Manifest, Box<[u8]>), String> {
            if name != self.manifest_name {
                return Err(format!("unknown manifest {name}"));
            }
            let manifest = parse_manifest(&self.manifest)?;
            Ok((manifest, self.manifest.clone().into_boxed_slice()))
        }

        fn file_url(&self, folder: &str, path: &RelPath) -> String {
            format!("mem://{folder}/{}", path.as_str())
        }

        fn fetch_file(&self, url: &str, dest: &Path, expect: Expect) -> Result<(), String> {
            self.fetches.fetch_add(1, Ordering::Relaxed);
            let key = Memory::key(url);
            let bytes = self
                .files
                .get(&key)
                .ok_or_else(|| format!("mem 404 {key}"))?;
            if let Some(parent) = dest.parent() {
                fs::create_dir_all(parent).map_err(|e| e.to_string())?;
            }
            fs::write(dest, bytes).map_err(|e| e.to_string())?;
            if !expect.matches(bytes.len() as u64, crc(bytes)) {
                // Like the real fetcher, never leave a rejected file behind.
                let _ = fs::remove_file(dest);
                return Err(format!("mem mismatch {key}"));
            }
            Ok(())
        }
    }

    struct Scratch {
        root: PathBuf,
        install: PathBuf,
        staging: PathBuf,
        logger: Logger,
    }

    impl Scratch {
        fn new(name: &str) -> Scratch {
            let root = std::env::temp_dir().join(format!(
                "zwift-sync-{name}-{}-{:?}",
                std::process::id(),
                std::thread::current().id()
            ));
            let _ = fs::remove_dir_all(&root);
            let install = root.join("install");
            let staging = root.join("staging");
            fs::create_dir_all(&install).unwrap();
            fs::create_dir_all(&staging).unwrap();
            let logger = Logger::new(root.join("log"), 0, false);
            Scratch {
                root,
                install,
                staging,
                logger,
            }
        }

        fn config(&self) -> SyncConfig<'_> {
            SyncConfig {
                install_dir: &self.install,
                staging_dir: &self.staging,
                jobs: 4,
            }
        }
    }

    impl Drop for Scratch {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.root);
        }
    }

    fn entry(bytes: &[u8], checksum: Option<u32>) -> FileEntry {
        FileEntry {
            length: bytes.len() as u64,
            checksum,
            modified: None,
        }
    }

    fn manifest(pairs: &[(&str, &[u8], Option<u32>)]) -> Manifest {
        Manifest::new(
            Version::parse("1.0.1").unwrap(),
            "F".to_string(),
            pairs
                .iter()
                .map(|(path, bytes, checksum)| {
                    (RelPath::parse(path).unwrap(), entry(bytes, *checksum))
                })
                .collect(),
        )
        .unwrap()
    }

    #[test]
    fn diff_finds_changed_new_and_removed_files() {
        let a = b"alpha".as_slice();
        let b = b"bravo".as_slice();
        let local = manifest(&[
            ("a.bin", a, Some(crc(a))),
            ("gone.bin", b"x", Some(crc(b"x"))),
        ]);
        let server = manifest(&[("a.bin", a, Some(crc(a))), ("new.bin", b, Some(crc(b)))]);
        let plan = diff(&local, &server);
        assert_eq!(
            plan.fetch,
            vec![(RelPath::parse("new.bin").unwrap(), entry(b, Some(crc(b))))]
        );
        assert_eq!(plan.delete, vec![RelPath::parse("gone.bin").unwrap()]);
    }

    #[test]
    fn diff_re_fetches_when_the_server_has_no_checksum() {
        let a = b"alpha".as_slice();
        // Same length, server checksum absent: length alone is not proof.
        let local = manifest(&[("a.bin", a, Some(crc(a)))]);
        let server = manifest(&[("a.bin", a, None)]);
        assert_eq!(diff(&local, &server).fetch.len(), 1);
        // And a length that differs is always fetched, checksum or not.
        let server = manifest(&[("a.bin", b"longer", Some(crc(b"longer")))]);
        assert_eq!(diff(&local, &server).fetch.len(), 1);
    }

    #[test]
    fn diff_of_an_empty_install_fetches_everything() {
        let a = b"alpha".as_slice();
        let b = b"bravo".as_slice();
        let server = manifest(&[("a.bin", a, Some(crc(a))), ("b.bin", b, Some(crc(b)))]);
        let plan = diff(&Manifest::empty(), &server);
        assert_eq!(plan.fetch.len(), 2);
        assert!(plan.delete.is_empty());
    }

    #[test]
    fn assess_reports_current_update_and_mandatory() {
        let current = parse_ver_cur(ver_cur_xml("1.0.5", "1.5.0 (5)", "m.xml").as_bytes()).unwrap();
        assert_eq!(
            assess(Version::parse("1.0.5").unwrap(), &current),
            Assessment::Current
        );
        assert_eq!(
            assess(Version::parse("1.0.4").unwrap(), &current),
            Assessment::Update { mandatory: false }
        );
        let forced = parse_ver_cur(
            "<Zwift version=\"1.0.5\" manifest=\"m.xml\" mandatory_version=\"1.0.5\"/>".as_bytes(),
        )
        .unwrap();
        assert_eq!(
            assess(Version::parse("1.0.0").unwrap(), &forced),
            Assessment::Update { mandatory: true }
        );
    }

    #[test]
    fn local_version_follows_the_installer_filename_indirection() {
        let scratch = Scratch::new("indirect");
        // The seed the Windows installer leaves: a 0.0.0 placeholder.
        fs::write(
            scratch.install.join(VER_CUR_FILE),
            ver_cur_xml("0.0.0", "0.0.0", "Zwift_0.0.0_manifest.xml"),
        )
        .unwrap();
        assert_eq!(local_version(&scratch.install), Version::ZERO);
        assert_eq!(local_manifest(&scratch.install).files().len(), 0);
        // A real install points at a versioned copy.
        fs::write(
            scratch.install.join("Zwift_ver_cur.168455.xml"),
            ver_cur_xml(
                "1.0.168455",
                "1.125.0 (168455)",
                "Zwift_1.0.168455_ab_manifest.xml",
            ),
        )
        .unwrap();
        fs::write(
            scratch.install.join(VER_CUR_FILENAME_FILE),
            b"Zwift_ver_cur.168455.xml\0",
        )
        .unwrap();
        assert_eq!(
            local_version(&scratch.install),
            Version::parse("1.0.168455").unwrap()
        );
        // Without the manifest copy the file set is unknown, so a diff fetches all.
        assert_eq!(local_manifest(&scratch.install).files().len(), 0);
    }

    #[test]
    fn sync_installs_from_nothing_and_records_the_version() {
        let scratch = Scratch::new("fresh");
        let a = b"alpha".as_slice();
        let b = b"bravo bravo".as_slice();
        let memory = Memory::new(
            ver_cur_xml("1.0.5", "1.5.0 (5)", "F_manifest.xml"),
            "F_manifest.xml",
            manifest_xml(
                "1.0.5",
                "F",
                &[("a.bin", a, Some(crc(a))), ("sub/b.bin", b, Some(crc(b)))],
            ),
            &[("F/a.bin", a), ("F/sub/b.bin", b)],
        );
        let seen = Mutex::new(Vec::new());
        let progress = |done: u64, total: u64| seen.lock().unwrap().push((done, total));
        let outcome = sync(&memory, &scratch.config(), &scratch.logger, &progress).unwrap();
        assert_eq!(
            outcome,
            Outcome::Updated {
                version: Version::parse("1.0.5").unwrap(),
                fetched: 2,
                deleted: 0,
            }
        );
        assert_eq!(fs::read(scratch.install.join("a.bin")).unwrap(), a);
        assert_eq!(fs::read(scratch.install.join("sub/b.bin")).unwrap(), b);
        // The version files are the record of what is installed.
        assert!(scratch.install.join(VER_CUR_FILE).is_file());
        assert!(scratch.install.join("F_manifest.xml").is_file());
        assert_eq!(
            local_version(&scratch.install),
            Version::parse("1.0.5").unwrap()
        );
        assert_eq!(local_manifest(&scratch.install).files().len(), 2);
        let seen = seen.into_inner().unwrap();
        assert_eq!(seen.first(), Some(&(0, a.len() as u64 + b.len() as u64)));
        assert_eq!(
            seen.last(),
            Some(&(
                a.len() as u64 + b.len() as u64,
                a.len() as u64 + b.len() as u64
            ))
        );
    }

    #[test]
    fn a_second_sync_is_a_no_op() {
        let scratch = Scratch::new("idempotent");
        let a = b"alpha".as_slice();
        let memory = Memory::new(
            ver_cur_xml("1.0.5", "1.5.0 (5)", "F_manifest.xml"),
            "F_manifest.xml",
            manifest_xml("1.0.5", "F", &[("a.bin", a, Some(crc(a)))]),
            &[("F/a.bin", a)],
        );
        let progress = |_, _| {};
        sync(&memory, &scratch.config(), &scratch.logger, &progress).unwrap();
        assert_eq!(memory.fetches.load(Ordering::Relaxed), 1);
        let outcome = sync(&memory, &scratch.config(), &scratch.logger, &progress).unwrap();
        assert_eq!(
            outcome,
            Outcome::Current {
                version: Version::parse("1.0.5").unwrap()
            }
        );
        // Nothing was transferred the second time.
        assert_eq!(memory.fetches.load(Ordering::Relaxed), 1);
    }

    #[test]
    fn sync_updates_changed_files_and_deletes_removed_ones() {
        let scratch = Scratch::new("update");
        let a1 = b"alpha one".as_slice();
        let a2 = b"alpha two!".as_slice();
        let gone = b"removed".as_slice();
        // An install of version 1.0.4 carrying a.bin (old bytes) and gone.bin.
        fs::write(
            scratch.install.join(VER_CUR_FILE),
            ver_cur_xml("1.0.4", "1.4.0 (4)", "F_old_manifest.xml"),
        )
        .unwrap();
        fs::write(
            scratch.install.join("F_old_manifest.xml"),
            manifest_xml(
                "1.0.4",
                "F",
                &[
                    ("a.bin", a1, Some(crc(a1))),
                    (r"olddir\gone.bin", gone, Some(crc(gone))),
                ],
            ),
        )
        .unwrap();
        fs::create_dir_all(scratch.install.join("olddir")).unwrap();
        fs::write(scratch.install.join("a.bin"), a1).unwrap();
        fs::write(scratch.install.join("olddir/gone.bin"), gone).unwrap();

        let memory = Memory::new(
            ver_cur_xml("1.0.5", "1.5.0 (5)", "F_manifest.xml"),
            "F_manifest.xml",
            manifest_xml("1.0.5", "F", &[("a.bin", a2, Some(crc(a2)))]),
            &[("F/a.bin", a2)],
        );
        let progress = |_, _| {};
        let outcome = sync(&memory, &scratch.config(), &scratch.logger, &progress).unwrap();
        assert_eq!(
            outcome,
            Outcome::Updated {
                version: Version::parse("1.0.5").unwrap(),
                fetched: 1,
                deleted: 1,
            }
        );
        assert_eq!(fs::read(scratch.install.join("a.bin")).unwrap(), a2);
        assert!(!scratch.install.join("olddir/gone.bin").exists());
        // The directory a deletion emptied is pruned too.
        assert!(!scratch.install.join("olddir").exists());
        assert_eq!(
            local_version(&scratch.install),
            Version::parse("1.0.5").unwrap()
        );
    }

    #[test]
    fn sync_clears_the_installer_filename_pointer_and_versioned_copy() {
        let scratch = Scratch::new("pointer");
        let a = b"alpha".as_slice();
        fs::write(
            scratch.install.join("Zwift_ver_cur.168455.xml"),
            ver_cur_xml("1.0.168455", "1.125.0 (168455)", "F_manifest.xml"),
        )
        .unwrap();
        fs::write(
            scratch.install.join(VER_CUR_FILENAME_FILE),
            b"Zwift_ver_cur.168455.xml\0",
        )
        .unwrap();
        // A file that merely looks similar must survive.
        fs::write(scratch.install.join("Launcher_ver_cur.xml"), "<x/>").unwrap();

        let memory = Memory::new(
            ver_cur_xml("1.0.168456", "1.126.0 (168456)", "F_manifest.xml"),
            "F_manifest.xml",
            manifest_xml("1.0.168456", "F", &[("a.bin", a, Some(crc(a)))]),
            &[("F/a.bin", a)],
        );
        let progress = |_, _| {};
        sync(&memory, &scratch.config(), &scratch.logger, &progress).unwrap();
        assert!(!scratch.install.join(VER_CUR_FILENAME_FILE).exists());
        assert!(!scratch.install.join("Zwift_ver_cur.168455.xml").exists());
        assert!(scratch.install.join("Launcher_ver_cur.xml").is_file());
        // `current_version` now reads the launcher's own copy: had the pointer and the
        // versioned file survived, it would still report the old 1.125.0.
        assert_eq!(crate::version::current_version(&scratch.install), "1.126.0");
    }

    #[test]
    fn stale_version_file_names_are_matched_precisely() {
        assert!(is_stale_version_file("Zwift_ver_cur.168455.xml"));
        assert!(is_stale_version_file("Zwift_ver_cur.0.xml"));
        // The canonical file, the manifest, and the launcher's own file are kept.
        assert!(!is_stale_version_file(VER_CUR_FILE));
        assert!(!is_stale_version_file("Zwift_1.0.1_ab_manifest.xml"));
        assert!(!is_stale_version_file("Launcher_ver_cur.xml"));
        assert!(!is_stale_version_file("Zwift_ver_cur.beta.xml"));
        assert!(!is_stale_version_file("Zwift_ver_cur..xml"));
    }
}
