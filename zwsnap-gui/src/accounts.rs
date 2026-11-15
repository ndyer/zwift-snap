//! The multi-account store: `$SNAP_USER_COMMON/accounts.json` plus one directory
//! per account under `$SNAP_USER_COMMON/accounts/`.
//!
//! The identity claims in an access token and the profile behind it are not
//! enough to launch with: the launcher needs to remember *several* riders and
//! offer them for selection. This module owns that list. Each account keeps its
//! own token (`accounts/<key>/token.json`) and avatar (`accounts/<key>/avatar.*`),
//! and selecting one copies its token over the launch token
//! (`$SNAP_USER_COMMON/pairing-token.json`) that the rest of the launcher already
//! understands.
//!
//! The directory key is a hex encoding of the account id, so a hostile or odd
//! `sub` claim can never contain a path separator or `..` and escape the store.
//!
//! Loading is tolerant in the same style as the settings file: a missing or
//! invalid `accounts.json` yields an empty store, and a single bad entry is
//! skipped rather than discarding the rest.

use crate::oauth::{save_token, Claims};
use crate::profile::Profile;
use serde_json::{json, Map, Value};
use std::fmt::Write as _;
use std::fs;
use std::io::{self, Write};
use std::path::{Path, PathBuf};

/// The store file, inside `$SNAP_USER_COMMON`.
pub const FILE_NAME: &str = "accounts.json";

/// The launch token the game itself is given, rewritten when an account is selected.
pub const LAUNCH_TOKEN: &str = "pairing-token.json";

/// Bumped if the on-disk layout ever changes shape.
const VERSION: u32 = 1;

/// The name shown when neither the profile nor the claims name the rider.
const FALLBACK_NAME: &str = "Zwift rider";

/// One remembered rider: the derived profile fields that make a card, plus the
/// name of the avatar file inside the account's directory.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Account {
    /// Hex-encoded account id; also the directory name under `accounts/`.
    pub id: String,
    /// The name shown on the card.
    pub name: String,
    pub first_name: Option<String>,
    pub last_name: Option<String>,
    pub email: Option<String>,
    /// Where the avatar was (or would be) fetched from.
    pub avatar_url: Option<String>,
    /// The avatar's file name inside the account directory, when one was stored.
    pub avatar: Option<String>,
    pub use_metric: Option<bool>,
    pub distance_meters: Option<i64>,
    pub elevation_meters: Option<i64>,
    pub time_minutes: Option<i64>,
    pub weight_grams: Option<i64>,
    pub ftp: Option<i64>,
    pub level: Option<i64>,
    pub height_millimeters: Option<i64>,
}

impl Account {
    /// Decode an account from its JSON object. Returns `None` when there is no
    /// usable id, so a corrupt entry is skipped rather than kept half-built.
    fn from_value(value: &Value) -> Option<Account> {
        let id = value.get("id")?.as_str()?.to_owned();
        if id.is_empty() {
            return None;
        }
        let text = |key: &str| value.get(key).and_then(Value::as_str).map(str::to_owned);
        let number = |key: &str| value.get(key).and_then(Value::as_i64);
        Some(Account {
            id,
            name: text("name").unwrap_or_else(|| FALLBACK_NAME.to_owned()),
            first_name: text("first_name"),
            last_name: text("last_name"),
            email: text("email"),
            avatar_url: text("avatar_url"),
            avatar: text("avatar"),
            use_metric: value.get("use_metric").and_then(Value::as_bool),
            distance_meters: number("distance_meters"),
            elevation_meters: number("elevation_meters"),
            time_minutes: number("time_minutes"),
            weight_grams: number("weight_grams"),
            ftp: number("ftp"),
            level: number("level"),
            height_millimeters: number("height_millimeters"),
        })
    }

