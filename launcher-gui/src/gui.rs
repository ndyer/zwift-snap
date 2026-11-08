//! GTK frontend and channel bridge for the launcher worker.

use crate::cancel::Cancellation;
use crate::display::DisplayMode;
use crate::install::InstallProgress;
use crate::oauth;
use crate::orchestration::{self, Failure, LauncherBackend, LauncherUi};
use gtk::glib;
use gtk::prelude::*;
use std::io;
use std::sync::mpsc::{self, Receiver, Sender};
use std::time::Duration;

/// Events are sent to the GTK main thread, never directly to GTK widgets.
pub enum Event {
    Progress(InstallProgress),
    Log(String),
    Failure {
        kind: Failure,
        detail: String,
        decision: Sender<bool>,
    },
    LoginRequired {
        decision: Sender<bool>,
    },
    GameReady,
    Finished(Result<(), String>),
}

/// UI adapter to be owned by a launcher worker thread.
#[derive(Clone)]
pub struct WorkerUi {
    events: Sender<Event>,
    cancellation: Cancellation,
    require_login: bool,
}

impl WorkerUi {
    pub fn new(events: Sender<Event>) -> Self {
        Self::with_cancellation(events, Cancellation::default())
    }

    pub fn with_cancellation(events: Sender<Event>, cancellation: Cancellation) -> Self {
        Self {
            events,
            cancellation,
            require_login: true,
        }
    }

    pub fn set_require_login(&mut self, require_login: bool) {
        self.require_login = require_login;
    }

    pub fn cancellation(&self) -> Cancellation {
        self.cancellation.clone()
    }

    pub fn progress(&self, progress: InstallProgress) {
        if self.events.send(Event::Progress(progress)).is_err() {
            eprintln!("Unable to deliver progress: GTK window is closed");
        }
    }

    pub fn log(&self, text: String) {
        if self.events.send(Event::Log(text)).is_err() {
            eprintln!("Unable to deliver log output: GTK window is closed");
        }
    }

    pub fn finished(&self, result: Result<(), String>) {
        if self.events.send(Event::Finished(result)).is_err() {
            eprintln!("Unable to deliver launcher result: GTK window is closed");
        }
    }
}

impl LauncherUi for WorkerUi {
    fn failure(&mut self, kind: Failure, detail: &str) -> bool {
        if self.cancellation.is_cancelled() {
            return false;
        }
        let (decision, reply) = mpsc::channel();
        if self
            .events
            .send(Event::Failure {
                kind,
                detail: detail.to_owned(),
                decision,
            })
            .is_err()
        {
            eprintln!("Unable to show launcher error: GTK window is closed");
            return false;
        }
        loop {
            if self.cancellation.is_cancelled() {
                return false;
            }
            match reply.recv_timeout(Duration::from_millis(100)) {
                Ok(choice) => return choice && !self.cancellation.is_cancelled(),
                Err(mpsc::RecvTimeoutError::Timeout) => {}
                Err(error) => {
                    eprintln!("Unable to receive launcher decision: {error}");
                    return false;
                }
            }
        }
    }

    fn game_ready(&mut self) {
        if self.events.send(Event::GameReady).is_err() {
            eprintln!("Unable to report game startup: GTK window is closed");
        }
    }

    fn login_required(&mut self) -> bool {
        if !self.require_login {
            return true;
        }
        if self.cancellation.is_cancelled() {
            return false;
        }
        let (decision, reply) = mpsc::channel();
        if self.events.send(Event::LoginRequired { decision }).is_err() {
            return false;
        }
        loop {
            if self.cancellation.is_cancelled() {
                return false;
            }
            match reply.recv_timeout(Duration::from_millis(100)) {
                Ok(confirmed) => return confirmed && !self.cancellation.is_cancelled(),
                Err(mpsc::RecvTimeoutError::Timeout) => {}
                Err(_) => return false,
            }
        }
    }
}

/// Run a lifecycle on the worker thread and forward its final result to the UI.
pub fn run_controller<B: LauncherBackend>(backend: &mut B, ui: &mut WorkerUi) {
    let cancellation = ui.cancellation.clone();
    let result = orchestration::run_cancellable(backend, ui, &cancellation)
        .map_err(|error| error.to_string());
    ui.finished(result);
}

