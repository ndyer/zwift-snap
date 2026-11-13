use std::process::ExitCode;

#[cfg(feature = "gtk-ui")]
use std::path::PathBuf;
#[cfg(feature = "gtk-ui")]
use zwsnap_gui::settings::Settings;

fn main() -> ExitCode {
    // The launcher's own switches come before the optional "--" separator, everything
    // after it is handed to the game.
    let args: Vec<String> = std::env::args().skip(1).collect();
    let skip_login = args.iter().any(|arg| arg == "--skip-login");
    let args: Vec<String> = args
        .into_iter()
        .filter(|arg| arg != "--skip-login")
        .collect();
    match args.as_slice() {
        #[cfg(feature = "gtk-ui")]
        [cmd] if cmd == "--preview" => match zwsnap_gui::gui::preview(preview_mode()) {
            Ok(()) => ExitCode::SUCCESS,
            Err(error) => {
                eprintln!("{error}");
                ExitCode::FAILURE
            }
        },
        [] => launch(Vec::new(), skip_login),
        [cmd, rest @ ..] if cmd == "--" => launch(rest.to_vec(), skip_login),
        _ => {
            eprintln!("usage: zwsnap-gui [--skip-login] [-- GAME_ARGUMENTS...]");
            #[cfg(feature = "gtk-ui")]
            eprintln!("       zwsnap-gui --preview");
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

fn launch(args: Vec<String>, skip_login: bool) -> ExitCode {
    let result = zwsnap_gui::application::Config::from_env().and_then(|mut config| {
        config.skip_login = skip_login;
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
