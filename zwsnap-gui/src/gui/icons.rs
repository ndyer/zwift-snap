//! The launcher's bundled icon set, the icon-button helpers built on it, and the
//! process-wide UI mode that lets icon-only controls render as text.
//!
//! The icons are vendored from Tabler Icons (MIT) and compiled into the binary as
//! a GResource (see `resources/icons/README.md`); [`init`] registers that resource
//! with GTK, so the names below resolve without any system icon theme. The names
//! are `zwsnap-<name>-symbolic`, exposed here as constants so call sites never
//! spell a raw icon name.
//!
//! The controls are otherwise icon-only, which is unreadable without hover text and
//! unreachable for assistive tooling. Every button therefore carries both an icon
//! name and a short `label`, and a process-wide [`UiMode`] decides which is shown:
//! in [`UiMode::Icons`] the icon is drawn (with the label as the accessible name),
//! in [`UiMode::Text`] the label replaces the icon. The text mode exists for
//! accessibility and for the Robot suites, which match the label by OCR instead of
//! carrying an icon screenshot.

use gtk::prelude::*;
use std::sync::atomic::{AtomicBool, Ordering};

/// The size of the icons drawn inside the buttons, in pixels.
const ICON_SIZE: i32 = 16;

/// The resource path GTK looks the bundled icons up under, matching the prefix in
/// `resources/icons.gresource.xml`.
const ICON_RESOURCE_PATH: &str = "/com/github/ndyer/ZwiftSnap/icons";

/// Bundled icon names. Each is the file name (minus `.svg`) of a file in
/// `resources/icons/`; see `resources/icons/README.md` for the provenance.
pub const REFRESH: &str = "zwsnap-refresh-symbolic";
pub const ADD: &str = "zwsnap-plus-symbolic";
pub const SETTINGS: &str = "zwsnap-settings-symbolic";
pub const QUIT: &str = "zwsnap-quit-symbolic";
pub const LOG: &str = "zwsnap-log-symbolic";
pub const REMOVE: &str = "zwsnap-remove-symbolic";
pub const DISTANCE: &str = "zwsnap-distance-symbolic";
pub const ELEVATION: &str = "zwsnap-elevation-symbolic";
pub const TIME: &str = "zwsnap-time-symbolic";

/// Every bundled icon name, for the test that checks they all resolve.
#[allow(dead_code)]
pub const ALL: &[&str] = &[
    REFRESH, ADD, SETTINGS, QUIT, LOG, REMOVE, DISTANCE, ELEVATION, TIME,
];

/// Short labels for the controls, kept distinct so a label is unmistakeable to OCR
/// (or a person): the card's `Remove` never collides with the dialog's
/// "Remove account", and the log toggle reads `Show log`, not a bare `Log`.
pub const LABEL_RETRY: &str = "Retry";
pub const LABEL_ADD: &str = "Add account";
pub const LABEL_SETTINGS: &str = "Settings";
pub const LABEL_QUIT: &str = "Quit";
pub const LABEL_LOG: &str = "Show log";
pub const LABEL_REMOVE: &str = "Remove";

/// Whether the launcher draws icon-only controls as icons or as their text label.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum UiMode {
    /// The default: controls are icon-only, with the label as a tooltip and
    /// accessible name.
    #[default]
    Icons,
    /// Controls show their text label instead of an icon, for accessibility and for
    /// the Robot suites, which then match the label by OCR.
    Text,
}

/// The active UI mode, read from a process static because GTK widget builders have
/// no other way to reach the startup decision. Set once by [`init`].
static TEXT_MODE: AtomicBool = AtomicBool::new(false);

/// Register the bundled icons with GTK and record the UI mode.
///
/// Call once, before any window is built. `mode` is the effective mode: the
/// `text_ui` setting overridden by a `--text-ui` command-line flag. Registering the
/// resource path is idempotent, so calling this more than once is harmless.
pub fn init(mode: UiMode) {
    TEXT_MODE.store(mode == UiMode::Text, Ordering::Relaxed);
    // Bring the compiled icon set into this process, then let GTK look the names
    // up in it. Registration is idempotent, so calling `init` twice is harmless.
    gtk::gio::resources_register_include!("icons.gresource")
        .expect("failed to register the bundled icon resource");
    if let Some(display) = gtk::gdk::Display::default() {
        gtk::IconTheme::for_display(&display).add_resource_path(ICON_RESOURCE_PATH);
    }
}

