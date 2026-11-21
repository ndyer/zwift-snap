//! The launcher log view and its show/hide toggle.
//!
//! The log used to live inside the Settings dialog; it now sits in the right
//! half of the window's bottom pane, beside the WebKit login panel, so the
//! launcher's own output is visible while the launcher works. The pane is hidden
//! by default and revealed by the "Show log" toggle in the window's top-right
//! corner.
//!
//! The logger writes `zwift.log` itself and hands every message to registered
//! sinks, so the view needs no tailing of the file: `build_window` forwards each
//! `Event::Log` to [`LogPanel::append`] on the GTK main thread.

use gtk::prelude::*;

use super::icons;

/// The maximum number of characters the log view keeps before trimming its
/// oldest output, matching the limit the launcher used before the pane existed.
const MAX_CHARS: i32 = 200_000;

/// The narrowest the log pane may become, matching its `min_content_width`.
pub const MIN_WIDTH: i32 = 320;

/// The widest the log pane opens at. The log wraps, so it needs far less room
/// than a full window; the leftover width goes to the login pane rather than
/// stretching the log across the screen.
pub const MAX_WIDTH: i32 = 420;

/// The width the login pane keeps to the left of the divider. Not squeezing it
/// below this is why a narrow window's split is unchanged when the log opens.
const LOGIN_WIDTH: i32 = 560;

/// The divider position that reveals the log at no more than [`MAX_WIDTH`], for
/// a paned of the given total `width`. The login pane is never squeezed below
/// [`LOGIN_WIDTH`] and the log never below [`MIN_WIDTH`].
pub fn divider_position(width: i32) -> i32 {
    (width - MAX_WIDTH).max(LOGIN_WIDTH).min(width - MIN_WIDTH)
}

/// The log pane: a scrollable, read-only, monospaced text view plus the toggle
/// button that shows and hides it.
pub struct LogPanel {
    scroll: gtk::ScrolledWindow,
    toggle: gtk::ToggleButton,
    view: gtk::TextView,
}

impl LogPanel {
    /// Builds the pane, hidden until its toggle is activated.
    pub fn new() -> Self {
        install_css();

        let view = gtk::TextView::new();
        view.set_editable(false);
        view.set_monospace(true);
        view.set_wrap_mode(gtk::WrapMode::WordChar);

        let scroll = gtk::ScrolledWindow::builder()
            .min_content_width(MIN_WIDTH)
            .min_content_height(180)
            .child(&view)
            .hexpand(true)
            .vexpand(true)
            .visible(false)
            .build();

        // The toggle carries either the bundled log icon or, in text mode, its
        // label. It keeps its full chrome (deliberately *not* given the `flat`
        // class) so it reads as a button, and its *checked* highlight shows whether
        // the log is currently visible.
        let toggle = icons::toggle_button(icons::LOG, icons::LABEL_LOG, "Show log");
        toggle.add_css_class("log-toggle");

        let panel = Self {
            scroll,
            toggle,
            view,
        };
        panel.wire();
        panel
    }

    /// The pane widget, to place in the window's bottom split.
    pub fn widget(&self) -> &gtk::ScrolledWindow {
        &self.scroll
    }

    /// The show/hide button, to place in the window's header.
    pub fn toggle_button(&self) -> &gtk::ToggleButton {
        &self.toggle
    }

    /// Appends launcher output to the view, trimming it to the character limit.
    pub fn append(&self, text: &str) {
        let buffer = self.view.buffer();
        buffer.insert(&mut buffer.end_iter(), text);
        if buffer.char_count() > MAX_CHARS {
            let mut start = buffer.start_iter();
            let mut cutoff = buffer.iter_at_offset(buffer.char_count() - MAX_CHARS);
            buffer.delete(&mut start, &mut cutoff);
        }
    }

    /// Caps the log pane's width when the toggle reveals it. The paned keeps the
    /// divider where it was, so without this the log would open at whatever
    /// width the window leaves it — the whole width minus the login pane on a
    /// wide or fullscreen window. Dragging the divider wider still works.
    pub fn limit_width(&self, paned: &gtk::Paned) {
        self.toggle.connect_toggled({
            let paned = paned.clone();
            move |button| {
                if !button.is_active() {
                    return;
                }
                let width = paned.width();
                if width > 0 {
                    paned.set_position(divider_position(width));
                }
            }
        });
    }

    /// Reveals or hides the pane, and flips the button's hover text to match.
    /// The visibility itself is shown by the toggle's checked highlight.
    fn wire(&self) {
        self.toggle.connect_toggled({
            let scroll = self.scroll.clone();
            move |button| {
                let active = button.is_active();
                scroll.set_visible(active);
                button.set_tooltip_text(Some(if active { "Hide log" } else { "Show log" }));
            }
        });
    }
}

impl Default for LogPanel {
    fn default() -> Self {
        Self::new()
    }
}

/// Register the toggle's CSS once per display. Unknown properties are ignored by
/// GTK, so this is best-effort and never fatal.
fn install_css() {
    use std::sync::Once;
    static ONCE: Once = Once::new();
    ONCE.call_once(|| {
        let Some(display) = gtk::gdk::Display::default() else {
            return;
        };
        let provider = gtk::CssProvider::new();
        provider.load_from_data(CSS);
        gtk::style_context_add_provider_for_display(
            &display,
            &provider,
            gtk::STYLE_PROVIDER_PRIORITY_APPLICATION,
        );
    });
}

/// The toggle's styles. The button keeps its default chrome; the `:checked`
/// state gets an accent tint so it is obvious the log pane is currently open.
const CSS: &str = "
.log-toggle:checked {
    background-image: none;
    background-color: alpha(#f26b1d, 0.35);
    color: white;
}
";

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wide_windows_open_the_log_at_the_cap() {
        // A wide window leaves room for far more than the log needs, so it opens
        // at MAX_WIDTH and the login pane takes the rest.
        assert_eq!(divider_position(1920), 1920 - MAX_WIDTH);
        assert_eq!(divider_position(1280), 1280 - MAX_WIDTH);
    }

    #[test]
    fn narrow_windows_leave_the_split_unchanged() {
        // At an 880-wide window the log would only be its minimum anyway, so the
        // divider stays where the login pane's preferred width puts it.
        assert_eq!(divider_position(880), LOGIN_WIDTH);
    }

    #[test]
    fn the_log_never_drops_below_its_minimum() {
        // A window too small for both preferences still gives the log MIN_WIDTH,
        // squeezing the login pane instead.
        assert_eq!(divider_position(600), 600 - MIN_WIDTH);
    }
}
