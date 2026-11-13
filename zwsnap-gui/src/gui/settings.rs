//! The Settings dialog: the launcher's user-editable settings, the launcher log,
//! and the maintenance actions that reset the Wine prefix or the game data.
//!
//! Everything here is optional to the launch itself: the settings file is read
//! before GTK starts (see `crate::settings`), so changes made in this dialog take
//! effect on the *next* launch, which the dialog says so explicitly. The status
//! section is read-only and reports what the launcher can observe right now.

use crate::display::DisplayMode;
use crate::graphics::Setting as GraphicsSetting;
use crate::settings::{self as store, GpuProbe, Settings};
use gtk::glib;
use gtk::prelude::*;
use std::path::PathBuf;
use std::rc::Rc;
use std::time::Duration;

/// The graphics profiles offered in the dialog, in `Setting::to_string` order.
const GRAPHICS_PROFILES: [&str; 6] = ["auto", "basic", "medium", "high", "ultra", "default"];

/// Where the launcher keeps the files the dialog reads and edits.
#[derive(Clone, Debug)]
pub struct SettingsContext {
    /// The user's settings file (`$SNAP_USER_COMMON/settings.json`).
    pub path: PathBuf,
    /// The Wine prefix, purged by "Reset Wine prefix".
    pub prefix: PathBuf,
    /// The saved OAuth token, whose presence is reported.
    pub token_file: PathBuf,
    /// The settings as they were loaded at startup; the dialog edits a copy.
    pub settings: Settings,
}

impl SettingsContext {
    /// Best-effort context for the preview, which runs outside a real snap.
    /// Missing paths are harmless: status shows "unknown" and purge is refused.
    pub fn preview() -> Self {
        let common = std::env::var_os("SNAP_USER_COMMON")
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from("/tmp/zwsnap-gui-preview"));
        let settings = std::env::var_os("SNAP")
            .map(PathBuf::from)
            .and_then(|snap| store::ensure_initialized(&snap, &common).ok())
            .map(|path| Settings::load(&path))
            .unwrap_or_default();
        Self {
            path: common.join(store::FILE_NAME),
            prefix: common.join(".wine"),
            token_file: common.join("pairing-token.json"),
            settings,
        }
    }
}

/// The dialog's own top-level window plus the widgets the launcher keeps in touch
/// with (the log view and the re-usable re-probe).
pub struct SettingsDialog {
    window: gtk::Window,
    log: gtk::TextView,
    log_scroll: gtk::ScrolledWindow,
    log_toggle: gtk::ToggleButton,
    display: gtk::Entry,
    graphics: gtk::DropDown,
    disable_egl: gtk::CheckButton,
    bluetooth_debug: gtk::CheckButton,
    verbosity: gtk::SpinButton,
    message: gtk::Label,
    gpu: gtk::Label,
    context: Rc<SettingsContext>,
}

