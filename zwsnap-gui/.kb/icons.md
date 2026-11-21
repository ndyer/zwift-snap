# Preface

How the launcher's icons and its icon-vs-text UI mode work: the bundled Tabler
icon set (a GResource compiled into the binary), the icon/label button helpers,
and the `text_ui` mode that draws controls as text for accessibility and for the
Robot suites. Read this before changing a button's icon, its label, or anything
that assumes a system icon theme.

IMPORTANT: Read the `.kb/agents.md` file.


# Laws

## §1 - Icons Are Bundled, Not From A Theme

The launcher names no system icon theme. Its icons are vendored from Tabler
(MIT) in `zwsnap-gui/resources/icons/` (see that directory's `README.md`), listed
in `resources/icons.gresource.xml`, compiled into the binary by `build.rs`, and
registered at startup by `icons::init` (which calls
`gtk::gio::resources_register_include!` and `IconTheme::add_resource_path`). The
snap drops the transitively-staged `adwaita-icon-theme` for this reason. Never
reintroduce a freedesktop icon name (`edit-delete-symbolic`, ...); add a bundled
icon and a constant in `gui/icons.rs` instead.

## §2 - The Resource Layout Is Flat

GTK treats files placed *directly* in a resource path as the hicolor theme's
ultimate fallback; files in size/category subdirectories would need a
`scalable/actions/` style layout. The icons therefore sit flat at the prefix root
(`/com/github/ndyer/ZwiftSnap/icons/zwsnap-<name>-symbolic.svg`), and the
`-symbolic` suffix is what makes GTK recolor the glyph to the widget foreground.
Adding a subdirectory to `resources/icons.gresource.xml` breaks the lookup.

## §3 - Every Control Carries A Label

An icon-only control is unreadable without hover text and unreachable for
assistive tooling, so `icon_button(name, label, tooltip)` takes both an icon name
and a short `label`. In icon mode the label is the accessible name
(`update_property(&[Property::Label(..)])`); in text mode it replaces the icon.
Keep the label short and distinct so it is unmistakeable to OCR: the card's
`Remove` never collides with the dialog's "Remove account", and the log toggle
reads `Show log`, not a bare `Log`.


# Concepts

## The UI mode

`icons::UiMode` is `Icons` (the default) or `Text`. The process-wide mode is set
once by `icons::init(mode)`, called from `gui::run_application` right after
`gtk::init`, so widget builders (`icon_button`, `toggle_button`, and
`accounts_panel::chip`) need no mode argument - they read it through
`icons::text_mode()`. The mode comes from `settings.json`'s `text_ui` (a
Settings-dialog checkbox) or the `--text-ui` command-line flag, which forces it
on for one run; `main.rs` parses the flag, `application::Config` carries it into
the `SettingsContext`, and `preview` folds it into the preview settings.

## Where it is used

- `gui.rs`: Retry (`zwsnap-refresh`), Settings (`zwsnap-settings`), and the
  fullscreen Quit (`zwsnap-quit`) buttons.
- `gui/accounts_panel.rs`: the add-account button (`zwsnap-plus`), the per-card
  remove control (`zwsnap-remove`), and the three stat chips, which use
  `icons::icon(name, size)` inline (they are not buttons).
- `gui/log_panel.rs`: the log toggle (`zwsnap-log`), built with `toggle_button`.

Icon names and labels are the `pub const`s in `gui/icons.rs`; `ALL` lists the
names for the `all_bundled_icons_resolve` test (which asserts every bundled icon
resolves through the registered resource, and runs in the `gtk-preview` harness).

## Why the tests use it

The Robot suites (`tests/smoke/`, `tests/gtk-preview/`) run the launcher with
`--text-ui` and match the controls by their text with OCR (`Match Text`/`Click
Target`) instead of carrying PNG icon templates - so an icon or theme change no
longer needs a template recaptured (`tests/smoke/README.md`).
