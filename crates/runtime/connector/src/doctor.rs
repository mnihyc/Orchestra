//! Connectivity, identity, clock and harness diagnostics.
//!
//! `orch-connector doctor` says, a line each, what stands between this machine and its system: whether it holds
//! an identity and its certificate holds (and when it renews), whether each address of the gateway answers over
//! TLS the way the machine trusts it, how far this machine's clock is from the gateway's, whether each harness its
//! configuration names is there, through which proxy the gateway is reached, and what its own log said lately that
//! was a warning or an error. Only the probes leave the machine, and nothing is changed.

use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use bytes::Bytes;
use hyper::Method;
use orch_link_contract::enrollment::Trust;
use orch_link_contract::session::CLOCK_SKEW_SECS;
use orch_link_contract::version::PATH_PREFIX;
use orch_pki::certificates::ServiceName;
use orch_pki::verification::{ServerTrust, machine_client_config};
use orch_types::{Clock, Time};

use crate::enrollment::DataDir;
use crate::https::{HttpsClient, HttpsError, https_base};

/// How long one address may take to answer.
pub const PROBE_LIMIT: Duration = Duration::from_secs(10);
/// How many lines of its own log the doctor reads back, newest files first: two days of them.
const LOG_DAYS: usize = 2;
/// How much of a logged line is shown.
const LINE_CHARS: usize = 200;

/// How a check came out.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Level {
    /// Nothing is in the way.
    Ok,
    /// Something will be in the way, or was lately.
    Warn,
    /// Something is in the way now.
    Fail,
}

/// One check: what it looked at, how it came out and what it saw.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Line {
    /// The check, as a short name.
    pub check: String,
    /// How it came out.
    pub level: Level,
    /// What it saw, and what to do where something is in the way.
    pub said: String,
}

impl Line {
    fn new(check: impl Into<String>, level: Level, said: impl Into<String>) -> Self {
        Self {
            check: check.into(),
            level,
            said: said.into(),
        }
    }

    /// The line as a person reads it.
    #[must_use]
    pub fn text(&self) -> String {
        let level = match self.level {
            Level::Ok => "ok  ",
            Level::Warn => "warn",
            Level::Fail => "FAIL",
        };
        format!("{level}  {:<12} {}", self.check, self.said)
    }
}

/// Looks at the machine whose data directory is `dir`, its configuration read with `var` for the environment,
/// at the time of `clock`.
pub async fn examine(
    dir: &Path,
    var: impl Fn(&str) -> Option<String>,
    clock: Arc<dyn Clock>,
) -> Vec<Line> {
    let mut lines = Vec::new();
    if !DataDir::enrolled_at(dir) {
        lines.push(Line::new(
            "identity",
            Level::Fail,
            format!(
                "{} holds no identity: enroll with `orch-connector enroll --url <address> --token <token>`",
                dir.display()
            ),
        ));
        return lines;
    }
    let data = match DataDir::open(dir) {
        Ok(data) => data,
        Err(error) => {
            lines.push(Line::new("identity", Level::Fail, error.to_string()));
            return lines;
        }
    };
    let identity = match data.identity() {
        Ok(identity) => identity,
        Err(error) => {
            lines.push(Line::new("identity", Level::Fail, error.to_string()));
            return lines;
        }
    };
    lines.push(certificate_line(
        identity.certificate.not_before(),
        identity.certificate.not_after(),
        clock.time(),
    ));
    let config = match data.config(var) {
        Ok(config) => config,
        Err(error) => {
            lines.push(Line::new("config", Level::Fail, error.to_string()));
            return lines;
        }
    };
    lines.push(Line::new("config", Level::Ok, "config.toml reads"));

    let endpoints = match data.endpoints(&config) {
        Ok(endpoints) => endpoints,
        Err(error) => {
            lines.push(Line::new("gateway", Level::Fail, error.to_string()));
            return lines;
        }
    };
    if endpoints.is_empty() {
        lines.push(Line::new(
            "gateway",
            Level::Fail,
            "no address of the gateway is configured: enroll again, or name one under [[endpoints]]",
        ));
    }
    let pinned = identity.tls().and_then(|tls| {
        machine_client_config(
            Some(ServerTrust::Authority(Box::new(identity.authority.clone()))),
            ServiceName::Gateway,
            Some(&tls),
            Arc::clone(&clock),
        )
    });
    let public = machine_client_config(
        Some(ServerTrust::Authority(Box::new(identity.authority.clone()))),
        ServiceName::Frontend,
        None,
        Arc::clone(&clock),
    );
    lines.push(proxy_line(
        &orch_link::outward::OutwardProxy::system(),
        endpoints.iter().map(|endpoint| endpoint.url.as_str()),
    ));
    let mut offset: Option<i64> = None;
    for (index, endpoint) in endpoints.iter().enumerate() {
        let check = format!("gateway-{}", index + 1);
        let tls = match endpoint.trust {
            Trust::Pinned => pinned.as_ref().map(Arc::clone),
            Trust::Public => public.as_ref().map(Arc::clone),
        };
        let tls = match tls {
            Ok(tls) => tls,
            Err(error) => {
                lines.push(Line::new(
                    check,
                    Level::Fail,
                    format!("{}: {error}", endpoint.url),
                ));
                continue;
            }
        };
        let (line, measured) = probe(&check, &endpoint.url, tls, clock.as_ref()).await;
        lines.push(line);
        offset = offset.or(measured);
    }
    lines.push(clock_line(offset));
    lines.extend(harness_lines(
        config
            .agent
            .harnesses
            .iter()
            .map(|harness| (harness.kind.as_str(), harness.path.as_str())),
    ));
    lines.push(log_line(&dir.join("logs")));
    lines
}

