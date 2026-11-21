//! The account picker: a wrapping grid of rider cards, one per remembered
//! account, mirroring the Zwift app's user-selection screen.
//!
//! Each card shows the rider's avatar (or an initials badge), their name, the
//! three stat chips the app shows (distance, elevation gain, ride time) - each an
//! icon beside the value, or the label and value together in text mode -
//! and a muted line with level, FTP and weight. A slim bar under that line fills
//! with progress toward the rider's next level when the profile carries it. The
//! whole card is the launch target: clicking it, or pressing Space/Enter once it
//! is highlighted, launches that account, and the arrow keys move the highlight
//! around the grid (native `GtkFlowBox` navigation). A small remove icon floats
//! over the card's top-right corner and forgets the account. The card used last
//! gets a "Last used" pill and starts highlighted and focused, so Enter
//! relaunches it; Escape quits the launcher. An "Add another account" button,
//! which the launcher places in the window header, adds a new account by logging
//! in.
//!
//! The panel owns the [`Store`] and rebuilds the grid from it on
//! [`AccountsPanel::refresh`]; selecting and removing are the panel's own
//! concern, while the launcher decides what a *selection* means (answering the
//! login gate). The launcher refreshes profiles and then calls `refresh`.

use crate::accounts::{Account, Store};
use crate::logging::Logger;
use crate::stats;
use gtk::glib;
use gtk::prelude::*;
use std::rc::Rc;
use std::sync::Arc;

use super::icons;

/// The diameter of a card's avatar, in pixels.
const AVATAR_SIZE: i32 = 72;

/// The size of the icon drawn inside a stat chip, in pixels (a little smaller than
/// the button icons, to sit within the chip's padding).
const CHIP_ICON_SIZE: i32 = 16;

/// The tallest the grid is allowed to grow before it starts scrolling. Below
/// this it sizes to its content, so a couple of accounts do not stretch the
/// launcher across the whole screen.
const MAX_GRID_HEIGHT: i32 = 320;

/// Callbacks the panel uses to hand a selection back to the launcher.
pub struct Handlers {
    /// Called with an account id when its card is chosen.
    pub select: Box<dyn Fn(String)>,
    /// Called when the user asks to add another account.
    pub add: Box<dyn Fn()>,
}

/// The picker: a scrolled grid of cards plus the "add account" button, which
/// the launcher places in the window header alongside the log toggle and
/// Settings.
pub struct AccountsPanel {
    flow: gtk::FlowBox,
    scroller: gtk::ScrolledWindow,
    add_button: gtk::Button,
    store: Rc<std::cell::RefCell<Store>>,
    logger: Option<Arc<Logger>>,
    select: Rc<dyn Fn(String)>,
    add: Rc<dyn Fn()>,
}

