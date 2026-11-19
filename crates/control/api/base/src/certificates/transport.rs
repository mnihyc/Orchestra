//! The built-in edge receives one complete policy from committed System settings.

use orch_pki::PkiError;
use serde::Deserialize;
use std::{num::NonZeroU64, path::Path};

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Transport {
    session_cache_mib: u64,
    session_timeout_secs: NonZeroU64,
    session_tickets: bool,
    early_data: bool,
    http2: bool,
    http3: bool,
    alt_svc_secs: u64,
    pub(super) reload_secs: NonZeroU64,
}

impl Transport {
    pub(super) fn render(&self) -> String {
        let switch = |on| if on { "on" } else { "off" };
        let cache = if self.session_cache_mib == 0 {
            "off".to_owned()
        } else {
            // A changed size gets a new zone: existing workers finish with their old zone during reload.
            format!("shared:orch_edge_{0}:{0}m", self.session_cache_mib)
        };
        let mut text = format!(
            "# reload_secs={}\nhttp2 {};\nssl_session_cache {cache};\nssl_session_timeout {}s;\nssl_session_tickets {};\nssl_early_data {};\nhttp3 {};\n",
            self.reload_secs,
            switch(self.http2),
            self.session_timeout_secs,
            switch(self.session_tickets),
            switch(self.early_data),
            switch(self.http3),
        );
        if self.http3 {
            text.push_str(&format!("listen 443 quic reuseport;\nadd_header Alt-Svc 'h3=\":$orch_https_port\"; ma={}' always;\n", self.alt_svc_secs));
        } else {
            text.push_str("add_header Alt-Svc 'clear' always;\n");
        }
        text
    }
}

/// The existing atomic writer keeps permissions and removes its temporary file on failure.
pub(super) fn publish(path: &Path, text: &str) -> Result<bool, PkiError> {
    if std::fs::read_to_string(path).ok().as_deref() == Some(text) {
        return Ok(false);
    }
    orch_pki::certificates::write_file(path, text)?;
    Ok(true)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn long_line_defaults_resume_sessions_and_advertise_quic_without_enabling_early_writes() {
        let value = orch_settings_contract::registry::get("edge.transport")
            .unwrap()
            .default_value()
            .unwrap();
        let transport: Transport = serde_json::from_value(value).unwrap();
        let text = transport.render();
        assert!(text.contains("ssl_session_cache shared:orch_edge_16:16m;"));
        assert!(text.contains("ssl_session_timeout 86400s;"));
        assert!(text.contains("ssl_session_tickets on;"));
        assert!(text.contains("ssl_early_data on;"));
        assert!(text.contains("http2 on;"));
        assert!(text.contains("listen 443 quic reuseport;"));
        assert!(text.contains("h3=\":$orch_https_port\"; ma=86400"));
    }

    #[test]
    fn an_operator_can_turn_each_protocol_off_and_increase_the_cache_without_a_ceiling() {
        let transport: Transport = serde_json::from_value(serde_json::json!({
            "session_cache_mib":128,"session_timeout_secs":172800,"session_tickets":false,
            "early_data":false,"http2":false,"http3":false,"alt_svc_secs":0,"reload_secs":3
        }))
        .unwrap();
        let text = transport.render();
        assert!(text.contains("shared:orch_edge_128:128m;"));
        assert!(text.contains("ssl_session_timeout 172800s;"));
        for setting in [
            "ssl_session_tickets off;",
            "ssl_early_data off;",
            "http2 off;",
            "http3 off;",
        ] {
            assert!(text.contains(setting));
        }
        assert!(!text.contains(" quic "));
        assert!(text.contains("Alt-Svc 'clear'"));
        let mut transport = transport;
        transport.session_cache_mib = 0;
        assert!(transport.render().contains("ssl_session_cache off;"));
    }

    #[test]
    fn replacing_a_policy_publishes_a_complete_file_and_keeps_an_unchanged_one() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("transport.include");
        assert!(publish(&path, "first complete policy\n").unwrap());
        assert!(!publish(&path, "first complete policy\n").unwrap());
        assert!(publish(&path, "replacement complete policy\n").unwrap());
        assert_eq!(
            std::fs::read_to_string(path).unwrap(),
            "replacement complete policy\n"
        );
        assert_eq!(std::fs::read_dir(directory.path()).unwrap().count(), 1);
    }
}
