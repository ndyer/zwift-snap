//! WebKitGTK panel pinned to the bottom of the launcher window.
//!
//! The panel always shows Zwift's launcher embed page: the registration variant
//! when no account is remembered, the plain page otherwise. When the launcher
//! reaches its login gate it asks the panel to start the OAuth login; the panel
//! loads the Keycloak authorization URL and, once WebKit navigates to the
//! `http://zwift/?code=...` redirect, exchanges the code for a token. The login
//! page is only ever shown *in place of* the embed while a login is in progress;
//! once it succeeds the embed page is put back (see [`LoginPanel::show_embed`]).
//!
//! The panel always runs on an **ephemeral** (RAM-only) session. A successful
//! login's Keycloak cookies are copied into the account's persistent session
//! (`accounts/<id>/webkit/`), which is the remembered login; the token itself is
//! handed to `crate::auth` and never written anywhere.
//!
//! Links in the page are handed to the system browser with `xdg-open` rather than
//! followed inside the panel, which is far too small for a web page.

use crate::auth::AuthToken;
use crate::cancel::Cancellation;
use crate::logging::Logger;
use crate::oauth;
use crate::profile;
use gtk::glib;
use gtk::prelude::*;
use serde_json::Value;
use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::sync::mpsc;
use std::sync::Arc;
use std::time::Duration;
use webkit6::prelude::*;

use super::session;
use crate::accounts::Store;

/// Page shown before Zwift is installed, and until the login gate opens.
pub const EMBED_URL: &str = "https://cdn.zwift.com/static/web/launcher/embed.html";

/// The same page in its registration variant, shown when no account is
/// remembered.
pub const REGISTRATION_URL: &str =
    "https://cdn.zwift.com/static/web/launcher/embed.html?registration=true";

/// The embed page to load. A run with no remembered account gets Zwift's
/// registration variant.
fn embed_url(registration: bool) -> &'static str {
    if registration {
        REGISTRATION_URL
    } else {
        EMBED_URL
    }
}

/// A URI that must be handed to the system browser rather than loaded in the
/// panel. Only the schemes a web page may reasonably link to are accepted;
/// `javascript:`, `file:` and the like stay with WebKit.
fn external_url(uri: &str) -> Option<&str> {
    let scheme = uri.split_once(':')?.0;
    let supported = scheme.eq_ignore_ascii_case("http")
        || scheme.eq_ignore_ascii_case("https")
        || scheme.eq_ignore_ascii_case("mailto");
    supported.then_some(uri)
}

/// Hand `uri` to the system browser. Inside the snap `xdg-open` is snapd's
/// `user-open` proxy, so this reaches the host's default handler for the scheme.
/// The opener runs detached from the UI thread while a helper thread collects its
/// output: when the host has no handler for the scheme the proxy fails and says
/// so on stderr, which would otherwise be lost with the child.
fn open_externally(uri: &str, logger: Option<&Arc<Logger>>) {
    let opener = std::env::var("ZWIFT_OPEN_COMMAND").unwrap_or_else(|_| "xdg-open".to_owned());
    if let Some(logger) = logger {
        logger.info(&format!("Opening {uri} in the system browser ({opener})"));
    }
    let spawned = std::process::Command::new(&opener)
        .arg(uri)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn();
    match spawned {
        Ok(child) => {
            // Waiting also reaps the child. A browser that stays open for the
            // session simply keeps this thread alive until the launcher exits.
            let uri = uri.to_owned();
            let logger = logger.cloned();
            std::thread::spawn(move || report_opener(&uri, child, logger));
        }
        Err(error) => match logger {
            Some(logger) => logger.error(&format!("Unable to open {uri} in a browser: {error}")),
            None => eprintln!("Unable to open {uri} in a browser: {error}"),
        },
    }
}

/// Log how the opener ended. snapd's `user-open` proxy exits non-zero and
/// explains itself on stderr when the host cannot open the URL, which is the only
/// signal a confined snap gets about the host's handlers.
fn report_opener(uri: &str, child: std::process::Child, logger: Option<Arc<Logger>>) {
    let Ok(output) = child.wait_with_output() else {
        return;
    };
    let Some(logger) = logger else {
        return;
    };
    let stderr = String::from_utf8_lossy(&output.stderr);
    let stderr = stderr.trim();
    if output.status.success() {
        if stderr.is_empty() {
            logger.debug(&format!("{uri} was handed to the host"));
        } else {
            logger.debug(&format!("{uri} was handed to the host: {stderr}"));
        }
    } else if stderr.is_empty() {
        logger.warning(&format!(
            "Unable to open {uri}: the opener exited with {}",
            output.status
        ));
    } else {
        logger.warning(&format!("Unable to open {uri}: {stderr}"));
    }
}

