//! Keep the desktop from blanking while a fullscreen game session runs.
//!
//! Nothing else inhibits the screensaver: the launcher only edits Zwift's
//! `prefs.xml` and never owns the game window, and Zwift itself is a Windows
//! program under Wine that never asks the session for an inhibitor. A fullscreen
//! session therefore lets the host blank and lock the screen mid-ride.
//!
//! The snap declares (and connects) `screen-inhibit-control`, which allows a
//! confined app to call `org.freedesktop.ScreenSaver.Inhibit` and
//! `org.gnome.SessionManager.Inhibit`, so the launcher takes one of those for the
//! life of the game and releases it afterwards. Inhibition is best-effort: a
//! desktop that offers neither service still gets a working game, and the reason
//! is logged rather than raised.
//!
//! The D-Bus calls need `gio`, which comes with the `gtk-ui` feature. A launcher
//! built without it reports that inhibition is unavailable instead of failing to
//! compile, so call sites need no feature gates.

use crate::display::DisplayMode;
use crate::logging::Logger;

#[cfg(feature = "gtk-ui")]
use gtk::prelude::*;

/// Application name and reason, which desktops show in their own inhibitor UI.
#[cfg(feature = "gtk-ui")]
const APPLICATION: &str = "Zwift";
#[cfg(feature = "gtk-ui")]
const REASON: &str = "Zwift is running fullscreen";

/// An inhibitor call may not hold up the launch: the session bus is local, so a
/// hung service is given up on rather than waited for.
#[cfg(feature = "gtk-ui")]
const CALL_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(2);

/// `org.gnome.SessionManager.Inhibit` flags: 4 inhibits session logout, 8
/// inhibits the screensaver and idle blanking. Both are wanted for a game.
#[cfg(feature = "gtk-ui")]
const GNOME_INHIBIT_FLAGS: u32 = 4 | 8;

/// A held inhibitor. [`ScreenInhibitor::release`] returns it, and dropping the
/// value releases the cookie as well.
pub struct ScreenInhibitor {
    backend: Backend,
}

enum Backend {
    /// A cookie handed out by `Inhibit`, with what is needed to return it. The
    /// service name rides along for the logs.
    #[cfg(feature = "gtk-ui")]
    Held {
        proxy: gtk::gio::DBusProxy,
        service: &'static str,
        uninhibit: &'static str,
        cookie: u32,
    },
    /// No inhibition: the game is windowed, or the desktop has no usable service.
    Inactive,
}

impl ScreenInhibitor {
    /// Inhibit idle blanking for as long as the returned value lives.
    ///
    /// Only a fullscreen session is inhibited. A windowed game leaves part of the
    /// desktop in view and the user keeps interacting with the session, so holding
    /// the screen awake there would be surprising. Acquisition never fails: an
    /// unavailable service is logged and the launcher carries on.
    pub fn acquire(display: &DisplayMode, logger: &Logger) -> Self {
        if *display != DisplayMode::Fullscreen {
            logger.debug("Not inhibiting the screensaver: Zwift is not fullscreen");
            return Self::inactive();
        }
        #[cfg(feature = "gtk-ui")]
        {
            Self::acquire_from_desktop(logger)
        }
        #[cfg(not(feature = "gtk-ui"))]
        {
            logger.debug("Screensaver inhibition needs a launcher built with the gtk-ui feature");
            Self::inactive()
        }
    }

    fn inactive() -> Self {
        Self {
            backend: Backend::Inactive,
        }
    }

    /// Whether an inhibitor is held.
    pub fn is_active(&self) -> bool {
        !matches!(self.backend, Backend::Inactive)
    }

    /// Return the inhibitor, logging the release.
    ///
    /// The launcher calls this before tearing the wine server down so the release
    /// is ordered ahead of the game's own cleanup; dropping the value releases the
    /// cookie as well, without a log line.
    pub fn release(&mut self, logger: &Logger) {
        #[cfg(feature = "gtk-ui")]
        {
            if let Backend::Held { service, .. } = &self.backend {
                logger.debug(&format!("Releasing the {service} screensaver inhibitor"));
            }
        }
        #[cfg(not(feature = "gtk-ui"))]
        {
            let _logger = logger; // Silence unused variable warning
        }
        self.backend = Backend::Inactive;
    }

    /// `org.freedesktop.ScreenSaver` first, then `org.gnome.SessionManager`.
    ///
    /// These are the two interfaces `screen-inhibit-control` allows. KDE, Xfce,
    /// wlroots compositors and Mir implement the freedesktop one, which is why it
    /// is tried first; GNOME keeps its own SessionManager interface from before the
    /// freedesktop name existed.
    #[cfg(feature = "gtk-ui")]
    fn acquire_from_desktop(logger: &Logger) -> Self {
        let mut failures = Vec::new();
        for attempt in [Backend::screensaver(logger), Backend::gnome_session(logger)] {
            match attempt {
                Ok(backend) => return Self { backend },
                Err(reason) => failures.push(reason),
            }
        }
        logger.debug(&format!(
            "The desktop offers no screensaver inhibitor ({}); the screen may blank",
            failures.join("; ")
        ));
        Self::inactive()
    }
}