/// How each address of the gateway is reached: directly, or through which proxy (never with its credentials).
fn proxy_line<'a>(
    proxy: &orch_link::outward::OutwardProxy,
    addresses: impl Iterator<Item = &'a str>,
) -> Line {
    use orch_link::outward::Via;
    let mut level = Level::Ok;
    let reached: Vec<String> = addresses
        .filter_map(|address| {
            let base = https_base(address).ok()?;
            let host = base.host_str()?.to_owned();
            let port = base.port_or_known_default()?;
            Some(match proxy.route(&host, port, true) {
                None => format!("{host} directly"),
                Some(route) => match route.via {
                    Via::Unspoken(_) => {
                        level = Level::Warn;
                        format!(
                            "{host} directly: the system names {}, a proxy this program does not speak to",
                            route.shown()
                        )
                    }
                    _ => format!(
                        "{host} through {}{}",
                        route.shown(),
                        if route.with_credentials() {
                            ", with its username and password"
                        } else {
                            ""
                        }
                    ),
                },
            })
        })
        .collect();
    if reached.is_empty() {
        return Line::new("proxy", Level::Ok, "no address of the gateway to reach");
    }
    Line::new("proxy", level, reached.join("; "))
}

/// The certificate as it stands at `now`: good, due for renewal (two thirds of its life passed), or ended.
fn certificate_line(not_before: Time, not_after: Time, now: Time) -> Line {
    let whole = not_after.unix_millis() - not_before.unix_millis();
    let renews = Time::new(not_before.get() + chrono::Duration::milliseconds(whole / 3 * 2));
    if now > not_after {
        Line::new(
            "certificate",
            Level::Fail,
            format!(
                "ended {not_after}: a connector that connects within the system's grace renews it; after that, \
                 enroll again"
            ),
        )
    } else if now >= renews {
        Line::new(
            "certificate",
            Level::Warn,
            format!(
                "good until {not_after}, due for renewal since {renews}: a running connector renews it as soon as \
                 it is connected"
            ),
        )
    } else {
        Line::new(
            "certificate",
            Level::Ok,
            format!("good until {not_after}, renewed from {renews}"),
        )
    }
}