    /// Encode an account as its JSON object, omitting fields that are unset.
    fn to_value(&self) -> Value {
        let mut object = Map::new();
        object.insert("id".into(), json!(self.id));
        object.insert("name".into(), json!(self.name));
        let mut put_str = |key: &str, value: &Option<String>| {
            if let Some(value) = value {
                object.insert(key.into(), json!(value));
            }
        };
        put_str("first_name", &self.first_name);
        put_str("last_name", &self.last_name);
        put_str("email", &self.email);
        put_str("avatar_url", &self.avatar_url);
        put_str("avatar", &self.avatar);
        let mut put_int = |key: &str, value: Option<i64>| {
            if let Some(value) = value {
                object.insert(key.into(), json!(value));
            }
        };
        put_int("distance_meters", self.distance_meters);
        put_int("elevation_meters", self.elevation_meters);
        put_int("time_minutes", self.time_minutes);
        put_int("weight_grams", self.weight_grams);
        put_int("ftp", self.ftp);
        put_int("level", self.level);
        put_int("height_millimeters", self.height_millimeters);
        if let Some(use_metric) = self.use_metric {
            object.insert("use_metric".into(), json!(use_metric));
        }
        Value::Object(object)
    }
}

/// The remembered accounts plus where they live.
pub struct Store {
    /// `$SNAP_USER_COMMON/accounts.json`.
    path: PathBuf,
    /// `$SNAP_USER_COMMON/accounts/`, one sub-directory per account.
    dir: PathBuf,
    /// `$SNAP_USER_COMMON/pairing-token.json`, the token the game is launched with.
    token_file: PathBuf,
    /// The account that was used last, by id.
    last_used: Option<String>,
    /// The remembered accounts, most recently added last.
    pub accounts: Vec<Account>,
}

impl Store {
    /// Point a store at the launcher's data directory. Nothing is read yet; call
    /// [`Store::load`].
    pub fn open(common: &Path) -> Store {
        Store {
            path: common.join(FILE_NAME),
            dir: common.join("accounts"),
            token_file: common.join(LAUNCH_TOKEN),
            last_used: None,
            accounts: Vec::new(),
        }
    }

    /// Read the store file. A missing file is an empty store; an unreadable or
    /// malformed one is reported and treated as empty.
    pub fn load(&mut self) {
        self.accounts.clear();
        self.last_used = None;
        let text = match fs::read_to_string(&self.path) {
            Ok(text) => text,
            Err(error) if error.kind() == io::ErrorKind::NotFound => return,
            Err(error) => {
                eprintln!("warning: cannot read {}: {error}", self.path.display());
                return;
            }
        };
        let value: Value = match serde_json::from_str(&text) {
            Ok(value) => value,
            Err(error) => {
                eprintln!(
                    "warning: {} is not valid JSON: {error}",
                    self.path.display()
                );
                return;
            }
        };
        self.last_used = value
            .get("last_used")
            .and_then(Value::as_str)
            .map(str::to_owned);
        if let Some(accounts) = value.get("accounts").and_then(Value::as_array) {
            self.accounts = accounts.iter().filter_map(Account::from_value).collect();
        }
        // A `last_used` naming a vanished account is dropped.
        if self
            .last_used
            .as_ref()
            .is_some_and(|id| self.find(id).is_none())
        {
            self.last_used = None;
        }
    }

    /// Write the store file atomically (temp file + rename), owner-only (0600).
    pub fn save(&self) -> io::Result<()> {
        let value = json!({
            "version": VERSION,
            "last_used": self.last_used,
            "accounts": self.accounts.iter().map(Account::to_value).collect::<Vec<_>>(),
        });
        let text = serde_json::to_string_pretty(&value)
            .map_err(|error| io::Error::other(error.to_string()))?;
        write_private_atomic(&self.path, text.as_bytes())
    }

    /// Whether the store knows about any account.
    pub fn is_empty(&self) -> bool {
        self.accounts.is_empty()
    }

    /// The account with `id`, if any.
    pub fn find(&self, id: &str) -> Option<&Account> {
        self.accounts.iter().find(|account| account.id == id)
    }

    /// The account that was used last, if any.
    pub fn last_used(&self) -> Option<&Account> {
        self.last_used.as_deref().and_then(|id| self.find(id))
    }

    /// The id of the account that was used last, if any.
    pub fn last_used_id(&self) -> Option<&str> {
        self.last_used.as_deref()
    }

