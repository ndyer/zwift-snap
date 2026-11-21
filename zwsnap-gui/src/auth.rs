//! The launcher's in-memory OAuth token.
//!
//! Tokens are **ephemeral**: the launcher never writes one to disk. A token is
//! minted when it is needed (a login, or a silent re-authentication from an
//! account's stored WebKit session) and handed to the game as
//! `--token=<json>` at launch time. What *is* remembered across restarts is each
//! account's WebKit cookie jar (`accounts/<id>/webkit/`), which carries the
//! Keycloak SSO session; see `gui::session`.
//!
//! [`AuthToken`] is the small shared cell for that token: the GUI thread sets it
//! (after a login or a silent re-auth) and the worker thread reads it at launch
//! time via [`TokenSource`]. It is deliberately trivial - no persistence -
//! because nothing about the token outlives the session.

use serde_json::Value;
use std::sync::{Arc, Mutex};

/// The token the game is about to be launched with, read at launch time.
///
/// This is the seam that replaced `LaunchConfig.token_file`: the launcher no
/// longer writes a token to disk for the worker to read, so the worker asks a
/// shared source instead.
pub trait TokenSource: Send + Sync {
    /// The token to launch with, or `None` for an unauthenticated launch.
    fn token(&self) -> Result<Option<Value>, String>;
}

/// The one token the launcher currently holds, shared between the GUI (which
/// mints it) and the worker (which launches with it).
///
/// `Send + Sync`, so the same handle can be used from the launcher worker thread.
#[derive(Default, Debug)]
pub struct AuthToken {
    token: Mutex<Option<Value>>,
}

impl AuthToken {
    /// A new, empty token cell.
    pub fn new() -> Arc<Self> {
        Arc::new(Self::default())
    }

    /// Hold `token`, replacing any previous one (a fresh login, or a silent
    /// re-authentication for the selected account).
    pub fn set(&self, token: Value) {
        *self.token.lock().expect("auth token poisoned") = Some(token);
    }

    /// Drop the token (a forget-on-exit purge, or before starting a fresh login).
    pub fn clear(&self) {
        *self.token.lock().expect("auth token poisoned") = None;
    }
}

impl TokenSource for AuthToken {
    fn token(&self) -> Result<Option<Value>, String> {
        Ok(self.token.lock().expect("auth token poisoned").clone())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn set_and_clear_round_trip() {
        let auth = AuthToken::new();
        assert_eq!(auth.token().unwrap(), None);
        auth.set(serde_json::json!({ "access_token": "t" }));
        assert!(auth.token().unwrap().is_some());
        auth.clear();
        assert_eq!(auth.token().unwrap(), None);
    }
}