impl AccountsPanel {
    /// Build the panel from the already-loaded `store`. Nothing is drawn until
    /// [`AccountsPanel::refresh`] runs.
    pub fn new(
        store: Rc<std::cell::RefCell<Store>>,
        logger: Option<Arc<Logger>>,
        handlers: Handlers,
    ) -> Rc<Self> {
        install_css();

        let flow = gtk::FlowBox::builder()
            .homogeneous(true)
            .max_children_per_line(4)
            .min_children_per_line(1)
            .row_spacing(12)
            .column_spacing(12)
            .valign(gtk::Align::Start)
            .halign(gtk::Align::Start)
            .build();
        flow.add_css_class("account-grid");

        // Size to the content rather than filling the window. A `FlowBox`
        // reports its natural height for its *natural* width, which packs
        // several narrow cards onto one line, so its own request is far too
        // short. Instead the scroller's height is pinned to the grid measured
        // at the width it is actually given (see [`Self::update_height`]), capped
        // at [`MAX_GRID_HEIGHT`] so many accounts scroll rather than stretch the
        // launcher down the whole window.
        let scroller = gtk::ScrolledWindow::builder()
            .child(&flow)
            .hscrollbar_policy(gtk::PolicyType::Never)
            .max_content_height(MAX_GRID_HEIGHT)
            .vexpand(false)
            .hexpand(true)
            .build();

        let add_button = icons::icon_button(icons::ADD, icons::LABEL_ADD, "Add another account");
        add_button.add_css_class("add-account");
        // The launcher appends this to the header; it starts hidden and is
        // revealed with the grid by [`AccountsPanel::set_visible`].
        add_button.set_visible(false);

        let panel = Rc::new(Self {
            flow,
            scroller: scroller.clone(),
            add_button: add_button.clone(),
            store,
            logger,
            select: Rc::from(handlers.select),
            add: Rc::from(handlers.add),
        });

        // The grid's height depends on the width it is given, which is only
        // known once the panel is mapped and allocated, so recompute it then as
        // well as on every refresh.
        let weak = Rc::downgrade(&panel);
        scroller.connect_map(move |_| {
            if let Some(panel) = weak.upgrade() {
                panel.request_height_update();
            }
        });

        let weak = Rc::downgrade(&panel);
        add_button.connect_clicked(move |_| {
            if let Some(panel) = weak.upgrade() {
                if let Some(logger) = &panel.logger {
                    logger.debug("Adding a new account from the picker");
                }
                (panel.add)();
            }
        });

        // A card is activated either by a click or by Space/Enter during
        // keyboard navigation. The native `FlowBox` handles both and reports the
        // child here; map its index back to the account id from the store order
        // built up in `refresh`.
        let weak = Rc::downgrade(&panel);
        panel.flow.connect_child_activated(move |_, child| {
            let Some(panel) = weak.upgrade() else { return };
            let index = child.index();
            if index < 0 {
                return;
            }
            let id = panel
                .store
                .borrow()
                .accounts
                .get(index as usize)
                .map(|account| account.id.clone());
            if let Some(id) = id {
                (panel.select)(id);
            }
        });

        panel
    }

    /// The widget to place in the window layout.
    pub fn widget(&self) -> &gtk::Widget {
        self.scroller.upcast_ref()
    }

    /// The "Add another account" button, for the launcher to place in the
    /// header.
    pub fn add_button(&self) -> &gtk::Button {
        &self.add_button
    }

    /// Show or hide the picker (the grid and the header's add button) together.
    pub fn set_visible(&self, visible: bool) {
        self.scroller.set_visible(visible);
        self.add_button.set_visible(visible);
    }

    /// Whether the store holds at least one account (so the launcher can decide
    /// between the picker and the plain login gate).
    pub fn has_accounts(&self) -> bool {
        !self.store.borrow().is_empty()
    }

    /// Rebuild the grid from the store, keeping the panel's current widget tree
    /// otherwise intact.
    pub fn refresh(self: &Rc<Self>) {
        while let Some(child) = self.flow.first_child() {
            self.flow.remove(&child);
        }

        let store = self.store.borrow();
        let last_used = store.last_used_id().map(str::to_owned);
        // Remember where the last-used account sits so it can seed the keyboard
        // cursor below (falling back to the first card when there is none).
        let preferred = last_used
            .as_deref()
            .and_then(|id| store.accounts.iter().position(|a| a.id == id))
            .unwrap_or(0);
        for account in &store.accounts {
            let card = self.card(&store, account, last_used.as_deref());
            // `insert(-1)` appends; the shorthand needs GTK 4.6, beyond the crate's API level.
            self.flow.insert(&card, -1);
        }
        drop(store);

        // Seed both the keyboard cursor and the `:selected` highlight on the
        // preferred card, so arrow keys and Enter work before any click. A focus
        // on the `FlowBoxChild` runs the flow box's own selection update.
        if let Some(child) = self.flow.child_at_index(preferred as i32) {
            child.child_focus(gtk::DirectionType::TabForward);
        }

        self.request_height_update();
    }

