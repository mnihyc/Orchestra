//! The suite every notifier passes: each kind of reach, the check of a channel when it is saved, a text for one notification and for several, a link made and followed, a retry, and a target that is gone.
//!
//! Deliveries are supplied by the caller and must name a disposable recipient. A retry, gone target or
//! denied private address needs a controlled far side; an omitted counterpart is reported as unverified.

use std::time::Duration;

use orch_notifier_contract::delivery::{Config, Delivery};
use orch_notifier_contract::descriptor::Reaches;
use orch_notifier_contract::notifier::Notifier;
use serde::{Deserialize, Serialize};

use crate::report::{Report, skip};

/// One far-side operation is bounded by the ordinary plugin deadline.
const WITHIN: Duration = Duration::from_secs(60);
/// A link poll should return immediately when no one followed the code.
const NO_WAIT: Duration = Duration::ZERO;
/// A synthetic linking code, with the same shape as a person's code.
const CODE: &str = "0123456789abcdef0123456789abcdef";

/// Explicit destinations and controlled failures for a notifier check.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Fixture {
    /// Values that the disposable far side accepts; checking these sends nothing.
    pub config: Config,
    /// One rendered notification and a rendered digest for the disposable recipient.
    pub deliveries: Vec<Delivery>,
    /// A delivery whose far side returns a retryable error.
    pub retry: Option<Delivery>,
    /// A delivery whose far side says the recipient is gone.
    pub gone: Option<Delivery>,
    /// A private destination disallowed by an explicitly empty private-host list.
    pub private: Option<Delivery>,
}

/// Checks every reach through the same trait, and sends only the supplied disposable deliveries.
pub async fn run(notifier: &dyn Notifier, fixture: Option<&Fixture>, report: &mut Report) {
    let descriptor = notifier.describe();
    report.checked(
        "notifier.descriptor",
        "Its fields, reaches and shipped texts form a usable channel",
        descriptor.check().map_err(|faults| {
            faults
                .iter()
                .map(ToString::to_string)
                .collect::<Vec<_>>()
                .join("; ")
                .into()
        }),
    );
    let Some(fixture) = fixture else {
        for rule in [
            "notifier.check",
            "notifier.single",
            "notifier.digest",
            "notifier.link",
            "notifier.retry",
            "notifier.gone",
            "notifier.private",
        ] {
            report.checked(
                rule,
                "Its far-side behavior is verified",
                Err(skip("supply a notifier fixture with a disposable far side")),
            );
        }
        return;
    };
    report
        .run(
            "notifier.check",
            "A valid channel can be checked without delivering",
            WITHIN,
            async {
                notifier
                    .check(&fixture.config)
                    .await
                    .map(|_| ())
                    .map_err(|error| error.to_string().into())
            },
        )
        .await;
    for (index, rule, count) in [(0, "notifier.single", false), (1, "notifier.digest", true)] {
        match fixture.deliveries.get(index) {
            Some(delivery) => {
                report
                    .run(
                        rule,
                        "A rendered message is taken by the far side",
                        WITHIN,
                        async {
                            if delivery.notifications.is_empty()
                                || (count && delivery.notifications.len() < 2)
                            {
                                return Err(
                                "the fixture must carry one notification or, for a digest, several"
                                    .into(),
                            );
                            }
                            notifier
                                .deliver(delivery.clone())
                                .await
                                .map_err(|error| error.to_string().into())
                        },
                    )
                    .await
            }
            None => report.checked(
                rule,
                "A rendered message is taken by the far side",
                Err(skip("no disposable delivery supplied")),
            ),
        }
    }
    report
        .run(
            "notifier.link",
            "Only a linking notifier makes a code-bearing link and polls it",
            WITHIN,
            async {
                let url = notifier.link_url(&fixture.config, CODE);
                if descriptor.reaches == Reaches::Link {
                    if !url.as_deref().is_some_and(|url| url.contains(CODE)) {
                        return Err(
                            "a linking notifier did not put the requested code in its link".into(),
                        );
                    }
                    let poll = notifier
                        .poll_links(&fixture.config, None, NO_WAIT)
                        .await
                        .map_err(|error| error.to_string())?;
                    if poll
                        .started
                        .iter()
                        .any(|entry| entry.code.is_empty() || entry.label.is_empty())
                    {
                        return Err("a linked identity has no code or readable label".into());
                    }
                    Ok(())
                } else if url.is_some() {
                    Err("a notifier that does not link people returned a link".into())
                } else {
                    match notifier.poll_links(&fixture.config, None, NO_WAIT).await {
                        Err(error) if error.code == "unsupported" => Ok(()),
                        _ => Err("a notifier that does not link people accepted polling".into()),
                    }
                }
            },
        )
        .await;
    for (rule, delivery, retryable, gone) in [
        ("notifier.retry", fixture.retry.as_ref(), true, false),
        ("notifier.gone", fixture.gone.as_ref(), false, true),
        ("notifier.private", fixture.private.as_ref(), false, false),
    ] {
        match delivery {
            Some(delivery) => report.run(rule, "Its refusal preserves the far-side delivery decision", WITHIN, async {
                if rule == "notifier.private" && delivery.config.allowed_private_hosts.as_ref()
                    .is_none_or(|hosts| !hosts.is_empty())
                {
                    return Err("the private-address fixture must explicitly permit no private hosts".into());
                }
                match notifier.deliver(delivery.clone()).await {
                    Err(error) if !error.code.is_empty() && error.retryable == retryable && error.gone == gone => Ok(()),
                    Err(_) => Err("the refusal lost its retryable or gone classification".into()),
                    Ok(()) => Err("a delivery the far side must refuse was accepted".into()),
                }
            }).await,
            None => report.checked(rule, "Its refusal preserves the far-side delivery decision", Err(skip("no controlled refusal supplied"))),
        }
    }
}
