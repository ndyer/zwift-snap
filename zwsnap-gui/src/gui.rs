//! GTK frontend and channel bridge for the launcher worker.

use crate::accounts::Store;
use crate::cancel::Cancellation;
use crate::display::DisplayMode;
use crate::install::{InstallProgress, InstallTask, TaskState};
use crate::logging::Logger;
use crate::oauth;
use crate::orchestration::{self, Failure, LauncherBackend, LauncherUi};
use crate::profile;
use crate::stats;
use gtk::glib;
use gtk::prelude::*;
use std::cell::Cell;
use std::io;
use std::rc::Rc;
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::Arc;
use std::time::Duration;

mod accounts_panel;
mod icons;
mod log_panel;
mod login_panel;
mod settings;

use accounts_panel::{AccountsPanel, Handlers as AccountHandlers};
use log_panel::LogPanel;
use login_panel::LoginPanel;
use settings::SettingsDialog;

pub use settings::SettingsContext;

#[derive(Clone, Copy, Default)]
struct TileCompletion {
    generation: u64,
    completed: bool,
}

impl TileCompletion {
    fn running(&mut self) {
        if self.completed {
            self.generation = self.generation.wrapping_add(1);
            self.completed = false;
        }
    }

    fn complete(&mut self) -> Option<u64> {
        if self.completed {
            return None;
        }
        self.completed = true;
        Some(self.generation)
    }

    fn matches(&self, generation: u64) -> bool {
        self.completed && self.generation == generation
    }
}