    /// Pin the scroller to the grid's content height, retrying briefly if the
    /// grid has not been allocated a width yet.
    ///
    /// The height-for-width measurement needs the grid's real width, but the
    /// panel is shown before its first allocation, so a single attempt at map
    /// time can still see a width of zero and leave the scroller clipped to
    /// nothing. Poll on a short timer until the width is known.
    fn request_height_update(self: &Rc<Self>) {
        if self.update_height() {
            return;
        }
        let weak = Rc::downgrade(self);
        glib::timeout_add_local(std::time::Duration::from_millis(16), move || {
            match weak.upgrade() {
                Some(panel) if panel.update_height() => glib::ControlFlow::Break,
                Some(_) => glib::ControlFlow::Continue,
                None => glib::ControlFlow::Break,
            }
        });
    }

    /// Measure the grid at the width it actually has and pin the scroller to
    /// that height, capped at [`MAX_GRID_HEIGHT`]. Returns whether the width was
    /// known (and the height therefore set).
    fn update_height(&self) -> bool {
        let width = self.flow.width();
        if width <= 0 {
            return false;
        }
        let (_, natural, _, _) = self.flow.measure(gtk::Orientation::Vertical, width);
        self.scroller
            .set_min_content_height(natural.clamp(0, MAX_GRID_HEIGHT));
        true
    }

    /// Forget `id`, saving the store and rebuilding the grid. Returns whether an
    /// account was removed.
    pub fn remove(self: &Rc<Self>, id: &str) -> bool {
        let removed = {
            let mut store = self.store.borrow_mut();
            match store.remove(id) {
                Ok(()) => true,
                Err(error) => {
                    if let Some(logger) = &self.logger {
                        logger.warning(&format!("Cannot remove account: {error}"));
                    }
                    false
                }
            }
        };
        if removed {
            if let Err(error) = self.store.borrow().save() {
                if let Some(logger) = &self.logger {
                    logger.warning(&format!("Cannot save the account list: {error}"));
                }
            }
            self.refresh();
        }
        removed
    }

    /// Build one rider card.
    fn card(
        self: &Rc<Self>,
        store: &Store,
        account: &Account,
        last_used: Option<&str>,
    ) -> gtk::Widget {
        // The card is a plain surface: the avatar and details sit inside it. The
        // `FlowBox` reports `child-activated` (a click, or Space/Enter during
        // keyboard navigation) and the launcher launches the rider. The remove
        // icon floats over the card's top-right corner as an `Overlay` child, so
        // a click on the icon is handled by the icon alone and never also
        // activates the card underneath.
        let top = gtk::Box::new(gtk::Orientation::Horizontal, 12);
        top.append(&self.avatar(store, account));

        // Name, the three stat chips, the muted level/FTP/weight line, and the
        // optional level-progress bar.
        let info = gtk::Box::new(gtk::Orientation::Vertical, 4);
        info.set_valign(gtk::Align::Center);
        info.set_hexpand(true);
        // Reserve room on the right for the floating remove icon, so a long name
        // or the "Last used" pill never runs underneath it.
        info.set_margin_end(28);

        let name_row = gtk::Box::new(gtk::Orientation::Horizontal, 8);
        let name = gtk::Label::new(Some(&account.name));
        name.set_xalign(0.0);
        name.set_wrap(true);
        name.set_wrap_mode(gtk::pango::WrapMode::WordChar);
        name.set_max_width_chars(20);
        name.add_css_class("title-3");
        name_row.append(&name);
        if last_used == Some(account.id.as_str()) {
            let pill = gtk::Label::new(Some("Last used"));
            pill.add_css_class("account-pill");
            name_row.append(&pill);
        }
        info.append(&name_row);

        let metric = account.use_metric.unwrap_or(true);
        let chips = gtk::Box::new(gtk::Orientation::Horizontal, 6);
        chips.append(&chip(
            icons::DISTANCE,
            stats::DISTANCE_LABEL,
            &stat_or_dash(account.distance_meters, |meters| {
                stats::distance(meters, metric)
            }),
        ));
        chips.append(&chip(
            icons::ELEVATION,
            stats::ELEVATION_LABEL,
            &stat_or_dash(account.elevation_meters, |meters| {
                stats::elevation(meters, metric)
            }),
        ));
        chips.append(&chip(
            icons::TIME,
            stats::TIME_LABEL,
            &stat_or_dash(account.time_minutes, stats::duration),
        ));
        info.append(&chips);

        let meta = gtk::Label::new(Some(&meta_line(account, metric)));
        meta.set_xalign(0.0);
        meta.set_wrap(true);
        meta.set_wrap_mode(gtk::pango::WrapMode::WordChar);
        meta.set_max_width_chars(28);
        meta.add_css_class("dim-label");
        info.append(&meta);

        // A slim bar under the meta line showing progress toward the next level.
        // Only drawn when the profile carried a level and its remainder; the
        // remainder is treated as a percentage (see `stats::level_fraction`).
        if let (Some(level), Some(progress)) = (account.level, account.level_progress) {
            let bar = gtk::ProgressBar::new();
            bar.set_fraction(stats::level_fraction(progress));
            bar.set_show_text(false);
            bar.set_hexpand(true);
            bar.set_valign(gtk::Align::Center);
            bar.set_margin_top(4);
            bar.add_css_class("account-level-bar");
            bar.set_tooltip_text(Some(&format!(
                "Level {level} — {progress}% to level {}",
                level + 1
            )));
            info.append(&bar);
        }

        top.append(&info);

        // The card surface itself carries the button styling, tooltip and
        // margins; activation is handled by the `FlowBox`'s `child-activated`
        // signal rather than a nested button.
        top.add_css_class("account-card");
        top.set_margin_top(6);
        top.set_margin_bottom(6);
        top.set_tooltip_text(Some(&format!("Launch {}", account.name)));

        let overlay = gtk::Overlay::new();
        overlay.set_halign(gtk::Align::Start);
        overlay.set_child(Some(&top));
        overlay.add_overlay(&self.remove_button(account));
        overlay.upcast()
    }