/// One address of the gateway: whether it answers over TLS as the machine trusts it, and the gateway's clock
/// against this machine's, in seconds, from its `Date`.
async fn probe(
    check: &str,
    url: &str,
    tls: Arc<rustls::ClientConfig>,
    clock: &dyn Clock,
) -> (Line, Option<i64>) {
    let base = match https_base(url) {
        Ok(base) => base,
        Err(error) => return (Line::new(check, Level::Fail, error.to_string()), None),
    };
    let before = clock.time();
    let answer = HttpsClient::new(tls)
        .send(
            &base,
            Method::GET,
            PATH_PREFIX,
            &[],
            Bytes::new(),
            PROBE_LIMIT,
        )
        .await;
    let after = clock.time();
    let took = after.unix_millis() - before.unix_millis();
    match answer {
        Ok(reply) => {
            let measured = reply
                .headers
                .get(hyper::header::DATE)
                .and_then(|value| value.to_str().ok())
                .and_then(|text| chrono::DateTime::parse_from_rfc2822(text).ok())
                .map(|theirs| {
                    let middle = before.unix_millis() + took / 2;
                    (theirs.timestamp_millis() - middle) / 1_000
                });
            (
                Line::new(
                    check,
                    Level::Ok,
                    format!("{url} answered over TLS in {took} ms"),
                ),
                measured,
            )
        }
        Err(HttpsError::Tls(error)) => (
            Line::new(
                check,
                Level::Fail,
                format!(
                    "{url}: TLS refused ({error}): the address is not this system's gateway, or this machine's \
                     certificate is not one it takes"
                ),
            ),
            None,
        ),
        Err(error) => (
            Line::new(check, Level::Fail, format!("{url}: {error}")),
            None,
        ),
    }
}

/// This machine's clock against the gateway's: the session is refused beyond the link's skew, and a clock half
/// that far off is drifting towards it.
fn clock_line(offset: Option<i64>) -> Line {
    match offset {
        None => Line::new(
            "clock",
            Level::Warn,
            "not measured: no address of the gateway answered with its time",
        ),
        Some(seconds) if seconds.abs() > CLOCK_SKEW_SECS => Line::new(
            "clock",
            Level::Fail,
            format!(
                "{seconds:+} s from the gateway's: a session is refused beyond {CLOCK_SKEW_SECS} s; set this \
                 machine's time (NTP)"
            ),
        ),
        Some(seconds) if seconds.abs() > CLOCK_SKEW_SECS / 2 => Line::new(
            "clock",
            Level::Warn,
            format!(
                "{seconds:+} s from the gateway's, more than half the {CLOCK_SKEW_SECS} s a session allows"
            ),
        ),
        Some(seconds) => Line::new(
            "clock",
            Level::Ok,
            format!("{seconds:+} s from the gateway's (to the second)"),
        ),
    }
}

/// The harnesses the configuration names: each where its path says.
fn harness_lines<'a>(harnesses: impl Iterator<Item = (&'a str, &'a str)>) -> Vec<Line> {
    let lines: Vec<Line> = harnesses
        .map(|(kind, path)| {
            if Path::new(path).is_file() {
                Line::new("harness", Level::Ok, format!("{kind} at {path}"))
            } else {
                Line::new(
                    "harness",
                    Level::Fail,
                    format!(
                        "{kind}: nothing at {path}; install it there, or change agent.harnesses"
                    ),
                )
            }
        })
        .collect();
    if lines.is_empty() {
        vec![Line::new(
            "harness",
            Level::Warn,
            "none named: `orch-connector policy set agent.harnesses '[{\"kind\":\"…\",\"path\":\"…\"}]'` names them",
        )]
    } else {
        lines
    }
}

