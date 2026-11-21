//! The multi-account store: `$SNAP_USER_COMMON/accounts.json` plus one directory
//! per account under `$SNAP_USER_COMMON/accounts/`.
//!
//! The identity claims in an access token and the profile behind it are not
//! enough to launch with: the launcher needs to remember *several* riders and
//! offer them for selection. This module owns that list. Each account keeps its
//! avatar (`accounts/<key>/avatar.*`) and, once it has logged in, its own WebKit
//! cookie jar (`accounts/<key>/webkit/`) - that jar is the Keycloak SSO session
//! and the only persisted login state (see `gui::session`). Tokens are never
//! stored: they are minted from the jar at need and kept in memory only.
//!
//! Selecting an account only records which one the launcher will start.
//!
//! The directory key is a hex encoding of the account id, so a hostile or odd
//! `sub` claim can never contain a path separator or `..` and escape the store.
//!
//! Loading is tolerant in the same style as the settings file: a missing or
//! invalid `accounts.json` yields an empty store, and a single bad entry is
//! skipped rather than discarding the rest.

use crate::oauth::Claims;
use crate::profile::Profile;
use serde_json::{json, Map, Value};
use std::fmt::Write as _;
use std::fs;
use std::io::{self, Write};
use std::path::{Path, PathBuf};

/// The store file, inside `$SNAP_USER_COMMON`.
pub const FILE_NAME: &str = "accounts.json";

/// The launch token of older launchers, imported and removed on first load.
pub const LEGACY_LAUNCH_TOKEN: &str = "pairing-token.json";

/// An account's own token file from older launchers, removed on first load.
const LEGACY_ACCOUNT_TOKEN: &str = "token.json";

/// The subdirectory of an account's directory holding its WebKit session (the
/// cookie jar that remembers the Keycloak login).
const SESSION_DIR: &str = "webkit";

