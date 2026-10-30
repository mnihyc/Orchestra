//! The gateway sees the front's real client address through the same trusted hops as the API.

#[cfg(test)]
mod forwarded_address_security_tests {
    use crate::listeners::{Kind, client_ip};
    use http::HeaderMap;

    fn proxies() -> Vec<String> {
        vec!["127.0.0.0/8".to_owned()]
    }

    #[test]
    fn a_caller_supplied_first_address_does_not_become_the_gateway_peer() {
        let mut headers = HeaderMap::new();
        headers.insert(
            "x-forwarded-for",
            "198.51.100.41, 203.0.113.12, 127.0.0.1".parse().unwrap(),
        );
        assert_eq!(
            client_ip(
                Kind::Edge,
                "127.0.0.1".parse().unwrap(),
                &headers,
                &proxies(),
                0,
            ),
            "203.0.113.12".parse::<std::net::IpAddr>().unwrap()
        );
    }

    #[test]
    fn a_direct_or_pinned_caller_cannot_choose_its_peer_with_a_header() {
        let mut headers = HeaderMap::new();
        headers.insert("x-forwarded-for", "198.51.100.41".parse().unwrap());
        let peer = "203.0.113.12".parse().unwrap();
        assert_eq!(client_ip(Kind::Edge, peer, &headers, &proxies(), 0), peer);
        let pinned = "127.0.0.1".parse().unwrap();
        assert_eq!(
            client_ip(Kind::Pinned, pinned, &headers, &proxies(), 0),
            pinned
        );
    }

    #[test]
    fn the_gateway_takes_the_api_proxy_setting() {
        let mut config = orch_config::ServerConfig::default();
        config.api.trusted_proxy_cidrs = proxies();
        let settings = crate::Settings::from_config(&config, "test");
        let mut headers = HeaderMap::new();
        headers.insert(
            "forwarded",
            "for=203.0.113.12, for=127.0.0.2".parse().unwrap(),
        );
        assert_eq!(
            client_ip(
                Kind::Edge,
                "127.0.0.1".parse().unwrap(),
                &headers,
                &settings.trusted_proxy_cidrs,
                0,
            ),
            "203.0.113.12".parse::<std::net::IpAddr>().unwrap()
        );
    }
}