    /// The avatar image, or an initials badge when none is stored.
    fn avatar(&self, store: &Store, account: &Account) -> gtk::Widget {
        if let Some(pixbuf) = store.avatar_path(account).and_then(|path| {
            gtk::gdk_pixbuf::Pixbuf::from_file_at_scale(path, AVATAR_SIZE, AVATAR_SIZE, true).ok()
        }) {
            let texture = gtk::gdk::Texture::for_pixbuf(&pixbuf);
            let picture = gtk::Picture::for_paintable(&texture);
            picture.set_size_request(AVATAR_SIZE, AVATAR_SIZE);
            picture.set_can_shrink(true);
            picture.set_halign(gtk::Align::Start);
            picture.set_valign(gtk::Align::Center);
            picture.add_css_class("account-avatar");
            return picture.upcast();
        }
        let badge = gtk::Label::new(Some(&stats::initials(
            account.first_name.as_deref(),
            account.last_name.as_deref(),
        )));
        badge.add_css_class("account-avatar");
        badge.add_css_class("account-initials");
        badge.set_size_request(AVATAR_SIZE, AVATAR_SIZE);
        badge.set_valign(gtk::Align::Center);
        badge.upcast()
    }

    /// The small remove control floating over a card's top-right corner. It owns
    /// the confirmation dialog, because forgetting an account deletes its saved
    /// login and avatar from this computer.
    fn remove_button(self: &Rc<Self>, account: &Account) -> gtk::Button {
        let button = icons::icon_button(icons::REMOVE, icons::LABEL_REMOVE, "Remove account");
        button.add_css_class("account-remove");
        button.set_halign(gtk::Align::End);
        button.set_valign(gtk::Align::Start);

        let remove_id = account.id.clone();
        let weak = self.weak();
        button.connect_clicked(move |button| {
            let Some(panel) = weak.upgrade() else { return };
            let confirm = gtk::MessageDialog::builder()
                .modal(true)
                .message_type(gtk::MessageType::Question)
                .buttons(gtk::ButtonsType::None)
                .text("Remove this account?")
                .secondary_text("Its saved login and avatar will be deleted from this computer.")
                .build();
            if let Some(root) = button.root() {
                if let Ok(window) = root.downcast::<gtk::Window>() {
                    confirm.set_transient_for(Some(&window));
                }
            }
            confirm.add_button("Cancel", gtk::ResponseType::Cancel);
            // Labelled differently from the card's own remove icon so a click
            // target is never ambiguous between the two.
            let remove = confirm.add_button("Remove account", gtk::ResponseType::Ok);
            remove.add_css_class("destructive-action");
            let panel = panel.clone();
            let remove_id = remove_id.clone();
            let confirm = confirm.clone();
            confirm.connect_response(move |confirm, response| {
                if response == gtk::ResponseType::Ok {
                    panel.remove(&remove_id);
                }
                // GTK4 dialogs no longer hide themselves on a response, so the
                // dialog has to be closed explicitly or it stays on screen.
                confirm.close();
            });
            confirm.present();
        });

        button
    }