    /// The directory holding an account's private files.
    pub fn account_dir(&self, id: &str) -> PathBuf {
        self.dir.join(id)
    }

    /// The path of an account's stored avatar, when one exists on disk.
    pub fn avatar_path(&self, account: &Account) -> Option<PathBuf> {
        let name = account.avatar.as_ref()?;
        let path = self.account_dir(&account.id).join(name);
        path.is_file().then_some(path)
    }

    /// Add or refresh an account from a freshly exchanged `token`, keyed by the
    /// identity in `claims` (falling back to the profile's own id). The profile
    /// may be empty when it could not be fetched; existing card data is kept in
    /// that case. Returns the account id.
    pub fn upsert(
        &mut self,
        token: &Value,
        profile: &Profile,
        claims: &Claims,
    ) -> Result<String, String> {
        let identity =
            identity_of(profile, claims).ok_or_else(|| "token names no rider".to_owned())?;
        let id = encode_key(&identity);
        let dir = self.account_dir(&id);
        fs::create_dir_all(&dir).map_err(|e| format!("cannot create {}: {e}", dir.display()))?;
        save_token(token, &dir.join("token.json"))
            .map_err(|e| format!("cannot save account token: {e}"))?;

        let name = display_name(profile, claims);
        match self.accounts.iter_mut().find(|account| account.id == id) {
            Some(existing) => merge_profile(existing, profile, &name),
            None => {
                let mut account = Account {
                    id: id.clone(),
                    name: name.clone(),
                    ..Account::default()
                };
                merge_profile(&mut account, profile, &name);
                self.accounts.push(account);
            }
        }
        Ok(id)
    }

    /// Store `bytes` as an account's avatar with the given extension, replacing
    /// any previous avatar file.
    pub fn write_avatar(&mut self, id: &str, bytes: &[u8], ext: &str) -> io::Result<()> {
        let ext = ext.trim_start_matches('.').to_ascii_lowercase();
        let dir = self.account_dir(id);
        fs::create_dir_all(&dir)?;
        let name = format!("avatar.{ext}");
        write_private_atomic(&dir.join(&name), bytes)?;
        // Remove any older avatar with a different extension.
        if let Ok(entries) = fs::read_dir(&dir) {
            for entry in entries.flatten() {
                let file_name = entry.file_name();
                let file_name = file_name.to_string_lossy();
                if file_name.starts_with("avatar.") && &*file_name != name.as_str() {
                    let _ = fs::remove_file(entry.path());
                }
            }
        }
        if let Some(account) = self.accounts.iter_mut().find(|account| account.id == id) {
            account.avatar = Some(name);
        }
        Ok(())
    }

    /// Merge a freshly fetched `profile` into the account with `id`, keeping
    /// every field the profile does not carry. Returns `false` when the account
    /// is gone, which happens when it was removed while the refresh was in
    /// flight.
    pub fn refresh(&mut self, id: &str, profile: &Profile) -> bool {
        let Some(account) = self.accounts.iter_mut().find(|account| account.id == id) else {
            return false;
        };
        merge_profile(account, profile, &profile_name(profile));
        true
    }

    /// Whether `id` is the account currently selected for launch.
    pub fn is_active(&self, id: &str) -> bool {
        self.last_used.as_deref() == Some(id)
    }

    /// Read the launch token (`pairing-token.json`) as written by the launcher's
    /// own login. Used right after adding an account to learn who just signed in.
    pub fn launch_token(&self) -> Result<Value, String> {
        read_json(&self.token_file)
    }

    /// Read an account's stored token (`accounts/<key>/token.json`), used to
    /// refresh its profile in the background.
    pub fn account_token(&self, id: &str) -> Result<Value, String> {
        read_json(&self.account_dir(id).join("token.json"))
    }

