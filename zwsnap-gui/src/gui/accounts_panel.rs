//! The account picker: a wrapping grid of rider cards, one per remembered
//! account, mirroring the Zwift app's user-selection screen.
//!
//! Each card shows the rider's avatar (or an initials badge), their name, the
//! three stat chips the app shows (total distance, elevation gain, total ride
//! time) and a muted line with level, FTP and weight. An orange play button
//! launches that account; a Remove action forgets it. The card used last gets a
//! "Last used" pill and is focused, so Enter relaunches it. An "Add another
//! account" button, which the launcher places in the window header, adds a new
//! account by logging in.
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

/// The diameter of a card's avatar, in pixels.
const AVATAR_SIZE: i32 = 72;

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
            .selection_mode(gtk::SelectionMode::None)
            .homogeneous(true)
            .max_children_per_line(4)
            .min_children_per_line(1)
            .row_spacing(12)
            .column_spacing(12)
            .valign(gtk::Align::Start)
            .halign(gtk::Align::Fill)
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

        let add_button = gtk::Button::with_label("Add another account");
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
        for account in &store.accounts {
            let card = self.card(&store, account, last_used.as_deref());
            // `insert(-1)` appends; the shorthand needs GTK 4.6, beyond the crate's API level.
            self.flow.insert(&card, -1);
        }
        drop(store);
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
        // The card stacks the avatar and details on top with the actions in a
        // row underneath, rather than beside them, so each card stays narrow
        // and the grid packs more riders per line.
        let card = gtk::Box::new(gtk::Orientation::Vertical, 8);
        card.add_css_class("account-card");
        card.set_margin_top(6);
        card.set_margin_bottom(6);

        let top = gtk::Box::new(gtk::Orientation::Horizontal, 12);
        top.append(&self.avatar(store, account));

        // Name, the three stat chips, and the muted level/FTP/weight line.
        let info = gtk::Box::new(gtk::Orientation::Vertical, 4);
        info.set_valign(gtk::Align::Center);
        info.set_hexpand(true);

        let name_row = gtk::Box::new(gtk::Orientation::Horizontal, 8);
        let name = gtk::Label::new(Some(&account.name));
        name.set_xalign(0.0);
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
        chips.append(&chip(&stat_or_dash(account.distance_meters, |meters| {
            stats::distance(meters, metric)
        })));
        chips.append(&chip(&stat_or_dash(account.elevation_meters, |meters| {
            stats::elevation(meters, metric)
        })));
        chips.append(&chip(&stat_or_dash(account.time_minutes, |minutes| {
            stats::duration(minutes)
        })));
        info.append(&chips);

        let meta = gtk::Label::new(Some(&meta_line(account, metric)));
        meta.set_xalign(0.0);
        meta.add_css_class("dim-label");
        info.append(&meta);

        top.append(&info);
        card.append(&top);
        card.append(&self.actions(account, last_used == Some(account.id.as_str())));

        card.upcast()
    }

    /// The avatar image, or an initials badge when none is stored.
    fn avatar(&self, store: &Store, account: &Account) -> gtk::Widget {
        if let Some(path) = store.avatar_path(account) {
            let picture = gtk::Picture::for_filename(&path);
            picture.set_size_request(AVATAR_SIZE, AVATAR_SIZE);
            picture.set_can_shrink(true);
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

    /// The Remove and Play buttons, laid out as a full-width row underneath the
    /// rider's details.
    fn actions(self: &Rc<Self>, account: &Account, is_last_used: bool) -> gtk::Widget {
        let box_ = gtk::Box::new(gtk::Orientation::Horizontal, 6);

        let remove = gtk::Button::with_label("Remove");
        remove.add_css_class("flat");
        remove.set_hexpand(true);
        let remove_id = account.id.clone();
        let weak = self.weak();
        remove.connect_clicked(move |button| {
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
            // Labelled differently from the card's own "Remove" button so a
            // click target is never ambiguous between the two.
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

        let play = gtk::Button::with_label("Play");
        play.add_css_class("account-play");
        play.add_css_class("suggested-action");
        play.set_hexpand(true);
        let play_id = account.id.clone();
        let weak = self.weak();
        play.connect_clicked(move |_| {
            if let Some(panel) = weak.upgrade() {
                (panel.select)(play_id.clone());
            }
        });
        if is_last_used {
            // Focus the last-used card's play button, so Enter relaunches it.
            glib::idle_add_local_once({
                let weak = play.downgrade();
                move || {
                    if let Some(play) = weak.upgrade() {
                        play.grab_focus();
                    }
                }
            });
        }

        box_.append(&remove);
        box_.append(&play);
        box_.upcast()
    }

    fn weak(self: &Rc<Self>) -> std::rc::Weak<AccountsPanel> {
        Rc::downgrade(self)
    }
}

/// A stat chip: a small rounded label. `text` is `None` when the field is
/// unknown, in which case an em dash is shown.
fn chip(text: &str) -> gtk::Widget {
    let label = gtk::Label::new(Some(text));
    label.add_css_class("account-chip");
    label.upcast()
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

/// The picker's styles. Deliberately small: a card surface, a round avatar, the
/// chips, the last-used pill and the orange play button.
const CSS: &str = "
.account-card {
    background-color: rgba(0, 0, 0, 0.10);
    border-radius: 12px;
    padding: 12px;
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
.account-pill {
    background-color: alpha(white, 0.2);
    border-radius: 6px;
    padding: 1px 8px;
    font-size: 0.8em;
}
.account-play {
    background-image: none;
    background-color: #f26b1d;
    color: white;
    border-radius: 9999px;
    padding: 6px 18px;
    font-weight: bold;
}
.account-play:hover {
    background-color: #ff7d33;
}
.account-grid {
    padding: 6px;
}
";
