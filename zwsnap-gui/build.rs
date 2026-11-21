//! Compiles the bundled icon set into a GResource the binary carries.
//!
//! The launcher ships its own icons (see `resources/icons/README.md`) so it does
//! not depend on a system icon theme. `glib-compile-resources` is only needed for
//! the GTK UI, so the work is skipped for builds without the `gtk-ui` feature. The
//! generated bundle is brought into the binary by `icons::init`, which calls
//! `gtk::gio::resources_register_include!("icons.gresource")`.

fn main() {
    println!("cargo:rerun-if-changed=resources/icons.gresource.xml");
    println!("cargo:rerun-if-changed=resources/icons");
    // `icons.rs` (which registers the resource) only exists with `gtk-ui`, so
    // nothing needs the compiled resource in a terminal-only build.
    if std::env::var_os("CARGO_FEATURE_GTK_UI").is_none() {
        return;
    }

    glib_build_tools::compile_resources(
        &["resources/icons"],
        "resources/icons.gresource.xml",
        "icons.gresource",
    );
}
