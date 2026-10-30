//! Certificate expiry notification.
//!
//! A machine renews its own certificate whenever it is connected and less than a
//! third of its life is left. One that is not connected does not, and its
//! certificate runs out; after the days the system still lets it renew, it has to
//! be enrolled again. So once a day the worker looks for the machines of people
//! whose certificate ends within `[pki] renewal_reminder_days`, or ended less than
//! `[pki] expired_renew_grace_days` ago, and tells their owner, once for each
//! certificate: a row in their Inbox, and the messages their routes tell at once.
//!
//! There is no kind of notification for a certificate; the machine that has not
//! connected to renew is the closest thing there is to a machine that is
//! offline, so the notification is of that kind and says why in its data.

use crate::schedule::{self, Job};
use crate::spans;
use chrono::{DateTime, Duration, Utc};
use orch_config::ServerConfig;
use orch_db::queries::notifications::outbox::Snapshot;
use orch_db::queries::notifications::people::{self, NewNotification};
use orch_db::queries::notifications::routes;
use orch_db::queries::notifications::upkeep::{self, ExpiringCertificate};
use orch_db::{Db, DbError};
use orch_types::Clock;
use serde_json::json;

/// The kind of notification a reminder is.
pub const KIND: &str = "machine.offline";

/// What a run of the job did.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Reminded {
    /// Machines whose certificate is about to end or ended lately.
    pub machines: u32,
    /// Owners who were told now, not before.
    pub told: u32,
}

/// The title and the first line of the reminder, in the language of the person
/// who owns the machine.
#[must_use]
pub fn words(
    language: &str,
    machine: &str,
    not_after: DateTime<Utc>,
    now: DateTime<Utc>,
    grace: Duration,
) -> (String, String) {
    let on = not_after.format("%Y-%m-%d").to_string();
    let ended = not_after <= now;
    let days_left = (grace - (now - not_after)).num_days().max(0);
    if language.to_ascii_lowercase().starts_with("zh") {
        if ended {
            (
                format!("{machine} 的证书已于 {on} 到期"),
                format!("它必须在 {days_left} 天内连接到系统才能续期，否则需要重新注册。"),
            )
        } else {
            (
                format!("{machine} 的证书将于 {on} 到期"),
                "请在此之前让它连接到系统，它会自动续期。".to_owned(),
            )
        }
    } else if ended {
        (
            format!("The certificate of {machine} ended on {on}"),
            format!(
                "It has to connect within {days_left} more days to be renewed; after that it has to be enrolled again."
            ),
        )
    } else {
        (
            format!("The certificate of {machine} ends on {on}"),
            "Connect it before then and it renews itself.".to_owned(),
        )
    }
}

/// Tells the person who owns each machine whose certificate is about to end, once for
/// each certificate, under the lock of the job. None where another run holds
/// the lock.
pub async fn certificates(
    db: &Db,
    clock: &dyn Clock,
    config: &ServerConfig,
    mail_shared: bool,
) -> Result<Option<Reminded>, DbError> {
    let Some(mut tx) = schedule::take(db, Job::CertReminders).await? else {
        return Ok(None);
    };
    let now = clock.now();
    let ahead = spans::days(config.pki.renewal_reminder_days);
    let grace = spans::days(config.pki.expired_renew_grace_days);
    let mut done = Reminded::default();
    for each in upkeep::expiring_certificates(&mut tx, now, ahead, grace).await? {
        done.machines += 1;
        if tell_owner(&mut tx, &each, now, grace, mail_shared).await? {
            done.told += 1;
        }
    }
    tx.commit().await.map_err(DbError::from)?;
    Ok(Some(done))
}

async fn tell_owner(
    conn: &mut orch_db::Conn,
    cert: &ExpiringCertificate,
    now: DateTime<Utc>,
    grace: Duration,
    mail_shared: bool,
) -> Result<bool, DbError> {
    let Some(owner) = people::recipient(conn, cert.owner_id).await? else {
        return Ok(false);
    };
    let (title, body) = words(&owner.locale, &cert.name, cert.not_after, now, grace);
    let data = json!({
        "reason": "certificate_expiring", "machine_id": cert.machine_id, "not_after": cert.not_after,
        "path": format!("/resources/machines/{}", cert.machine_id),
    });
    // One for each certificate: a machine that renewed has another end.
    let dedupe_key = format!(
        "{KIND}:certificate:{}:{}",
        cert.machine_id,
        cert.not_after.timestamp()
    );
    let Some(told) = people::announce(
        conn,
        &NewNotification {
            user_id: owner.id,
            kind: KIND,
            node_id: None,
            thread_id: None,
            approval_id: None,
            chat_id: None,
            title: &title,
            body: &body,
            data: &data,
            dedupe_key: &dedupe_key,
            at: now,
        },
    )
    .await?
    else {
        return Ok(false);
    };
    let snapshot = Snapshot {
        id: Some(told.id),
        kind: KIND.to_owned(),
        at: now.into(),
        title,
        body,
        node_id: None,
        thread_id: None,
        approval_id: None,
        chat_id: None,
        data,
    };
    routes::tell_routes(conn, owner.id, &snapshot, None, mail_shared, now).await?;
    Ok(true)
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    #[test]
    fn the_reminder_says_when_the_certificate_ends_or_ended_and_how_long_it_can_still_be_renewed() {
        let now = Utc.with_ymd_and_hms(2026, 10, 11, 12, 0, 0).unwrap();
        let grace = Duration::days(90);
        let ahead = Utc.with_ymd_and_hms(2026, 10, 20, 0, 0, 0).unwrap();
        let (title, body) = words("en", "Dana's laptop", ahead, now, grace);
        assert_eq!(title, "The certificate of Dana's laptop ends on 2026-10-20");
        assert!(body.contains("renews itself"));
        let past = Utc.with_ymd_and_hms(2026, 9, 21, 12, 0, 0).unwrap();
        let (title, body) = words("en", "Dana's laptop", past, now, grace);
        assert_eq!(
            title,
            "The certificate of Dana's laptop ended on 2026-09-21"
        );
        assert!(body.contains("within 70 more days"), "{body}");
        let (chinese, body) = words("zh-CN", "笔记本", ahead, now, grace);
        assert!(chinese.contains("到期") && chinese.contains("2026-10-20"));
        assert!(body.contains("续期"));
        assert!(words("zh-CN", "笔记本", past, now, grace).1.contains("70"));
    }
}
