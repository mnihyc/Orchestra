//! For a channel whose kind links people: ask who followed a link, only while a code waits, and link the route.
//!
//! A bot can write only to somebody who started a chat with it. So a route
//! through it is linked: the person asks for a link ([`start`]), the server makes
//! a code of 128 random bits, keeps its hash on the route for ten minutes, and
//! gives out the address that opens the bot with the code in it. When the person
//! presses Start there, the bot has been sent the code. The system never
//! listens for the vendor: [`follow`], the `bot_links` job, asks the notifier
//! who followed its links, and only for a channel that has a route with a code
//! that is still good. No code waiting, no request. A code that comes back and
//! is good gives its route the address the notifier learned and the name of
//! whoever followed, and is spent; a code nobody asked for, or one past its
//! time, matches nothing and is dropped.

use crate::channels::{ChannelError, Notifiers, config_of_channel};
use crate::schedule::Job;
use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use chrono::{DateTime, Duration, Utc};
use orch_db::queries::job_leases::Lease;
use orch_db::queries::notifications::channels::{self as rows, ChannelRow, owner};
use orch_db::queries::notifications::routes;
use orch_db::queries::transaction_guard;
use orch_db::secrets::MasterKey;
use orch_db::{Db, DbError};
use orch_notifier_contract::delivery::{Config, LinkStarted};
use orch_types::{Clock, Id, Json};
use sha2::{Digest, Sha256};

/// How long a code is good: long enough to open a phone and press Start, short
/// enough that a link left lying about is of no use.
pub const LINK_LIFE: Duration = Duration::minutes(10);

/// How long one reading of who followed a link may wait for an answer, which the
/// notifier is told: a long poll, kept under the minute that proxies and
/// firewalls leave a quiet connection.
pub const POLL_WAIT: std::time::Duration = std::time::Duration::from_secs(25);

/// How much longer than it was told to wait a notifier is given before it is
/// given up on.
pub const POLL_GRACE: std::time::Duration = std::time::Duration::from_secs(15);

/// The random bytes of a code: 128 bits.
const CODE_BYTES: usize = 16;

