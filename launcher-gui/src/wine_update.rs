//! Wine and wget adapter for the update state machine.

use crate::logging::Logger;
use crate::process::run_output_timeout;
use crate::update::UpdateBackend;
use crate::version::{current_version, parse_sversion};
use std::io;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::time::Duration;

const VERSION_URL: &str = "https://cdn.zwift.com/gameassets/Zwift_Updates_Root/Zwift_ver_cur.xml";

/// Executable names can be replaced with stubs without changing the environment.
pub struct Tools {
    pub wine: PathBuf,
    pub winepath: PathBuf,
    pub wineserver: PathBuf,
    pub wget: PathBuf,
}

impl Default for Tools {
    fn default() -> Self {
        Self {
            wine: "wine".into(),
            winepath: "winepath".into(),
            wineserver: "wineserver".into(),
            wget: "wget".into(),
        }
    }
}

pub struct WineUpdate<'a> {
    pub install_dir: &'a Path,
    pub wine_prefix: Option<&'a Path>,
    pub tools: &'a Tools,
    pub logger: &'a Logger,
    pub on_progress: &'a mut dyn FnMut(u8, &str),
}

fn checked_output(logger: &Logger, command: &mut Command, name: &str) -> io::Result<Output> {
    let output = logger.command_output(command)?;
    if !output.status.success() {
        return Err(io::Error::other(format!(
            "{name} failed ({}): {}",
            output.status,
            String::from_utf8_lossy(&output.stderr).trim()
        )));
    }
    Ok(output)
}