/// The cookie database inside [`SESSION_DIR`].
const SESSION_COOKIE_DB: &str = "cookies.sqlite";

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
    /// Progress toward the next level, 0-99 percent (from `achievement_level % 100`).
    pub level_progress: Option<i64>,
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
            level_progress: number("level_progress"),
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
        put_int("level_progress", self.level_progress);
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
    /// The account that was used last, by id.
    last_used: Option<String>,
    /// Whether to forget the last used account on exit (when "Remember me" was unchecked).
    forget_on_exit: bool,
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
            last_used: None,
            forget_on_exit: false,
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
        self.forget_on_exit = value
            .get("forget_on_exit")
            .and_then(Value::as_bool)
            .unwrap_or(false);
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
            "forget_on_exit": self.forget_on_exit,
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

    /// Whether to forget the last used account on exit.
    pub fn forget_on_exit(&self) -> bool {
        self.forget_on_exit
    }

    /// Set whether to forget the last used account on exit.
    pub fn set_forget_on_exit(&mut self, forget: bool) {
        self.forget_on_exit = forget;
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

    /// The directory holding an account's WebKit session: the cookie jar that
    /// remembers its Keycloak login.
    pub fn session_dir(&self, id: &str) -> PathBuf {
        self.account_dir(id).join(SESSION_DIR)
    }

    /// The cookie database of an account's WebKit session.
    pub fn session_cookie_db(&self, id: &str) -> PathBuf {
        self.session_dir(id).join(SESSION_COOKIE_DB)
    }

    /// Whether `id` has a stored WebKit session, i.e. a login the launcher can
    /// re-authenticate from without asking. False until the account has logged in
    /// with "Remember me".
    pub fn has_session(&self, id: &str) -> bool {
        self.session_cookie_db(id).is_file()
    }

    /// Whether any remembered account has a usable session.
    pub fn has_any_session(&self) -> bool {
        self.accounts
            .iter()
            .any(|account| self.has_session(&account.id))
    }

    /// Add or refresh an account, keyed by the identity in `claims` (falling back
    /// to the profile's own id). The profile may be empty when it could not be
    /// fetched; existing card data is kept in that case. No token is stored: the
    /// caller keeps the freshly exchanged token in memory for the launch.
    /// Returns the account id.
    pub fn upsert(&mut self, profile: &Profile, claims: &Claims) -> Result<String, String> {
        let identity =
            identity_of(profile, claims).ok_or_else(|| "token names no rider".to_owned())?;
        let id = encode_key(&identity);
        let dir = self.account_dir(&id);
        fs::create_dir_all(&dir).map_err(|e| format!("cannot create {}: {e}", dir.display()))?;

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

    /// Make `id` the account the launcher will start.
    pub fn select(&mut self, id: &str) -> Result<(), String> {
        if self.find(id).is_none() {
            return Err(format!("unknown account {id}"));
        }
        self.last_used = Some(id.to_owned());
        Ok(())
    }

    /// Forget an account: drop it from the list and delete its directory (which
    /// holds its avatar and its WebKit session, so the login is gone too). When
    /// it was the selected account, the selection is cleared.
    pub fn remove(&mut self, id: &str) -> Result<(), String> {
        self.accounts.retain(|account| account.id != id);
        let dir = self.account_dir(id);
        if dir.exists() {
            fs::remove_dir_all(&dir)
                .map_err(|e| format!("cannot remove {}: {e}", dir.display()))?;
        }
        // Clearing the selection when the last account goes away keeps an emptied
        // store from being re-adopted on the next startup (which would hide the
        // registration page and resurrect a forgotten rider).
        if self.accounts.is_empty() || self.last_used.as_deref() == Some(id) {
            self.last_used = None;
        }
        Ok(())
    }

    /// Delete token files left by older launchers. The launcher no longer keeps
    /// tokens in files, and a token cannot be turned into a WebKit session, so a
    /// legacy `pairing-token.json` (or `accounts/<id>/token.json`) is simply
    /// removed: the rider logs in again. Best-effort; runs once at startup.
    pub fn cleanup_legacy(&mut self) {
        if let Some(common) = self.dir.parent() {
            let _ = fs::remove_file(common.join(LEGACY_LAUNCH_TOKEN));
        }
        for account in &self.accounts {
            let _ = fs::remove_file(self.account_dir(&account.id).join(LEGACY_ACCOUNT_TOKEN));
        }
    }
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
    // `achievement_level` packs the level as `level * 100 + progress`: zwift-offline's
    // `api_users_login` derives the level with `achievement_level // 100`, so the
    // quotient is the rider's real level and the remainder is progress toward the
    // next one (the card's level bar). A raw stored level self-heals on the next
    // profile refresh.
    if let Some(raw) = profile.achievement_level {
        account.level = Some(raw / 100);
        account.level_progress = Some(raw % 100);
    }
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

    fn open(common: &Path) -> Store {
        Store::open(common)
    }

    fn claims(sub: &str, username: &str) -> Claims {
        Claims {
            sub: Some(sub.to_owned()),
            preferred_username: Some(username.to_owned()),
            email: None,
        }
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
            achievement_level: Some(4200),
            use_metric: Some(true),
            large_avatar_url: Some("https://cdn.zwift.com/a.jpg".to_owned()),
            ..Profile::default()
        }
    }

    /// Give `id` a WebKit session so it counts as launchable.
    fn with_session(store: &Store, id: &str) {
        fs::create_dir_all(store.session_dir(id)).unwrap();
        fs::write(store.session_cookie_db(id), b"sqlite").unwrap();
    }

    #[test]
    fn upsert_then_reload_round_trips() {
        let common = scratch("roundtrip");
        let mut store = open(&common);
        let id = store
            .upsert(&profile("Sara", "Campbell"), &claims("sub-1", "sara"))
            .unwrap();
        store.save().unwrap();

        let mut reloaded = open(&common);
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
        let mut store = open(&common);
        let id = store
            .upsert(&Profile::default(), &claims("../../etc/passwd", "x"))
            .unwrap();
        // Hex only, so no separators or dots survive.
        assert!(id.chars().all(|c| c.is_ascii_hexdigit()));
        let dir = store.account_dir(&id);
        assert!(dir.is_dir());
        assert!(dir.starts_with(&common));

        let _ = fs::remove_dir_all(&common);
    }

    #[test]
    fn selecting_remembers_the_active_account() {
        let common = scratch("select");
        let mut store = open(&common);
        let id = store
            .upsert(&profile("Sara", "Campbell"), &claims("sub-1", "s"))
            .unwrap();
        store.select(&id).unwrap();
        assert!(store.is_active(&id));
        assert!(!common.join(LEGACY_LAUNCH_TOKEN).exists());

        let _ = fs::remove_dir_all(&common);
    }

    #[test]
    fn session_presence_tracks_the_cookie_database() {
        let common = scratch("session");
        let mut store = open(&common);
        let id = store
            .upsert(&profile("Sara", "Campbell"), &claims("sub-1", "s"))
            .unwrap();
        assert!(!store.has_session(&id));
        assert!(!store.has_any_session());
        with_session(&store, &id);
        assert!(store.has_session(&id));
        assert!(store.has_any_session());

        let _ = fs::remove_dir_all(&common);
    }

    #[test]
    fn removing_the_active_account_deletes_its_session() {
        let common = scratch("remove");
        let mut store = open(&common);
        let id = store
            .upsert(&profile("Sara", "Campbell"), &claims("sub-1", "s"))
            .unwrap();
        with_session(&store, &id);
        store.select(&id).unwrap();
        store.remove(&id).unwrap();
        assert!(store.is_empty());
        assert!(store.last_used_id().is_none());
        assert!(!store.account_dir(&id).exists());

        let _ = fs::remove_dir_all(&common);
    }

    #[test]
    fn removing_an_inactive_account_keeps_the_active_selection() {
        let common = scratch("removeinactive");
        let mut store = open(&common);
        let a = store
            .upsert(&profile("A", "One"), &claims("sub-a", "a"))
            .unwrap();
        let b = store
            .upsert(&profile("B", "Two"), &claims("sub-b", "b"))
            .unwrap();
        store.select(&a).unwrap();
        store.remove(&b).unwrap();
        assert_eq!(store.accounts.len(), 1);
        assert!(store.is_active(&a));

        let _ = fs::remove_dir_all(&common);
    }

    #[test]
    fn removing_the_last_account_clears_the_selection() {
        let common = scratch("removelast");
        let mut store = open(&common);
        let a = store
            .upsert(&profile("A", "One"), &claims("sub-a", "a"))
            .unwrap();
        let b = store
            .upsert(&profile("B", "Two"), &claims("sub-b", "b"))
            .unwrap();
        store.select(&b).unwrap();
        store.remove(&a).unwrap();
        // Removing the inactive account leaves the selection alone.
        assert!(store.is_active(&b));
        store.remove(&b).unwrap();
        assert!(store.is_empty());
        assert!(store.last_used_id().is_none());

        let _ = fs::remove_dir_all(&common);
    }

    #[test]
    fn a_second_upsert_merges_rather_than_duplicates() {
        let common = scratch("merge");
        let mut store = open(&common);
        let id = store
            .upsert(&profile("Sara", "Campbell"), &claims("sub-1", "s"))
            .unwrap();
        // A later refresh updates one field and omits the rest.
        let mut update = profile("Sara", "Campbell");
        update.total_distance_in_meters = Some(999_999);
        update.ftp = None;
        let id2 = store.upsert(&update, &claims("sub-1", "s")).unwrap();
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
        let mut store = open(&common);
        let id = store
            .upsert(&profile("Sara", "Campbell"), &claims("sub-1", "s"))
            .unwrap();
        // A refresh that carries new stats but no name or FTP: the card keeps
        // the name it had and the FTP it knew.
        let mut update = profile("", "");
        update.first_name = None;
        update.last_name = None;
        update.ftp = None;
        update.achievement_level = Some(5100);
        update.total_distance_in_meters = Some(500_000);
        assert!(store.refresh(&id, &update));
        let account = store.find(&id).unwrap();
        assert_eq!(account.name, "Sara Campbell");
        assert_eq!(account.ftp, Some(233));
        assert_eq!(account.level, Some(51));
        assert_eq!(account.level_progress, Some(0));
        assert_eq!(account.distance_meters, Some(500_000));
        // Refreshing an account that was removed meanwhile is not an error.
        assert!(!store.refresh("nope", &update));

        let _ = fs::remove_dir_all(&common);
    }

    #[test]
    fn achievement_level_splits_into_level_and_progress() {
        let common = scratch("levelsplit");
        let mut store = open(&common);
        let mut prof = profile("Sara", "Campbell");
        prof.achievement_level = Some(2834); // level 28, 34% to level 29
        let id = store.upsert(&prof, &claims("sub-1", "s")).unwrap();
        let account = store.find(&id).unwrap();
        assert_eq!(account.level, Some(28));
        assert_eq!(account.level_progress, Some(34));

        let _ = fs::remove_dir_all(&common);
    }

    #[test]
    fn avatar_replacement_removes_the_older_file() {
        let common = scratch("avatar");
        let mut store = open(&common);
        let id = store
            .upsert(&profile("Sara", "Campbell"), &claims("sub-1", "s"))
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
        let mut store = open(&common);
        let id = store
            .upsert(&profile("Sara", "Campbell"), &claims("sub-1", "s"))
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
    fn legacy_token_files_are_deleted_and_nothing_is_adopted() {
        let common = scratch("legacy");
        // A pre-picker install: a launch token and a per-account token file.
        crate::oauth::save_token(
            &json!({ "access_token": "legacy" }),
            &common.join(LEGACY_LAUNCH_TOKEN),
        )
        .unwrap();
        let mut store = open(&common);
        let id = store
            .upsert(&profile("Sara", "Campbell"), &claims("sub-1", "s"))
            .unwrap();
        crate::oauth::save_token(
            &json!({ "access_token": "old" }),
            &store.account_dir(&id).join(LEGACY_ACCOUNT_TOKEN),
        )
        .unwrap();

        store.cleanup_legacy();
        assert!(!common.join(LEGACY_LAUNCH_TOKEN).exists());
        assert!(!store.account_dir(&id).join(LEGACY_ACCOUNT_TOKEN).exists());
        // No account was adopted and no session was invented.
        assert!(!store.has_session(&id));

        let _ = fs::remove_dir_all(&common);
    }

    #[test]
    fn missing_or_broken_store_loads_empty() {
        let common = scratch("broken");
        let mut store = open(&common);
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
        let mut store = open(&common);
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
        let mut store = open(&common);
        // A profile with no id and no claims cannot be keyed.
        let result = store.upsert(&Profile::default(), &Claims::default());
        assert!(result.is_err());

        let _ = fs::remove_dir_all(&common);
    }
}
