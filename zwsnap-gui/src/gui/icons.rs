//! Shared icon-button helper.
//!
//! The launcher's header and the rider cards use a handful of small, icon-only
//! controls. Every one of them is built through [`icon_button`] so that they all
//! carry hover text (a tooltip) — an icon-only control with no label is
//! otherwise unreadable and unreachable for assistive tooling.
//!
//! The icon names are freedesktop symbolic names resolved from the Adwaita icon
//! theme, which ships with GTK and is already staged into the snap.

use gtk::prelude::*;

/// The size of the icons drawn inside the buttons, in pixels.
const ICON_SIZE: i32 = 16;

/// Build a flat, icon-only button with hover text.
///
/// `name` is a symbolic icon name from the freedesktop icon theme (for example
/// `edit-delete-symbolic`); `tooltip` is the hover text. The button is given the
/// `flat` class so it reads as a quiet, borderless control until hovered.
pub fn icon_button(name: &str, tooltip: &str) -> gtk::Button {
    let button = gtk::Button::from_icon_name(name);
    button.set_tooltip_text(Some(tooltip));
    button.add_css_class("flat");
    if let Some(image) = button.child().and_downcast::<gtk::Image>() {
        image.set_pixel_size(ICON_SIZE);
    }
    button
}