impl SettingsDialog {
    /// Builds the dialog. The window is created hidden; present it to show it.
    pub fn new(parent: &gtk::ApplicationWindow, context: SettingsContext) -> Rc<Self> {
        let context = Rc::new(context);
        let settings = &context.settings;

        let window = gtk::Window::builder()
            .title("Zwift launcher settings")
            .transient_for(parent)
            .modal(true)
            .default_width(560)
            .build();

        let content = gtk::Box::new(gtk::Orientation::Vertical, 12);
        content.set_margin_top(18);
        content.set_margin_bottom(18);
        content.set_margin_start(18);
        content.set_margin_end(18);

        let heading = gtk::Label::new(Some("Settings"));
        heading.set_xalign(0.0);
        heading.add_css_class("title-2");
        content.append(&heading);

        let note = gtk::Label::new(Some("Changes take effect the next time Zwift is launched."));
        note.set_xalign(0.0);
        note.set_wrap(true);
        content.append(&note);

        // --- Editable settings ---------------------------------------------
        let grid = gtk::Grid::builder()
            .row_spacing(8)
            .column_spacing(12)
            .build();

        let display = gtk::Entry::new();
        display.set_text(&settings.display.to_string());
        display.set_hexpand(true);
        grid.attach(&row_label("Display mode"), 0, 0, 1, 1);
        grid.attach(&display, 1, 0, 1, 1);

        let graphics = gtk::DropDown::from_strings(&GRAPHICS_PROFILES);
        if let Some(index) = GRAPHICS_PROFILES
            .iter()
            .position(|name| *name == settings.graphics.to_string())
        {
            graphics.set_selected(index as u32);
        }
        grid.attach(&row_label("Graphics profile"), 0, 1, 1, 1);
        grid.attach(&graphics, 1, 1, 1, 1);

        let disable_egl = gtk::CheckButton::with_label("Disable EGL (use GLX)");
        disable_egl.set_active(settings.disable_egl);
        grid.attach(&disable_egl, 0, 2, 2, 1);

        let bluetooth_debug = gtk::CheckButton::with_label("Wine Bluetooth debug tracing");
        bluetooth_debug.set_active(settings.bluetooth_debug);
        grid.attach(&bluetooth_debug, 0, 3, 2, 1);

        let verbosity = gtk::SpinButton::with_range(0.0, 3.0, 1.0);
        verbosity.set_value(f64::from(settings.verbosity));
        grid.attach(&row_label("Log verbosity"), 0, 4, 1, 1);
        grid.attach(&verbosity, 1, 4, 1, 1);

        content.append(&grid);

        let message = gtk::Label::new(None);
        message.set_xalign(0.0);
        message.set_wrap(true);
        content.append(&message);

        // --- Read-only status ----------------------------------------------
        content.append(&gtk::Separator::new(gtk::Orientation::Horizontal));
        let status_heading = gtk::Label::new(Some("Status"));
        status_heading.set_xalign(0.0);
        status_heading.add_css_class("title-3");
        content.append(&status_heading);

        let status = gtk::Label::new(None);
        status.set_xalign(0.0);
        status.set_wrap(true);
        status.set_selectable(true);
        content.append(&status);

        let gpu = gtk::Label::new(Some("Graphics probe: not run yet"));
        gpu.set_xalign(0.0);
        gpu.set_wrap(true);
        gpu.set_selectable(true);
        content.append(&gpu);

        let reprobe = gtk::Button::with_label("Run graphics probe");
        reprobe.set_halign(gtk::Align::Start);
        content.append(&reprobe);

        // --- Maintenance ----------------------------------------------------
        content.append(&gtk::Separator::new(gtk::Orientation::Horizontal));
        let maintenance = gtk::Label::new(Some("Maintenance"));
        maintenance.set_xalign(0.0);
        maintenance.add_css_class("title-3");
        content.append(&maintenance);

        let purge_actions = gtk::Box::new(gtk::Orientation::Horizontal, 8);
        let purge_prefix = gtk::Button::with_label("Reset Wine prefix…");
        let purge_game = gtk::Button::with_label("Delete game data…");
        purge_actions.append(&purge_prefix);
        purge_actions.append(&purge_game);
        content.append(&purge_actions);

        // --- Buttons and log ------------------------------------------------
        // The log view and the button bar live outside the scroller (which holds
        // only the fixed settings above), so the log gets real vertical space
        // when shown and Close/Save stay reachable on a screen too short for the
        // whole dialog.
        let cancel = gtk::Button::with_label("Close");
        let save = gtk::Button::with_label("Save");
        save.add_css_class("suggested-action");
        let buttons = gtk::Box::new(gtk::Orientation::Horizontal, 8);
        buttons.set_halign(gtk::Align::End);
        buttons.set_margin_top(6);
        buttons.set_margin_bottom(12);
        buttons.set_margin_start(18);
        buttons.set_margin_end(18);
        buttons.append(&cancel);
        buttons.append(&save);

        let log_toggle = gtk::ToggleButton::with_label("Show log");
        log_toggle.set_halign(gtk::Align::Start);
        log_toggle.set_margin_start(18);
        log_toggle.set_margin_end(18);
        log_toggle.set_margin_top(6);
        let log = gtk::TextView::new();
        log.set_editable(false);
        log.set_monospace(true);
        log.set_wrap_mode(gtk::WrapMode::WordChar);
        let log_scroll = gtk::ScrolledWindow::builder()
            .min_content_height(180)
            .child(&log)
            .vexpand(true)
            .margin_start(18)
            .margin_end(18)
            .margin_bottom(6)
            .visible(false)
            .build();

        let scroller = gtk::ScrolledWindow::builder()
            .child(&content)
            .hscrollbar_policy(gtk::PolicyType::Never)
            .propagate_natural_height(true)
            .vexpand(true)
            .build();

        let outer = gtk::Box::new(gtk::Orientation::Vertical, 0);
        outer.append(&scroller);
        outer.append(&gtk::Separator::new(gtk::Orientation::Horizontal));
        outer.append(&log_toggle);
        outer.append(&log_scroll);
        outer.append(&buttons);
        window.set_child(Some(&outer));

        let dialog = Rc::new(Self {
            window,
            log,
            log_scroll,
            log_toggle,
            display,
            graphics,
            disable_egl,
            bluetooth_debug,
            verbosity,
            message,
            gpu,
            context,
        });

        dialog.fill_status(&status);
        dialog.wire(
            &cancel,
            &save,
            &reprobe,
            &purge_prefix,
            &purge_game,
            &status,
        );
        dialog
    }

