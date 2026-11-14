//! The Zwift game API: the rider's own profile, and the images it references.
//!
//! The launcher's multi-account picker needs more than the opaque OAuth token:
//! the name, avatar and riding statistics that make a card recognisable all come
//! from `GET /api/profiles/me`, which the game itself reads as a protobuf-lite
//! `PlayerProfile`. This module fetches that endpoint and decodes only the fields
//! the picker uses, tolerating the many fields it does not know about.
//!
//! Everything here is best-effort: the caller keeps whatever it cached when a
//! fetch fails, so a flaky network never blocks a launch.

use std::io::Read;
use std::path::PathBuf;
use std::time::Duration;

/// Where profiles are fetched from when nothing overrides it.
pub const DEFAULT_API_BASE: &str = "https://us-or-rly101.zwift.com";

/// Environment override for the API base, mirroring `ZWIFT_CDN_BASE`. A
/// `file://` base reads from the local filesystem, which is how the offline
/// tests serve a profile without a network.
pub const API_BASE_ENV: &str = "ZWIFT_API_BASE";

/// The profile endpoint this module reads.
const PROFILE_PATH: &str = "/api/profiles/me";

/// Deadline for the connect and read phases of a profile request.
const REQUEST_TIMEOUT: Duration = Duration::from_secs(30);

/// A profile response is a single small message; anything larger is a mistake.
const MAX_PROFILE_BYTES: u64 = 1 << 20;

/// Avatars are images; allow generously but not unboundedly.
const MAX_IMAGE_BYTES: u64 = 8 << 20;

/// The rider's profile, reduced to the fields the account cards use. Protobuf
/// omits fields equal to their default, so every field is optional.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Profile {
    pub id: Option<i64>,
    pub email: Option<String>,
    pub first_name: Option<String>,
    pub last_name: Option<String>,
    pub weight_in_grams: Option<i64>,
    pub ftp: Option<i64>,
    pub total_distance_in_meters: Option<i64>,
    pub elevation_gain_in_meters: Option<i64>,
    pub time_ridden_in_minutes: Option<i64>,
    pub height_in_millimeters: Option<i64>,
    pub total_xp: Option<i64>,
    pub achievement_level: Option<i64>,
    pub use_metric: Option<bool>,
    pub large_avatar_url: Option<String>,
    pub public_id: Option<String>,
}

/// Where a profile or image comes from: the real API, or a local tree for tests.
enum Source {
    Http(ureq::Agent),
    Local(PathBuf),
}

/// A profile endpoint plus the client used to reach it.
pub struct Api {
    base: String,
    source: Source,
}

impl Api {
    /// Build a client for `base`, for example `https://us-or-rly101.zwift.com`.
    /// A `file://` base reads from the local filesystem instead.
    pub fn new(base: &str) -> Api {
        let base = base.trim_end_matches('/').to_string();
        let source = match base.strip_prefix("file://") {
            Some(root) => Source::Local(PathBuf::from(root)),
            None => {
                let agent = ureq::AgentBuilder::new()
                    .user_agent(concat!("zwift-snap/", env!("CARGO_PKG_VERSION")))
                    .timeout_connect(REQUEST_TIMEOUT)
                    .timeout_read(REQUEST_TIMEOUT)
                    .build();
                Source::Http(agent)
            }
        };
        Api { base, source }
    }

    /// Build a client from `ZWIFT_API_BASE`, or the default base when unset.
    pub fn from_env() -> Api {
        let base = std::env::var(API_BASE_ENV).unwrap_or_else(|_| DEFAULT_API_BASE.to_owned());
        Api::new(&base)
    }

    /// Fetch and decode `access_token`'s owner profile.
    pub fn fetch(&self, access_token: &str) -> Result<Profile, String> {
        let bytes = self.get(PROFILE_PATH, access_token)?;
        Ok(decode(&bytes))
    }