/// Why a link could not be made or followed.
#[derive(Debug, thiserror::Error)]
pub enum LinkError {
    /// The database refused.
    #[error(transparent)]
    Db(#[from] DbError),
    /// A channel could not be read.
    #[error(transparent)]
    Channel(#[from] ChannelError),
    /// The notifier of the channel does not link people.
    #[error("{0} does not link people")]
    NotLinkable(String),
}

/// A code to be sent to a bot, as the person is to type nothing of: it is part
/// of the address they open.
#[must_use]
pub fn new_code() -> String {
    let bytes: [u8; CODE_BYTES] = rand::random();
    URL_SAFE_NO_PAD.encode(bytes)
}

/// The hash a code is kept as on the route: nothing of the code itself is kept.
#[must_use]
pub fn hash_code(code: &str) -> [u8; 32] {
    Sha256::digest(code.as_bytes()).into()
}

/// The address a person opens to be linked, and until when it counts.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Started {
    /// The address that opens the bot with the code.
    pub url: String,
    /// When the code stops counting.
    pub expires_at: DateTime<Utc>,
}

/// Gives the route of `person` through `channel` a new code and the address
/// that carries it, making the route (off) if they have none. A code asked for
/// before is replaced. With `master`, the code is kept sealed until it lapses,
/// so that the person's own list shows the same link again.
pub async fn start(
    conn: &mut orch_db::Conn,
    notifiers: &Notifiers,
    master: Option<&MasterKey>,
    person: Id,
    channel: &ChannelRow,
    now: DateTime<Utc>,
) -> Result<Started, LinkError> {
    let notifier = notifiers
        .get(&channel.kind)
        .ok_or_else(|| ChannelError::UnknownKind(channel.kind.clone()))?;
    if !notifier.describe().reaches.links_people() {
        return Err(LinkError::NotLinkable(channel.kind.clone()));
    }
    let code = new_code();
    // Only the values of the channel, which hold what the address is made of; the
    // secrets are not needed to make it.
    let config = Config {
        values: channel.config.clone(),
        ..Config::default()
    };
    let url = notifier
        .link_url(&config, &code)
        .ok_or_else(|| LinkError::NotLinkable(channel.kind.clone()))?;
    let route = routes::put_route(
        conn,
        orch_types::new_id(),
        person,
        channel.id,
        &routes::RouteChange::default(),
        now,
    )
    .await?;
    let expires_at = now + LINK_LIFE;
    routes::start_link(conn, route.id, &hash_code(&code), expires_at, now).await?;
    if let Some(master) = master {
        // The code is kept sealed, for its person's own list to show the same link again until
        // it lapses or is pressed.
        let name = link_secret_name(route.id);
        let sealed = crate::channels::seal_secret(master, owner::CHANNEL, channel.id, &name, &code)
            .map_err(|_| LinkError::Channel(ChannelError::Sealed { name: name.clone() }))?;
        orch_db::queries::settings::put_secret(
            conn,
            owner::CHANNEL,
            channel.id,
            &name,
            &sealed,
            Some(person),
            now,
        )
        .await?;
    }
    Ok(Started { url, expires_at })
}

/// What the names of kept link codes begin with, among the secrets of their channel.
pub const LINK_SECRET_PREFIX: &str = "route_link:";

/// The name a route's waiting code is kept under, sealed, among its channel's secrets.
#[must_use]
pub fn link_secret_name(route: Id) -> String {
    format!("{LINK_SECRET_PREFIX}{route}")
}

/// The link of a route that waits to be pressed, made again from its kept code: none where no
/// code is kept or it does not open.
pub async fn kept_url(
    conn: &mut orch_db::Conn,
    master: &MasterKey,
    notifiers: &Notifiers,
    channel: &ChannelRow,
    route: Id,
) -> Result<Option<String>, LinkError> {
    let Some(notifier) = notifiers.get(&channel.kind) else {
        return Ok(None);
    };
    let name = link_secret_name(route);
    let Some(kept) =
        orch_db::queries::settings::secret(conn, owner::CHANNEL, channel.id, &name).await?
    else {
        return Ok(None);
    };
    let Some(code) = master
        .open(
            &kept.sealed,
            &crate::channels::secret_binding(owner::CHANNEL, channel.id, &name),
        )
        .ok()
        .and_then(|bytes| String::from_utf8(bytes).ok())
    else {
        return Ok(None);
    };
    let config = Config {
        values: channel.config.clone(),
        ..Config::default()
    };
    Ok(notifier.link_url(&config, &code))
}

/// What a run of the job did.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Followed {
    /// Channels whose notifier was asked.
    pub asked: u32,
    /// Routes that were linked.
    pub linked: u32,
}

/// The address a route keeps of a follower: what the notifier learned, with the
/// name of whoever followed beside it so that the person sees it is theirs.
fn address_of(started: &LinkStarted) -> Json {
    let mut address = match &started.address {
        Json::Object(members) => members.clone(),
        other => {
            let mut members = serde_json::Map::new();
            members.insert("address".to_owned(), other.clone());
            members
        }
    };
    address.insert("label".to_owned(), Json::String(started.label.clone()));
    Json::Object(address)
}

/// How long a reading's lease holds before it is renewed after each channel: two minutes, three times
/// the longest a notifier is given ([`POLL_WAIT`] and [`POLL_GRACE`]), so that a worker that died
/// leaves the readings to another well before a code ([`LINK_LIFE`]) runs out.
const FOLLOW_LEASE: Duration = Duration::minutes(2);

/// Asks the notifier of each channel with a code waiting who followed its
/// link, links the routes whose code came back, and keeps where the reading
/// ended. Runs under the lease of the job, so one reading is waiting at a time.
pub async fn follow(
    db: &Db,
    clock: &dyn Clock,
    notifiers: &Notifiers,
    master: &MasterKey,
) -> Result<Followed, LinkError> {
    let lease = Lease::for_run(Job::BotLinks.key(), FOLLOW_LEASE);
    if !lease.hold(db, clock.now()).await? {
        return Ok(Followed::default());
    }
    let followed = follow_held(db, clock, notifiers, master, &lease).await;
    lease.release(db).await?;
    followed
}

/// A short transaction under the exclusive authority gate, which each write of the job takes. None is
/// open while a notifier is asked: a reading may wait for most of a minute, and the gate stops every
/// action of the system meanwhile.
async fn gated(db: &Db) -> Result<orch_db::Tx, DbError> {
    let mut tx = transaction_guard::scope(false, db.begin()).await?;
    transaction_guard::lock_authority(&mut tx).await?;
    Ok(tx)
}

