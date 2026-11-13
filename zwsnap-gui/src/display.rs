//! Shared launcher and game display mode.
//!
//! The mode is `fullscreen` (default) or `WIDTHxHEIGHT`, read from the settings
//! file. Zwift's own `prefs.xml` is the single source of truth for the window
//! size, so only its `<CONFIG>` section is edited and everything else is preserved.

use std::fmt;
use std::fs;
use std::path::Path;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DisplayMode {
    Fullscreen,
    Windowed { width: u32, height: u32 },
}

#[derive(Debug, PartialEq, Eq)]
pub enum DisplayError {
    InvalidMode(String),
    InvalidPrefs,
}

impl fmt::Display for DisplayError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            DisplayError::InvalidMode(m) => write!(
                f,
                "Invalid display mode: {m}. Use fullscreen or WIDTHxHEIGHT."
            ),
            DisplayError::InvalidPrefs => write!(f, "Unable to update display mode"),
        }
    }
}

impl std::error::Error for DisplayError {}

fn positive(s: &str) -> Option<u32> {
    if s.is_empty() || s.starts_with('0') || !s.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    s.parse().ok()
}

impl DisplayMode {
    pub fn parse(s: &str) -> Result<Self, DisplayError> {
        if s == "fullscreen" {
            return Ok(DisplayMode::Fullscreen);
        }
        s.split_once('x')
            .and_then(|(w, h)| {
                Some(DisplayMode::Windowed {
                    width: positive(w)?,
                    height: positive(h)?,
                })
            })
            .ok_or_else(|| DisplayError::InvalidMode(s.to_string()))
    }
}

impl fmt::Display for DisplayMode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            DisplayMode::Fullscreen => write!(f, "fullscreen"),
            DisplayMode::Windowed { width, height } => write!(f, "{width}x{height}"),
        }
    }
}

/// Position just after the opening tag `<name ...>` or `<name .../>` that starts at `at`
/// (`at` points to the `<`). Returns (end, self_closing).
fn open_tag(s: &str, at: usize, name: &str) -> Option<(usize, bool)> {
    let b = s.as_bytes();
    let mut i = at + 1 + name.len();
    if !s[at + 1..].starts_with(name) {
        return None;
    }
    while i < b.len() && b[i].is_ascii_whitespace() {
        i += 1;
    }
    match b.get(i)? {
        b'>' => Some((i + 1, false)),
        b'/' if b.get(i + 1) == Some(&b'>') => Some((i + 2, true)),
        _ => None,
    }
}

/// Finds the first opening tag `<name>` or `<name/>` at or after `from`: (start, end of the tag, self_closing).
fn find_open_tag(s: &str, name: &str, from: usize) -> Option<(usize, usize, bool)> {
    let pat = format!("<{name}");
    let mut pos = from;
    while let Some(rel) = s[pos..].find(&pat) {
        let start = pos + rel;
        if let Some((end, self_closing)) = open_tag(s, start, name) {
            return Some((start, end, self_closing));
        }
        pos = start + 1;
    }
    None
}

/// Finds the first `<name>` or `<name/>` at or after `from`: (start, end after the element, self_closing).
/// A non-empty element must hold only text and close with `</name>`.
fn find_element(s: &str, name: &str, from: usize) -> Option<(usize, usize, bool)> {
    let pat = format!("<{name}");
    let close = format!("</{name}>");
    let mut pos = from;
    while let Some(rel) = s[pos..].find(&pat) {
        let start = pos + rel;
        if let Some((end, self_closing)) = open_tag(s, start, name) {
            if self_closing {
                return Some((start, end, true));
            }
            let inner = &s[end..];
            let text_len = inner.find('<').unwrap_or(inner.len());
            if inner[text_len..].starts_with(&close) {
                return Some((start, end + text_len + close.len(), false));
            }
        }
        pos = start + 1;
    }
    None
}

