//! Building the command line of the child process `ZwiftLauncher.exe` starts.
//!
//! The arguments are copied through verbatim, without quoting or escaping, and
//! that is deliberate. `ZwiftApp.exe` does not parse `--token=<json>` through a
//! CRT argv parser: it reads the raw command line and takes everything after
//! `--token=`, so the JSON's own double quotes have to appear in that line as
//! real quotes.
//!
//! `wine start` — which the launcher uses — does the opposite. Its
//! `build_command_line()` re-quotes every argument, so the token arrives here as
//! `"--token={\"access_token\":...}"`. Passing that tail of our own command line
//! straight on made the game log
//! `[LoginFlow] Error message: [23] Error parsing json` and show its own login
//! form instead of logging in with the token. Asking for our own argv — which
//! the CRT has already unescaped back into real quotes — and rebuilding the
//! child's line from it is what undoes that.

/// Build a child command line from its image name and arguments.
///
/// An argument with no blank in it is copied verbatim, which is what keeps the
/// `--token` JSON readable by the game. Only an argument that contains a blank
/// (a user's own `zwift -- …` game argument, never the token) is quoted, so that
/// it stays a single argument. The image itself is resolved with `SearchPathW`,
/// so quoting it cannot affect which program runs.
pub fn child<S: AsRef<str>>(name: &str, args: &[S]) -> String {
    let mut line = String::new();
    push_argument(&mut line, name);
    for argument in args {
        line.push(' ');
        push_argument(&mut line, argument.as_ref());
    }
    line
}

/// Append one argument, quoting it with the MSVCRT rules only if it has to be.
fn push_argument(line: &mut String, argument: &str) {
    if !argument.is_empty() && !argument.contains([' ', '\t']) {
        line.push_str(argument);
        return;
    }
    // The CRT quotes when the argument has a blank, so it has to unquote it again.
    line.push('"');
    let mut backslashes = 0;
    for character in argument.chars() {
        match character {
            // A backslash only needs doubling when a quote follows it, so they are
            // counted here and written out when that is known.
            '\\' => backslashes += 1,
            '"' => {
                // An odd count leaves the quote itself unescaped in the result.
                line.extend(std::iter::repeat_n('\\', 2 * backslashes + 1));
                line.push('"');
                backslashes = 0;
            }
            _ => {
                line.extend(std::iter::repeat_n('\\', backslashes));
                line.push(character);
                backslashes = 0;
            }
        }
    }
    // Backslashes before the closing quote must be doubled, or they escape it.
    line.extend(std::iter::repeat_n('\\', 2 * backslashes));
    line.push('"');
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_image_on_its_own_is_left_unquoted() {
        assert_eq!(child("ZwiftApp.exe", &[] as &[&str]), "ZwiftApp.exe");
    }

    #[test]
    fn arguments_are_separated_by_single_spaces() {
        assert_eq!(
            child("ZwiftApp.exe", &["--fullscreen", "--no-launcher"]),
            "ZwiftApp.exe --fullscreen --no-launcher"
        );
    }

    #[test]
    fn an_image_name_with_a_blank_is_quoted() {
        assert_eq!(
            child(r"C:\Program Files (x86)\Zwift\ZwiftApp.exe", &["--token=1"]),
            r#""C:\Program Files (x86)\Zwift\ZwiftApp.exe" --token=1"#
        );
    }

    /// The regression this module exists for: `wine start` hands the token over
    /// escaped, and the game needs it back with real quotes or it logs
    /// `[23] Error parsing json` and shows its own login form.
    #[test]
    fn a_json_token_is_passed_through_with_real_quotes() {
        let token = r#"--token={"access_token":"eyJhbGci","token_type":"bearer"}"#;
        let line = child("ZwiftApp.exe", &[token]);
        assert_eq!(line, format!("ZwiftApp.exe {token}"));
        assert!(!line.contains(r#"\""#));
    }

    /// Compact JSON from the launcher's serialiser, as it actually arrives.
    #[test]
    fn the_launcher_token_shape_survives_unchanged() {
        let token = concat!(
            r#"--token={"access_token":"eyJhbGciOiAiUlMyNTYiLCJ0eXAiOiAiSldUIn0"#,
            r#"","expires_in":21600,"refresh_token":"ZmFrZQ","token_type":"bearer","#,
            r#""id_token":"ZmFrZQ","not-before-policy":0,"scope":"openid"}"#
        );
        assert_eq!(
            child("ZwiftApp.exe", &[token]),
            format!("ZwiftApp.exe {token}")
        );
    }

    /// A user's own `zwift -- …` argument, which must stay one argument.
    #[test]
    fn an_argument_with_a_blank_is_quoted() {
        assert_eq!(
            child("ZwiftApp.exe", &["--player-name=a b"]),
            r#"ZwiftApp.exe "--player-name=a b""#
        );
        assert_eq!(child("ZwiftApp.exe", &[""]), r#"ZwiftApp.exe """#);
    }

    /// The CRT rules: a backslash is only doubled in front of a quote, and the
    /// ones before the closing quote always are.
    #[test]
    fn backslashes_before_a_quote_are_doubled() {
        assert_eq!(
            child("ZwiftApp.exe", &[r#"a "b" c"#]),
            r#"ZwiftApp.exe "a \"b\" c""#
        );
        assert_eq!(
            child("ZwiftApp.exe", &[r"C:\dir with space\"]),
            r#"ZwiftApp.exe "C:\dir with space\\""#
        );
    }
}