    /// Read `path` (relative to the base), with the bearer token when the source
    /// is HTTP. `path` must start with `/`.
    fn get(&self, path: &str, access_token: &str) -> Result<Vec<u8>, String> {
        match &self.source {
            Source::Http(agent) => {
                let url = format!("{}{path}", self.base);
                let response = agent
                    .get(&url)
                    .set("Authorization", &format!("Bearer {access_token}"))
                    .set("Accept", "application/x-protobuf-lite")
                    .call()
                    .map_err(|error| describe(&url, error))?;
                read_limited(response.into_reader(), &url, MAX_PROFILE_BYTES)
            }
            Source::Local(root) => {
                let file = root.join(path.trim_start_matches('/'));
                std::fs::read(&file).map_err(|e| format!("{}: {e}", file.display()))
            }
        }
    }
}

/// Convenience wrapper over [`Api::from_env`] for the common single call site.
pub fn fetch(access_token: &str) -> Result<Profile, String> {
    Api::from_env().fetch(access_token)
}

/// Download an image over HTTPS. Any other scheme is refused, so a hostile or
/// mistaken `large_avatar_url` can never make the launcher read a local file.
pub fn fetch_image(url: &str) -> Result<Vec<u8>, String> {
    if !url.starts_with("https://") {
        return Err(format!("refusing non-https image URL: {url}"));
    }
    let agent = ureq::AgentBuilder::new()
        .user_agent(concat!("zwift-snap/", env!("CARGO_PKG_VERSION")))
        .timeout_connect(REQUEST_TIMEOUT)
        .timeout_read(REQUEST_TIMEOUT)
        .build();
    let response = agent
        .get(url)
        .call()
        .map_err(|error| describe(url, error))?;
    read_limited(response.into_reader(), url, MAX_IMAGE_BYTES)
}

/// Read a whole response body, refusing anything larger than `max`.
fn read_limited(mut reader: impl Read, url: &str, max: u64) -> Result<Vec<u8>, String> {
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
    Ok(bytes)
}

/// Turn a transport error into a short message. A non-2xx reply is reported by
/// status alone; the body is never echoed, so a token cannot leak into a log.
fn describe(url: &str, error: ureq::Error) -> String {
    match error {
        ureq::Error::Status(status, _) => format!("{url}: HTTP {status}"),
        other => format!("{url}: {other}"),
    }
}

/// Decode a protobuf-lite `PlayerProfile`, keeping only the fields the cards use
/// and skipping every other field, whether known or not. An unknown wire type
/// stops decoding early rather than failing: a partial profile still labels a card.
pub fn decode(bytes: &[u8]) -> Profile {
    let mut profile = Profile::default();
    let mut reader = Wire::new(bytes);
    while let Some((field, value)) = reader.next_field() {
        match (field, value) {
            (1, WireValue::Varint(v)) => profile.id = Some(v as i64),
            (3, WireValue::Bytes(b)) => profile.email = Some(text(b)),
            (4, WireValue::Bytes(b)) => profile.first_name = Some(text(b)),
            (5, WireValue::Bytes(b)) => profile.last_name = Some(text(b)),
            (9, WireValue::Varint(v)) => profile.weight_in_grams = Some(v as i64),
            (10, WireValue::Varint(v)) => profile.ftp = Some(v as i64),
            (35, WireValue::Varint(v)) => profile.total_distance_in_meters = Some(v as i64),
            (36, WireValue::Varint(v)) => profile.elevation_gain_in_meters = Some(v as i64),
            (37, WireValue::Varint(v)) => profile.time_ridden_in_minutes = Some(v as i64),
            (42, WireValue::Varint(v)) => profile.height_in_millimeters = Some(v as i64),
            (46, WireValue::Varint(v)) => profile.total_xp = Some(v as i64),
            (49, WireValue::Varint(v)) => profile.achievement_level = Some(v as i64),
            (50, WireValue::Varint(v)) => profile.use_metric = Some(v != 0),
            (58, WireValue::Bytes(b)) => profile.large_avatar_url = Some(text(b)),
            (154, WireValue::Bytes(b)) => profile.public_id = Some(text(b)),
            _ => {}
        }
    }
    profile
}