    /// Make `id` the account the launcher will start: copy its token over the
    /// launch token and remember it as the last used.
    pub fn select(&mut self, id: &str) -> Result<(), String> {
        let token_path = self.account_dir(id).join("token.json");
        let text = fs::read_to_string(&token_path)
            .map_err(|e| format!("cannot read {}: {e}", token_path.display()))?;
        let token: Value = serde_json::from_str(&text)
            .map_err(|e| format!("{} is not valid JSON: {e}", token_path.display()))?;
        save_token(&token, &self.token_file)
            .map_err(|e| format!("cannot update launch token: {e}"))?;
        self.last_used = Some(id.to_owned());
        Ok(())
    }

    /// Forget an account: drop it from the list, delete its directory, and clear
    /// the launch token when it was the selected account.
    pub fn remove(&mut self, id: &str) -> Result<(), String> {
        self.accounts.retain(|account| account.id != id);
        let dir = self.account_dir(id);
        if dir.exists() {
            fs::remove_dir_all(&dir)
                .map_err(|e| format!("cannot remove {}: {e}", dir.display()))?;
        }
        if self.last_used.as_deref() == Some(id) {
            self.last_used = None;
            let _ = fs::remove_file(&self.token_file);
        }
        Ok(())
    }

    /// Migrate a legacy install: when the store is empty but a launch token
    /// already exists, adopt that token as the first account. Returns the new
    /// account id, or `None` when there is nothing to migrate.
    pub fn import_legacy(&mut self, claims: &Claims) -> Result<Option<String>, String> {
        if !self.accounts.is_empty() || !self.token_file.is_file() {
            return Ok(None);
        }
        let text = fs::read_to_string(&self.token_file)
            .map_err(|e| format!("cannot read {}: {e}", self.token_file.display()))?;
        let token: Value = serde_json::from_str(&text)
            .map_err(|e| format!("{} is not valid JSON: {e}", self.token_file.display()))?;
        let id = self.upsert(&token, &Profile::default(), claims)?;
        self.last_used = Some(id.clone());
        Ok(Some(id))
    }
}

/// Read a JSON file, mapping IO and parse errors to a message.
fn read_json(path: &Path) -> Result<Value, String> {
    let text =
        fs::read_to_string(path).map_err(|e| format!("cannot read {}: {e}", path.display()))?;
    serde_json::from_str(&text).map_err(|e| format!("{} is not valid JSON: {e}", path.display()))
}

/// Hex-encode a raw id, so it is always a safe single path component.
fn encode_key(raw: &str) -> String {
    let mut out = String::with_capacity(raw.len() * 2);
    for byte in raw.as_bytes() {
        let _ = write!(out, "{byte:02x}");
    }
    out
}

/// The stable identity for an account: the token's `sub`, then the profile's
/// public id or numeric id, then the email. Claims win because they survive a
/// profile that could not be fetched.
fn identity_of(profile: &Profile, claims: &Claims) -> Option<String> {
    let prefer = |value: Option<String>| value.filter(|value| !value.is_empty());
    prefer(claims.sub.clone())
        .or_else(|| prefer(profile.public_id.clone()))
        .or_else(|| profile.id.map(|id| id.to_string()))
        .or_else(|| prefer(claims.email.clone()))
        .or_else(|| prefer(profile.email.clone()))
}

/// The name the profile itself gives, or an empty string when it names nobody.
fn profile_name(profile: &Profile) -> String {
    let joined = match (
        profile.first_name.as_deref().filter(|s| !s.is_empty()),
        profile.last_name.as_deref().filter(|s| !s.is_empty()),
    ) {
        (Some(first), Some(last)) => Some(format!("{first} {last}")),
        (Some(first), None) => Some(first.to_owned()),
        (None, Some(last)) => Some(last.to_owned()),
        (None, None) => None,
    };
    joined.unwrap_or_default()
}

/// The name to show, preferring the profile's own names.
fn display_name(profile: &Profile, claims: &Claims) -> String {
    let joined = Some(profile_name(profile)).filter(|s| !s.is_empty());
    joined
        .or_else(|| claims.preferred_username.clone().filter(|s| !s.is_empty()))
        .unwrap_or_else(|| FALLBACK_NAME.to_owned())
}