#[cfg(feature = "gtk-ui")]
impl Backend {
    fn screensaver(logger: &Logger) -> Result<Self, String> {
        Self::inhibit(
            "org.freedesktop.ScreenSaver",
            "/org/freedesktop/ScreenSaver",
            "Inhibit",
            "UnInhibit",
            &(APPLICATION, REASON).to_variant(),
            logger,
        )
    }

    fn gnome_session(logger: &Logger) -> Result<Self, String> {
        Self::inhibit(
            "org.gnome.SessionManager",
            "/org/gnome/SessionManager",
            "Inhibit",
            "Uninhibit",
            &(APPLICATION, 0u32, REASON, GNOME_INHIBIT_FLAGS).to_variant(),
            logger,
        )
    }

    /// Take one cookie from `service`.
    ///
    /// `DO_NOT_AUTO_START` stops the launcher from D-Bus-activating a service the
    /// session does not already run, and it turns "no such service" into an
    /// immediate error instead of an activation timeout.
    fn inhibit(
        service: &'static str,
        path: &str,
        inhibit: &str,
        uninhibit: &'static str,
        arguments: &gtk::glib::Variant,
        logger: &Logger,
    ) -> Result<Self, String> {
        let proxy = gtk::gio::DBusProxy::for_bus_sync(
            gtk::gio::BusType::Session,
            gtk::gio::DBusProxyFlags::DO_NOT_AUTO_START
                | gtk::gio::DBusProxyFlags::DO_NOT_LOAD_PROPERTIES,
            None,
            service,
            path,
            service,
            gtk::gio::Cancellable::NONE,
        )
        .map_err(|error| format!("{service}: {error}"))?;
        let reply = proxy
            .call_sync(
                inhibit,
                Some(arguments),
                gtk::gio::DBusCallFlags::NONE,
                CALL_TIMEOUT.as_millis() as i32,
                gtk::gio::Cancellable::NONE,
            )
            .map_err(|error| format!("{service}: {error}"))?;
        // A method reply is a tuple of the out arguments, even for one `u`.
        let cookie = reply
            .get::<(u32,)>()
            .map(|(cookie,)| cookie)
            .ok_or_else(|| format!("{service}: {inhibit} returned no cookie"))?;
        logger.debug(&format!(
            "Inhibiting the screensaver through {service} (cookie {cookie})"
        ));
        Ok(Self::Held {
            proxy,
            service,
            uninhibit,
            cookie,
        })
    }
}

#[cfg(feature = "gtk-ui")]
impl Drop for Backend {
    fn drop(&mut self) {
        let Self::Held {
            proxy,
            uninhibit,
            cookie,
            ..
        } = self
        else {
            return;
        };
        // There is nobody to report a failure to: the launcher is exiting, and the
        // desktop drops the inhibitor with the session bus connection even if this
        // call does not arrive. The parameters still have to be a tuple.
        let _ = proxy.call_sync(
            uninhibit,
            Some(&(*cookie,).to_variant()),
            gtk::gio::DBusCallFlags::NONE,
            CALL_TIMEOUT.as_millis() as i32,
            gtk::gio::Cancellable::NONE,
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    struct Fixture {
        dir: PathBuf,
        logger: Logger,
    }

    fn fixture(name: &str) -> Fixture {
        let dir = std::env::temp_dir().join(format!("zwift-inhibit-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let logger = Logger::new(dir.join("log"), 0, false);
        // The logger only appends to a log file that already exists.
        logger.init().unwrap();
        Fixture { dir, logger }
    }

    #[test]
    fn windowed_sessions_are_not_inhibited() {
        let fixture = fixture("windowed");
        let display = DisplayMode::Windowed {
            width: 1280,
            height: 720,
        };
        let inhibitor = ScreenInhibitor::acquire(&display, &fixture.logger);
        assert!(!inhibitor.is_active());
        drop(inhibitor);
        let log = std::fs::read_to_string(fixture.dir.join("log")).unwrap_or_default();
        assert!(log.contains("Not inhibiting the screensaver"), "{log}");
    }

    /// The release path must be safe on an inactive inhibitor, because the
    /// launcher calls it unconditionally before tearing the wine server down.
    #[test]
    fn releasing_an_inactive_inhibitor_is_harmless() {
        let fixture = fixture("inactive");
        let mut inhibitor = ScreenInhibitor::acquire(
            &DisplayMode::Windowed {
                width: 800,
                height: 600,
            },
            &fixture.logger,
        );
        inhibitor.release(&fixture.logger);
        assert!(!inhibitor.is_active());
    }
}