    /// Appends launcher log output to the dialog's log view, trimming it to the
    /// same 200k characters the launcher used to keep in its main window.
    pub fn append_log(&self, text: &str) {
        let buffer = self.log.buffer();
        buffer.insert(&mut buffer.end_iter(), text);
        if buffer.char_count() > 200_000 {
            let mut start = buffer.start_iter();
            let mut cutoff = buffer.iter_at_offset(buffer.char_count() - 200_000);
            buffer.delete(&mut start, &mut cutoff);
        }
    }

    /// Shows the dialog.
    pub fn present(&self) {
        self.window.present();
    }

    /// Fills the read-only status block: token presence, the Bluetooth plug, and
    /// where the settings and game files live.
    fn fill_status(&self, status: &gtk::Label) {
        let token = present(&self.context.token_file);
        let bluez = match store::snap_is_connected("bluez") {
            Some(true) => "connected",
            Some(false) => "not connected (Bluetooth will not work)",
            None => "unknown (not running inside the snap)",
        };
        status.set_text(&format!(
            "Zwift account token: {token}\n\
             Bluetooth (bluez) plug: {bluez}\n\
             Settings file: {}\n\
             Wine prefix: {}",
            self.context.path.display(),
            self.context.prefix.display(),
        ));
    }

    fn wire(
        self: &Rc<Self>,
        cancel: &gtk::Button,
        save: &gtk::Button,
        reprobe: &gtk::Button,
        purge_prefix: &gtk::Button,
        purge_game: &gtk::Button,
        status: &gtk::Label,
    ) {
        let weak = Rc::downgrade(self);
        self.log_toggle.connect_toggled(move |button| {
            if let Some(dialog) = weak.upgrade() {
                dialog.log_scroll.set_visible(button.is_active());
            }
            button.set_label(if button.is_active() {
                "Hide log"
            } else {
                "Show log"
            });
        });

        let weak = Rc::downgrade(self);
        cancel.connect_clicked(move |_| {
            if let Some(dialog) = weak.upgrade() {
                dialog.window.close();
            }
        });

        let weak = Rc::downgrade(self);
        save.connect_clicked(move |_| {
            if let Some(dialog) = weak.upgrade() {
                dialog.save();
            }
        });

        let weak = Rc::downgrade(self);
        reprobe.connect_clicked(move |_| {
            if let Some(dialog) = weak.upgrade() {
                dialog.run_probe();
            }
        });

        let weak = Rc::downgrade(self);
        let weak_status = status.clone();
        purge_prefix.connect_clicked(move |_| {
            let Some(dialog) = weak.upgrade() else { return };
            dialog.confirm(
                "Reset Wine prefix?",
                "This deletes the Wine prefix (the game and its Windows environment). \
                 Zwift will be reinstalled and you will need to log in again.",
                dialog.context.prefix.clone(),
                store::purge_wine_prefix,
                &weak_status,
            );
        });

        let weak = Rc::downgrade(self);
        let weak_status = status.clone();
        purge_game.connect_clicked(move |_| {
            let Some(dialog) = weak.upgrade() else { return };
            dialog.confirm(
                "Delete game data?",
                "This deletes the installed Zwift files and its user data, but keeps \
                 the Wine prefix. The game will be downloaded again on the next launch.",
                dialog.context.prefix.clone(),
                store::purge_game_data,
                &weak_status,
            );
        });
    }

    /// Turns the widgets back into a `Settings`, warning (and refusing) on a bad
    /// display mode. `None` means nothing should be saved.
    fn collect(&self) -> Option<Settings> {
        let display = match DisplayMode::parse(self.display.text().as_str()) {
            Ok(mode) => mode,
            Err(error) => {
                self.message.set_text(&format!("Cannot save: {error}"));
                return None;
            }
        };
        let graphics = match GRAPHICS_PROFILES
            .get(self.graphics.selected() as usize)
            .map(|name| GraphicsSetting::parse(name))
        {
            Some(Ok(setting)) => setting,
            _ => {
                self.message
                    .set_text("Cannot save: unknown graphics profile");
                return None;
            }
        };
        Some(Settings {
            display,
            graphics,
            disable_egl: self.disable_egl.is_active(),
            bluetooth_debug: self.bluetooth_debug.is_active(),
            verbosity: self.verbosity.value() as u8,
        })
    }