async fn follow_held(
    db: &Db,
    clock: &dyn Clock,
    notifiers: &Notifiers,
    master: &MasterKey,
    lease: &Lease,
) -> Result<Followed, LinkError> {
    let mut done = Followed::default();
    let channels = {
        let mut tx = gated(db).await?;
        routes::drop_expired_codes(&mut tx, clock.now()).await?;
        let channels = rows::channels_with_waiting_codes(&mut tx, clock.now()).await?;
        tx.commit().await.map_err(DbError::from)?;
        channels
    };
    for channel in channels {
        let Some(notifier) = notifiers.get(&channel.kind) else {
            continue;
        };
        if !notifier.describe().reaches.links_people() {
            continue;
        }
        let config = {
            let mut conn = db.acquire().await?;
            config_of_channel(&mut conn, master, &channel).await
        };
        let config = match config {
            Ok(config) => config,
            Err(error @ ChannelError::Sealed { .. }) => {
                let mut tx = gated(db).await?;
                rows::record_channel_error(&mut tx, channel.id, &error.to_string()).await?;
                tx.commit().await.map_err(DbError::from)?;
                continue;
            }
            Err(error) => return Err(error.into()),
        };
        done.asked += 1;
        let asked = tokio::time::timeout(
            POLL_WAIT + POLL_GRACE,
            notifier.poll_links(&config, channel.link_cursor.clone(), POLL_WAIT),
        )
        .await;
        let mut tx = gated(db).await?;
        match asked {
            Ok(Ok(poll))
                if serde_json::to_value(&poll).ok().is_none_or(|value| {
                    crate::channels::contains_secret(&value, &config.secrets)
                }) =>
            {
                rows::record_channel_error(
                    &mut tx,
                    channel.id,
                    "the notifier returned an unsafe result",
                )
                .await?;
            }
            Ok(Ok(poll)) => {
                // The codes that wait are looked at when the answer is in, since the
                // reading may have taken a while.
                let now = clock.now();
                let waiting = routes::waiting_codes(&mut tx, channel.id, now).await?;
                for started in &poll.started {
                    let hash = hash_code(&started.code);
                    if let Some(route) = waiting.iter().find(|each| each.link_hash == hash)
                        && routes::complete_link(
                            &mut tx,
                            route.route_id,
                            &hash,
                            &address_of(started),
                            now,
                        )
                        .await?
                    {
                        done.linked += 1;
                    }
                }
                rows::set_link_cursor(&mut tx, channel.id, poll.cursor.as_deref()).await?;
            }
            Ok(Err(error)) => {
                let error = crate::channels::scrub_error(error, &config.secrets);
                rows::record_channel_error(&mut tx, channel.id, &crate::outbox::error_line(&error))
                    .await?;
            }
            Err(_) => {
                rows::record_channel_error(
                    &mut tx,
                    channel.id,
                    "the reading of who followed the link did not answer in time",
                )
                .await?;
            }
        }
        tx.commit().await.map_err(DbError::from)?;
        if !lease.hold(db, clock.now()).await? {
            break;
        }
    }
    Ok(done)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn a_code_is_128_random_bits_in_characters_a_link_can_carry_and_is_kept_only_as_a_hash() {
        let (a, b) = (new_code(), new_code());
        assert_ne!(a, b);
        assert_eq!(a.len(), 22, "16 bytes in base64url without padding");
        assert!(
            a.chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
        );
        assert_eq!(URL_SAFE_NO_PAD.decode(&a).unwrap().len(), 16);
        assert_eq!(hash_code(&a), hash_code(&a));
        assert_ne!(hash_code(&a), hash_code(&b));
        assert!(!hex_of(&hash_code(&a)).contains(&a));
    }

    fn hex_of(bytes: &[u8]) -> String {
        bytes.iter().map(|b| format!("{b:02x}")).collect()
    }

    #[test]
    fn whoever_followed_is_named_beside_the_address_the_notifier_learned() {
        let followed = LinkStarted {
            code: "c".into(),
            address: json!({ "chat_id": 4242 }),
            label: "Dana".into(),
        };
        assert_eq!(
            address_of(&followed),
            json!({ "chat_id": 4242, "label": "Dana" })
        );
        let bare = LinkStarted {
            address: json!("room-9"),
            ..followed
        };
        assert_eq!(
            address_of(&bare),
            json!({ "address": "room-9", "label": "Dana" })
        );
    }
}
