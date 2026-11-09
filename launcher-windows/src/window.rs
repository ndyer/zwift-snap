//! Deciding whether an enumerated window is the one being waited for.

/// Whether a window belongs to `target_pid` and would actually be seen by the
/// user.
///
/// `width` and `height` are from `GetWindowRect`: wine creates helper windows
/// with an empty rectangle, which must not count as the game having a window.
pub fn is_game_window(
    owner_pid: u32,
    target_pid: u32,
    visible: bool,
    minimized: bool,
    width: i32,
    height: i32,
) -> bool {
    owner_pid == target_pid && visible && !minimized && width > 0 && height > 0
}

#[cfg(test)]
mod tests {
    use super::*;

    const GAME: u32 = 4242;

    #[test]
    fn a_visible_window_of_the_game_counts() {
        assert!(is_game_window(GAME, GAME, true, false, 1280, 720));
    }

    #[test]
    fn a_window_of_another_process_does_not_count() {
        assert!(!is_game_window(GAME + 1, GAME, true, false, 1280, 720));
    }

    #[test]
    fn a_hidden_or_minimized_window_does_not_count() {
        assert!(!is_game_window(GAME, GAME, false, false, 1280, 720));
        assert!(!is_game_window(GAME, GAME, true, true, 1280, 720));
    }

    /// Wine keeps a set of zero-sized helper windows for a process; a real
    /// window has a non-empty rectangle.
    #[test]
    fn an_empty_rectangle_does_not_count() {
        assert!(!is_game_window(GAME, GAME, true, false, 0, 720));
        assert!(!is_game_window(GAME, GAME, true, false, 1280, 0));
        assert!(!is_game_window(GAME, GAME, true, false, -1, -1));
    }
}
