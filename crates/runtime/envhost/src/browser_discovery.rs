//! Installed browser and display programs, discovered in order with one sandbox probe.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::Duration;

use orch_link_contract::browser::{
    EnvironmentFactsBrowser, EnvironmentFactsProgram, EnvironmentFactsVnc,
};
use tokio::process::{Child, Command};

use crate::browser_cdp::identify;

/// A version command must be quick; a stopped program must not hold discovery up.
const VERSION_WAIT: Duration = Duration::from_secs(3);
/// A browser has twenty seconds to answer its local endpoint.
pub const START_WAIT: Duration = Duration::from_secs(20);
/// A short poll sees the browser's chosen port without inventing a port of our own.
pub const PORT_POLL: Duration = Duration::from_millis(100);

/// Builds a browser process with a core limit of zero and an independent process group.
pub fn browser_command(program: &Path, variables: &BTreeMap<String, String>) -> Command {
    browser_command_in(program, variables, &[])
}

/// The same browser invocation within the environment's proved aggregate resource group.
pub fn browser_command_in(
    program: &Path,
    variables: &BTreeMap<String, String>,
    launcher: &[String],
) -> Command {
    #[cfg(unix)]
    let mut command = {
        let mut command = Command::from(crate::process_limits::command("/bin/sh", launcher));
        command
            .args(["-c", "ulimit -S -c 0 || exit; exec \"$@\"", "orch-browser"])
            .arg(program);
        command.process_group(0);
        command
    };
    #[cfg(windows)]
    let mut command = Command::from(crate::process_limits::command(program, launcher));
    command.env_clear();
    // Only normal process settings are inherited. Environment secrets and connector credentials are never browser variables.
    for (name, value) in std::env::vars() {
        if matches!(
            name.as_str(),
            "PATH"
                | "HOME"
                | "USER"
                | "LOGNAME"
                | "LANG"
                | "TZ"
                | "TMPDIR"
                | "TMP"
                | "TEMP"
                | "SYSTEMROOT"
                | "SystemRoot"
                | "LD_LIBRARY_PATH"
        ) || name.starts_with("LC_")
        {
            command.env(name, value);
        }
    }
    command.envs(variables).kill_on_drop(true);
    command
}

/// Finds a named executable without running a shell or downloading anything.
#[must_use]
pub fn find_program(name: &str) -> Option<PathBuf> {
    let path = std::env::var_os("PATH")?;
    for directory in std::env::split_paths(&path) {
        let candidate = directory.join(name);
        if executable(&candidate) {
            return Some(candidate);
        }
        #[cfg(windows)]
        if executable(&directory.join(format!("{name}.exe"))) {
            return Some(directory.join(format!("{name}.exe")));
        }
    }
    None
}

fn executable(path: &Path) -> bool {
    let Ok(metadata) = std::fs::metadata(path) else {
        return false;
    };
    if !metadata.is_file() {
        return false;
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        metadata.permissions().mode() & 0o111 != 0
    }
    #[cfg(windows)]
    {
        true
    }
}

async fn version(path: &Path, arg: &str) -> Option<String> {
    let out = tokio::time::timeout(
        VERSION_WAIT,
        browser_command(path, &BTreeMap::new())
            .arg(arg)
            .kill_on_drop(true)
            .output(),
    )
    .await
    .ok()?
    .ok()?;
    if !out.status.success() {
        return None;
    }
    let text = if out.stdout.is_empty() {
        &out.stderr
    } else {
        &out.stdout
    };
    String::from_utf8_lossy(text)
        .split_whitespace()
        .find(|s| s.starts_with(|c: char| c.is_ascii_digit()))
        .map(str::to_owned)
}

