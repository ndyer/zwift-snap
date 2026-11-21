//! Per-account WebKit sessions: the cookie jar that remembers a rider's login,
//! and the silent re-authentication that mints a fresh token from it.
//!
//! Keycloak keeps one SSO session **per cookie jar**, so each account gets its
//! own persistent [`webkit6::NetworkSession`] rooted at `accounts/<id>/webkit/`.
//! That directory *is* the remembered login. Tokens themselves are never stored;
//! [`authorize`] re-runs the OIDC code flow against the jar (silently, with
//! `prompt=none`) to obtain one when it is needed.

use crate::cancel::Cancellation;
use crate::oauth;
use gtk::glib;
use gtk::prelude::*;
use std::cell::{Cell, RefCell};
use std::path::Path;
use std::rc::Rc;
use std::sync::mpsc;
use std::time::Duration;
use webkit6::prelude::*;

/// The Keycloak realm cookie scope: the cookies that carry the SSO session.
pub const COOKIE_URI: &str = "https://secure.zwift.com/auth/realms/zwift/";

/// How a (silent or interactive) OIDC authorization ended.
#[allow(dead_code)] // The failure detail is logged at the call site, not read here.
pub enum LoginOutcome {
    /// The flow produced an access token.
    Token(serde_json::Value),
    /// No session to re-authenticate from: the user must log in.
    NeedsLogin,
    /// The flow failed for another reason (network, exchange error).
    Failed(String),
}

/// Ensure `dir` exists (owner-only) with an owner-only cookie database, then
/// build a persistent session rooted there.
///
/// The directory and database hold live session cookies, so they are created
/// 0700/0600 before WebKit opens them. Returns `None` if the store cannot be
/// prepared, in which case the caller must fall back to an interactive login.
pub fn account_session(dir: &Path) -> Option<webkit6::NetworkSession> {
    use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt};
    std::fs::DirBuilder::new()
        .recursive(true)
        .mode(0o700)
        .create(dir)
        .ok()?;
    let db = dir.join("cookies.sqlite");
    // Create the empty database owner-only; SQLite keeps the mode when it opens it.
    std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .mode(0o600)
        .open(&db)
        .ok()?;
    let session = webkit6::NetworkSession::new(dir.to_str(), None);
    if let Some(cookies) = session.cookie_manager() {
        cookies.set_persistent_storage(
            &db.to_string_lossy(),
            webkit6::CookiePersistentStorage::Sqlite,
        );
    }
    Some(session)
}

/// A WebView on an ephemeral (RAM-only) session: the interactive login and the
/// embed page use this, so no login cookie lingers in the panel.
pub fn ephemeral_webview() -> webkit6::WebView {
    webkit6::WebView::builder()
        .network_session(&webkit6::NetworkSession::new_ephemeral())
        .build()
}

/// Copy the Keycloak realm cookies from `from` into `to`, preserving their
/// expiry so a "Remember me" session survives a restart.
///
/// Used after an interactive login: the (ephemeral) panel session holds the
/// fresh SSO cookies, and they are copied into the account's persistent session,
/// which becomes the remembered login.
pub fn copy_realm_cookies(from: &webkit6::NetworkSession, to: webkit6::NetworkSession) {
    let Some(source) = from.cookie_manager() else {
        return;
    };
    let cancellable = gtk::gio::Cancellable::new();
    // The cookies are copied as-is: `soup::Cookie` carries its own expiry, which
    // `add_cookie` preserves when WebKit writes the persistent store. `to` is
    // owned by the callback, keeping the session (and its persistent store) alive
    // until WebKit has written the cookies.
    source.cookies(COOKIE_URI, Some(&cancellable), move |result| {
        if let Ok(cookies) = result {
            if let Some(target) = to.cookie_manager() {
                for cookie in cookies {
                    target.add_cookie(&cookie, gtk::gio::Cancellable::NONE, |_| {});
                }
            }
        }
    });
}