/// Whether the text UI mode is active (see [`UiMode`]).
///
/// Widget builders that are not buttons (the stat chips, say) use this to choose
/// between an icon and a text label.
pub fn text_mode() -> bool {
    TEXT_MODE.load(Ordering::Relaxed)
}

/// Build a flat button that is either icon-only or a text label, depending on the
/// active [`UiMode`], with hover text either way.
///
/// `name` is a bundled icon name (a `zwsnap-…` constant above); `label` is the
/// short button text, used as the visible text in [`UiMode::Text`] and as the
/// accessible name otherwise; `tooltip` is the hover text. The button is given the
/// `flat` class so it reads as a quiet, borderless control until hovered.
pub fn icon_button(name: &str, label: &str, tooltip: &str) -> gtk::Button {
    let button = if text_mode() {
        gtk::Button::with_label(label)
    } else {
        gtk::Button::from_icon_name(name)
    };
    button.set_tooltip_text(Some(tooltip));
    button.update_property(&[gtk::accessible::Property::Label(label)]);
    button.add_css_class("flat");
    // In Icons mode the child is the button's `Image`; shrink it to the control
    // size. (In Text mode the child is a label and is left alone.)
    if let Some(image) = button.child().and_downcast::<gtk::Image>() {
        image.set_pixel_size(ICON_SIZE);
    }
    button
}

/// Build a toggle whose content is either icon-only or a text label, depending on
/// the active [`UiMode`].
///
/// Used for the log toggle, which `log_panel` builds itself rather than through a
/// shared button helper. `name` must be a bundled icon name.
pub fn toggle_button(name: &str, label: &str, tooltip: &str) -> gtk::ToggleButton {
    let toggle = gtk::ToggleButton::new();
    if text_mode() {
        toggle.set_label(label);
    } else {
        let image = gtk::Image::from_icon_name(name);
        image.set_pixel_size(ICON_SIZE);
        toggle.set_child(Some(&image));
    }
    toggle.set_tooltip_text(Some(tooltip));
    toggle.update_property(&[gtk::accessible::Property::Label(label)]);
    toggle
}

/// An inline icon for use inside a widget (a stat chip, say), sized in pixels.
///
/// Unlike [`icon_button`] this is not a control; the caller places it. It always
/// draws an icon (there is no text alternative at the widget level); callers that
/// need a text fallback branch on the active mode themselves.
pub fn icon(name: &str, size: i32) -> gtk::Image {
    let image = gtk::Image::from_icon_name(name);
    image.set_pixel_size(size);
    image
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every bundled icon resolves through the registered resource. Needs a GTK
    /// display (the preview compositor), hence `#[ignore]`.
    #[test]
    #[ignore = "requires the GTK preview compositor"]
    fn all_bundled_icons_resolve() {
        gtk::init().unwrap();
        init(UiMode::Icons);
        let theme = gtk::IconTheme::for_display(&gtk::gdk::Display::default().unwrap());
        for name in ALL {
            assert!(theme.has_icon(name), "icon {name} did not resolve");
        }
    }

    /// In text mode a button shows its label; in icon mode it holds an image.
    #[test]
    #[ignore = "requires the GTK preview compositor"]
    fn text_mode_renders_labels() {
        gtk::init().unwrap();
        init(UiMode::Text);
        let button = icon_button(REMOVE, LABEL_REMOVE, "Remove account");
        assert_eq!(button.label().as_deref(), Some(LABEL_REMOVE));
        init(UiMode::Icons);
        let button = icon_button(REMOVE, LABEL_REMOVE, "Remove account");
        assert!(button.child().is_some_and(|child| child.is::<gtk::Image>()));
    }
}