/// A length-delimited string as text, lossily, so invalid UTF-8 never aborts a decode.
fn text(bytes: &[u8]) -> String {
    String::from_utf8_lossy(bytes).into_owned()
}

/// A decoded protobuf field value, by wire type.
enum WireValue<'a> {
    Varint(u64),
    Fixed32,
    Fixed64,
    Bytes(&'a [u8]),
}

/// A minimal protobuf wire reader. Only what [`decode`] needs: enough to walk the
/// message field by field, skipping values it does not recognise.
struct Wire<'a> {
    bytes: &'a [u8],
    pos: usize,
}

impl<'a> Wire<'a> {
    fn new(bytes: &'a [u8]) -> Self {
        Wire { bytes, pos: 0 }
    }

    /// Read the next field as `(field_number, value)`, or `None` at the end (or on
    /// a malformed field, which is treated as the end).
    fn next_field(&mut self) -> Option<(u64, WireValue<'a>)> {
        let key = self.varint()?;
        let field = key >> 3;
        let wire_type = key & 0x7;
        let value = match wire_type {
            0 => WireValue::Varint(self.varint()?),
            1 => {
                self.skip(8)?;
                WireValue::Fixed64
            }
            2 => {
                let len = self.varint()? as usize;
                let start = self.pos;
                self.skip(len)?;
                WireValue::Bytes(&self.bytes[start..self.pos])
            }
            5 => {
                self.skip(4)?;
                WireValue::Fixed32
            }
            _ => return None,
        };
        Some((field, value))
    }

    /// Read a base-128 varint, at most ten bytes (64 bits).
    fn varint(&mut self) -> Option<u64> {
        let mut result = 0u64;
        for shift in 0..10 {
            let byte = *self.bytes.get(self.pos)?;
            self.pos += 1;
            result |= u64::from(byte & 0x7f) << (shift * 7);
            if byte & 0x80 == 0 {
                return Some(result);
            }
        }
        None
    }

