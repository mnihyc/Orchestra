//! What both ways of asking Google about a reCAPTCHA token share: the widget, and so the hosts a page that shows it
//! needs a Content-Security-Policy to allow.

use orch_human_check_contract::EdgeHosts;

/// The hosts Google asks a Content-Security-Policy to allow by host. It names none for styles.
pub const EDGE_HOSTS: EdgeHosts = EdgeHosts {
    script: &[
        "https://www.google.com/recaptcha/",
        "https://www.gstatic.com/recaptcha/",
    ],
    frame: &[
        "https://www.google.com/recaptcha/",
        "https://recaptcha.google.com/recaptcha/",
    ],
    connect: &["https://www.google.com/recaptcha/"],
    style: &[],
};

/// The same through `www.recaptcha.net`, which Google offers where `www.google.com` cannot be reached: its guide says
/// to use it in place of `www.google.com/recaptcha/` wherever that is written.
pub const EDGE_HOSTS_ALTERNATE: EdgeHosts = EdgeHosts {
    script: &[
        "https://www.recaptcha.net/recaptcha/",
        "https://www.gstatic.com/recaptcha/",
    ],
    frame: &[
        "https://www.recaptcha.net/recaptcha/",
        "https://recaptcha.google.com/recaptcha/",
    ],
    connect: &["https://www.recaptcha.net/recaptcha/"],
    style: &[],
};
