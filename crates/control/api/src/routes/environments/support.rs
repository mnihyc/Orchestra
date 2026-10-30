//! What an environment was found to have for browsers and displays, as the facts its machine reported hold it: the programs, the window manager, the command line an agent drives a browser with, and when the environment looked.
//!
//! Nothing is assumed. Whatever the facts hold nothing of is told as nothing found: no program, no
//! window, no command line, and `looked_at` empty while the environment has not looked yet. A member
//! that is there but wrong in one place is read as far as it goes, so that one odd program in a list
//! does not hide the others.

use orch_api_contract::environment::{
    EnvironmentSupport, EnvironmentSupportBrowser, EnvironmentSupportProgram, EnvironmentSupportVnc,
};
use orch_types::{Json, Time};

/// The programs a list of the facts holds, each as far as it can be read.
fn programs(found: Option<&Json>) -> Vec<EnvironmentSupportProgram> {
    found
        .and_then(Json::as_array)
        .map(|list| {
            list.iter()
                .filter_map(|each| serde_json::from_value(each.clone()).ok())
                .collect()
        })
        .unwrap_or_default()
}

/// A text member that is there, and nothing for one that is missing, `null` or no text.
fn text(found: Option<&Json>) -> Option<String> {
    found.and_then(Json::as_str).map(str::to_owned)
}

/// What the facts of an environment say it has for browsers and displays.
pub(super) fn of(facts: &Json) -> EnvironmentSupport {
    let browser = facts.get("browser");
    let vnc = facts.get("vnc");
    EnvironmentSupport {
        looked_at: text(facts.get("looked_at")).and_then(|moment| moment.parse::<Time>().ok()),
        browser: EnvironmentSupportBrowser {
            programs: programs(browser.and_then(|found| found.get("programs"))),
            window: browser
                .and_then(|found| found.get("window"))
                .and_then(Json::as_bool)
                .unwrap_or(false),
            playwright_cli: text(browser.and_then(|found| found.get("playwright_cli"))),
            sandbox: browser
                .and_then(|found| found.get("sandbox"))
                .and_then(Json::as_bool),
        },
        vnc: EnvironmentSupportVnc {
            servers: programs(vnc.and_then(|found| found.get("servers"))),
            window_manager: text(vnc.and_then(|found| found.get("window_manager"))),
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn an_environment_whose_facts_hold_nothing_of_it_was_found_to_have_nothing() {
        assert_eq!(of(&json!({})), EnvironmentSupport::nothing_found());
        assert_eq!(
            of(&json!({ "os": "linux", "browser": null, "vnc": [] })),
            EnvironmentSupport::nothing_found()
        );
    }

    #[test]
    fn what_the_facts_hold_is_told_as_it_was_found() {
        let support = of(&json!({
            "looked_at": "2026-10-12T09:30:00.000Z",
            "browser": {
                "programs": [{ "name": "chromium", "path": "/opt/ms-playwright/chromium/chrome", "version": "155.0.8059.12" }],
                "window": true, "playwright_cli": "0.1.22", "sandbox": false
            },
            "vnc": { "servers": [{ "name": "Xvnc", "path": "/usr/bin/Xvnc", "version": null }], "window_manager": "openbox" }
        }));
        assert_eq!(support.browser.programs[0].name, "chromium");
        assert_eq!(
            support.browser.programs[0].version.as_deref(),
            Some("155.0.8059.12")
        );
        assert!(support.browser.window);
        assert_eq!(support.browser.playwright_cli.as_deref(), Some("0.1.22"));
        assert_eq!(support.browser.sandbox, Some(false));
        assert_eq!(support.vnc.servers[0].version, None);
        assert_eq!(support.vnc.window_manager.as_deref(), Some("openbox"));
        assert!(support.looked_at.is_some());
    }

    #[test]
    fn one_odd_entry_does_not_hide_the_others() {
        let support = of(&json!({
            "browser": { "programs": [
                { "name": "chromium", "path": "/usr/bin/chromium" },
                { "name": 7 },
                { "name": "chrome", "path": "/usr/bin/google-chrome", "version": "154.0" }
            ] },
            "vnc": { "window_manager": 3, "servers": "Xvnc" }
        }));
        let names: Vec<_> = support
            .browser
            .programs
            .iter()
            .map(|program| program.name.as_str())
            .collect();
        assert_eq!(names, ["chromium", "chrome"]);
        assert!(!support.browser.window);
        assert_eq!(support.browser.sandbox, None, "not tried is not false");
        assert!(support.vnc.servers.is_empty());
        assert_eq!(support.vnc.window_manager, None);
    }
}