/// What the connector's own log said lately: its warnings and errors of the last two days' files, and the last
/// error.
fn log_line(dir: &Path) -> Line {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return Line::new("log", Level::Ok, format!("no log yet in {}", dir.display()));
    };
    let mut files: Vec<std::path::PathBuf> = entries
        .flatten()
        .map(|entry| entry.path())
        .filter(|path| {
            path.file_name()
                .and_then(|name| name.to_str())
                .is_some_and(|name| name.starts_with("connector-") && name.ends_with(".log"))
        })
        .collect();
    // Named by their date, so the newest sort last.
    files.sort();
    let (mut warnings, mut errors, mut last_error) = (0_usize, 0_usize, None::<String>);
    for file in files.iter().rev().take(LOG_DAYS).rev() {
        let Ok(text) = std::fs::read_to_string(file) else {
            continue;
        };
        for line in text.lines() {
            if line.contains(" ERROR ") {
                errors += 1;
                last_error = Some(line.chars().take(LINE_CHARS).collect());
            } else if line.contains(" WARN ") {
                warnings += 1;
            }
        }
    }
    match (warnings, errors, last_error) {
        (0, 0, _) => Line::new(
            "log",
            Level::Ok,
            format!(
                "no warning or error in the last {LOG_DAYS} days ({})",
                dir.display()
            ),
        ),
        (warnings, errors, last) => Line::new(
            "log",
            Level::Warn,
            format!(
                "{errors} errors and {warnings} warnings in the last {LOG_DAYS} days ({}){}",
                dir.display(),
                last.map(|line| format!("; the last error: {line}"))
                    .unwrap_or_default()
            ),
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_gateway_s_route_names_the_proxy_without_its_credentials_or_says_it_is_reached_directly()
    {
        let proxy = orch_link::outward::OutwardProxy::from_values(
            Some("socks5h://ada:secret@proxy.corp:1080"),
            Some("direct.example"),
        )
        .unwrap();
        let line = proxy_line(
            &proxy,
            ["wss://gw.example:9443", "https://direct.example"].into_iter(),
        );
        assert_eq!(line.level, Level::Ok);
        assert_eq!(
            line.said,
            "gw.example through socks5h://proxy.corp:1080, with its username and password; direct.example directly"
        );
        let unspoken =
            orch_link::outward::OutwardProxy::from_values(Some("https://proxy.corp:443"), None)
                .unwrap();
        let line = proxy_line(&unspoken, ["wss://gw.example:9443"].into_iter());
        assert_eq!(line.level, Level::Warn);
        assert!(
            line.said.starts_with("gw.example directly"),
            "{}",
            line.said
        );
    }

    fn at(text: &str) -> Time {
        text.parse().unwrap()
    }

    #[test]
    fn a_certificate_is_good_then_due_then_ended() {
        let (from, to) = (
            at("2026-01-01T00:00:00.000Z"),
            at("2026-01-31T00:00:00.000Z"),
        );
        assert_eq!(
            certificate_line(from, to, at("2026-01-10T00:00:00.000Z")).level,
            Level::Ok
        );
        assert_eq!(
            certificate_line(from, to, at("2026-01-25T00:00:00.000Z")).level,
            Level::Warn
        );
        assert_eq!(
            certificate_line(from, to, at("2026-02-01T00:00:00.000Z")).level,
            Level::Fail
        );
    }

    #[test]
    fn a_clock_off_by_more_than_the_link_allows_fails_and_half_of_it_warns() {
        assert_eq!(clock_line(Some(3)).level, Level::Ok);
        assert_eq!(
            clock_line(Some(-(CLOCK_SKEW_SECS / 2 + 1))).level,
            Level::Warn
        );
        assert_eq!(clock_line(Some(CLOCK_SKEW_SECS + 1)).level, Level::Fail);
        assert_eq!(clock_line(None).level, Level::Warn);
    }

    #[test]
    fn the_log_counts_warnings_and_errors_of_the_newest_days_and_shows_the_last_error() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join("connector-2026-10-01.log"),
            "2026-10-01T00:00:00Z ERROR old: long ago\n",
        )
        .unwrap();
        std::fs::write(
            dir.path().join("connector-2026-10-13.log"),
            "2026-10-13T00:00:00Z  WARN link: slow\n",
        )
        .unwrap();
        std::fs::write(
            dir.path().join("connector-2026-10-14.log"),
            "2026-10-14T00:00:00Z  INFO fine\n2026-10-14T01:00:00Z ERROR link: refused\n",
        )
        .unwrap();
        let line = log_line(dir.path());
        assert_eq!(line.level, Level::Warn);
        assert!(
            line.said.starts_with("1 errors and 1 warnings"),
            "{}",
            line.said
        );
        assert!(line.said.ends_with("ERROR link: refused"), "{}", line.said);
        let quiet = tempfile::tempdir().unwrap();
        assert_eq!(log_line(quiet.path()).level, Level::Ok);
    }

    #[test]
    fn a_harness_named_where_nothing_is_fails() {
        let dir = tempfile::tempdir().unwrap();
        let present = dir.path().join("codex");
        std::fs::write(&present, b"").unwrap();
        let present = present.to_string_lossy().into_owned();
        let lines = harness_lines(
            [
                ("codex", present.as_str()),
                ("claude_code", "/nowhere/claude"),
            ]
            .into_iter(),
        );
        assert_eq!(lines[0].level, Level::Ok);
        assert_eq!(lines[1].level, Level::Fail);
        assert_eq!(harness_lines(std::iter::empty()).len(), 1);
    }
}