struct PreviewBackend {
    ui: WorkerUi,
    installed: bool,
    failed_once: bool,
}

impl LauncherBackend for PreviewBackend {
    type Error = io::Error;

    fn prepare(&mut self) -> Result<(), Self::Error> {
        self.ui
            .log("Preview only: no Wine commands are run.\n".into());
        self.ui.progress(InstallProgress::Phase {
            percent: 8,
            message: "Preparing Zwift".into(),
        });
        Ok(())
    }

    fn installed(&mut self) -> Result<bool, Self::Error> {
        Ok(self.installed)
    }

    fn install(&mut self) -> Result<(), Self::Error> {
        if !self.failed_once {
            self.ui.cancellation.wait(Duration::from_millis(500))?;
            self.ui.progress(InstallProgress::Sample(42));
            self.ui.log("Example installer output\n".into());
            self.failed_once = true;
            Err(io::Error::other("Example installation error"))
        } else {
            self.installed = true;
            Ok(())
        }
    }

    fn update(&mut self) -> Result<(), Self::Error> {
        Err(io::Error::other("Unexpected update in preview"))
    }

    fn configure_display(&mut self) -> Result<(), Self::Error> {
        Ok(())
    }

    fn start(&mut self) -> Result<(), Self::Error> {
        self.ui.progress(InstallProgress::Phase {
            percent: 95,
            message: "Starting Zwift".into(),
        });
        Ok(())
    }

    fn wait_for_exit(&mut self) -> Result<(), Self::Error> {
        Ok(())
    }

    fn cleanup(&mut self) -> Result<(), Self::Error> {
        Ok(())
    }
}

/// Preview the interface with simulated events. Does not launch Wine.
pub fn preview(mode: DisplayMode) -> io::Result<()> {
    run_application(mode, Cancellation::default(), true, |mut ui| {
        let cancellation = ui.cancellation();
        let mut backend = PreviewBackend {
            ui: ui.clone(),
            installed: false,
            failed_once: false,
        };
        orchestration::run_cancellable(&mut backend, &mut ui, &cancellation)
            .map_err(|error| error.to_string())
    })
}

/// Run the real backend on a worker while GTK owns the main thread.
pub fn launch(
    mode: DisplayMode,
    cancellation: Cancellation,
    worker: impl FnOnce(WorkerUi) -> Result<(), String> + Send + 'static,
) -> io::Result<()> {
    run_application(mode, cancellation, false, worker)
}

type Completion = std::rc::Rc<std::cell::RefCell<Option<Result<(), String>>>>;
type LoginDecision = std::rc::Rc<std::cell::RefCell<Option<Sender<bool>>>>;

/// Show a login dialog with an embedded WebKitGTK WebView for Zwift OAuth login.
///
/// The WebView loads the Zwift OAuth authorization URL. After the user logs in,
/// A WebView whose cookies persist in `$SNAP_USER_COMMON/webkit`, so Zwift's
/// "Remember me" cookie survives a launcher restart. The directory and cookie
/// database hold live session cookies, so they are created owner-only (0700/0600),
/// before WebKit opens them. Returns `None` if the store cannot be prepared; the
/// caller then falls back to a non-persistent view.
fn persistent_webview() -> Option<webkit6::WebView> {
    use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt};
    use webkit6::prelude::*;

    let common = std::env::var("SNAP_USER_COMMON").ok()?;
    let data_dir = std::path::PathBuf::from(common).join("webkit");
    std::fs::DirBuilder::new()
        .recursive(true)
        .mode(0o700)
        .create(&data_dir)
        .ok()?;
    let db_path = data_dir.join("cookies.sqlite");
    // Create the empty database owner-only; SQLite keeps the mode when it opens the file.
    std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .mode(0o600)
        .open(&db_path)
        .ok()?;

    let session = webkit6::NetworkSession::new(data_dir.to_str(), None);
    if let Some(cookies) = session.cookie_manager() {
        cookies.set_persistent_storage(
            &db_path.to_string_lossy(),
            webkit6::CookiePersistentStorage::Sqlite,
        );
    }
    Some(
        webkit6::WebView::builder()
            .network_session(&session)
            .build(),
    )
}

