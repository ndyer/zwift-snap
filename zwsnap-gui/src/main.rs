use std::process::ExitCode;

#[cfg(feature = "gtk-ui")]
use std::path::PathBuf;
#[cfg(feature = "gtk-ui")]
use zwsnap_gui::settings::Settings;

fn main() -> ExitCode {
    // A fatal signal would otherwise leave no trace inside a snap (see the
    // module docs); print a backtrace to stderr if the launcher crashes.
    zwsnap_gui::crash::install();
    // The launcher's own switches come before the optional "--" separator, everything
    // after it is handed to the game.
    let args: Vec<String> = std::env::args().skip(1).collect();
    let skip_login = args.iter().any(|arg| arg == "--skip-login");
    // Force the text UI mode for this run, overriding the saved setting. The Robot
    // suites use it so they can match the controls by OCR instead of by a screenshot.
    let text_ui = args.iter().any(|arg| arg == "--text-ui");
    let args: Vec<String> = args
        .into_iter()
        .filter(|arg| arg != "--skip-login" && arg != "--text-ui")
        .collect();
    match args.as_slice() {
        #[cfg(feature = "gtk-ui")]
        [cmd] if cmd == "--preview" => match zwsnap_gui::gui::preview(preview_mode(), text_ui) {
            Ok(()) => ExitCode::SUCCESS,
            Err(error) => {
                eprintln!("{error}");
                ExitCode::FAILURE
            }
        },
        [] => launch(Vec::new(), skip_login, text_ui),
        [cmd, rest @ ..] if cmd == "--" => launch(rest.to_vec(), skip_login, text_ui),
        _ => {
            eprintln!("usage: zwsnap-gui [--skip-login] [--text-ui] [-- GAME_ARGUMENTS...]");
            #[cfg(feature = "gtk-ui")]
            eprintln!("       zwsnap-gui --preview [--text-ui]");
            ExitCode::from(2)
        }
    }
}

/// The display mode for `--preview`, read from the settings file when it can be
/// located, otherwise the default.
#[cfg(feature = "gtk-ui")]
fn preview_mode() -> zwsnap_gui::display::DisplayMode {
    match (
        std::env::var_os("SNAP"),
        std::env::var_os("SNAP_USER_COMMON"),
    ) {
        (Some(snap), Some(common)) => {
            let snap = PathBuf::from(snap);
            let common = PathBuf::from(common);
            match zwsnap_gui::settings::ensure_initialized(&snap, &common) {
                Ok(path) => Settings::load(&path).display,
                Err(_) => Settings::default().display,
            }
        }
        _ => Settings::default().display,
    }
}

fn launch(args: Vec<String>, skip_login: bool, text_ui: bool) -> ExitCode {
    let result = zwsnap_gui::application::Config::from_env().and_then(|mut config| {
        config.skip_login = skip_login;
        config.text_ui |= text_ui;
        zwsnap_gui::application::launch(config, args)
    });
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("{error}");
            ExitCode::FAILURE
        }
    }
}