fn bundled_browsers() -> Vec<PathBuf> {
    let Some(root) = std::env::var_os("PLAYWRIGHT_BROWSERS_PATH") else {
        return Vec::new();
    };
    let root = PathBuf::from(root);
    let Ok(directories) = std::fs::read_dir(&root) else {
        return Vec::new();
    };
    let mut found = Vec::new();
    for directory in directories
        .flatten()
        .filter(|d| d.file_name().to_string_lossy().starts_with("chromium-"))
    {
        let Ok(children) = std::fs::read_dir(directory.path()) else {
            continue;
        };
        for child in children
            .flatten()
            .filter(|d| d.file_name().to_string_lossy().starts_with("chrome-linux"))
        {
            let executable_path = child.path().join("chrome");
            if executable(&executable_path) {
                found.push(executable_path);
            }
        }
        for relative in [
            "chrome-win/chrome.exe",
            "chrome-win64/chrome.exe",
            "chrome-win32/chrome.exe",
            "chrome-mac/Chromium.app/Contents/MacOS/Chromium",
            "chrome-mac-arm64/Google Chrome for Testing.app/Contents/MacOS/Google Chrome for Testing",
            "chrome-mac-x64/Google Chrome for Testing.app/Contents/MacOS/Google Chrome for Testing",
        ] {
            let executable_path = directory.path().join(relative);
            if executable(&executable_path) {
                found.push(executable_path);
            }
        }
    }
    let preferred =
        cli_browser_revision().map(|revision| root.join(format!("chromium-{revision}")));
    found.sort_by_key(|path| {
        (
            preferred
                .as_ref()
                .is_none_or(|preferred| !path.starts_with(preferred)),
            path.clone(),
        )
    });
    found
}

fn cli_browser_revision() -> Option<String> {
    let program = std::fs::canonicalize(find_program("playwright-cli")?).ok()?;
    for directory in program.parent()?.ancestors() {
        for relative in [
            "node_modules/playwright-core/browsers.json",
            "node_modules/playwright/node_modules/playwright-core/browsers.json",
            "node_modules/@playwright/cli/node_modules/playwright-core/browsers.json",
        ] {
            let Some(value) = std::fs::read(directory.join(relative))
                .ok()
                .and_then(|bytes| serde_json::from_slice::<serde_json::Value>(&bytes).ok())
            else {
                continue;
            };
            if let Some(revision) = value["browsers"]
                .as_array()?
                .iter()
                .find(|browser| browser["name"] == "chromium")
                .and_then(|browser| browser["revision"].as_str())
                .filter(|revision| revision.bytes().all(|b| b.is_ascii_digit()))
            {
                return Some(revision.to_owned());
            }
        }
    }
    None
}

/// Finds all supported programs and measures the first browser's sandbox once.
pub async fn discover(
    probe_dir: &Path,
    variables: &BTreeMap<String, String>,
    managed: bool,
    cached_sandbox: Option<(String, bool)>,
) -> (EnvironmentFactsBrowser, EnvironmentFactsVnc) {
    let mut candidates: Vec<(String, PathBuf)> = bundled_browsers()
        .into_iter()
        .map(|p| ("chromium".to_owned(), p))
        .collect();
    for name in [
        "chromium",
        "chromium-browser",
        "google-chrome",
        "google-chrome-stable",
        "microsoft-edge",
    ] {
        if let Some(path) = find_program(name) {
            candidates.push((name.to_owned(), path));
        }
    }
    #[cfg(target_os = "macos")]
    for (name, path) in [
        (
            "google-chrome",
            "/Applications/Google Chrome.app/Contents/MacOS/Google Chrome",
        ),
        (
            "microsoft-edge",
            "/Applications/Microsoft Edge.app/Contents/MacOS/Microsoft Edge",
        ),
    ] {
        if executable(Path::new(path)) {
            candidates.push((name.to_owned(), PathBuf::from(path)));
        }
    }
    #[cfg(windows)]
    for root in ["PROGRAMFILES", "PROGRAMFILES(X86)", "LOCALAPPDATA"]
        .iter()
        .filter_map(|key| std::env::var_os(key))
    {
        for (name, path) in [
            ("google-chrome", "Google/Chrome/Application/chrome.exe"),
            ("microsoft-edge", "Microsoft/Edge/Application/msedge.exe"),
        ] {
            let path = PathBuf::from(&root).join(path);
            if executable(&path) {
                candidates.push((name.to_owned(), path));
            }
        }
    }
    let mut seen = BTreeSet::new();
    let mut programs = Vec::new();
    for (name, path) in candidates {
        let real = std::fs::canonicalize(&path).unwrap_or(path.clone());
        if seen.insert(real) {
            programs.push(EnvironmentFactsProgram {
                name,
                path: path.to_string_lossy().into_owned(),
                version: version(&path, "--version").await,
            });
        }
    }
    let playwright_cli = match find_program("playwright-cli") {
        Some(path) => version(&path, "--version").await,
        None => None,
    };
    let mut servers = Vec::new();
    for name in ["Xvnc", "Xtigervnc", "tightvncserver"] {
        if let Some(path) = find_program(name) {
            servers.push(EnvironmentFactsProgram {
                name: name.to_owned(),
                version: version(&path, "-version").await,
                path: path.to_string_lossy().into_owned(),
            });
        }
    }
    let window_manager = ["openbox", "fluxbox", "twm"]
        .into_iter()
        .find(|name| find_program(name).is_some())
        .map(str::to_owned);
    let virtual_window = !servers.is_empty();
    let window = virtual_window
        || cfg!(target_os = "macos")
        || cfg!(windows)
        || std::env::var_os("DISPLAY").is_some()
        || std::env::var_os("WAYLAND_DISPLAY").is_some();
    let sandbox = match programs.first() {
        Some(program) => Some(match cached_sandbox {
            Some((path, allowed)) if path == program.path => allowed,
            _ => probe(Path::new(&program.path), probe_dir, variables, managed).await,
        }),
        None => None,
    };
    (
        EnvironmentFactsBrowser {
            virtual_window: Some(virtual_window),
            programs,
            window,
            playwright_cli,
            sandbox,
        },
        EnvironmentFactsVnc {
            servers,
            window_manager,
        },
    )
}

