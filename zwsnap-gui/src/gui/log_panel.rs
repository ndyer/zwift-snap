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

/// The maximum number of characters the log view keeps before trimming its
/// oldest output, matching the limit the launcher used before the pane existed.
const MAX_CHARS: i32 = 200_000;

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
        let view = gtk::TextView::new();
        view.set_editable(false);
        view.set_monospace(true);
        view.set_wrap_mode(gtk::WrapMode::WordChar);

        let scroll = gtk::ScrolledWindow::builder()
            .min_content_width(320)
            .min_content_height(180)
            .child(&view)
            .hexpand(true)
            .vexpand(true)
            .visible(false)
            .build();

        // A plain, fully-chromed toggle button: it must read as a button in the
        // header, so it is deliberately *not* given the `flat` class.
        let toggle = gtk::ToggleButton::with_label("Show log");

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

    /// Reveals or hides the pane and relabels the toggle to match.
    fn wire(&self) {
        self.toggle.connect_toggled({
            let scroll = self.scroll.clone();
            move |button| {
                scroll.set_visible(button.is_active());
                button.set_label(if button.is_active() {
                    "Hide log"
                } else {
                    "Show log"
                });
            }
        });
    }
}

impl Default for LogPanel {
    fn default() -> Self {
        Self::new()
    }
}