/// Outcome of inspecting a WebKit navigation target.
#[derive(Debug, PartialEq, Eq)]
enum Redirect {
    /// The OAuth redirect carried an authorization code.
    Code(String),
    /// The redirect's `state` did not match the one we generated.
    BadState,
    /// The redirect carried no code; optionally an `error` parameter.
    Failed(Option<String>),
}

/// Parse a navigation target. Returns `None` when the URI is not the OAuth
/// redirect (`http://zwift/?...`), so unrelated navigations are left alone.
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

/// PKCE and pending-decision state shared between the panel and its callbacks.
struct LoginState {
    verifier: String,
    state: String,
    pending: Option<mpsc::Sender<bool>>,
}

/// The bottom panel: a WebKit view plus the login bookkeeping around it. The
/// panel always runs on an ephemeral (RAM-only) session; a login's cookies are
/// copied into the account's persistent session on success (see `gui::session`).
pub struct LoginPanel {
    webview: RefCell<webkit6::WebView>,
    container: gtk::ScrolledWindow,
    state: Rc<RefCell<LoginState>>,
    cancellation: Cancellation,
    logger: Option<Arc<Logger>>,
    store: RefCell<Option<Rc<RefCell<Store>>>>,
    /// The ephemeral token the launcher launches with; the panel sets it on a
    /// successful login.
    auth: Arc<AuthToken>,
    /// Whether a login through this panel consults the "Remember me" cookie to
    /// decide persistence. The launcher's own gate does; an "add another account"
    /// login from the picker does not (its ephemeral session has no cookie) and is
    /// always remembered.
    remember_session: Rc<Cell<bool>>,
}

impl LoginPanel {
    /// Build the panel on a fresh ephemeral session and immediately load the
    /// Zwift launcher embed page, or its registration variant when `registration`
    /// is set. `logger` is the launcher's own log, used to record link handling.
    pub fn new(
        cancellation: Cancellation,
        registration: bool,
        logger: Option<Arc<Logger>>,
        store: Option<Rc<RefCell<Store>>>,
        auth: Arc<AuthToken>,
    ) -> Self {
        let store = RefCell::new(store);
        let webview = session::ephemeral_webview();
        let state = Rc::new(RefCell::new(LoginState {
            verifier: String::new(),
            state: String::new(),
            pending: None,
        }));
        let remember_session = Rc::new(Cell::new(true));
        wire_webview(
            &webview,
            &state,
            &cancellation,
            &logger,
            &store,
            &auth,
            &remember_session,
        );
        webview.load_uri(embed_url(registration));

        let container = gtk::ScrolledWindow::builder()
            .child(&webview)
            .min_content_height(320)
            .vexpand(true)
            .hexpand(true)
            .build();

        Self {
            webview: RefCell::new(webview),
            container,
            state,
            cancellation,
            logger,
            store,
            auth,
            remember_session,
        }
    }

    /// Whether a login through this panel is remembered. The launcher's gate
    /// leaves this `true` (it consults the "Remember me" cookie); the picker's
    /// "add another account" sets it `false`, so the added rider is always
    /// remembered.
    pub fn set_remember_session(&self, remember: bool) {
        self.remember_session.set(remember);
    }

    /// The widget to append to the window layout.
    pub fn widget(&self) -> &gtk::Widget {
        self.container.upcast_ref()
    }

    /// Whether a login is already in progress (a decision is awaiting an answer).
    pub fn has_pending(&self) -> bool {
        self.state.borrow().pending.is_some()
    }

    /// Rebuild the panel's WebView on a fresh ephemeral session, dropping any
    /// cookies the previous view held. Used before a fresh login (the picker's
    /// "add another account") so an in-flight SSO cookie cannot sign the new
    /// login into the account already shown. Pending-decision state is left
    /// untouched, so the caller follows this with [`LoginPanel::start_login`].
    pub fn reset_session(&self) {
        let webview = session::ephemeral_webview();
        wire_webview(
            &webview,
            &self.state,
            &self.cancellation,
            &self.logger,
            &self.store,
            &self.auth,
            &self.remember_session,
        );
        self.container.set_child(Some(&webview));
        *self.webview.borrow_mut() = webview;
    }