/// Reads both lines, including a socket path with no trailing newline.
pub fn active_port(profile: &Path) -> Option<(u16, String)> {
    let text = std::fs::read_to_string(profile.join("DevToolsActivePort")).ok()?;
    let (port, path) = text.split_once('\n')?;
    let port = port.trim().parse::<u16>().ok().filter(|p| *p != 0)?;
    let path = path.trim();
    if !path.starts_with("/devtools/browser/") || path.contains('\n') {
        return None;
    }
    Some((port, path.to_owned()))
}

async fn probe(
    program: &Path,
    root: &Path,
    variables: &BTreeMap<String, String>,
    managed: bool,
) -> bool {
    let Ok(profile) = tempfile::Builder::new().prefix("sandbox-").tempdir_in(root) else {
        return false;
    };
    let mut command = browser_command(program, variables);
    command
        .args([
            "--headless",
            "--remote-debugging-address=127.0.0.1",
            "--remote-debugging-port=0",
            "--no-first-run",
            "--no-default-browser-check",
        ])
        .arg(format!("--user-data-dir={}", profile.path().display()))
        .arg("about:blank")
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    if managed {
        command.arg("--disable-dev-shm-usage");
    }
    let Ok(mut child) = command.spawn() else {
        return false;
    };
    let deadline = tokio::time::Instant::now() + START_WAIT;
    let mut allowed = false;
    while tokio::time::Instant::now() < deadline {
        if let Some((port, _)) = active_port(profile.path())
            && matches!(
                tokio::time::timeout_at(deadline, identify(&format!("http://127.0.0.1:{port}")))
                    .await,
                Ok(Ok(_))
            )
        {
            allowed = true;
            break;
        }
        if child.try_wait().ok().flatten().is_some() {
            break;
        }
        tokio::time::sleep(PORT_POLL).await;
    }
    terminate(&mut child).await;
    allowed
}

/// Ends an owned browser process group and reaps the parent.
pub async fn terminate(child: &mut Child) {
    #[cfg(unix)]
    if let Some(pid) = child
        .id()
        .and_then(|p| i32::try_from(p).ok())
        .and_then(rustix::process::Pid::from_raw)
    {
        let _ = rustix::process::kill_process_group(pid, rustix::process::Signal::KILL);
    }
    let _ = child.kill().await;
    let _ = child.wait().await;
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn the_browser_port_file_does_not_need_a_final_newline() {
        let profile = tempfile::tempdir().unwrap();
        std::fs::write(
            profile.path().join("DevToolsActivePort"),
            "41873\n/devtools/browser/abc",
        )
        .unwrap();
        assert_eq!(
            active_port(profile.path()),
            Some((41873, "/devtools/browser/abc".to_owned()))
        );
    }
}