    fn weak(self: &Rc<Self>) -> std::rc::Weak<AccountsPanel> {
        Rc::downgrade(self)
    }
}

/// A stat chip: an icon beside the stat's value, or - in text mode - the label and
/// the value as one label. `value` is `—` when the field is unknown. The icon names
/// the statistic (a route, a mountain, a clock); the label spells it out.
fn chip(icon_name: &str, label: &str, value: &str) -> gtk::Widget {
    if icons::text_mode() {
        let text = gtk::Label::new(Some(&format!("{label} {value}")));
        text.add_css_class("account-chip");
        return text.upcast();
    }
    let chip = gtk::Box::new(gtk::Orientation::Horizontal, 4);
    chip.add_css_class("account-chip");
    chip.append(&icons::icon(icon_name, CHIP_ICON_SIZE));
    chip.append(&gtk::Label::new(Some(value)));
    chip.upcast()
}

/// Format a stat from an optional raw value, or an em dash when it is missing.
fn stat_or_dash(value: Option<i64>, format: impl FnOnce(i64) -> String) -> String {
    value.map(format).unwrap_or_else(|| "—".to_owned())
}

/// The muted line under the chips: level, FTP and weight.
fn meta_line(account: &Account, metric: bool) -> String {
    let mut parts = Vec::new();
    if let Some(level) = account.level {
        parts.push(format!("level {level}"));
    }
    if let Some(ftp) = account.ftp {
        parts.push(format!("{ftp} W"));
    }
    if let Some(weight) = account.weight_grams {
        parts.push(stats::weight(weight, metric));
    }
    if parts.is_empty() {
        "No stats yet".to_owned()
    } else {
        parts.join(" · ")
    }
}

/// Register the picker's CSS once per display. Unknown properties are ignored by
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

/// The picker's styles. Deliberately small: a clickable card surface, a round
/// avatar, the chips, the last-used pill and the floating remove icon.
const CSS: &str = "
/* The built-in theme paints a selected `flowboxchild` with the accent-blue
   `background-color` (and white text) and adds a 2px focus `outline` when it is
   focus-visible. Because the card's own background is translucent, that fill
   bleeds through the whole card. Neutralise the child node so the only highlight
   is the card border below. */