/// WebKit redirects to `http://zwift/?code=...`; we intercept that redirect,
/// extract the authorization code, exchange it for a token, save the token JSON
/// to disk, and update the login status label.
fn show_login_dialog(
    login_button: &gtk::Button,
    login_status: &gtk::Label,
    window: &glib::WeakRef<gtk::ApplicationWindow>,
    pending_login: LoginDecision,
    cancellation: Cancellation,
) {
    let Some(parent) = window.upgrade() else {
        return;
    };
    let dialog = gtk::Dialog::builder()
        .title("Log in to Zwift")
        .modal(true)
        .transient_for(&parent)
        .destroy_with_parent(true)
        .build();
    dialog.set_default_size(900, 700);
    login_button.set_sensitive(false);
    login_status.set_text("Complete login in the browser to continue");

    // Generate PKCE verifier and challenge for OAuth flow.
    // Must keep the verifier to exchange the code for a token later.
    let verifier = oauth::generate_pkce_verifier();
    let challenge = oauth::compute_pkce_challenge(&verifier);
    let state = oauth::generate_random_state();

    // WebView for OAuth authorization page.
    use webkit6::prelude::*;
    use webkit6::{PolicyDecisionType, WebView};
    let webview = persistent_webview().unwrap_or_else(WebView::new);
    let dialog_weak = dialog.downgrade();
    let status_weak = login_status.downgrade();
    let login_button_weak = login_button.downgrade();
    let verifier_owned = verifier.clone();
    let uri = oauth::build_authorization_url(&challenge, &state);

    // Intercept navigation to capture the OAuth redirect before WebKit loads it.
    webview.connect_decide_policy(move |_webview, decision, decision_type| {
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
        let uri = uri.as_str();
        if !uri.starts_with("http://zwift/?") {
            return false;
        }

        let query = uri
            .split_once('?')
            .map(|(_, query)| query)
            .unwrap_or_default();
        decision.ignore();
        let parameter = |name: &str| {
            query
                .split('&')
                .filter_map(|part| part.split_once('='))
                .find_map(|(key, value)| (key == name).then_some(value))
                .and_then(|value| urlencoding::decode(value).ok())
                .map(|value| value.into_owned())
        };
        if parameter("state").as_deref() != Some(state.as_str()) {
            if let Some(status) = status_weak.upgrade() {
                status.set_text("Login failed: invalid OAuth state. Close the dialog and retry.");
            }
            return true;
        }
        if let Some(code) = parameter("code").filter(|code| !code.is_empty()) {
            if let Some(dialog) = dialog_weak.upgrade() {
                dialog.set_visible(false);
            }
            if let Some(status) = status_weak.upgrade() {
                status.set_text("Exchanging code for token...");
            }
            // Only the HTTP exchange runs off-thread; GTK stays on its main loop.
            let verifier_clone = verifier_owned.clone();
            let (result_sender, result_receiver) = mpsc::channel();
            let status = status_weak.clone();
            let button = login_button_weak.clone();
            let pending_login = pending_login.clone();
            let cancellation = cancellation.clone();
            std::thread::spawn(move || {
                let result = oauth::exchange_and_save(&code, &verifier_clone);
                let _ = result_sender.send(result);
            });
            glib::timeout_add_local(Duration::from_millis(100), move || {
                if cancellation.is_cancelled() || status.upgrade().is_none() {
                    return glib::ControlFlow::Break;
                }
                let result = match result_receiver.try_recv() {
                    Ok(result) => result,
                    Err(mpsc::TryRecvError::Empty) => return glib::ControlFlow::Continue,
                    Err(mpsc::TryRecvError::Disconnected) => {
                        Err("exchange worker stopped".to_owned())
                    }
                };
                match result {
                    Ok(()) => {
                        if let Some(status) = status.upgrade() {
                            status.set_text("Logged in successfully");
                        }
                        if let Some(reply) = pending_login.borrow_mut().take() {
                            let _ = reply.send(true);
                        }
                    }
                    Err(error) => {
                        if let Some(status) = status.upgrade() {
                            status.set_text(&format!("Login failed: {error}. Please retry."));
                        }
                        if let Some(button) = button.upgrade() {
                            button.set_sensitive(true);
                        }
                    }
                }
                glib::ControlFlow::Break
            });
        } else if let Some(status) = status_weak.upgrade() {
            match parameter("error").filter(|error| !error.is_empty()) {
                Some(error) => status.set_text(&format!(
                    "Login was not completed ({error}). Close the dialog and retry."
                )),
                None => status.set_text("Login was not completed. Close the dialog and retry."),
            }
        }
        true
    });

    let scrolled = gtk::ScrolledWindow::builder()
        .child(&webview)
        .vexpand(true)
        .hexpand(true)
        .build();
    dialog.content_area().append(&scrolled);

    dialog.add_button("Close", gtk::ResponseType::Cancel);
    let button = login_button.downgrade();
    dialog.connect_response(move |dialog, _| {
        if let Some(button) = button.upgrade() {
            button.set_sensitive(true);
        }
        dialog.close();
    });
    let button = login_button.downgrade();
    dialog.connect_close_request(move |_| {
        if let Some(button) = button.upgrade() {
            button.set_sensitive(true);
        }
        glib::Propagation::Proceed
    });

    webview.load_uri(&uri);
    dialog.present();
}