/// Replaces every `<key>..</key>` and `<key/>` in `config` by `<key>value</key>`,
/// or adds it at the end of the section when there is none.
fn set_pref(config: &str, key: &str, value: &str) -> String {
    let tag = format!("<{key}>{value}</{key}>");
    let mut out = String::new();
    let mut pos = 0;
    let mut replaced = false;
    while let Some((start, end, _)) = find_element(config, key, pos) {
        out.push_str(&config[pos..start]);
        out.push_str(&tag);
        pos = end;
        replaced = true;
    }
    out.push_str(&config[pos..]);
    if replaced {
        return out;
    }
    match out.rfind("</CONFIG>") {
        Some(i) => format!("{}        {tag}\n    {}", &out[..i], &out[i..]),
        None => out,
    }
}

/// Returns `xml` with the display mode set in its `<CONFIG>` section.
/// A fresh install has `<CONFIG/>`, which is expanded; a missing section is created.
/// Fullscreen keeps an existing USER_RESOLUTION_PREF.
pub fn apply_mode(xml: &str, mode: &DisplayMode) -> Result<String, DisplayError> {
    let invalid = DisplayError::InvalidPrefs;
    if !xml.contains("</ZWIFT>")
        || !xml
            .match_indices("<ZWIFT")
            .any(|(i, _)| open_tag(xml, i, "ZWIFT").is_some_and(|t| !t.1))
    {
        return Err(invalid);
    }

    let mut xml = xml.to_string();
    while let Some((start, end, true)) = find_open_tag(&xml, "CONFIG", 0) {
        xml.replace_range(start..end, "<CONFIG></CONFIG>");
    }
    if find_open_tag(&xml, "CONFIG", 0).is_none() {
        let at = xml.find("</ZWIFT>").ok_or(DisplayError::InvalidPrefs)?;
        xml.insert_str(at, "    <CONFIG></CONFIG>\n");
    }

    let (start, open_end, _) =
        find_open_tag(&xml, "CONFIG", 0).ok_or(DisplayError::InvalidPrefs)?;
    let close = xml[start..]
        .find("</CONFIG>")
        .ok_or(DisplayError::InvalidPrefs)?
        + start
        + "</CONFIG>".len();
    debug_assert!(open_end <= close);

    let (fullscreen, resolution) = match mode {
        DisplayMode::Fullscreen => ("1", None),
        DisplayMode::Windowed { width, height } => ("0", Some(format!("{width}x{height}"))),
    };
    let mut config = set_pref(&xml[start..close], "FULLSCREEN", fullscreen);
    if let Some(res) = resolution {
        config = set_pref(&config, "USER_RESOLUTION_PREF", &res);
    }
    Ok(format!("{}{config}{}", &xml[..start], &xml[close..]))
}

