//! Persistent WebKitGTK panel pinned to the bottom of the launcher window.
//!
//! The panel always shows Zwift's launcher embed page: the registration variant
//! on the very first run, the plain page on every run after that. When the
//! launcher reaches its login gate it asks the panel to start the OAuth login;
//! the panel loads the Keycloak authorization URL and, once WebKit navigates to
//! the `http://zwift/?code=...` redirect, exchanges the code for a token and
//! unblocks the worker. The page content is never replaced after login (the game
//! window takes over instead).
//!
//! Links in the page are handed to the system browser with `xdg-open` rather than
//! followed inside the panel, which is far too small for a web page.

use crate::cancel::Cancellation;
use crate::logging::Logger;
use crate::oauth;
use gtk::glib;
use gtk::prelude::*;
use std::cell::RefCell;
use std::rc::Rc;
use std::sync::mpsc;
use std::sync::Arc;
use std::time::Duration;
use webkit6::prelude::*;

/// Page shown before Zwift is installed, and until the login gate opens.
pub const EMBED_URL: &str = "https://cdn.zwift.com/static/web/launcher/embed.html";

/// The same page in its registration variant, shown on the very first run.
pub const REGISTRATION_URL: &str =
    "https://cdn.zwift.com/static/web/launcher/embed.html?registration=true";

/// Marker file under `$SNAP_USER_COMMON` recording that the launcher has run
/// before. Its absence is what makes a run the first one.
const FIRST_RUN_MARKER: &str = "first-run";

/// The embed page to load. A first run gets Zwift's registration variant.
fn embed_url(first_run: bool) -> &'static str {
    if first_run {
        REGISTRATION_URL
    } else {
        EMBED_URL
    }
}

/// Consume the first-run marker under `base`: this returns true exactly once, on
/// the call that creates it. A marker that cannot be created reports a returning
/// user, so a broken store can never turn the registration page into the
/// permanent landing page.
fn take_first_run(base: &std::path::Path) -> bool {
    let marker = base.join(FIRST_RUN_MARKER);
    if marker.exists() {
        return false;
    }
    std::fs::create_dir_all(base).is_ok() && std::fs::File::create(&marker).is_ok()
}

/// Whether this is the launcher's first run, and so whether it should show the
/// registration page. A preview never consumes the marker, so preview runs stay
/// reproducible, and a missing store is treated as a returning user.
fn is_first_run(preview: bool) -> bool {
    if preview {
        return false;
    }
    match std::env::var_os("SNAP_USER_COMMON") {
        Some(base) => take_first_run(std::path::Path::new(&base)),
        None => false,
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

/// The bottom panel: a WebKit view plus the login bookkeeping around it.
pub struct LoginPanel {
    webview: webkit6::WebView,
    container: gtk::ScrolledWindow,
    state: Rc<RefCell<LoginState>>,
}

impl LoginPanel {
    /// Build the panel and immediately load the Zwift launcher embed page.
    /// `logger` is the launcher's own log, used to record link handling.
    pub fn new(cancellation: Cancellation, preview: bool, logger: Option<Arc<Logger>>) -> Self {
        use webkit6::PolicyDecisionType;

        let webview = super::persistent_webview().unwrap_or_default();
        let state = Rc::new(RefCell::new(LoginState {
            verifier: String::new(),
            state: String::new(),
            pending: None,
        }));

        // Intercept the OAuth redirect and every link the page tries to follow;
        // the closure outlives individual logins.
        let weak_state = Rc::downgrade(&state);
        let cancellation = cancellation.clone();
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
                                start_exchange(webview.clone(), state, code, cancellation.clone());
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

        webview.load_uri(embed_url(is_first_run(preview)));

        let container = gtk::ScrolledWindow::builder()
            .child(&webview)
            .min_content_height(320)
            .vexpand(true)
            .hexpand(true)
            .build();

        Self {
            webview,
            container,
            state,
        }
    }

    /// The widget to append to the window layout.
    pub fn widget(&self) -> &gtk::Widget {
        self.container.upcast_ref()
    }

    /// Whether a login is already in progress (a decision is awaiting an answer).
    pub fn has_pending(&self) -> bool {
        self.state.borrow().pending.is_some()
    }

    /// Start the OAuth login, storing `decision` to be answered when it completes.
    pub fn start_login(&self, decision: mpsc::Sender<bool>) {
        {
            let mut state = self.state.borrow_mut();
            state.pending = Some(decision);
        }
        load_login_page(&self.webview, &self.state);
    }

    /// Abort a pending login without completing it (window closed or cancelled).
    pub fn cancel_login(&self) {
        if let Some(reply) = self.state.borrow_mut().pending.take() {
            let _ = reply.send(false);
        }
    }
}

/// Regenerate PKCE material and load the Keycloak authorization URL.
fn load_login_page(webview: &webkit6::WebView, state: &Rc<RefCell<LoginState>>) {
    let verifier = oauth::generate_pkce_verifier();
    let challenge = oauth::compute_pkce_challenge(&verifier);
    let state_value = oauth::generate_random_state();
    let uri = oauth::build_authorization_url(&challenge, &state_value);
    {
        let mut state = state.borrow_mut();
        state.verifier = verifier;
        state.state = state_value;
    }
    webview.load_uri(&uri);
}

/// Exchange the authorization code for a token off-thread, then answer the
/// pending decision (or reload the login page on failure so the user can retry).
fn start_exchange(
    webview: webkit6::WebView,
    state: Rc<RefCell<LoginState>>,
    code: String,
    cancellation: Cancellation,
) {
    let verifier = state.borrow().verifier.clone();
    let (result_sender, result_receiver) = mpsc::channel();
    std::thread::spawn(move || {
        let result = oauth::exchange_and_save(&code, &verifier);
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
            Ok(()) => {
                if let Some(reply) = state.borrow_mut().pending.take() {
                    let _ = reply.send(true);
                }
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
    fn first_run_selects_the_registration_page() {
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

    #[test]
    fn first_run_marker_is_consumed_once() {
        let base = std::env::temp_dir().join(format!("zwsnap-first-run-{}", std::process::id()));
        std::fs::remove_dir_all(&base).ok();
        assert!(
            take_first_run(&base),
            "the first run should report a first run"
        );
        assert!(base.join(FIRST_RUN_MARKER).exists());
        assert!(!take_first_run(&base), "the second run should not");
        std::fs::remove_dir_all(&base).ok();
    }

    #[test]
    fn unwritable_marker_reports_a_returning_user() {
        // A file where the marker's directory should be makes the marker
        // impossible to create, so the registration page must not be promised.
        let path =
            std::env::temp_dir().join(format!("zwsnap-first-run-file-{}", std::process::id()));
        std::fs::write(&path, b"not a directory").ok();
        assert!(!take_first_run(&path.join("nested")));
        std::fs::remove_file(&path).ok();
    }
}