fn run_application(
    mode: DisplayMode,
    cancellation: Cancellation,
    preview: bool,
    worker: impl FnOnce(WorkerUi) -> Result<(), String> + Send + 'static,
) -> io::Result<()> {
    gtk::init().map_err(|error| io::Error::other(format!("Unable to initialize GTK: {error}")))?;
    let app = gtk::Application::builder()
        .application_id(if preview {
            "com.github.ndyer.ZwiftSnap.Preview"
        } else {
            "com.github.ndyer.ZwiftSnap"
        })
        .flags(gtk::gio::ApplicationFlags::NON_UNIQUE)
        .build();
    let worker = std::rc::Rc::new(std::cell::RefCell::new(Some(worker)));
    let completion: Completion = Default::default();
    let result = completion.clone();
    app.connect_activate(move |app| {
        let Some(worker) = worker.borrow_mut().take() else {
            for window in app.windows() {
                window.present();
            }
            return;
        };
        let (sender, receiver) = mpsc::channel();
        let window = build_window(
            app,
            &mode,
            receiver,
            cancellation.clone(),
            preview,
            completion.clone(),
        );
        window.present();

        let cancellation = cancellation.clone();
        std::thread::spawn(move || {
            let mut ui = WorkerUi::with_cancellation(sender, cancellation);
            ui.set_require_login(!preview);
            let outcome = worker(ui.clone());
            ui.finished(outcome);
        });
    });
    app.run_with_args(&["zwift-gui"]);
    let outcome = result.borrow_mut().take();
    if preview {
        return Ok(());
    }
    outcome
        .ok_or_else(|| io::Error::other("GTK exited before the launcher completed"))?
        .map_err(io::Error::other)
}

