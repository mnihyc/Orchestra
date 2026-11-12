//! Deployment identity shared by all Ask address readers. No URL is fetched here.

pub(super) fn web(value: &str) -> Option<url::Url> {
    if value
        .chars()
        .any(|c| c == '\\' || c <= ' ' || c == '\u{7f}')
    {
        return None;
    }
    let url = url::Url::parse(value).ok()?;
    (matches!(url.scheme(), "http" | "https")
        && url.username().is_empty()
        && url.password().is_none())
    .then_some(url)
}

/// Match the browser LINKS reader: relative paths, the address in use and each
/// declared deployment origin. Scheme, normalized host and port all participate.
pub(super) fn read(value: &str, current: &url::Url, declared: &[String]) -> Option<url::Url> {
    let clean = value.trim();
    if clean
        .chars()
        .any(|c| c == '\\' || c <= ' ' || c == '\u{7f}')
        || !(clean.starts_with('/')
            || clean
                .get(..7)
                .is_some_and(|v| v.eq_ignore_ascii_case("http://"))
            || clean
                .get(..8)
                .is_some_and(|v| v.eq_ignore_ascii_case("https://")))
    {
        return None;
    }
    let candidate = if clean.starts_with('/') {
        current.join(clean).ok()?
    } else {
        web(clean)?
    };
    if !matches!(candidate.scheme(), "http" | "https")
        || !candidate.username().is_empty()
        || candidate.password().is_some()
    {
        return None;
    }
    if candidate.origin() != current.origin()
        && !declared
            .iter()
            .filter_map(|v| web(v))
            .any(|url| url.origin() == candidate.origin())
    {
        return None;
    }
    Some(candidate)
}

#[cfg(any(test, feature = "test-support"))]
#[allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a deciding case fails by panicking"
)]
pub mod tests {

    use super::*;
    fn declarations() -> Vec<String> {
        [
            "https://public.orch.test",
            "https://inside.orch.test:8443",
            "http://192.0.2.15:8080",
            "https://[2001:db8::15]",
            "https://edge.orch.test",
        ]
        .map(str::to_owned)
        .into()
    }

    #[cfg(any(test, feature = "test-support"))]
    // EN browser: paste public, alternate and gateway links into Ask; open each citation.
    // 中文浏览器：把公开、备用和网关地址粘贴到“问一问”，依次打开来源并核对同一对象。
    fn links_accept_only_the_current_and_declared_normalized_origins() {
        let current = web("https://browser.orch.test/").unwrap();
        for origin in std::iter::once("https://browser.orch.test".to_owned()).chain(declarations())
        {
            let value = format!(
                "{origin}/doc/00000000-0000-4000-8000-000000000041?block=00000000-0000-4000-8000-000000000042#section"
            );
            let parsed = read(&value, &current, &declarations()).unwrap();
            assert_eq!(parsed.path(), "/doc/00000000-0000-4000-8000-000000000041");
            assert!(parsed.query().is_some());
            assert_eq!(parsed.fragment(), Some("section"));
        }
        assert!(
            read(
                "https://PUBLIC.ORCH.TEST:443/doc/one",
                &current,
                &declarations()
            )
            .is_some()
        );
        assert!(read(" /doc/one ", &current, &declarations()).is_some());
        assert!(read("//public.orch.test/doc/one", &current, &declarations()).is_some());
    }
    #[cfg(any(test, feature = "test-support"))]
    #[doc(hidden)]
    pub fn __api_run_links_accept_only_the_current_and_declared_normalized_origins() {
        self::links_accept_only_the_current_and_declared_normalized_origins()
    }

    #[cfg(any(test, feature = "test-support"))]
    fn links_reject_foreign_credentials_ports_schemes_controls_and_backslashes() {
        let current = web("https://browser.orch.test/").unwrap();
        for origin in [
            "https://elsewhere.test",
            "https://public.orch.test.evil.test",
            "https://public-orch.test",
            "https://public.orch.test:8443",
            "http://192.0.2.15",
            "http://192.0.2.16:8080",
            "https://[2001:db8::16]",
            "https://public.orch.test@evil.test",
            "https://someone@public.orch.test",
            "http://public.orch.test",
        ] {
            assert!(
                read(&format!("{origin}/doc/one"), &current, &declarations()).is_none(),
                "{origin}"
            );
        }
        for value in [
            "https://elsewhere.test\\@public.orch.test/doc/one",
            "//elsewhere.test/doc/one",
            "/doc/a b",
            "/doc/a\nb",
            "javascript:alert(1)",
            "doc/one",
        ] {
            assert!(read(value, &current, &declarations()).is_none(), "{value}");
        }
    }
    #[cfg(any(test, feature = "test-support"))]
    #[doc(hidden)]
    pub fn __api_run_links_reject_foreign_credentials_ports_schemes_controls_and_backslashes() {
        self::links_reject_foreign_credentials_ports_schemes_controls_and_backslashes()
    }
}
