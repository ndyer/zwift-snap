# Bundled icons

The launcher's icon-only controls draw these icons. They are vendored from
[Tabler Icons](https://tabler.io/icons) (outline set) and compiled into the
binary as a GResource (`resources/icons.gresource.xml`, built by `build.rs`),
so the launcher does not depend on any system icon theme.

The files are renamed `zwsnap-<name>-symbolic.svg`: the `zwsnap-` prefix keeps the
resource path from ever shadowing a desktop theme, and the `-symbolic` suffix is
what makes GTK recolor the glyph to the widget's foreground colour. The icon name
each control asks for (`zwsnap-refresh-symbolic`, ...) is defined in
`src/gui/icons.rs`.

Tabler Icons is released under the MIT licence, which permits redistribution, so
the icons can be shipped in the snap. Each file keeps a one-line comment naming
the upstream icon.

| file | upstream | used for |
| ---- | -------- | -------- |
| `zwsnap-refresh-symbolic.svg` | `refresh` | Retry |
| `zwsnap-plus-symbolic.svg` | `plus` | Add another account |
| `zwsnap-settings-symbolic.svg` | `settings` | Settings |
| `zwsnap-quit-symbolic.svg` | `logout` | Quit |
| `zwsnap-log-symbolic.svg` | `logs` | Show log |
| `zwsnap-remove-symbolic.svg` | `trash` | Remove account |
| `zwsnap-distance-symbolic.svg` | `route` | Distance chip |
| `zwsnap-elevation-symbolic.svg` | `mountain` | Elevation chip |
| `zwsnap-time-symbolic.svg` | `clock-pin` | Time chip |