/// Applies the mode to the prefs file. The file is replaced atomically and stays
/// untouched when it cannot be updated.
pub fn configure_file(prefs: &Path, mode: &DisplayMode) -> Result<(), Box<dyn std::error::Error>> {
    let xml = match fs::read_to_string(prefs) {
        Ok(xml) => xml,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            if let Some(parent) = prefs.parent() {
                fs::create_dir_all(parent)?;
            }
            "<ZWIFT><CONFIG/></ZWIFT>".to_string()
        }
        Err(error) => return Err(error.into()),
    };
    let updated = apply_mode(&xml, mode)?;
    let mut tmp = prefs.as_os_str().to_owned();
    tmp.push(format!(".{}.tmp", std::process::id()));
    fs::write(&tmp, updated)?;
    if let Err(e) = fs::rename(&tmp, prefs) {
        let _ = fs::remove_file(&tmp);
        return Err(e.into());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fs_res(xml: &str, mode: &DisplayMode) -> String {
        apply_mode(xml, mode).unwrap()
    }

    fn win(w: u32, h: u32) -> DisplayMode {
        DisplayMode::Windowed {
            width: w,
            height: h,
        }
    }

    #[test]
    fn parse_modes() {
        assert_eq!(
            DisplayMode::parse("fullscreen"),
            Ok(DisplayMode::Fullscreen)
        );
        assert_eq!(DisplayMode::parse("1920x1080"), Ok(win(1920, 1080)));
        for bad in [
            "garbage",
            "0x10",
            "10x0",
            "01x10",
            "1920X1080",
            "1920x",
            "x10",
            "-1x5",
            "1x2x3",
            "",
        ] {
            assert!(DisplayMode::parse(bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn expands_self_closing_config() {
        let out = fs_res(
            "<ZWIFT>\r\n  <CONFIG/>\r\n</ZWIFT>\r\n",
            &DisplayMode::Fullscreen,
        );
        assert!(out.contains("<FULLSCREEN>1</FULLSCREEN>"));
        assert!(!out.contains("<CONFIG/>"));
        let out = fs_res("<ZWIFT><CONFIG /></ZWIFT>\n", &win(1920, 1080));
        assert!(
            out.contains("<FULLSCREEN>0</FULLSCREEN>")
                && out.contains("<USER_RESOLUTION_PREF>1920x1080</USER_RESOLUTION_PREF>")
        );
    }

    #[test]
    fn creates_missing_config_and_keeps_rest() {
        let out = fs_res(
            "<ZWIFT><WORKOUTS><USE_ERG>1</USE_ERG></WORKOUTS></ZWIFT>\n",
            &DisplayMode::Fullscreen,
        );
        assert!(out.contains("<USE_ERG>1</USE_ERG>"));
        assert!(out.contains("<CONFIG>") && out.contains("<FULLSCREEN>1</FULLSCREEN>"));
    }

    #[test]
    fn fullscreen_keeps_resolution_and_is_idempotent() {
        let xml = "<ZWIFT><CONFIG><FULLSCREEN>0</FULLSCREEN><USER_RESOLUTION_PREF>1920x1080</USER_RESOLUTION_PREF><LANGUAGE>en</LANGUAGE></CONFIG></ZWIFT>\n";
        let once = fs_res(xml, &DisplayMode::Fullscreen);
        assert_eq!(
            once,
            "<ZWIFT><CONFIG><FULLSCREEN>1</FULLSCREEN><USER_RESOLUTION_PREF>1920x1080</USER_RESOLUTION_PREF><LANGUAGE>en</LANGUAGE></CONFIG></ZWIFT>\n"
        );
        assert_eq!(fs_res(&once, &DisplayMode::Fullscreen), once);
    }

    #[test]
    fn custom_resolution_updates_existing() {
        let xml = "<ZWIFT><CONFIG><FULLSCREEN>1</FULLSCREEN><USER_RESOLUTION_PREF>1280x720</USER_RESOLUTION_PREF></CONFIG></ZWIFT>\n";
        let out = fs_res(xml, &win(1920, 1080));
        assert!(out.contains("<FULLSCREEN>0</FULLSCREEN>"));
        assert!(out.contains("<USER_RESOLUTION_PREF>1920x1080</USER_RESOLUTION_PREF>"));
        assert!(!out.contains("1280x720"));
    }

    #[test]
    fn similar_tag_names_are_not_touched() {
        let xml = "<ZWIFT><CONFIG><FULLSCREEN_X>5</FULLSCREEN_X></CONFIG></ZWIFT>";
        let out = fs_res(xml, &DisplayMode::Fullscreen);
        assert!(
            out.contains("<FULLSCREEN_X>5</FULLSCREEN_X>")
                && out.contains("<FULLSCREEN>1</FULLSCREEN>")
        );
    }

    #[test]
    fn invalid_prefs_are_rejected() {
        assert_eq!(
            apply_mode("<ZWIFT><CONFIG>\n", &DisplayMode::Fullscreen),
            Err(DisplayError::InvalidPrefs)
        );
        assert_eq!(
            apply_mode("not xml", &DisplayMode::Fullscreen),
            Err(DisplayError::InvalidPrefs)
        );
        // an unclosed CONFIG inside a valid ZWIFT element
        assert_eq!(
            apply_mode("<ZWIFT><CONFIG></ZWIFT>", &DisplayMode::Fullscreen),
            Err(DisplayError::InvalidPrefs)
        );
    }

    #[test]
    fn invalid_file_is_left_untouched() {
        let dir = std::env::temp_dir().join(format!("zwsnap-gui-prefs-{}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        let p = dir.join("prefs.xml");
        fs::write(&p, "<ZWIFT><CONFIG>\n").unwrap();
        assert!(configure_file(&p, &DisplayMode::Fullscreen).is_err());
        assert_eq!(fs::read_to_string(&p).unwrap(), "<ZWIFT><CONFIG>\n");
        assert_eq!(fs::read_dir(&dir).unwrap().count(), 1);
    }
}
