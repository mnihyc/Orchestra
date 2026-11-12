//! Names and installation paths of browsers supported by environment discovery.
//!
//! Runtime discovery consumes this catalogue in order and checks each actual executable.

/// Executables below a browser bundle's revision directory.
pub const BUNDLED_EXECUTABLES: [&str; 6] = [
    "chrome-win/chrome.exe",
    "chrome-win64/chrome.exe",
    "chrome-win32/chrome.exe",
    "chrome-mac/Chromium.app/Contents/MacOS/Chromium",
    "chrome-mac-arm64/Google Chrome for Testing.app/Contents/MacOS/Google Chrome for Testing",
    "chrome-mac-x64/Google Chrome for Testing.app/Contents/MacOS/Google Chrome for Testing",
];

/// Browser names to look up on the host's executable search path.
pub const PATH_EXECUTABLES: [&str; 5] = [
    "chromium",
    "chromium-browser",
    "google-chrome",
    "google-chrome-stable",
    "microsoft-edge",
];

/// Browser names and their standard application executable on macOS.
pub const MACOS_APPLICATIONS: [(&str, &str); 2] = [
    (
        "google-chrome",
        "/Applications/Google Chrome.app/Contents/MacOS/Google Chrome",
    ),
    (
        "microsoft-edge",
        "/Applications/Microsoft Edge.app/Contents/MacOS/Microsoft Edge",
    ),
];

/// Browser names and paths relative to each Windows application directory.
pub const WINDOWS_APPLICATIONS: [(&str, &str); 2] = [
    ("google-chrome", "Google/Chrome/Application/chrome.exe"),
    ("microsoft-edge", "Microsoft/Edge/Application/msedge.exe"),
];
