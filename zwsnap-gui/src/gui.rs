//! GTK frontend and channel bridge for the launcher worker.

use crate::accounts::Store;
use crate::cancel::Cancellation;
use crate::display::DisplayMode;
use crate::install::{InstallProgress, InstallTask, TaskState};
use crate::logging::Logger;
use crate::oauth;
use crate::orchestration::{self, Failure, LauncherBackend, LauncherUi};
use crate::profile;
use gtk::glib;
use gtk::prelude::*;
use std::io;
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::Arc;
use std::time::Duration;

mod accounts_panel;
mod login_panel;
mod settings;

use accounts_panel::{AccountsPanel, Handlers as AccountHandlers};
use login_panel::LoginPanel;
use settings::SettingsDialog;

pub use settings::SettingsContext;

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
        Ok(())
    }

    fn installed(&mut self) -> Result<bool, Self::Error> {
        Ok(self.installed)
    }

    fn install(&mut self) -> Result<(), Self::Error> {
        if !self.failed_once {
            self.ui.cancellation.wait(Duration::from_millis(500))?;
            self.ui.progress(InstallProgress::Task {
                task: InstallTask::Game,
                state: TaskState::Fraction(42),
            });
            self.ui.log("Example installer output\n".into());
            self.failed_once = true;
            Err(io::Error::other("Example installation error"))
        } else {
            self.installed = true;
            self.ui.progress(InstallProgress::Task {
                task: InstallTask::Game,
                state: TaskState::Done,
            });
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
        self.ui.progress(InstallProgress::Launch {
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
    let settings = SettingsContext::preview();
    run_application(
        mode,
        Cancellation::default(),
        true,
        None,
        settings,
        |mut ui| {
            let cancellation = ui.cancellation();
            let mut backend = PreviewBackend {
                ui: ui.clone(),
                installed: false,
                failed_once: false,
            };
            orchestration::run_cancellable(&mut backend, &mut ui, &cancellation)
                .map_err(|error| error.to_string())
        },
    )
}

/// Run the real backend on a worker while GTK owns the main thread.
pub fn launch(
    mode: DisplayMode,
    cancellation: Cancellation,
    logger: Arc<Logger>,
    settings: SettingsContext,
    worker: impl FnOnce(WorkerUi) -> Result<(), String> + Send + 'static,
) -> io::Result<()> {
    run_application(mode, cancellation, false, Some(logger), settings, worker)
}

type Completion = std::rc::Rc<std::cell::RefCell<Option<Result<(), String>>>>;

/// Which WebKit session a panel runs on.
///
/// The launcher's own gate uses [`SessionMode::Persistent`], so Zwift's "Remember
/// me" cookie survives a restart. Logging in a *new* account from the picker uses
/// [`SessionMode::Ephemeral`] instead: an ephemeral session shares no cookie
/// storage, so the current account's Keycloak SSO cookie cannot silently sign the
/// new login back into the account already in the store.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SessionMode {
    /// Cookies persist in `$SNAP_USER_COMMON/webkit` across launcher runs.
    Persistent,
    /// No persistent cookie or cache storage; nothing survives the view.
    Ephemeral,
}

/// Build a WebView for `session`, falling back to a default view when the
/// persistent store cannot be prepared.
pub(super) fn webview_for(session: SessionMode) -> webkit6::WebView {
    match session {
        SessionMode::Persistent => persistent_webview(),
        SessionMode::Ephemeral => Some(
            webkit6::WebView::builder()
                .network_session(&webkit6::NetworkSession::new_ephemeral())
                .build(),
        ),
    }
    .unwrap_or_default()
}

/// A WebView whose cookies persist in `$SNAP_USER_COMMON/webkit`, so Zwift's
/// "Remember me" cookie survives a launcher restart. The directory and cookie
/// database hold live session cookies, so they are created owner-only (0700/0600),
/// before WebKit opens them. Returns `None` if the store cannot be prepared; the
/// caller then falls back to a non-persistent view.
fn persistent_webview() -> Option<webkit6::WebView> {
    use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt};

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

fn run_application(
    mode: DisplayMode,
    cancellation: Cancellation,
    preview: bool,
    logger: Option<Arc<Logger>>,
    settings: SettingsContext,
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
            UiContext {
                cancellation: cancellation.clone(),
                preview,
                completion: completion.clone(),
                logger: logger.clone(),
                settings: settings.clone(),
            },
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
    app.run_with_args(&["zwsnap-gui"]);
    let outcome = result.borrow_mut().take();
    if preview {
        return Ok(());
    }
    outcome
        .ok_or_else(|| io::Error::other("GTK exited before the launcher completed"))?
        .map_err(io::Error::other)
}

/// Runtime collaborators handed to the launcher window: everything it needs
/// beyond the application object and the display mode. Bundled into a struct to
/// keep [`build_window`]'s signature manageable.
struct UiContext {
    cancellation: Cancellation,
    preview: bool,
    completion: Completion,
    logger: Option<Arc<Logger>>,
    settings: SettingsContext,
}

fn build_window(
    app: &gtk::Application,
    mode: &DisplayMode,
    events: Receiver<Event>,
    context: UiContext,
) -> gtk::ApplicationWindow {
    let UiContext {
        cancellation,
        preview,
        completion,
        logger,
        settings,
    } = context;
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

    let login_panel = std::rc::Rc::new(LoginPanel::new(
        cancellation.clone(),
        preview,
        logger.clone(),
    ));

    // The Settings dialog is built eagerly, while hidden, so its log view exists
    // before the Logger sinks register on the worker thread.
    let dialog = SettingsDialog::new(&window, settings.clone());

    // --- Account picker ---------------------------------------------------
    // The picker sits above the launcher's own login panel: once at least one
    // account is remembered the grid appears at the top (where the install
    // tiles were) and the WebView keeps showing its embed page below. With no
    // account remembered the WebView shows the login page and the grid stays
    // hidden.
    let common = settings
        .path
        .parent()
        .map(std::path::Path::to_path_buf)
        .unwrap_or_else(|| std::path::PathBuf::from("."));
    let store = std::rc::Rc::new(std::cell::RefCell::new(Store::open(&common)));
    let grid_decision: std::rc::Rc<std::cell::RefCell<Option<Sender<bool>>>> = Default::default();
    let add_result: std::rc::Rc<std::cell::RefCell<Option<Receiver<bool>>>> = Default::default();

    let accounts_panel = {
        let select = {
            let store = store.clone();
            let grid_decision = grid_decision.clone();
            Box::new(move |id: String| {
                {
                    let mut store = store.borrow_mut();
                    match store.select(&id) {
                        Ok(()) => {
                            if let Err(error) = store.save() {
                                eprintln!("Unable to save accounts: {error}");
                            }
                        }
                        Err(error) => eprintln!("Unable to select account: {error}"),
                    }
                }
                if let Some(reply) = grid_decision.borrow_mut().take() {
                    if reply.send(true).is_err() {
                        eprintln!("Unable to confirm account selection: worker stopped");
                    }
                }
            }) as Box<dyn Fn(String)>
        };
        let add = {
            let login_panel = login_panel.clone();
            let add_result = add_result.clone();
            Box::new(move || {
                // A fresh, cookie-less session: the current account's SSO cookie
                // must not silently sign the new login back into itself.
                login_panel.set_session(SessionMode::Ephemeral);
                let (sender, receiver) = mpsc::channel();
                *add_result.borrow_mut() = Some(receiver);
                login_panel.start_login(sender);
            }) as Box<dyn Fn()>
        };
        AccountsPanel::new(
            store.clone(),
            logger.clone(),
            AccountHandlers { select, add },
        )
    };

    let content = gtk::Box::new(gtk::Orientation::Vertical, 12);
    content.set_margin_top(24);
    content.set_margin_bottom(24);
    content.set_margin_start(24);
    content.set_margin_end(24);
    let title = gtk::Label::new(Some("Getting Zwift ready"));
    title.set_wrap(true);
    title.set_xalign(0.0);

    // One tile per install task, laid out in a row so the parallel tracks are
    // visible side by side. Every tile is built up front, hidden, and revealed when
    // its task first reports progress; the row disappears once they are all done.
    let install_tiles: [(InstallTask, gtk::Box, gtk::ProgressBar); 4] = [
        InstallTask::WineSetup,
        InstallTask::D3dCompiler,
        InstallTask::Vulkan,
        InstallTask::Game,
    ]
    .map(|task| {
        let tile = gtk::Box::new(gtk::Orientation::Vertical, 6);
        tile.set_hexpand(true);
        let label = gtk::Label::new(Some(task.label()));
        label.set_wrap(true);
        label.set_xalign(0.0);
        let bar = gtk::ProgressBar::new();
        bar.set_show_text(true);
        tile.append(&label);
        tile.append(&bar);
        tile.set_visible(false);
        (task, tile, bar)
    });
    let install_area = gtk::Box::new(gtk::Orientation::Horizontal, 12);
    install_area.set_homogeneous(true);
    for (_, tile, _) in &install_tiles {
        install_area.append(tile);
    }

    // The launch spinner replaces the install tiles once the game is starting.
    let launch_spinner = gtk::Spinner::new();
    let launch_status = gtk::Label::new(None);
    launch_status.set_wrap(true);
    launch_status.set_xalign(0.0);
    launch_status.set_hexpand(true);
    let launch_area = gtk::Box::new(gtk::Orientation::Horizontal, 12);
    launch_area.append(&launch_spinner);
    launch_area.append(&launch_status);
    launch_area.set_visible(false);

    let detail = gtk::Label::new(None);
    detail.set_wrap(true);
    detail.set_xalign(0.0);
    let buttons = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    let retry = gtk::Button::with_label("Retry");
    let quit = gtk::Button::with_label("Quit");
    buttons.append(&retry);
    buttons.append(&quit);
    buttons.set_visible(false);
    let actions = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    let open_settings = gtk::Button::with_label("Settings");
    open_settings.set_halign(gtk::Align::End);
    open_settings.set_hexpand(true);
    actions.append(&open_settings);
    content.append(&title);
    content.append(&install_area);
    // The picker takes the top spot the install tiles occupied; both sit above
    // the WebView, which keeps showing its embed page at the bottom.
    content.append(accounts_panel.widget());
    content.append(&launch_area);
    content.append(&detail);
    content.append(&buttons);
    content.append(&actions);
    content.append(login_panel.widget());
    accounts_panel.widget().set_visible(false);

    window.set_child(Some(&content));

    let dialog_for_button = dialog.clone();
    open_settings.connect_clicked(move |_| dialog_for_button.present());
    let pending = std::rc::Rc::new(std::cell::RefCell::new(None::<Sender<bool>>));
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
    let close_panel = login_panel.clone();
    let close_grid_decision = grid_decision.clone();
    let close_add_result = add_result.clone();
    let close_cancellation = cancellation.clone();
    window.connect_close_request(move |_| {
        if close_finished.get() {
            return glib::Propagation::Proceed;
        }
        close_requested.set(true);
        close_cancellation.cancel();
        close_title.set_text("Stopping Zwift launcher");
        close_buttons.set_visible(false);
        close_panel.cancel_login();
        if let Some(reply) = close_grid_decision.borrow_mut().take() {
            let _ = reply.send(false);
        }
        close_add_result.borrow_mut().take();
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
        // A login started from the picker's "add another account" button answers
        // through its own channel; finalise it before touching the event queue.
        let add_outcome = add_result.borrow().as_ref().map(Receiver::try_recv);
        match add_outcome {
            Some(Ok(true)) => {
                add_result.borrow_mut().take();
                finish_add_account(&store, &accounts_panel, &grid_decision, logger.as_ref());
                // The login page was only shown to add the account: put the embed
                // page back now that the login has succeeded.
                login_panel.show_embed();
            }
            Some(Ok(false)) => {
                add_result.borrow_mut().take();
                // The added login was cancelled or failed: back to the embed page
                // with the picker still showing.
                login_panel.show_embed();
                accounts_panel.widget().set_visible(true);
            }
            Some(Err(mpsc::TryRecvError::Disconnected)) => {
                add_result.borrow_mut().take();
            }
            _ => {}
        }
        for event in events.try_iter().take(100) {
            match event {
                Event::Progress(InstallProgress::Task { task, state }) => {
                    // The heading names the step in hand, including the update
                    // check, which has no tile of its own.
                    if title.text().as_str() != task.label() {
                        title.set_text(task.label());
                    }
                    for (candidate, tile, bar) in &install_tiles {
                        if *candidate != task {
                            continue;
                        }
                        tile.set_visible(true);
                        match state {
                            TaskState::Running => {
                                bar.set_fraction(0.0);
                                bar.pulse();
                            }
                            TaskState::Fraction(percent) => {
                                bar.set_fraction(f64::from(percent.min(99)) / 100.0);
                            }
                            TaskState::Done => {
                                bar.set_fraction(1.0);
                                // The install is over for good once every tile that
                                // was shown has finished, so the whole area goes.
                                let all_done = install_tiles.iter().all(|(_, tile, bar)| {
                                    !tile.is_visible() || bar.fraction() >= 1.0
                                });
                                if all_done {
                                    install_area.set_visible(false);
                                }
                            }
                        }
                    }
                }
                Event::Progress(InstallProgress::Launch { message }) => {
                    install_area.set_visible(false);
                    launch_area.set_visible(true);
                    launch_spinner.start();
                    launch_status.set_text(&message);
                    title.set_text(&message);
                }
                Event::Log(text) => {
                    dialog.append_log(&text);
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
                    if cancellation.is_cancelled() || login_panel.has_pending() {
                        let _ = decision.send(false);
                        continue;
                    }
                    // The install tiles have served their purpose; the picker or
                    // the WebView login takes over the window from here.
                    install_area.set_visible(false);
                    let remembered = {
                        {
                            let mut guard = store.borrow_mut();
                            guard.load();
                            // An install from before the picker existed has a
                            // launch token but no account list: adopt that rider
                            // so an upgrade does not ask them to log in again.
                            if guard.is_empty() {
                                if let Ok(token) = guard.launch_token() {
                                    let claims = oauth::token_claims(&token);
                                    match guard.import_legacy(&claims) {
                                        Ok(Some(_)) => {
                                            if let Err(error) = guard.save() {
                                                eprintln!("Unable to save accounts: {error}");
                                            }
                                        }
                                        Ok(None) => {}
                                        Err(error) => {
                                            eprintln!(
                                                "Unable to adopt the existing login: {error}"
                                            );
                                        }
                                    }
                                }
                            }
                        }
                        accounts_panel.has_accounts()
                    };
                    if !remembered {
                        // First run, or every account was forgotten: the WebView
                        // loads the launcher's login page. This and "add another
                        // account" are the only times the embed page is replaced;
                        // it is put back once the login succeeds. Clear the
                        // heading first, so the last install message ("Downloading
                        // Zwift") does not linger over the login page, which
                        // carries its own prompt.
                        title.set_text("");
                        login_panel.start_login(decision);
                        continue;
                    }
                    refresh_accounts(&store, &accounts_panel);
                    accounts_panel.refresh();
                    *grid_decision.borrow_mut() = Some(decision);
                    title.set_text("Choose a rider");
                    detail.set_text("Pick an account to launch Zwift, or add another.");
                    // The grid appears at the top; the WebView keeps its embed page.
                    accounts_panel.widget().set_visible(true);
                }
                Event::GameReady => {
                    title.set_text("Zwift started");
                    launch_spinner.stop();
                    launch_area.set_visible(false);
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

/// Refresh the remembered accounts' cards from the Zwift API in the background.
///
/// The picker opens with the stats cached when each account was added and then
/// quietly updates itself. Every step is best-effort: the launcher is routinely
/// used offline, so a failed fetch keeps the cached values and the account is
/// left untouched.
fn refresh_accounts(
    store: &std::rc::Rc<std::cell::RefCell<Store>>,
    accounts_panel: &std::rc::Rc<AccountsPanel>,
) {
    let jobs: Vec<(String, String)> = {
        let store = store.borrow();
        store
            .accounts
            .iter()
            .filter_map(|account| {
                let token = store.account_token(&account.id).ok()?;
                let access = token.get("access_token")?.as_str()?.to_owned();
                Some((account.id.clone(), access))
            })
            .collect()
    };
    if jobs.is_empty() {
        return;
    }
    let (sender, receiver) = mpsc::channel();
    std::thread::spawn(move || {
        let refreshed = jobs
            .into_iter()
            .filter_map(|(id, access)| profile::fetch(&access).ok().map(|profile| (id, profile)))
            .collect::<Vec<_>>();
        let _ = sender.send(refreshed);
    });

    let store = store.clone();
    let accounts_panel = accounts_panel.clone();
    glib::timeout_add_local(Duration::from_millis(100), move || {
        match receiver.try_recv() {
            Ok(refreshed) => {
                if !refreshed.is_empty() {
                    {
                        let mut store = store.borrow_mut();
                        for (id, profile) in refreshed {
                            store.refresh(&id, &profile);
                        }
                        if let Err(error) = store.save() {
                            eprintln!("Unable to save accounts: {error}");
                        }
                    }
                    accounts_panel.refresh();
                }
                glib::ControlFlow::Break
            }
            Err(mpsc::TryRecvError::Empty) => glib::ControlFlow::Continue,
            Err(mpsc::TryRecvError::Disconnected) => glib::ControlFlow::Break,
        }
    });
}

/// Record the account that just logged in through the picker's "add another
/// account" flow, then launch with it.
///
/// The login writes its token to the launch token file (`pairing-token.json`),
/// exactly as the launcher's own gate does, so this reads it back, derives the
/// identity from its claims, fetches the profile best-effort, stores the account
/// (token, card fields, avatar) and answers the worker's pending decision so the
/// game starts as the new rider.
fn finish_add_account(
    store: &std::rc::Rc<std::cell::RefCell<Store>>,
    accounts_panel: &std::rc::Rc<AccountsPanel>,
    grid_decision: &std::rc::Rc<std::cell::RefCell<Option<Sender<bool>>>>,
    logger: Option<&Arc<Logger>>,
) {
    let token = match store.borrow().launch_token() {
        Ok(token) => token,
        Err(error) => {
            eprintln!("Unable to read the new account's token: {error}");
            return;
        }
    };
    let claims = oauth::token_claims(&token);
    // The profile is best-effort: if the API cannot be reached the account is
    // still remembered (with whatever the token's claims name it).
    let profile = token
        .get("access_token")
        .and_then(serde_json::Value::as_str)
        .and_then(|access| profile::fetch(access).ok())
        .unwrap_or_default();
    let id = match store.borrow_mut().upsert(&token, &profile, &claims) {
        Ok(id) => id,
        Err(error) => {
            eprintln!("Unable to remember the new account: {error}");
            return;
        }
    };
    if let Some(url) = profile.large_avatar_url.as_deref() {
        match profile::fetch_image(url) {
            Ok(bytes) => {
                if let Err(error) =
                    store
                        .borrow_mut()
                        .write_avatar(&id, &bytes, image_extension(url))
                {
                    if let Some(logger) = logger {
                        logger.warning(&format!("Unable to save the account avatar: {error}"));
                    }
                }
            }
            Err(error) => {
                if let Some(logger) = logger {
                    logger.debug(&format!("Unable to fetch the account avatar: {error}"));
                }
            }
        }
    }
    {
        let mut store = store.borrow_mut();
        if let Err(error) = store.select(&id) {
            eprintln!("Unable to select the new account: {error}");
        }
        if let Err(error) = store.save() {
            eprintln!("Unable to save accounts: {error}");
        }
    }
    accounts_panel.refresh();
    if let Some(reply) = grid_decision.borrow_mut().take() {
        if reply.send(true).is_err() {
            eprintln!("Unable to confirm account selection: worker stopped");
        }
    }
}

/// The file extension implied by an image URL's path, defaulting to `jpg`.
fn image_extension(url: &str) -> &str {
    let without_query = url.split(['?', '#']).next().unwrap_or(url);
    let name = without_query.rsplit('/').next().unwrap_or("");
    match name.rsplit_once('.') {
        Some((_, ext)) if ext.len() <= 4 && ext.chars().all(|c| c.is_ascii_alphanumeric()) => ext,
        _ => "jpg",
    }
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
            ui.progress(InstallProgress::Task {
                task: InstallTask::Game,
                state: TaskState::Fraction(35),
            });
            ui.failure(Failure::Update, "network error")
        });
        assert!(matches!(
            receiver.recv().unwrap(),
            Event::Progress(InstallProgress::Task {
                task: InstallTask::Game,
                state: TaskState::Fraction(35)
            })
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