    /// Advance the cursor by `count` bytes, or fail if that would run past the end.
    fn skip(&mut self, count: usize) -> Option<()> {
        let end = self.pos.checked_add(count)?;
        if end > self.bytes.len() {
            return None;
        }
        self.pos = end;
        Some(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Append a varint field to `out`.
    fn varint_field(out: &mut Vec<u8>, field: u64, value: u64) {
        put_varint(out, field << 3);
        put_varint(out, value);
    }

    /// Append a length-delimited (string/bytes) field to `out`.
    fn bytes_field(out: &mut Vec<u8>, field: u64, value: &[u8]) {
        put_varint(out, (field << 3) | 2);
        put_varint(out, value.len() as u64);
        out.extend_from_slice(value);
    }

    fn put_varint(out: &mut Vec<u8>, mut value: u64) {
        loop {
            let byte = (value & 0x7f) as u8;
            value >>= 7;
            if value == 0 {
                out.push(byte);
                break;
            }
            out.push(byte | 0x80);
        }
    }

    #[test]
    fn decodes_the_fields_the_cards_use() {
        let mut bytes = Vec::new();
        varint_field(&mut bytes, 1, 98765);
        bytes_field(&mut bytes, 4, b"Sara");
        bytes_field(&mut bytes, 5, b"Campbell");
        varint_field(&mut bytes, 9, 70000);
        varint_field(&mut bytes, 10, 233);
        varint_field(&mut bytes, 35, 375_600);
        varint_field(&mut bytes, 36, 2444);
        varint_field(&mut bytes, 37, 834);
        varint_field(&mut bytes, 42, 1750);
        varint_field(&mut bytes, 46, 12345);
        varint_field(&mut bytes, 49, 42);
        varint_field(&mut bytes, 50, 1);
        bytes_field(&mut bytes, 58, b"https://cdn.zwift.com/avatar.jpg");
        bytes_field(&mut bytes, 154, b"public-1");
        let profile = decode(&bytes);
        assert_eq!(profile.id, Some(98765));
        assert_eq!(profile.first_name.as_deref(), Some("Sara"));
        assert_eq!(profile.last_name.as_deref(), Some("Campbell"));
        assert_eq!(profile.weight_in_grams, Some(70000));
        assert_eq!(profile.ftp, Some(233));
        assert_eq!(profile.total_distance_in_meters, Some(375_600));
        assert_eq!(profile.elevation_gain_in_meters, Some(2444));
        assert_eq!(profile.time_ridden_in_minutes, Some(834));
        assert_eq!(profile.height_in_millimeters, Some(1750));
        assert_eq!(profile.total_xp, Some(12345));
        assert_eq!(profile.achievement_level, Some(42));
        assert_eq!(profile.use_metric, Some(true));
        assert_eq!(
            profile.large_avatar_url.as_deref(),
            Some("https://cdn.zwift.com/avatar.jpg")
        );
        assert_eq!(profile.public_id.as_deref(), Some("public-1"));
    }

    #[test]
    fn unknown_and_out_of_order_fields_are_skipped() {
        let mut bytes = Vec::new();
        bytes_field(&mut bytes, 5, b"Campbell");
        // A sub-message (length-delimited) the decoder must skip.
        bytes_field(&mut bytes, 113, &[0x08, 0x01]);
        varint_field(&mut bytes, 1, 7);
        // A fixed64 field and a fixed32 field, both skipped.
        put_varint(&mut bytes, (200 << 3) | 1);
        bytes.extend_from_slice(&[0; 8]);
        put_varint(&mut bytes, (201 << 3) | 5);
        bytes.extend_from_slice(&[0; 4]);
        bytes_field(&mut bytes, 4, b"Sara");
        let profile = decode(&bytes);
        assert_eq!(profile.id, Some(7));
        assert_eq!(profile.first_name.as_deref(), Some("Sara"));
        assert_eq!(profile.last_name.as_deref(), Some("Campbell"));
    }

    #[test]
    fn truncated_input_is_tolerated() {
        let mut bytes = Vec::new();
        bytes_field(&mut bytes, 4, b"Sara");
        // A field header claiming more string bytes than remain.
        put_varint(&mut bytes, (5 << 3) | 2);
        put_varint(&mut bytes, 100);
        bytes.extend_from_slice(b"short");
        let profile = decode(&bytes);
        assert_eq!(profile.first_name.as_deref(), Some("Sara"));
        assert_eq!(profile.last_name, None);
    }

    #[test]
    fn empty_message_decodes_to_defaults() {
        assert_eq!(decode(&[]), Profile::default());
    }

    #[test]
    fn flags_distinguish_zero_from_unset() {
        // Field 50 present as 0 is `Some(false)`, not `None`.
        let mut bytes = Vec::new();
        varint_field(&mut bytes, 50, 0);
        assert_eq!(decode(&bytes).use_metric, Some(false));
    }

    #[test]
    fn image_fetch_refuses_non_https() {
        for url in [
            "http://cdn.zwift.com/a.png",
            "file:///etc/passwd",
            "ftp://example.com/a.png",
        ] {
            assert!(fetch_image(url).is_err());
        }
    }

    #[test]
    fn local_source_reads_the_profile_path() {
        let dir = std::env::temp_dir().join(format!("zwsnap-profile-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("api/profiles")).unwrap();
        let mut bytes = Vec::new();
        bytes_field(&mut bytes, 4, b"Offline");
        varint_field(&mut bytes, 35, 1000);
        std::fs::write(dir.join("api/profiles/me"), &bytes).unwrap();

        let api = Api::new(&format!("file://{}", dir.display()));
        let profile = api.fetch("ignored").unwrap();
        assert_eq!(profile.first_name.as_deref(), Some("Offline"));
        assert_eq!(profile.total_distance_in_meters, Some(1000));

        let _ = std::fs::remove_dir_all(&dir);
    }
}