/// Copy the card fields from `profile` onto `account`, leaving anything the
/// profile does not carry untouched (so a failed refresh keeps the last values).
fn merge_profile(account: &mut Account, profile: &Profile, name: &str) {
    if !name.is_empty() {
        account.name = name.to_owned();
    }
    let keep = |slot: &mut Option<String>, value: &Option<String>| {
        if let Some(value) = value.clone().filter(|s| !s.is_empty()) {
            *slot = Some(value);
        }
    };
    keep(&mut account.first_name, &profile.first_name);
    keep(&mut account.last_name, &profile.last_name);
    keep(&mut account.email, &profile.email);
    keep(&mut account.avatar_url, &profile.large_avatar_url);
    let set = |slot: &mut Option<i64>, value: Option<i64>| {
        if value.is_some() {
            *slot = value;
        }
    };
    set(&mut account.weight_grams, profile.weight_in_grams);
    set(&mut account.ftp, profile.ftp);
    set(
        &mut account.distance_meters,
        profile.total_distance_in_meters,
    );
    set(
        &mut account.elevation_meters,
        profile.elevation_gain_in_meters,
    );
    set(&mut account.time_minutes, profile.time_ridden_in_minutes);
    set(
        &mut account.height_millimeters,
        profile.height_in_millimeters,
    );
    set(&mut account.level, profile.achievement_level);
    if profile.use_metric.is_some() {
        account.use_metric = profile.use_metric;
    }
}