flowbox > flowboxchild {
    background-color: transparent;
    color: inherit;
    outline: none;
    box-shadow: none;
}
flowbox > flowboxchild:selected {
    background-color: transparent;
    color: inherit;
    outline: none;
}
.account-card {
    background-image: none;
    background-color: rgba(0, 0, 0, 0.10);
    border: 2px solid transparent;
    box-shadow: none;
    border-radius: 12px;
    padding: 12px;
    cursor: pointer;
}
.account-card:hover {
    background-color: rgba(0, 0, 0, 0.18);
}
flowboxchild:selected .account-card {
    border: 2px solid #4A90D9;
    background-color: rgba(74, 144, 217, 0.15);
}
.account-remove {
    min-width: 0;
    min-height: 0;
    /* Anchored to the card's top-right corner, so the top/right margins are the
       gap from that corner; nudge it inward a little so it does not crowd it. */
    margin: 8px;
    padding: 2px;
    border-radius: 6px;
    background-image: none;
    background-color: transparent;
}
.account-remove:hover {
    background-color: alpha(black, 0.25);
}
.account-avatar {
    border-radius: 9999px;
}
.account-initials {
    background-color: alpha(white, 0.15);
    font-size: 20px;
    font-weight: bold;
}
.account-chip {
    background-color: alpha(black, 0.35);
    border-radius: 6px;
    padding: 2px 8px;
}
.account-chip image {
    margin-right: 4px;
}
.account-pill {
    background-color: alpha(white, 0.2);
    border-radius: 6px;
    padding: 1px 8px;
    font-size: 0.8em;
}
.account-level-bar {
    min-height: 6px;
    border-radius: 3px;
}
.account-level-bar > trough {
    min-height: 6px;
    border-radius: 3px;
    background-color: alpha(black, 0.35);
}
.account-level-bar > progress {
    min-height: 6px;
    border-radius: 3px;
    background-color: #4A90D9;
}
.account-grid {
    padding: 6px;
}
";

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    #[ignore = "requires the GTK preview compositor"]
    fn card_bounds_large_avatar_and_wraps_long_name() {
        gtk::init().unwrap();
        let dir = std::env::temp_dir().join(format!("zwsnap-card-layout-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let image =
            gtk::gdk_pixbuf::Pixbuf::new(gtk::gdk_pixbuf::Colorspace::Rgb, true, 8, 1024, 1024)
                .unwrap();
        image.fill(0xff8000ff);
        let image_path = dir.join("source.png");
        image.savev(&image_path, "png", &[]).unwrap();
        let mut store = Store::open(&dir);
        store.accounts.push(Account {
            id: "rider".to_owned(),
            name: "A Very Long Rider Name That Should Wrap Instead Of Stretching The Card"
                .to_owned(),
            ..Account::default()
        });
        store.accounts.push(Account {
            id: "second".to_owned(),
            name: "Second Rider".to_owned(),
            ..Account::default()
        });
        store
            .write_avatar("rider", &std::fs::read(image_path).unwrap(), "png")
            .unwrap();
        let store = Rc::new(std::cell::RefCell::new(store));
        let panel = AccountsPanel::new(
            store,
            None,
            Handlers {
                select: Box::new(|_| {}),
                add: Box::new(|| {}),
            },
        );
        panel.refresh();
        let window = gtk::Window::builder()
            .default_width(1200)
            .default_height(600)
            .build();
        window.set_child(Some(panel.widget()));
        window.present();
        let main_loop = glib::MainLoop::new(None, false);
        let stop = main_loop.clone();
        glib::timeout_add_local_once(std::time::Duration::from_millis(250), move || stop.quit());
        main_loop.run();

        let child = panel
            .flow
            .first_child()
            .unwrap()
            .downcast::<gtk::FlowBoxChild>()
            .unwrap();
        let overlay = child.child().unwrap().downcast::<gtk::Overlay>().unwrap();
        // The card is no longer wrapped in a button: the `FlowBox` handles
        // activation, so the overlay's child is the card's own box.
        let top = overlay.child().unwrap().downcast::<gtk::Box>().unwrap();
        let picture = top
            .first_child()
            .unwrap()
            .downcast::<gtk::Picture>()
            .unwrap();
        let info = picture
            .next_sibling()
            .unwrap()
            .downcast::<gtk::Box>()
            .unwrap();
        let name_row = info.first_child().unwrap().downcast::<gtk::Box>().unwrap();
        let name = name_row
            .first_child()
            .unwrap()
            .downcast::<gtk::Label>()
            .unwrap();
        assert_eq!(
            (picture.width(), picture.height()),
            (AVATAR_SIZE, AVATAR_SIZE)
        );
        assert!(overlay.width() < 600, "card width was {}", overlay.width());
        assert!(name.layout().line_count() > 1);
        let next = child.next_sibling().unwrap();
        let gap = next.allocation().x() - child.allocation().x() - child.width();
        assert!(gap <= 24, "gap between grid cells was {gap}");

        window.close();
        std::fs::remove_dir_all(dir).unwrap();
    }
}
