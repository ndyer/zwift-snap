//! Persistent WebKitGTK panel pinned to the bottom of the launcher window.
//!
//! The panel always shows Zwift's launcher embed page. When the launcher reaches
//! its login gate it asks the panel to start the OAuth login; the panel loads the
//! Keycloak authorization URL and, once WebKit navigates to the
//! `http://zwift/?code=...` redirect, exchanges the code for a token and unblocks
//! the worker. The page content is never replaced after login (the game window
//! takes over instead).

use crate::cancel::Cancellation;
use crate::oauth;
use gtk::glib;
use gtk::prelude::*;
use std::cell::RefCell;
use std::rc::Rc;
use std::sync::mpsc;
use std::time::Duration;
use webkit6::prelude::*;

/// Page shown before Zwift is installed, and until the login gate opens.
pub const EMBED_URL: &str = "https://cdn.zwift.com/static/web/launcher/embed.html";

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
    pub fn new(cancellation: Cancellation) -> Self {
        use webkit6::PolicyDecisionType;

        let webview = super::persistent_webview().unwrap_or_default();
        let state = Rc::new(RefCell::new(LoginState {
            verifier: String::new(),
            state: String::new(),
            pending: None,
        }));

        // Intercept the OAuth redirect once; the closure outlives individual logins.
        let weak_state = Rc::downgrade(&state);
        let cancellation = cancellation.clone();
        webview.connect_decide_policy(move |webview, decision, decision_type| {
            if decision_type != PolicyDecisionType::NavigationAction {
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
            let Some(state) = weak_state.upgrade() else {
                return false;
            };
            let expected = state.borrow().state.clone();
            let Some(outcome) = redirect_outcome(uri.as_str(), &expected) else {
                return false;
            };
            // Never let WebKit load the redirect; we consume it here.
            decision.ignore();
            match outcome {
                Redirect::BadState => {
                    eprintln!("Zwift login failed: invalid OAuth state");
                }
                Redirect::Failed(error) => {
                    match error {
                        Some(error) => eprintln!("Zwift login was not completed ({error})"),
                        None => eprintln!("Zwift login was not completed"),
                    }
                    // Return to the login page so the user can try again.
                    load_login_page(webview, &state);
                }
                Redirect::Code(code) => {
                    start_exchange(webview.clone(), state, code, cancellation.clone());
                }
            }
            true
        });

        webview.load_uri(EMBED_URL);

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
}