impl UpdateBackend for WineUpdate<'_> {
    type Error = io::Error;

    fn latest_version(&mut self) -> io::Result<String> {
        let output = checked_output(
            self.logger,
            Command::new(&self.tools.wget).args(["--no-cache", "--quiet", "-O", "-", VERSION_URL]),
            "wget",
        )?;
        let xml = std::str::from_utf8(&output.stdout)
            .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))?;
        parse_sversion(xml)
            .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, "missing Zwift sversion"))
    }

    fn current_version(&mut self) -> io::Result<String> {
        Ok(current_version(self.install_dir))
    }

    fn start_launcher(&mut self) -> io::Result<()> {
        let mut winepath = Command::new(&self.tools.winepath);
        if let Some(prefix) = self.wine_prefix {
            winepath.env("WINEPREFIX", prefix);
        }
        let output = checked_output(
            self.logger,
            winepath.arg("-w").arg(self.install_dir),
            "winepath",
        )?;
        let path = std::str::from_utf8(&output.stdout)
            .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))?
            .trim();
        if path.is_empty() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "winepath returned an empty path",
            ));
        }
        let mut wine = Command::new(&self.tools.wine);
        if let Some(prefix) = self.wine_prefix {
            wine.env("WINEPREFIX", prefix);
        }
        let status = self.logger.run_inherited_output(wine.args([
            "start",
            "/d",
            path,
            "ZwiftLauncher.exe",
            "SilentLaunch",
        ]))?;
        if !status.success() {
            return Err(io::Error::other(format!("wine start failed ({status})")));
        }
        Ok(())
    }

    fn launcher_running(&mut self) -> io::Result<bool> {
        let mut wine = Command::new(&self.tools.wine);
        if let Some(prefix) = self.wine_prefix {
            wine.env("WINEPREFIX", prefix);
        }
        let output = checked_output(
            self.logger,
            wine.args([
                "tasklist",
                "/fo",
                "list",
                "/fi",
                "IMAGENAME eq ZwiftLauncher.exe",
            ]),
            "wine tasklist",
        )?;
        Ok(!output.stdout.is_empty())
    }

    fn stop_launcher(&mut self) -> io::Result<()> {
        for task in ["ZwiftLauncher.exe", "ZwiftWindowsCrashHandler.exe"] {
            let mut wine = Command::new(&self.tools.wine);
            if let Some(prefix) = self.wine_prefix {
                wine.env("WINEPREFIX", prefix);
            }
            let result = self
                .logger
                .command_output(wine.args(["taskkill", "/f", "/im", task]));
            match result {
                Ok(output) if output.status.success() => {}
                Ok(output) => self.logger.warning(&format!(
                    "Unable to stop {task}: {}",
                    String::from_utf8_lossy(&output.stderr).trim()
                )),
                Err(error) => self
                    .logger
                    .warning(&format!("Unable to stop {task}: {error}")),
            }
        }

        let mut server = Command::new(&self.tools.wineserver);
        if let Some(prefix) = self.wine_prefix {
            server.env("WINEPREFIX", prefix);
        }
        let output = run_output_timeout(server.arg("-k"), Duration::from_secs(10))?;
        if !output.status.success() {
            return Err(io::Error::other(format!(
                "wineserver -k failed ({}): {}",
                output.status,
                String::from_utf8_lossy(&output.stderr).trim()
            )));
        }
        Ok(())
    }

    fn sleep(&mut self, duration: Duration) -> io::Result<()> {
        self.logger.cancellation().wait(duration)
    }

    fn progress(&mut self, percent: u8, message: &str) {
        (self.on_progress)(percent, message);
    }

    fn message(&mut self, message: &str) {
        self.logger.info(message);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::update::{update_if_needed, UpdateOutcome};
    use std::fs;
    use std::os::unix::fs::PermissionsExt;

    fn script(path: &Path, contents: &str) {
        fs::write(path, format!("#!/bin/sh\nset -eu\n{contents}\n")).unwrap();
        fs::set_permissions(path, fs::Permissions::from_mode(0o755)).unwrap();
    }

    #[test]
    fn updater_runs_wine_tools_and_streams_output() {
        let _lock = crate::SCRIPT_TEST_LOCK.lock().unwrap();
        let dir = std::env::temp_dir().join(format!("zwift-wine-update-{}", std::process::id()));
        fs::create_dir(&dir).unwrap();
        let install = dir.join("install");
        fs::create_dir(&install).unwrap();
        fs::write(
            install.join("Zwift_ver_cur.xml"),
            r#"<Zwift sversion="1.100.0 old"/>"#,
        )
        .unwrap();
        let wine = dir.join("wine");
        let winepath = dir.join("winepath");
        let wineserver = dir.join("wineserver");
        let wget = dir.join("wget");
        script(
            &wine,
            &format!(
                "case \"$1\" in\n\
                 start) (i=0; while [ ! -f '{0}/release' ]; do\n\
                   i=$((i + 1)); if [ \"$i\" -ge 200 ]; then : > '{0}/timed-out'; exit 1; fi; sleep 0.05;\n\
                   done; printf 'wine descendant output\\n'; : > '{0}/done') &\n\
                   printf '<Zwift sversion=\"1.125.0 new\"/>' > '{1}';;\n\
                 tasklist) : > '{0}/release'; printf 'PID: 123\\n';;\n\
                 taskkill) :;;\n\
                 *) exit 1;;\n\
                 esac",
                dir.display(),
                install.join("Zwift_ver_cur.xml").display(),
            ),
        );
        script(&winepath, "printf 'C:\\\\Zwift\\n'");
        script(
            &wineserver,
            &format!(
                "test \"$1\" = -k\n\
                 i=0; while [ ! -f '{0}/done' ]; do\n\
                   i=$((i + 1)); [ \"$i\" -lt 200 ] || exit 1; sleep 0.05;\n\
                 done",
                dir.display()
            ),
        );
        script(&wget, "printf '<Zwift sversion=\"1.124.0 latest\"/>'");
        let tools = Tools {
            wine,
            winepath,
            wineserver,
            wget,
        };
        let logger = Logger::new(dir.join("zwift.log"), 0, false);
        logger.init().unwrap();
        let mut progress = Vec::new();
        let mut on_progress = |percent, _: &str| progress.push(percent);
        let mut backend = WineUpdate {
            install_dir: &install,
            wine_prefix: None,
            tools: &tools,
            logger: &logger,
            on_progress: &mut on_progress,
        };
        assert_eq!(
            update_if_needed(&mut backend, 2, Duration::ZERO).unwrap(),
            UpdateOutcome::Updated("1.125.0".into())
        );
        assert_eq!(progress, [5, 10, 50]);
        logger.finish_inherited_output().unwrap();
        assert!(fs::read_to_string(logger.path())
            .unwrap()
            .contains("wine descendant output"));
        assert!(!dir.join("timed-out").exists());
        fs::remove_dir_all(dir).unwrap();
    }
}