/// Run the OIDC authorization-code flow in a hidden WebView on `session`.
///
/// With `interactive` false the authorize URL carries `prompt=none`, so a
/// session that still holds a valid SSO cookie is redirected straight to the
/// `http://zwift/?code=...` redirect with no form; a session that cannot be
/// re-authenticated silently comes back as [`LoginOutcome::NeedsLogin`].
///
/// `on_result` runs on the GTK main thread. The WebView lives only for this one
/// flow and is dropped once the result fires.
pub fn authorize(
    session: webkit6::NetworkSession,
    interactive: bool,
    cancellation: Cancellation,
    on_result: impl FnOnce(LoginOutcome) + 'static,
) {
    let verifier = oauth::generate_pkce_verifier();
    let challenge = oauth::compute_pkce_challenge(&verifier);
    let state_value = oauth::generate_random_state();

    // The token exchange runs off the main thread; its result is polled below.
    let (result_sender, result_receiver) = mpsc::channel::<LoginOutcome>();

    let webview = webkit6::WebView::builder()
        .network_session(&session)
        .build();
    // Keep the WebView alive for the life of the flow. The policy handler below
    // captures only weak references, so this strong reference is what stops the
    // view (and its session worker) being torn down before the redirect arrives.
    let keep_alive: Rc<RefCell<Option<webkit6::WebView>>> =
        Rc::new(RefCell::new(Some(webview.clone())));

    let expected = state_value.clone();
    let verifier_for_exchange = verifier.clone();
    let sender_for_policy = result_sender.clone();
    let fired = Rc::new(Cell::new(false));
    let fired_policy = fired.clone();
    let cancellable_for_policy = cancellation.clone();
    webview.connect_decide_policy(move |_webview, decision, decision_type| {
        if decision_type != webkit6::PolicyDecisionType::NavigationAction {
            return false;
        }
        let Some(mut navigation) = decision
            .downcast_ref::<webkit6::NavigationPolicyDecision>()
            .and_then(|decision| decision.navigation_action())
        else {
            return false;
        };
        let Some(uri) = navigation.request().and_then(|request| request.uri()) else {
            return false;
        };
        let Some(outcome) = redirect_outcome(uri.as_str(), &expected) else {
            return false;
        };
        // Never let WebKit load the redirect; we consume it here.
        decision.ignore();
        if fired_policy.replace(true) {
            return true;
        }
        match outcome {
            Redirect::Code(code) => {
                let sender = sender_for_policy.clone();
                let verifier = verifier_for_exchange.clone();
                let cancellation = cancellable_for_policy.clone();
                std::thread::spawn(move || {
                    if cancellation.is_cancelled() {
                        return;
                    }
                    let outcome = match oauth::exchange_code_for_token(&code, &verifier) {
                        Ok(token) => LoginOutcome::Token(token),
                        Err(error) => LoginOutcome::Failed(error),
                    };
                    let _ = sender.send(outcome);
                });
            }
            Redirect::Failed(_) => {
                // A silent attempt that could not reuse the session reports
                // `login_required`; either way the rider must log in again.
                let _ = sender_for_policy.send(LoginOutcome::NeedsLogin);
            }
            Redirect::BadState => {
                let _ = sender_for_policy.send(LoginOutcome::Failed("invalid OAuth state".into()));
            }
        }
        true
    });

    let url =
        oauth::build_authorization_url(&challenge, &state_value, (!interactive).then_some("none"));

    // A page that fails to load (no network, or the IdP is unreachable) must not
    // leave the flow hanging: report that a login is needed instead.
    let sender_for_load = result_sender.clone();
    let fired_load = fired.clone();
    webview.connect_load_failed(move |_webview, _event, _uri, _error| {
        if !fired_load.replace(true) {
            let _ = sender_for_load.send(LoginOutcome::NeedsLogin);
        }
        false
    });

    webview.load_uri(&url);

    // Poll for the exchange result on the main thread, then deliver it once.
    let on_result = Rc::new(RefCell::new(Some(
        Box::new(on_result) as Box<dyn FnOnce(LoginOutcome)>
    )));
    let on_result_poll = on_result.clone();
    let keep_alive_poll = keep_alive.clone();
    glib::timeout_add_local(Duration::from_millis(100), move || {
        if cancellation.is_cancelled() {
            keep_alive_poll.borrow_mut().take();
            return glib::ControlFlow::Break;
        }
        match result_receiver.try_recv() {
            Ok(outcome) => {
                keep_alive_poll.borrow_mut().take();
                if let Some(callback) = on_result_poll.borrow_mut().take() {
                    callback(outcome);
                }
                glib::ControlFlow::Break
            }
            Err(mpsc::TryRecvError::Empty) => glib::ControlFlow::Continue,
            Err(mpsc::TryRecvError::Disconnected) => {
                keep_alive_poll.borrow_mut().take();
                glib::ControlFlow::Break
            }
        }
    });
}

/// Outcome of inspecting a navigation target (mirrors the login panel's parser).
#[allow(dead_code)] // The error detail is not surfaced in the silent flow.
enum Redirect {
    Code(String),
    BadState,
    Failed(Option<String>),
}

/// Parse an authorization redirect (`http://zwift/?...`). `None` for anything
/// else, so unrelated navigations are left to WebKit.
fn redirect_outcome(uri: &str, expected_state: &str) -> Option<Redirect> {
    let query = uri.strip_prefix("http://zwift/?")?;
    let parameter = |name: &str| {
        query
            .split('&')
            .filter_map(|part| part.split_once('='))
            .find_map(|(key, value)| (key == name).then_some(value))
            .and_then(|value| urlencoding::decode(value).ok())
            .map(|value| value.into_owned())
    };
    if parameter("state").as_deref() != Some(expected_state) {
        return Some(Redirect::BadState);
    }
    match parameter("code").filter(|code| !code.is_empty()) {
        Some(code) => Some(Redirect::Code(code)),
        None => Some(Redirect::Failed(
            parameter("error").filter(|error| !error.is_empty()),
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::redirect_outcome;

    #[test]
    fn parses_code_and_state() {
        // A matching state with a code is the happy path.
        let outcome = redirect_outcome("http://zwift/?code=abc&state=xyz", "xyz");
        assert!(matches!(outcome, Some(super::Redirect::Code(code)) if code == "abc"));
    }

    #[test]
    fn mismatched_state_is_bad() {
        assert!(matches!(
            redirect_outcome("http://zwift/?code=abc&state=other", "xyz"),
            Some(super::Redirect::BadState)
        ));
    }

    #[test]
    fn error_without_code_is_failed() {
        // `prompt=none` against a session with no SSO cookie yields this.
        assert!(matches!(
            redirect_outcome("http://zwift/?error=login_required&state=xyz", "xyz"),
            Some(super::Redirect::Failed(Some(_)))
        ));
    }

    #[test]
    fn unrelated_navigation_is_ignored() {
        assert!(redirect_outcome("https://secure.zwift.com/login", "xyz").is_none());
    }
}
