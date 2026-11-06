use std::path::PathBuf;
use std::process::ExitCode;

use zwift_gui::display::{configure_file, DisplayMode};

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match args.as_slice() {
        #[cfg(feature = "gtk-ui")]
        [cmd] if cmd == "--preview" => {
            let mode = match DisplayMode::from_env() {
                Ok(mode) => mode,
                Err(error) => {
                    eprintln!("{error}");
                    return ExitCode::FAILURE;
                }
            };
            match zwift_gui::gui::preview(mode) {
                Ok(()) => ExitCode::SUCCESS,
                Err(error) => {
                    eprintln!("{error}");
                    ExitCode::FAILURE
                }
            }
        }
        [cmd, prefs] if cmd == "set-display-mode" => {
            let result = DisplayMode::from_env()
                .map_err(|e| e.to_string())
                .and_then(|mode| {
                    configure_file(&PathBuf::from(prefs), &mode)
                        .map(|_| mode)
                        .map_err(|e| e.to_string())
                });
            match result {
                Ok(mode) => {
                    println!("Set Zwift display mode to {mode}");
                    ExitCode::SUCCESS
                }
                Err(e) => {
                    eprintln!("{e}");
                    ExitCode::FAILURE
                }
            }
        }
        [] => launch(Vec::new()),
        [cmd, rest @ ..] if cmd == "--" => launch(rest.to_vec()),
        _ => {
            eprintln!(
                "usage: zwift-gui set-display-mode PREFS_FILE (mode from ZWIFT_DISPLAY_MODE)"
            );
            #[cfg(feature = "gtk-ui")]
            eprintln!("       zwift-gui --preview");
            eprintln!("       zwift-gui [-- GAME_ARGUMENTS...]");
            ExitCode::from(2)
        }
    }
}

fn launch(args: Vec<String>) -> ExitCode {
    match zwift_gui::application::Config::from_env()
        .and_then(|config| zwift_gui::application::launch(config, args))
    {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("{error}");
            ExitCode::FAILURE
        }
    }
}