fn build_window(
    app: &gtk::Application,
    mode: &DisplayMode,
    events: Receiver<Event>,
    cancellation: Cancellation,
    preview: bool,
    completion: Completion,
) -> gtk::ApplicationWindow {
    let window = gtk::ApplicationWindow::builder()
        .application(app)
        .title(if preview {
            "Zwift launcher preview"
        } else {
            "Zwift"
        })
        .build();
    match mode {
        DisplayMode::Fullscreen => window.fullscreen(),
        DisplayMode::Windowed { width, height } => {
            let width = i32::try_from(*width).unwrap_or(i32::MAX);
            let height = i32::try_from(*height).unwrap_or(i32::MAX);
            window.set_default_size(width, height);
        }
    }

    let content = gtk::Box::new(gtk::Orientation::Vertical, 12);
    content.set_margin_top(24);
    content.set_margin_bottom(24);
    content.set_margin_start(24);
    content.set_margin_end(24);
    let title = gtk::Label::new(Some("Getting Zwift ready"));
    title.set_wrap(true);
    title.set_xalign(0.0);
    let login_status = gtk::Label::new(Some("Not logged in"));
    login_status.set_xalign(0.0);
    let progress = gtk::ProgressBar::new();
    progress.set_show_text(true);
    let detail = gtk::Label::new(None);
    detail.set_wrap(true);
    detail.set_xalign(0.0);
    let log_toggle = gtk::ToggleButton::with_label("Show log");
    let log = gtk::TextView::new();
    log.set_editable(false);
    log.set_monospace(true);
    log.set_wrap_mode(gtk::WrapMode::WordChar);
    let scroll = gtk::ScrolledWindow::builder()
        .min_content_height(180)
        .vexpand(true)
        .child(&log)
        .visible(false)
        .build();
    let buttons = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    let login = gtk::Button::with_label("Login to Zwift");
    let login_row = gtk::Box::new(gtk::Orientation::Horizontal, 12);
    login_row.append(&login);
    login_row.append(&login_status);
    login_row.set_visible(false);
    let retry = gtk::Button::with_label("Retry");
    let quit = gtk::Button::with_label("Quit");
    buttons.append(&retry);
    buttons.append(&quit);
    buttons.set_visible(false);
    content.append(&title);
    content.append(&progress);
    content.append(&detail);
    content.append(&login_row);
    content.append(&log_toggle);
    content.append(&scroll);
    content.append(&buttons);
    window.set_child(Some(&content));

    let scroll_ref = scroll.clone();
    log_toggle.connect_toggled(move |button| {
        scroll_ref.set_visible(button.is_active());
        button.set_label(if button.is_active() {
            "Hide log"
        } else {
            "Show log"
        });
    });
    let pending = std::rc::Rc::new(std::cell::RefCell::new(None::<Sender<bool>>));
    let pending_login: LoginDecision = Default::default();
    let login_decision = pending_login.clone();
    let login_status_for_dialog = login_status.clone();
    let window_weak_for_login = window.downgrade();
    let login_cancellation = cancellation.clone();
    login.connect_clicked(move |button| {
        if login_decision.borrow().is_some() && !login_cancellation.is_cancelled() {
            show_login_dialog(
                button,
                &login_status_for_dialog,
                &window_weak_for_login,
                login_decision.clone(),
                login_cancellation.clone(),
            );
        }
    });
    for (button, decision) in [(&retry, true), (&quit, false)] {
        let pending = pending.clone();
        let buttons = buttons.clone();
        let detail = detail.clone();
        button.connect_clicked(move |_| {
            if let Some(reply) = pending.borrow_mut().take() {
                if reply.send(decision).is_err() {
                    eprintln!("Unable to send launcher decision: worker stopped");
                }
            }
            buttons.set_visible(false);
            detail.set_text("");
        });
    }
    let pending_on_close = pending.clone();
    let closing = std::rc::Rc::new(std::cell::Cell::new(false));
    let finished = std::rc::Rc::new(std::cell::Cell::new(false));
    let close_requested = closing.clone();
    let close_finished = finished.clone();
    let close_title = title.clone();
    let close_buttons = buttons.clone();
    let close_login = login.clone();
    let close_login_decision = pending_login.clone();
    let close_cancellation = cancellation.clone();
    window.connect_close_request(move |_| {
        if close_finished.get() {
            return glib::Propagation::Proceed;
        }
        close_requested.set(true);
        close_cancellation.cancel();
        close_title.set_text("Stopping Zwift launcher");
        close_buttons.set_visible(false);
        close_login.set_sensitive(false);
        if let Some(reply) = close_login_decision.borrow_mut().take() {
            let _ = reply.send(false);
        }
        if let Some(reply) = pending_on_close.borrow_mut().take() {
            if reply.send(false).is_err() {
                eprintln!("Unable to cancel launcher decision: worker stopped");
            }
        }
        glib::Propagation::Stop
    });

    let window_weak = window.downgrade();
    glib::timeout_add_local(Duration::from_millis(50), move || {
        if window_weak.upgrade().is_none() {
            return glib::ControlFlow::Break;
        }
        for event in events.try_iter().take(100) {
            match event {
                Event::Progress(InstallProgress::Phase { percent, message }) => {
                    title.set_text(&message);
                    progress.set_fraction(f64::from(percent.min(99)) / 100.0);
                }
                Event::Progress(InstallProgress::Sample(percent)) => {
                    progress.set_fraction(f64::from(percent.min(99)) / 100.0);
                }
                Event::Log(text) => {
                    let buffer = log.buffer();
                    buffer.insert(&mut buffer.end_iter(), &text);
                    if buffer.char_count() > 200_000 {
                        let mut start = buffer.start_iter();
                        let mut cutoff = buffer.iter_at_offset(buffer.char_count() - 200_000);
                        buffer.delete(&mut start, &mut cutoff);
                    }
                }
                Event::Failure {
                    kind,
                    detail: message,
                    decision,
                } => {
                    if pending.borrow().is_some() {
                        eprintln!("A launcher decision is already pending");
                        if decision.send(false).is_err() {
                            eprintln!("Unable to reject duplicate launcher decision");
                        }
                        continue;
                    }
                    title.set_text(match kind {
                        Failure::Install => "Zwift installation failed",
                        Failure::Update => "Zwift update failed",
                    });
                    retry.set_label(match kind {
                        Failure::Install => "Retry",
                        Failure::Update => "Launch anyway",
                    });
                    detail.set_text(&message);
                    *pending.borrow_mut() = Some(decision);
                    buttons.set_visible(true);
                }
                Event::LoginRequired { decision } => {
                    if cancellation.is_cancelled() || pending_login.borrow().is_some() {
                        let _ = decision.send(false);
                        continue;
                    }
                    title.set_text("Log in to Zwift to continue");
                    login_status.set_text("Login is required before the game can start");
                    login.set_sensitive(true);
                    login_row.set_visible(true);
                    *pending_login.borrow_mut() = Some(decision);
                }
                Event::GameReady => {
                    login_row.set_visible(false);
                    title.set_text("Zwift started");
                    if !preview {
                        if let Some(window) = window_weak.upgrade() {
                            window.set_visible(false);
                        }
                    }
                }
                Event::Finished(result) => {
                    finished.set(true);
                    let success = result.is_ok();
                    *completion.borrow_mut() = Some(result.clone());
                    if closing.get() || cancellation.is_cancelled() || (!preview && success) {
                        if let Err(error) = &result {
                            eprintln!("{error}");
                        }
                        if let Some(window) = window_weak.upgrade() {
                            window.close();
                        }
                        return glib::ControlFlow::Break;
                    }
                    match result {
                        Ok(()) => title.set_text("Preview complete"),
                        Err(error) => {
                            title.set_text("Launcher stopped");
                            detail.set_text(&error);
                        }
                    }
                    if let Some(window) = window_weak.upgrade() {
                        window.present();
                    }
                    buttons.set_visible(false);
                    return glib::ControlFlow::Break;
                }
            }
        }
        glib::ControlFlow::Continue
    });
    window
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Arc, Mutex};

    struct StubBackend {
        calls: Arc<Mutex<Vec<&'static str>>>,
        installed: bool,
        attempts: u8,
    }

    impl LauncherBackend for StubBackend {
        type Error = &'static str;

        fn prepare(&mut self) -> Result<(), Self::Error> {
            self.calls.lock().unwrap().push("prepare");
            Ok(())
        }

        fn installed(&mut self) -> Result<bool, Self::Error> {
            Ok(self.installed)
        }

        fn install(&mut self) -> Result<(), Self::Error> {
            self.calls.lock().unwrap().push("install");
            self.attempts += 1;
            if self.attempts == 1 {
                Err("install error")
            } else {
                self.installed = true;
                Ok(())
            }
        }

        fn update(&mut self) -> Result<(), Self::Error> {
            panic!("fresh install must not update")
        }

        fn configure_display(&mut self) -> Result<(), Self::Error> {
            self.calls.lock().unwrap().push("display");
            Ok(())
        }

        fn start(&mut self) -> Result<(), Self::Error> {
            self.calls.lock().unwrap().push("start");
            Ok(())
        }

        fn wait_for_exit(&mut self) -> Result<(), Self::Error> {
            self.calls.lock().unwrap().push("wait");
            Ok(())
        }

        fn cleanup(&mut self) -> Result<(), Self::Error> {
            self.calls.lock().unwrap().push("cleanup");
            Ok(())
        }
    }

    #[test]
    fn controller_forwards_retry_or_quit_and_always_cleans_up() {
        for retry in [true, false] {
            let (sender, receiver) = mpsc::channel();
            let calls = Arc::new(Mutex::new(Vec::new()));
            let mut backend = StubBackend {
                calls: calls.clone(),
                installed: false,
                attempts: 0,
            };
            let thread = std::thread::spawn(move || {
                run_controller(&mut backend, &mut WorkerUi::new(sender));
            });
            match receiver.recv().unwrap() {
                Event::Failure {
                    kind,
                    detail,
                    decision,
                } => {
                    assert_eq!(kind, Failure::Install);
                    assert_eq!(detail, "install error");
                    decision.send(retry).unwrap();
                }
                _ => panic!("expected installation failure"),
            }
            if retry {
                match receiver.recv_timeout(Duration::from_secs(2)).unwrap() {
                    Event::LoginRequired { decision } => {
                        assert!(!calls.lock().unwrap().contains(&"start"));
                        assert!(matches!(
                            receiver.try_recv(),
                            Err(mpsc::TryRecvError::Empty)
                        ));
                        decision.send(true).unwrap();
                    }
                    _ => panic!("expected login gate before starting game"),
                }
                assert!(matches!(receiver.recv().unwrap(), Event::GameReady));
            }
            match receiver.recv().unwrap() {
                Event::Finished(result) => assert_eq!(result.is_ok(), retry),
                _ => panic!("expected final result"),
            }
            thread.join().unwrap();
            let calls = calls.lock().unwrap();
            assert_eq!(calls.last(), Some(&"cleanup"));
            assert_eq!(
                calls.iter().filter(|&&call| call == "install").count(),
                if retry { 2 } else { 1 }
            );
            assert_eq!(calls.contains(&"start"), retry);
        }
    }

    #[test]
    fn login_gate_rejects_failed_decision_and_closed_gui() {
        let (events, receiver) = mpsc::channel();
        let thread = std::thread::spawn(move || WorkerUi::new(events).login_required());
        match receiver.recv_timeout(Duration::from_secs(2)).unwrap() {
            Event::LoginRequired { decision } => decision.send(false).unwrap(),
            _ => panic!("expected login request"),
        }
        assert!(!thread.join().unwrap());
        let (events, receiver) = mpsc::channel();
        drop(receiver);
        assert!(!WorkerUi::new(events).login_required());
    }

    #[test]
    fn cancellation_unblocks_login_without_starting_game() {
        let (sender, receiver) = mpsc::channel();
        let cancellation = Cancellation::default();
        let mut ui = WorkerUi::with_cancellation(sender, cancellation.clone());
        let calls = Arc::new(Mutex::new(Vec::new()));
        let mut backend = StubBackend {
            calls: calls.clone(),
            installed: false,
            attempts: 1,
        };
        let worker = std::thread::spawn(move || run_controller(&mut backend, &mut ui));
        let decision = match receiver.recv_timeout(Duration::from_secs(2)).unwrap() {
            Event::LoginRequired { decision } => decision,
            _ => panic!("expected login request"),
        };
        assert!(!calls.lock().unwrap().contains(&"start"));
        cancellation.cancel();
        match receiver.recv_timeout(Duration::from_secs(2)).unwrap() {
            Event::Finished(Err(error)) => assert!(error.contains("cancelled")),
            _ => panic!("expected cancellation result"),
        }
        worker.join().unwrap();
        assert!(decision.send(true).is_err());
        assert_eq!(
            *calls.lock().unwrap(),
            ["prepare", "install", "display", "cleanup"]
        );
    }

    #[test]
    fn preview_does_not_request_real_login() {
        let (sender, receiver) = mpsc::channel();
        let mut ui = WorkerUi::new(sender);
        ui.set_require_login(false);
        assert!(ui.login_required());
        assert!(matches!(
            receiver.try_recv(),
            Err(mpsc::TryRecvError::Empty)
        ));
    }

    #[test]
    fn worker_forwards_progress_and_waits_for_failure_decision() {
        let (events, receiver) = mpsc::channel();
        let thread = std::thread::spawn(move || {
            let mut ui = WorkerUi::new(events);
            ui.progress(InstallProgress::Sample(35));
            ui.failure(Failure::Update, "network error")
        });
        assert!(matches!(
            receiver.recv().unwrap(),
            Event::Progress(InstallProgress::Sample(35))
        ));
        match receiver.recv().unwrap() {
            Event::Failure {
                kind,
                detail,
                decision,
            } => {
                assert_eq!(kind, Failure::Update);
                assert_eq!(detail, "network error");
                decision.send(true).unwrap();
            }
            _ => panic!("expected failure"),
        }
        assert!(thread.join().unwrap());
    }

    #[test]
    fn closed_gui_rejects_failure_decision() {
        let (sender, receiver) = mpsc::channel();
        drop(receiver);
        assert!(!WorkerUi::new(sender).failure(Failure::Install, "failure"));
    }

    #[test]
    fn cancellation_unblocks_pending_decision_and_runs_cleanup() {
        let (sender, receiver) = mpsc::channel();
        let cancellation = Cancellation::default();
        let mut ui = WorkerUi::with_cancellation(sender, cancellation.clone());
        let calls = Arc::new(Mutex::new(Vec::new()));
        let mut backend = StubBackend {
            calls: calls.clone(),
            installed: false,
            attempts: 0,
        };
        let worker = std::thread::spawn(move || run_controller(&mut backend, &mut ui));
        let decision = match receiver.recv_timeout(Duration::from_secs(2)).unwrap() {
            Event::Failure { decision, .. } => decision,
            _ => panic!("expected installation failure"),
        };
        cancellation.cancel();
        let event = receiver.recv_timeout(Duration::from_secs(2)).unwrap();
        match event {
            Event::Finished(Err(error)) => assert!(error.contains("cancelled")),
            _ => panic!("expected cancellation result"),
        }
        worker.join().unwrap();
        assert!(decision.send(true).is_err());
        assert_eq!(*calls.lock().unwrap(), ["prepare", "install", "cleanup"]);
    }

    #[test]
    fn cancellation_during_install_runs_cleanup_without_retry() {
        struct BlockingBackend {
            cancellation: Cancellation,
            started: Sender<()>,
            cleaned: Arc<Mutex<bool>>,
        }
        impl LauncherBackend for BlockingBackend {
            type Error = io::Error;
            fn prepare(&mut self) -> io::Result<()> {
                Ok(())
            }
            fn installed(&mut self) -> io::Result<bool> {
                Ok(false)
            }
            fn install(&mut self) -> io::Result<()> {
                self.started.send(()).unwrap();
                self.cancellation.wait(Duration::from_secs(30))
            }
            fn update(&mut self) -> io::Result<()> {
                panic!("unexpected update")
            }
            fn configure_display(&mut self) -> io::Result<()> {
                panic!("unexpected display")
            }
            fn start(&mut self) -> io::Result<()> {
                panic!("unexpected start")
            }
            fn wait_for_exit(&mut self) -> io::Result<()> {
                panic!("unexpected wait")
            }
            fn cleanup(&mut self) -> io::Result<()> {
                *self.cleaned.lock().unwrap() = true;
                Ok(())
            }
        }
        let (events, receiver) = mpsc::channel();
        let (started, ready) = mpsc::channel();
        let cancellation = Cancellation::default();
        let cleaned = Arc::new(Mutex::new(false));
        let mut backend = BlockingBackend {
            cancellation: cancellation.clone(),
            started,
            cleaned: cleaned.clone(),
        };
        let mut ui = WorkerUi::with_cancellation(events, cancellation.clone());
        let worker = std::thread::spawn(move || run_controller(&mut backend, &mut ui));
        ready.recv_timeout(Duration::from_secs(2)).unwrap();
        cancellation.cancel();
        match receiver.recv_timeout(Duration::from_secs(2)).unwrap() {
            Event::Finished(Err(error)) => assert!(error.contains("cancelled")),
            _ => panic!("expected cancellation result"),
        }
        worker.join().unwrap();
        assert!(*cleaned.lock().unwrap());
    }
}