/// Write `data` to `path` atomically, with owner-only permissions, creating the
/// parent directory.
fn write_private_atomic(path: &Path, data: &[u8]) -> io::Result<()> {
    use std::os::unix::ffi::OsStrExt;
    use std::os::unix::fs::OpenOptionsExt;
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
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

#[cfg(test)]
mod tests {
    use super::*;

    /// A unique, private scratch directory for one test.
    fn scratch(tag: &str) -> PathBuf {
        use std::sync::atomic::{AtomicU32, Ordering};
        static COUNTER: AtomicU32 = AtomicU32::new(0);
        let n = COUNTER.fetch_add(1, Ordering::Relaxed);
        let dir =
            std::env::temp_dir().join(format!("zwsnap-accounts-{tag}-{}-{n}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn claims(sub: &str, username: &str) -> Claims {
        Claims {
            sub: Some(sub.to_owned()),
            preferred_username: Some(username.to_owned()),
            email: None,
        }
    }

    fn token(access: &str) -> Value {
        json!({ "access_token": access, "refresh_token": "r" })
    }

    fn profile(first: &str, last: &str) -> Profile {
        Profile {
            id: Some(42),
            first_name: Some(first.to_owned()),
            last_name: Some(last.to_owned()),
            weight_in_grams: Some(70_000),
            ftp: Some(233),
            total_distance_in_meters: Some(233_400),
            elevation_gain_in_meters: Some(2444),
            time_ridden_in_minutes: Some(834),
            achievement_level: Some(42),
            use_metric: Some(true),
            large_avatar_url: Some("https://cdn.zwift.com/a.jpg".to_owned()),
            ..Profile::default()
        }
    }

    #[test]
    fn upsert_then_reload_round_trips() {
        let common = scratch("roundtrip");
        let mut store = Store::open(&common);
        let id = store
            .upsert(
                &token("abc"),
                &profile("Sara", "Campbell"),
                &claims("sub-1", "sara"),
            )
            .unwrap();
        store.save().unwrap();

        let mut reloaded = Store::open(&common);
        reloaded.load();
        assert_eq!(reloaded.accounts.len(), 1);
        let account = reloaded.find(&id).unwrap();
        assert_eq!(account.name, "Sara Campbell");
        assert_eq!(account.distance_meters, Some(233_400));
        assert_eq!(account.ftp, Some(233));
        assert_eq!(account.use_metric, Some(true));

        let _ = fs::remove_dir_all(&common);
    }

    #[test]
    fn the_directory_key_encodes_hostile_ids() {
        let common = scratch("hostile");
        let mut store = Store::open(&common);
        let id = store
            .upsert(
                &token("abc"),
                &Profile::default(),
                &claims("../../etc/passwd", "x"),
            )
            .unwrap();
        // Hex only, so no separators or dots survive.
        assert!(id.chars().all(|c| c.is_ascii_hexdigit()));
        let dir = store.account_dir(&id);
        assert!(dir.is_dir());
        assert!(dir.starts_with(&common));

        let _ = fs::remove_dir_all(&common);
    }

    #[test]
    fn select_writes_the_launch_token_and_remembers_the_account() {
        let common = scratch("select");
        let mut store = Store::open(&common);
        let id = store
            .upsert(
                &token("chosen"),
                &profile("Sara", "Campbell"),
                &claims("sub-1", "s"),
            )
            .unwrap();
        store.select(&id).unwrap();
        assert!(store.is_active(&id));

        let launch = fs::read_to_string(common.join(LAUNCH_TOKEN)).unwrap();
        assert!(launch.contains("chosen"));

        let _ = fs::remove_dir_all(&common);
    }

    #[test]
    fn remove_deletes_the_account_and_clears_an_active_launch_token() {
        let common = scratch("remove");
        let mut store = Store::open(&common);
        let id = store
            .upsert(
                &token("abc"),
                &profile("Sara", "Campbell"),
                &claims("sub-1", "s"),
            )
            .unwrap();
        store.select(&id).unwrap();
        store.remove(&id).unwrap();
        assert!(store.is_empty());
        assert!(store.last_used_id().is_none());
        assert!(!common.join(LAUNCH_TOKEN).exists());
        assert!(!store.account_dir(&id).exists());

        let _ = fs::remove_dir_all(&common);
    }

    #[test]
    fn removing_an_inactive_account_keeps_the_launch_token() {
        let common = scratch("removeinactive");
        let mut store = Store::open(&common);
        let a = store
            .upsert(&token("a"), &profile("A", "One"), &claims("sub-a", "a"))
            .unwrap();
        let b = store
            .upsert(&token("b"), &profile("B", "Two"), &claims("sub-b", "b"))
            .unwrap();
        store.select(&a).unwrap();
        store.remove(&b).unwrap();
        assert_eq!(store.accounts.len(), 1);
        assert!(store.is_active(&a));
        assert!(common.join(LAUNCH_TOKEN).exists());

        let _ = fs::remove_dir_all(&common);
    }

    #[test]
    fn a_second_upsert_merges_rather_than_duplicates() {
        let common = scratch("merge");
        let mut store = Store::open(&common);
        let id = store
            .upsert(
                &token("v1"),
                &profile("Sara", "Campbell"),
                &claims("sub-1", "s"),
            )
            .unwrap();
        // A later refresh updates one field and omits the rest.
        let mut update = profile("Sara", "Campbell");
        update.total_distance_in_meters = Some(999_999);
        update.ftp = None;
        let id2 = store
            .upsert(&token("v2"), &update, &claims("sub-1", "s"))
            .unwrap();
        assert_eq!(id, id2);
        assert_eq!(store.accounts.len(), 1);
        let account = store.find(&id).unwrap();
        assert_eq!(account.distance_meters, Some(999_999));
        assert_eq!(account.ftp, Some(233)); // kept from before

        let _ = fs::remove_dir_all(&common);
    }

    #[test]
    fn a_refresh_updates_the_card_and_keeps_what_it_omits() {
        let common = scratch("refresh");
        let mut store = Store::open(&common);
        let id = store
            .upsert(
                &token("a"),
                &profile("Sara", "Campbell"),
                &claims("sub-1", "s"),
            )
            .unwrap();
        // A refresh that carries new stats but no name or FTP: the card keeps
        // the name it had and the FTP it knew.
        let mut update = profile("", "");
        update.first_name = None;
        update.last_name = None;
        update.ftp = None;
        update.achievement_level = Some(51);
        update.total_distance_in_meters = Some(500_000);
        assert!(store.refresh(&id, &update));
        let account = store.find(&id).unwrap();
        assert_eq!(account.name, "Sara Campbell");
        assert_eq!(account.ftp, Some(233));
        assert_eq!(account.level, Some(51));
        assert_eq!(account.distance_meters, Some(500_000));
        // Refreshing an account that was removed meanwhile is not an error.
        assert!(!store.refresh("nope", &update));

        let _ = fs::remove_dir_all(&common);
    }

    #[test]
    fn avatar_replacement_removes_the_older_file() {
        let common = scratch("avatar");
        let mut store = Store::open(&common);
        let id = store
            .upsert(
                &token("a"),
                &profile("Sara", "Campbell"),
                &claims("sub-1", "s"),
            )
            .unwrap();
        store.write_avatar(&id, b"png-bytes", "png").unwrap();
        store.write_avatar(&id, b"jpg-bytes", "jpg").unwrap();
        let account = store.find(&id).unwrap();
        assert_eq!(account.avatar.as_deref(), Some("avatar.jpg"));
        let path = store.avatar_path(account).unwrap();
        assert_eq!(fs::read(&path).unwrap(), b"jpg-bytes");
        assert!(!store.account_dir(&id).join("avatar.png").exists());

        let _ = fs::remove_dir_all(&common);
    }

    #[test]
    fn avatar_replacement_keeps_old_file_if_write_fails() {
        let common = scratch("avatar-failed-write");
        let mut store = Store::open(&common);
        let id = store
            .upsert(
                &token("a"),
                &profile("Sara", "Campbell"),
                &claims("sub-1", "s"),
            )
            .unwrap();
        store.write_avatar(&id, b"png-bytes", "png").unwrap();
        fs::create_dir(store.account_dir(&id).join("avatar.jpg.tmp")).unwrap();
        assert!(store.write_avatar(&id, b"jpg-bytes", "jpg").is_err());
        let account = store.find(&id).unwrap();
        assert_eq!(account.avatar.as_deref(), Some("avatar.png"));
        assert_eq!(
            fs::read(store.avatar_path(account).unwrap()).unwrap(),
            b"png-bytes"
        );

        let _ = fs::remove_dir_all(&common);
    }

    #[test]
    fn legacy_install_is_adopted_once() {
        let common = scratch("legacy");
        // A pre-picker install: only the launch token exists.
        save_token(&token("legacy-token"), &common.join(LAUNCH_TOKEN)).unwrap();

        let mut store = Store::open(&common);
        let id = store
            .import_legacy(&claims("legacy-sub", "old.rider"))
            .unwrap()
            .expect("legacy token should be adopted");
        assert_eq!(store.accounts.len(), 1);
        assert_eq!(store.find(&id).unwrap().name, "old.rider");
        assert!(store.is_active(&id));
        // A second import is a no-op now that the store is not empty.
        assert_eq!(store.import_legacy(&claims("x", "y")).unwrap(), None);

        let _ = fs::remove_dir_all(&common);
    }

    #[test]
    fn missing_or_broken_store_loads_empty() {
        let common = scratch("broken");
        let mut store = Store::open(&common);
        store.load();
        assert!(store.is_empty());

        fs::write(common.join(FILE_NAME), b"not json").unwrap();
        store.load();
        assert!(store.is_empty());

        let _ = fs::remove_dir_all(&common);
    }

    #[test]
    fn a_bad_entry_is_skipped_and_the_rest_kept() {
        let common = scratch("badentry");
        let text = json!({
            "version": 1,
            "last_used": "gone",
            "accounts": [
                { "name": "no id" },
                { "id": "aa", "name": "Kept" },
                { "id": "", "name": "empty id" },
            ]
        })
        .to_string();
        fs::write(common.join(FILE_NAME), text).unwrap();
        let mut store = Store::open(&common);
        store.load();
        assert_eq!(store.accounts.len(), 1);
        assert_eq!(store.accounts[0].name, "Kept");
        // `last_used` names an account that is not in the list, so it is dropped.
        assert!(store.last_used_id().is_none());

        let _ = fs::remove_dir_all(&common);
    }

    #[test]
    fn upsert_requires_an_identity() {
        let common = scratch("noid");
        let mut store = Store::open(&common);
        // A token with no claims and a profile with no id cannot be keyed.
        let result = store.upsert(&token("x"), &Profile::default(), &Claims::default());
        assert!(result.is_err());

        let _ = fs::remove_dir_all(&common);
    }
}