    fn save(&self) {
        let Some(settings) = self.collect() else {
            return;
        };
        match settings.save(&self.context.path) {
            Ok(()) => self.message.set_text(&format!(
                "Saved to {}. Restart the launcher to apply.",
                self.context.path.display()
            )),
            Err(error) => self
                .message
                .set_text(&format!("Unable to save settings: {error}")),
        }
    }

    /// Measures the GPU on a worker thread so the (potentially slow, EGL-loading)
    /// probe never freezes the dialog, then reports the profile it implies.
    fn run_probe(&self) {
        self.gpu.set_text("Graphics probe: running…");
        run_off_thread(self.gpu.clone(), store::probe_gpu, describe_probe);
    }

    /// Asks for confirmation before running a destructive `action`, then runs it
    /// on a worker thread and reports the result in the status label.
    fn confirm(
        &self,
        title: &str,
        body: &str,
        path: PathBuf,
        action: fn(&std::path::Path) -> std::io::Result<()>,
        status: &gtk::Label,
    ) {
        let confirm = gtk::MessageDialog::builder()
            .transient_for(&self.window)
            .modal(true)
            .message_type(gtk::MessageType::Warning)
            .buttons(gtk::ButtonsType::YesNo)
            .text(title)
            .secondary_text(body)
            .build();
        let status = status.clone();
        confirm.connect_response(move |dialog, response| {
            dialog.close();
            if response != gtk::ResponseType::Yes {
                return;
            }
            let target = path.clone();
            let removed = target.clone();
            run_off_thread(
                status.clone(),
                move || action(&target),
                move |outcome| match outcome {
                    Ok(()) => format!("Removed {}.", removed.display()),
                    Err(error) => format!("Unable to remove {}: {error}", removed.display()),
                },
            );
        });
        confirm.present();
    }
}

/// Runs `action` on a worker thread and writes `format` of its result into
/// `label` from the GTK main thread. Widgets are not `Send`, so the value travels
/// through a channel that the main loop polls instead.
fn run_off_thread<T: Send + 'static>(
    label: gtk::Label,
    action: impl FnOnce() -> T + Send + 'static,
    format: impl FnOnce(T) -> String + 'static,
) {
    let (sender, receiver) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let _ = sender.send(action());
    });
    let mut format = Some(format);
    glib::timeout_add_local(Duration::from_millis(100), move || {
        match receiver.try_recv() {
            Ok(value) => {
                if let Some(format) = format.take() {
                    label.set_text(&format(value));
                }
                glib::ControlFlow::Break
            }
            Err(std::sync::mpsc::TryRecvError::Empty) => glib::ControlFlow::Continue,
            Err(std::sync::mpsc::TryRecvError::Disconnected) => glib::ControlFlow::Break,
        }
    });
}

fn row_label(text: &str) -> gtk::Label {
    let label = gtk::Label::new(Some(text));
    label.set_xalign(0.0);
    label
}

fn present(path: &std::path::Path) -> &'static str {
    if path.is_file() {
        "present"
    } else {
        "not found"
    }
}

fn describe_probe(outcome: Result<GpuProbe, String>) -> String {
    match outcome {
        Ok(probe) => format!(
            "Graphics probe: {:.2} ms on {} — suggests profile '{}'",
            probe.frame_ms, probe.renderer, probe.profile
        ),
        Err(error) => format!("Graphics probe failed: {error}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn describe_probe_reports_profile_and_renderer() {
        let text = describe_probe(Ok(GpuProbe {
            renderer: "llvmpipe".into(),
            frame_ms: 12.5,
            profile: crate::graphics::Profile::Medium,
        }));
        assert!(text.contains("12.50 ms"), "{text}");
        assert!(text.contains("llvmpipe"), "{text}");
        assert!(text.contains("medium"), "{text}");
    }

    #[test]
    fn describe_probe_reports_failure() {
        assert_eq!(
            describe_probe(Err("no libEGL".into())),
            "Graphics probe failed: no libEGL"
        );
    }

    #[test]
    fn graphics_profiles_round_trip_through_parse() {
        for name in GRAPHICS_PROFILES {
            let setting = GraphicsSetting::parse(name).unwrap();
            assert_eq!(setting.to_string(), name);
        }
    }

    #[test]
    fn presence_reports_file_state() {
        let dir = std::env::temp_dir().join(format!("zwsnap-gui-presence-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        assert_eq!(present(&dir.join("missing")), "not found");
        std::fs::write(dir.join("there"), "").unwrap();
        assert_eq!(present(&dir.join("there")), "present");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn preview_context_is_self_consistent() {
        let context = SettingsContext::preview();
        assert_eq!(context.path.file_name().unwrap(), store::FILE_NAME);
        assert_eq!(context.prefix, context.path.parent().unwrap().join(".wine"));
    }
}