fn update_install_tile(
    tile: &gtk::Box,
    bar: &gtk::ProgressBar,
    completion: &Rc<Cell<TileCompletion>>,
    state: TaskState,
) {
    match state {
        TaskState::Running | TaskState::Fraction { .. } => {
            let mut current = completion.get();
            current.running();
            completion.set(current);
            tile.set_visible(true);
            match state {
                TaskState::Running => {
                    bar.set_fraction(0.0);
                    bar.set_text(None);
                    bar.pulse();
                }
                TaskState::Fraction { percent, rate } => {
                    bar.set_fraction(f64::from(percent.min(99)) / 100.0);
                    let label = stats::transfer_progress(percent, rate);
                    bar.set_text(Some(label.as_str()));
                }
                TaskState::Done => unreachable!(),
            }
        }
        TaskState::Done => {
            let mut current = completion.get();
            if let Some(generation) = current.complete() {
                completion.set(current);
                tile.set_visible(true);
                bar.set_text(None);
                bar.set_fraction(1.0);
                let tile = tile.downgrade();
                let completion = Rc::clone(completion);
                glib::timeout_add_local_once(Duration::from_secs(2), move || {
                    if completion.get().matches(generation) {
                        if let Some(tile) = tile.upgrade() {
                            tile.set_visible(false);
                        }
                    }
                });
            }
        }
    }
}

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
                state: TaskState::Fraction {
                    percent: 42,
                    rate: Some(8_400_000.0),
                },
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

    // The Settings dialog is built while hidden; `present` shows it on demand.
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
    // The heading is a single line: a wrapping label that does not expand is
    // allocated only its minimum width (the widest word) once it sits in a box
    // beside the spinner, so it wrapped even with space to spare. Long messages
    // go to the log instead, so the heading never needs to wrap.
    let title = gtk::Label::new(Some("Getting Zwift ready"));
    title.set_wrap(false);
    title.set_xalign(0.0);

    // One tile per install task, laid out in a row so the parallel tracks are
    // visible side by side. Tiles are revealed as their tasks report progress.
    let install_tiles = [
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
        (
            task,
            tile,
            bar,
            Rc::new(Cell::new(TileCompletion::default())),
        )
    });
    let install_area = gtk::Box::new(gtk::Orientation::Horizontal, 12);
    install_area.set_homogeneous(true);
    for (_, tile, _, _) in &install_tiles {
        install_area.append(tile);
    }

    // The launch spinner trails the status heading, so the window never grows a
    // second line of text: a long failure or error message goes to the log
    // (reachable through the header's "Show log" toggle), while the heading stays
    // short. The heading and spinner share a box that takes the free space, so
    // the spinner sits immediately after the text rather than drifting over to
    // the right-hand button group.
    let launch_spinner = gtk::Spinner::new();
    launch_spinner.set_valign(gtk::Align::Center);
    let status = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    status.set_hexpand(true);
    status.append(&title);
    status.append(&launch_spinner);
    // Retry a failed install (or launch anyway after a failed update). It joins
    // the header's button group and is revealed only when a failure needs an
    // answer; it does not deserve a row of its own. It is icon-only, so its
    // hover text carries the label.
    let retry = icons::icon_button("view-refresh-symbolic", "Retry");
    retry.set_visible(false);

    // The header row: the launch spinner and status heading on the left, and the
    // launcher's own controls pinned to the top-right corner — the picker's
    // add-account button, the log toggle, Settings, and, in a fullscreen session,
    // Quit.
    let log_panel = LogPanel::new();
    let open_settings = icons::icon_button("preferences-other-symbolic", "Settings");
    let header = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    header.append(&status);
    header.append(&retry);
    header.append(accounts_panel.add_button());
    header.append(log_panel.toggle_button());
    header.append(&open_settings);
    if matches!(mode, DisplayMode::Fullscreen) {
        // A fullscreen window has no decorations to close from, so offer an
        // explicit Quit that takes the same cancellation-and-cleanup path as
        // closing the window.
        let quit_launcher = icons::icon_button("application-exit-symbolic", "Quit");
        quit_launcher.connect_clicked({
            let window = window.clone();
            move |_| window.close()
        });
        header.append(&quit_launcher);
    }

    // The bottom pane is split: the WebKit login panel on the left, the log view
    // on the right, hidden until its toggle is activated. The log pane is the
    // end child, so hiding it leaves the WebView the full width.
    let body = gtk::Paned::new(gtk::Orientation::Horizontal);
    body.set_position(560);
    body.set_wide_handle(true);
    body.set_start_child(Some(login_panel.widget()));
    body.set_end_child(Some(log_panel.widget()));
    body.set_resize_start_child(true);
    body.set_shrink_start_child(false);
    body.set_resize_end_child(false);
    body.set_shrink_end_child(false);
    body.set_vexpand(true);

    content.append(&header);
    content.append(&install_area);
    // The picker takes the top spot the install tiles occupied; both sit above
    // the split, whose WebView keeps showing its embed page at the bottom.
    content.append(accounts_panel.widget());
    content.append(&body);
    accounts_panel.set_visible(false);

    window.set_child(Some(&content));

    let dialog_for_button = dialog.clone();
    open_settings.connect_clicked(move |_| dialog_for_button.present());
    let pending = std::rc::Rc::new(std::cell::RefCell::new(None::<Sender<bool>>));
    // Only Retry offers a decision here; declining the failure (the `false`
    // reply) is done by closing the window, which has no decorations in
    // fullscreen but offers the header's Quit instead.
    retry.connect_clicked({
        let pending = pending.clone();
        let retry = retry.clone();
        move |_| {
            if let Some(reply) = pending.borrow_mut().take() {
                if reply.send(true).is_err() {
                    eprintln!("Unable to send launcher decision: worker stopped");
                }
            }
            retry.set_visible(false);
        }
    });
    let pending_on_close = pending.clone();
    let closing = std::rc::Rc::new(std::cell::Cell::new(false));
    let finished = std::rc::Rc::new(std::cell::Cell::new(false));
    let close_requested = closing.clone();
    let close_finished = finished.clone();
    let close_title = title.clone();
    let close_retry = retry.clone();
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
        close_retry.set_visible(false);
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
                accounts_panel.set_visible(true);
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
                    for (candidate, tile, bar, completion) in &install_tiles {
                        if *candidate != task {
                            continue;
                        }
                        update_install_tile(tile, bar, completion, state);
                    }
                }
                Event::Progress(InstallProgress::Launch { message }) => {
                    install_area.set_visible(false);
                    launch_spinner.start();
                    title.set_text(&message);
                }
                Event::Log(text) => {
                    log_panel.append(&text);
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
                    let headline = match kind {
                        Failure::Install => "Zwift installation failed",
                        Failure::Update => "Zwift update failed",
                    };
                    title.set_text(headline);
                    // The heading stays short; the full reason goes to the log,
                    // alongside the rest of the installer output.
                    log_panel.append(&format!("{headline}: {message}\n"));
                    // The icon stays the same; the hover text says what the
                    // button will do.
                    retry.set_tooltip_text(Some(match kind {
                        Failure::Install => "Retry",
                        Failure::Update => "Launch anyway",
                    }));
                    *pending.borrow_mut() = Some(decision);
                    retry.set_visible(true);
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
                        accounts_panel.set_visible(false);
                        login_panel.start_login(decision);
                        continue;
                    }
                    refresh_accounts(&store, &accounts_panel);
                    accounts_panel.refresh();
                    *grid_decision.borrow_mut() = Some(decision);
                    title.set_text("Choose a rider");
                    // The grid appears at the top; the WebView keeps its embed page.
                    accounts_panel.set_visible(true);
                }
                Event::GameReady => {
                    title.set_text("Zwift started");
                    launch_spinner.stop();
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
                            // The heading stays short; the full error goes to the log.
                            log_panel.append(&format!("{error}\n"));
                        }
                    }
                    if let Some(window) = window_weak.upgrade() {
                        window.present();
                    }
                    retry.set_visible(false);
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

    #[test]
    fn tile_completion_ignores_duplicates_and_invalidates_old_timeouts() {
        let mut tile = TileCompletion::default();
        let first = tile.complete().unwrap();
        assert!(tile.matches(first));
        assert_eq!(tile.complete(), None);

        tile.running();
        assert!(!tile.matches(first));
        let second = tile.complete().unwrap();
        assert_ne!(first, second);
        assert!(!tile.matches(first));
        assert!(tile.matches(second));
    }

    #[test]
    #[ignore = "requires the GTK preview compositor"]
    fn completed_tiles_hide_independently_on_the_gtk_main_loop() {
        gtk::init().unwrap();
        let window = gtk::Window::new();
        let row = gtk::Box::new(gtk::Orientation::Horizontal, 12);
        window.set_child(Some(&row));
        let tiles = [0, 1, 2].map(|_| {
            let tile = gtk::Box::new(gtk::Orientation::Vertical, 6);
            let bar = gtk::ProgressBar::new();
            tile.append(&bar);
            tile.set_visible(false);
            row.append(&tile);
            (tile, bar, Rc::new(Cell::new(TileCompletion::default())))
        });
        window.present();
        let wait = |duration| {
            let main_loop = glib::MainLoop::new(None, false);
            let stop = main_loop.clone();
            glib::timeout_add_local_once(duration, move || stop.quit());
            main_loop.run();
        };
        let (first_tile, first_bar, first_completion) = &tiles[0];
        let (second_tile, second_bar, second_completion) = &tiles[1];
        let (running_tile, running_bar, running_completion) = &tiles[2];
        update_install_tile(first_tile, first_bar, first_completion, TaskState::Done);
        update_install_tile(
            running_tile,
            running_bar,
            running_completion,
            TaskState::Running,
        );
        wait(Duration::from_secs(1));
        assert!(first_tile.is_visible());
        assert_eq!(first_bar.fraction(), 1.0);
        update_install_tile(first_tile, first_bar, first_completion, TaskState::Done);
        update_install_tile(second_tile, second_bar, second_completion, TaskState::Done);
        wait(Duration::from_millis(1200));
        assert!(!first_tile.is_visible());
        assert!(second_tile.is_visible());
        assert!(running_tile.is_visible());
        update_install_tile(first_tile, first_bar, first_completion, TaskState::Done);
        assert!(!first_tile.is_visible());
        update_install_tile(
            second_tile,
            second_bar,
            second_completion,
            TaskState::Running,
        );
        update_install_tile(second_tile, second_bar, second_completion, TaskState::Done);
        wait(Duration::from_millis(1100));
        assert!(second_tile.is_visible());
        wait(Duration::from_millis(1100));
        assert!(!second_tile.is_visible());
        assert!(running_tile.is_visible());
        window.close();
    }

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
                state: TaskState::Fraction {
                    percent: 35,
                    rate: Some(4_200_000.0),
                },
            });
            ui.failure(Failure::Update, "network error")
        });
        assert!(matches!(
            receiver.recv().unwrap(),
            Event::Progress(InstallProgress::Task {
                task: InstallTask::Game,
                state: TaskState::Fraction {
                    percent: 35,
                    rate: Some(_)
                }
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