    /// Start the OAuth login, storing `decision` to be answered when it completes.
    pub fn start_login(&self, decision: mpsc::Sender<bool>) {
        {
            let mut state = self.state.borrow_mut();
            state.pending = Some(decision);
        }
        load_login_page(&self.webview.borrow(), &self.state);
    }

    /// Put the launcher's embed page back after a login replaced it, on a fresh
    /// ephemeral session so the completed login's cookies are dropped.
    pub fn show_embed(&self) {
        self.reset_session();
        self.webview.borrow().load_uri(EMBED_URL);
    }

    /// Abort a pending login without completing it (window closed or cancelled).
    pub fn cancel_login(&self) {
        if let Some(reply) = self.state.borrow_mut().pending.take() {
            let _ = reply.send(false);
        }
    }
}

/// Intercept the OAuth redirect and every link the page tries to follow on
/// `webview`. The wiring outlives individual logins and is re-applied whenever a
/// fresh WebView replaces the panel's previous one.
fn wire_webview(
    webview: &webkit6::WebView,
    state: &Rc<RefCell<LoginState>>,
    cancellation: &Cancellation,
    logger: &Option<Arc<Logger>>,
    store: &RefCell<Option<Rc<RefCell<Store>>>>,
    auth: &Arc<AuthToken>,
    remember_session: &Rc<Cell<bool>>,
) {
    use webkit6::PolicyDecisionType;

    let weak_state = Rc::downgrade(state);
    let cancellation = cancellation.clone();
    let logger = logger.clone();
    let store = store.borrow().clone();
    let auth = auth.clone();
    let remember_session = remember_session.clone();
    webview.connect_decide_policy(move |webview, decision, decision_type| {
        let navigating = decision_type == PolicyDecisionType::NavigationAction;
        // A link click arrives as a navigation action and a page asking for a
        // new window as a new-window action. Everything else is WebKit's.
        if !navigating && decision_type != PolicyDecisionType::NewWindowAction {
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
        if let Some(logger) = &logger {
            let kind = if navigating {
                "navigation"
            } else {
                "new window"
            };
            logger.debug(&format!("WebKit requested a {kind} to {uri}"));
        }

        if navigating {
            if let Some(state) = weak_state.upgrade() {
                let expected = state.borrow().state.clone();
                if let Some(outcome) = redirect_outcome(uri.as_str(), &expected) {
                    // Never let WebKit load the redirect; we consume it here.
                    decision.ignore();
                    match outcome {
                        Redirect::BadState => {
                            eprintln!("Zwift login failed: invalid OAuth state");
                        }
                        Redirect::Failed(error) => {
                            match error {
                                Some(error) => {
                                    eprintln!("Zwift login was not completed ({error})")
                                }
                                None => eprintln!("Zwift login was not completed"),
                            }
                            // Return to the login page so the user can try again.
                            load_login_page(webview, &state);
                        }
                        Redirect::Code(code) => {
                            start_exchange(
                                webview.clone(),
                                state,
                                code,
                                cancellation.clone(),
                                store.clone(),
                                auth.clone(),
                                remember_session.get(),
                                logger.clone(),
                            );
                        }
                    }
                    return true;
                }
            }
            // The launcher loads the Keycloak page itself and the user
            // completes it by submitting a form; only a deliberate link
            // click should leave the panel.
            if navigation.navigation_type() != webkit6::NavigationType::LinkClicked {
                return false;
            }
        }

        // Follow the link in the system browser instead: the panel is far too
        // small for a web page and offers no way back.
        let Some(url) = external_url(uri.as_str()) else {
            if let Some(logger) = &logger {
                logger.debug(&format!("Leaving {uri} to WebKit"));
            }
            return false;
        };
        decision.ignore();
        open_externally(url, logger.as_ref());
        true
    });
}

/// Regenerate PKCE material and load the Keycloak authorization URL.
fn load_login_page(webview: &webkit6::WebView, state: &Rc<RefCell<LoginState>>) {
    let verifier = oauth::generate_pkce_verifier();
    let challenge = oauth::compute_pkce_challenge(&verifier);
    let state_value = oauth::generate_random_state();
    let uri = oauth::build_authorization_url(&challenge, &state_value, None);
    {
        let mut state = state.borrow_mut();
        state.verifier = verifier;
        state.state = state_value;
    }
    webview.load_uri(&uri);
}

/// Exchange the authorization code for a token off-thread, then record the
/// account, hold the token and answer the pending decision (or reload the login
/// page on failure so the user can retry).
///
/// The account is recorded at login time so the picker lists the rider next
/// launch. Whether the login is *remembered* depends on the "Remember me"
/// cookie: a remembered login's Keycloak cookies are copied into the account's
/// persistent session; a forgotten one copies nothing (NEVER-WRITE).
#[allow(clippy::too_many_arguments)] // One call site; the arguments are distinct collaborators.
fn start_exchange(
    webview: webkit6::WebView,
    state: Rc<RefCell<LoginState>>,
    code: String,
    cancellation: Cancellation,
    store: Option<Rc<RefCell<Store>>>,
    auth: Arc<AuthToken>,
    remember_session: bool,
    logger: Option<Arc<Logger>>,
) {
    let verifier = state.borrow().verifier.clone();
    let (result_sender, result_receiver) = mpsc::channel();
    std::thread::spawn(move || {
        let result = oauth::exchange_code_for_token(&code, &verifier);
        let _ = result_sender.send(result);
    });
    glib::timeout_add_local(Duration::from_millis(100), move || {
        if cancellation.is_cancelled() {
            return glib::ControlFlow::Break;
        }
        let result = match result_receiver.try_recv() {
            Ok(result) => result,
            Err(mpsc::TryRecvError::Empty) => return glib::ControlFlow::Continue,
            Err(mpsc::TryRecvError::Disconnected) => Err("exchange worker stopped".to_owned()),
        };
        match result {
            Ok(token) => {
                // Hold the token so the launch can read it, then record the
                // account so the picker lists the rider.
                auth.set(token.clone());
                let id = store
                    .as_ref()
                    .and_then(|store| record_account(store, &token, logger.as_ref()));
                // Answer the decision so the game may launch with the token.
                if let Some(reply) = state.borrow_mut().pending.take() {
                    let _ = reply.send(true);
                }
                // Persist the login: the gate honours the "Remember me" cookie,
                // an added account is always remembered.
                if let (Some(store), Some(id)) = (store.clone(), id) {
                    if remember_session {
                        check_remember_me_cookie(&webview, &store, &id, logger.clone());
                    } else {
                        apply_remember(&webview, &store, &id, true, logger.as_ref());
                    }
                }
                // Put the embed page back in place of the login page.
                webview.load_uri(EMBED_URL);
            }
            Err(error) => {
                eprintln!("Zwift login failed: {error}");
                // Keep the pending decision and return to the login page.
                load_login_page(&webview, &state);
            }
        }
        glib::ControlFlow::Break
    });
}

/// Record the just-logged-in account (identity and card fields) and select it.
/// Returns its id, or `None` when no identity could be derived.
fn record_account(
    store: &Rc<RefCell<Store>>,
    token: &Value,
    logger: Option<&Arc<Logger>>,
) -> Option<String> {
    let claims = oauth::token_claims(token);
    let mut store = store.borrow_mut();
    match store.upsert(&profile::Profile::default(), &claims) {
        Ok(id) => {
            if let Err(error) = store.select(&id) {
                warn(
                    logger,
                    &format!("Unable to select the new account: {error}"),
                );
            }
            if let Err(error) = store.save() {
                warn(logger, &format!("Unable to save accounts: {error}"));
            }
            Some(id)
        }
        Err(error) => {
            warn(
                logger,
                &format!("Unable to remember the new account: {error}"),
            );
            None
        }
    }
}

/// Persist, or drop, the login. On "remember", the fresh Keycloak cookies are
/// copied into the account's persistent session, which becomes the remembered
/// login; on "forget", nothing is copied and the session is marked to be
/// forgotten at exit (NEVER-WRITE).
fn apply_remember(
    webview: &webkit6::WebView,
    store: &Rc<RefCell<Store>>,
    id: &str,
    remember: bool,
    logger: Option<&Arc<Logger>>,
) {
    if remember {
        let dir = store.borrow().session_dir(id);
        match session::account_session(&dir) {
            Some(target) => {
                if let Some(source) = webview.network_session() {
                    // Ownership of `target` moves into the copy, keeping the
                    // session alive until WebKit has written the cookies.
                    session::copy_realm_cookies(&source, target);
                }
            }
            None => warn(
                logger,
                &format!("Unable to prepare the session store for {id}"),
            ),
        }
    }
    {
        let mut store = store.borrow_mut();
        store.set_forget_on_exit(!remember);
        if let Err(error) = store.save() {
            warn(logger, &format!("Unable to save accounts: {error}"));
        }
    }
}

fn warn(logger: Option<&Arc<Logger>>, message: &str) {
    match logger {
        Some(logger) => logger.warning(message),
        None => eprintln!("{message}"),
    }
}

/// Read the "Remember me" cookie from the panel's session, then persist or drop
/// the login accordingly. An absent cookie means the user unchecked "Remember
/// me", so the login is not persisted (NEVER-WRITE). When the cookie cannot be
/// read at all the login is kept, so a remembered rider is never left
/// unlaunchable.
fn check_remember_me_cookie(
    webview: &webkit6::WebView,
    store: &Rc<RefCell<Store>>,
    id: &str,
    logger: Option<Arc<Logger>>,
) {
    let Some(cookie_manager) = webview.network_session().and_then(|s| s.cookie_manager()) else {
        eprintln!("Warning: unable to read the session cookies; keeping the login");
        apply_remember(webview, store, id, true, logger.as_ref());
        return;
    };
    let cancellable = gtk::gio::Cancellable::new();
    let webview = webview.clone();
    let store = store.clone();
    let id = id.to_owned();
    cookie_manager.cookies(session::COOKIE_URI, Some(&cancellable), move |result| {
        let remember = match result {
            Ok(cookies) => cookies
                .into_iter()
                .any(|mut cookie| cookie.name().as_deref() == Some("KEYCLOAK_REMEMBER_ME")),
            Err(error) => {
                eprintln!(
                    "Warning: unable to read the session cookies ({error}); keeping the login"
                );
                true
            }
        };
        apply_remember(&webview, &store, &id, remember, logger.as_ref());
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn non_redirect_navigation_is_ignored() {
        assert_eq!(
            redirect_outcome("https://secure.zwift.com/login", "abc"),
            None
        );
        assert_eq!(redirect_outcome("http://zwift/", "abc"), None);
    }

    #[test]
    fn redirect_with_code_returns_code() {
        assert_eq!(
            redirect_outcome("http://zwift/?code=xyz&state=abc", "abc"),
            Some(Redirect::Code("xyz".to_owned()))
        );
    }

    #[test]
    fn redirect_with_wrong_state_is_rejected() {
        assert_eq!(
            redirect_outcome("http://zwift/?code=xyz&state=other", "abc"),
            Some(Redirect::BadState)
        );
    }

    #[test]
    fn redirect_with_error_is_failed() {
        assert_eq!(
            redirect_outcome("http://zwift/?error=access_denied&state=abc", "abc"),
            Some(Redirect::Failed(Some("access_denied".to_owned())))
        );
        assert_eq!(
            redirect_outcome("http://zwift/?state=abc", "abc"),
            Some(Redirect::Failed(None))
        );
    }

    #[test]
    fn redirect_url_decodes_values() {
        assert_eq!(
            redirect_outcome("http://zwift/?code=a%2Bb&state=abc", "abc"),
            Some(Redirect::Code("a+b".to_owned()))
        );
    }

    #[test]
    fn registration_flag_selects_the_registration_page() {
        assert_eq!(embed_url(true), REGISTRATION_URL);
        assert_eq!(embed_url(false), EMBED_URL);
    }

    #[test]
    fn only_web_schemes_are_external() {
        for uri in [
            "https://zwift.com/support",
            "http://zwift.com/",
            "HTTPS://zwift.com/",
            "mailto:support@zwift.com",
        ] {
            assert_eq!(external_url(uri), Some(uri), "{uri} should open externally");
        }
        for uri in [
            "javascript:alert(1)",
            "file:///etc/passwd",
            "ftp://zwift.com/",
            "zwift.com/support",
            "",
        ] {
            assert_eq!(external_url(uri), None, "{uri} should stay with WebKit");
        }
    }
}
