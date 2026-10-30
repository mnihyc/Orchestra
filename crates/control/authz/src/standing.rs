//! A person's standing in the system: suspended altogether or not, and a system admin or not; the acts that change it, whoever does them.
//!
//! A system admin does these in Management, and the operator on the host
//! when nobody can sign in. They are one function each, so that a person is
//! left in the same state whichever way it was done: a suspension ends every
//! session of the person at once (an API token is refused for as long as the
//! person is suspended, and works again when they are not), the system keeps
//! an active system admin, the authorization epoch moves so that whatever is
//! held open on the strength of the old standing is looked at again, and the
//! audit trail has its line, written by the actor given. An act is done in
//! the transaction of the caller; one that changes nothing writes nothing.

use crate::epoch;
use chrono::{DateTime, Utc};
use orch_access_contract::audit::AuditAction;
use orch_db::audit::{self, Actor, Entry, Origin};
use orch_db::queries::identity::{self, Standing, UserRow};
use orch_db::{Conn, DbError};
use orch_types::{Id, Json};
use serde_json::json;

/// Who does an act, from where, and when.
#[derive(Debug, Clone, Copy)]
pub struct Acting<'a> {
    /// The actor the audit line names: a system admin, or the system itself
    /// where the operator acts on the host.
    pub by: Actor,
    /// Where the act came from.
    pub origin: &'a Origin,
    /// When it is done.
    pub now: DateTime<Utc>,
}

/// What an act came to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Done {
    /// Whether the person's standing changed; false where it was what was
    /// asked for already.
    pub changed: bool,
    /// How many sessions the act ended: those of a person who was suspended.
    pub sessions_ended: u64,
}

/// Why an act was not done.
#[derive(Debug, thiserror::Error)]
pub enum Refused {
    /// The database failed.
    #[error(transparent)]
    Db(#[from] DbError),
    /// The authorization epoch could not be moved.
    #[error(transparent)]
    Authz(#[from] crate::Error),
    /// There is no such person: a service identity is nobody here.
    #[error("no such user: {0}")]
    NoSuchUser(Id),
    /// The act would leave the system without an active system admin.
    #[error("{username} is the last system admin who can sign in")]
    LastAdmin {
        /// The person the act was about.
        username: String,
    },
}

const NOTHING_CHANGED: Done = Done {
    changed: false,
    sessions_ended: 0,
};

/// The person `user`, who has to be a human being.
async fn person(conn: &mut Conn, user: Id) -> Result<UserRow, Refused> {
    identity::user_by_id(conn, user)
        .await?
        .filter(|row| row.kind == "human")
        .ok_or(Refused::NoSuchUser(user))
}

/// What the database said of a change: whether it was made.
fn made(standing: Standing, who: &UserRow) -> Result<bool, Refused> {
    match standing {
        Standing::Changed => Ok(true),
        Standing::AsItWas => Ok(false),
        Standing::LastAdmin => Err(Refused::LastAdmin {
            username: who.username.clone(),
        }),
    }
}

/// Moves the epoch and writes the line of an act whose change was made.
async fn recorded(
    conn: &mut Conn,
    acting: &Acting<'_>,
    action: AuditAction,
    who: &UserRow,
    summary: &str,
    details: Json,
) -> Result<(), Refused> {
    epoch::bump(conn).await?;
    let mut entry = Entry::new(
        acting.by,
        action.as_str(),
        action.object_kind().unwrap_or("user"),
        Some(who.id),
        summary,
    );
    entry.history = action.is_history_line();
    entry.details = details;
    audit::append(conn, &entry, acting.origin, acting.now).await?;
    Ok(())
}

/// Suspends a person altogether: they can no longer sign in anywhere, every
/// session of theirs ends now, and what they own stays theirs. The last
/// system admin who can sign in is never suspended.
pub async fn suspend(conn: &mut Conn, user: Id, acting: &Acting<'_>) -> Result<Done, Refused> {
    let who = person(conn, user).await?;
    let standing = identity::set_suspended(conn, user, true, acting.now).await?;
    if !made(standing, &who)? {
        return Ok(NOTHING_CHANGED);
    }
    let sessions_ended = identity::revoke_sessions_of(conn, user, None, acting.now).await?;
    recorded(
        conn,
        acting,
        orch_db::audit::action!(AuditAction::UserSuspended),
        &who,
        &format!("{} was suspended", who.username),
        json!({ "sessions_ended": sessions_ended }),
    )
    .await?;
    Ok(Done {
        changed: true,
        sessions_ended,
    })
}

/// Lifts the suspension of a person. Their sessions, which the suspension
/// ended, are not brought back: they sign in again.
pub async fn resume(conn: &mut Conn, user: Id, acting: &Acting<'_>) -> Result<Done, Refused> {
    let who = person(conn, user).await?;
    let standing = identity::set_suspended(conn, user, false, acting.now).await?;
    if !made(standing, &who)? {
        return Ok(NOTHING_CHANGED);
    }
    recorded(
        conn,
        acting,
        orch_db::audit::action!(AuditAction::UserResumed),
        &who,
        &format!("the suspension of {} was lifted", who.username),
        json!({}),
    )
    .await?;
    Ok(Done {
        changed: true,
        sessions_ended: 0,
    })
}

/// Makes a person a system admin.
pub async fn grant_admin(conn: &mut Conn, user: Id, acting: &Acting<'_>) -> Result<Done, Refused> {
    set_admin(conn, user, true, acting).await
}

/// Makes a system admin an ordinary person again. The last system admin who
/// can sign in stays one.
pub async fn revoke_admin(conn: &mut Conn, user: Id, acting: &Acting<'_>) -> Result<Done, Refused> {
    set_admin(conn, user, false, acting).await
}

async fn set_admin(
    conn: &mut Conn,
    user: Id,
    admin: bool,
    acting: &Acting<'_>,
) -> Result<Done, Refused> {
    let who = person(conn, user).await?;
    let standing = identity::set_system_admin(conn, user, admin, acting.now).await?;
    if !made(standing, &who)? {
        return Ok(NOTHING_CHANGED);
    }
    let (action, summary) = if admin {
        (
            AuditAction::UserAdminGranted,
            format!("{} was made a system admin", who.username),
        )
    } else {
        (
            orch_db::audit::action!(AuditAction::UserAdminRevoked),
            format!("{} stopped being a system admin", who.username),
        )
    };
    recorded(conn, acting, action, &who, &summary, json!({})).await?;
    Ok(Done {
        changed: true,
        sessions_ended: 0,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Duration;
    use orch_db::queries::identity::testing::{insert_session, insert_user, make_admin};
    use orch_testkit::{clock, database::TestDb};
    use orch_types::new_id;

    fn by(actor: Actor, origin: &Origin) -> Acting<'_> {
        Acting {
            by: actor,
            origin,
            now: clock::start(),
        }
    }

    async fn person(conn: &mut Conn, name: &str, admin: bool) -> Id {
        let id = new_id();
        insert_user(conn, id, name).await.unwrap();
        if admin {
            make_admin(conn, id).await.unwrap();
        }
        id
    }

    async fn live_sessions_of(conn: &mut Conn, user: Id) -> usize {
        identity::live_sessions(conn, user, clock::start())
            .await
            .unwrap()
            .len()
    }

    async fn lines(conn: &mut Conn) -> Vec<(String, String, Option<Id>)> {
        sqlx::query_as("SELECT action, actor_kind, actor_id FROM audit_log ORDER BY at, id")
            .fetch_all(conn)
            .await
            .unwrap()
    }

    #[tokio::test]
    async fn suspending_a_person_ends_every_session_of_theirs_moves_the_epoch_and_leaves_its_line()
    {
        let test = TestDb::fresh().await;
        let mut conn = test.pool.acquire().await.unwrap();
        let boss = person(&mut conn, "boss", true).await;
        let ada = person(&mut conn, "ada", false).await;
        for token in [b"t1", b"t2"] {
            let until = clock::start() + Duration::days(1);
            insert_session(&mut conn, new_id(), ada, token, until)
                .await
                .unwrap();
        }
        let epoch_before = epoch::current(&mut conn).await.unwrap();
        let origin = Origin::default();

        let done = suspend(&mut conn, ada, &by(Actor::User(boss), &origin))
            .await
            .unwrap();

        assert_eq!(
            done,
            Done {
                changed: true,
                sessions_ended: 2
            }
        );
        assert_eq!(live_sessions_of(&mut conn, ada).await, 0);
        let row = identity::user_by_id(&mut conn, ada).await.unwrap().unwrap();
        assert!(row.suspended());
        assert_eq!(epoch::current(&mut conn).await.unwrap(), epoch_before + 1);
        assert_eq!(
            lines(&mut conn).await,
            vec![("user.suspended".to_owned(), "user".to_owned(), Some(boss))]
        );
    }

    #[tokio::test]
    async fn lifting_a_suspension_lets_the_person_sign_in_again_but_does_not_bring_back_their_sessions()
     {
        let test = TestDb::fresh().await;
        let mut conn = test.pool.acquire().await.unwrap();
        let ada = person(&mut conn, "ada", false).await;
        let until = clock::start() + Duration::days(1);
        insert_session(&mut conn, new_id(), ada, b"t", until)
            .await
            .unwrap();
        let origin = Origin::default();
        let acting = by(Actor::System, &origin);
        suspend(&mut conn, ada, &acting).await.unwrap();

        let done = resume(&mut conn, ada, &acting).await.unwrap();

        assert!(done.changed);
        let row = identity::user_by_id(&mut conn, ada).await.unwrap().unwrap();
        assert!(!row.suspended());
        assert_eq!(live_sessions_of(&mut conn, ada).await, 0);
        let kinds: Vec<String> = lines(&mut conn).await.into_iter().map(|l| l.0).collect();
        assert_eq!(kinds, ["user.suspended", "user.resumed"]);
    }

    #[tokio::test]
    async fn an_act_that_changes_nothing_writes_nothing_and_moves_nothing() {
        let test = TestDb::fresh().await;
        let mut conn = test.pool.acquire().await.unwrap();
        let ada = person(&mut conn, "ada", false).await;
        let before = epoch::current(&mut conn).await.unwrap();
        let origin = Origin::default();
        let acting = by(Actor::System, &origin);

        let resumed = resume(&mut conn, ada, &acting).await.unwrap();
        let unmade = revoke_admin(&mut conn, ada, &acting).await.unwrap();

        assert!(!resumed.changed && !unmade.changed);
        assert!(lines(&mut conn).await.is_empty());
        assert_eq!(epoch::current(&mut conn).await.unwrap(), before);
    }

    #[tokio::test]
    async fn the_last_system_admin_who_can_sign_in_is_neither_suspended_nor_made_an_ordinary_person()
     {
        let test = TestDb::fresh().await;
        let mut conn = test.pool.acquire().await.unwrap();
        let only = person(&mut conn, "only", true).await;
        let origin = Origin::default();
        let acting = by(Actor::System, &origin);

        let suspended = suspend(&mut conn, only, &acting).await;
        let revoked = revoke_admin(&mut conn, only, &acting).await;

        assert!(matches!(suspended, Err(Refused::LastAdmin { .. })));
        assert!(matches!(revoked, Err(Refused::LastAdmin { .. })));
        let row = identity::user_by_id(&mut conn, only)
            .await
            .unwrap()
            .unwrap();
        assert!(row.is_system_admin && !row.suspended());
        assert!(
            lines(&mut conn).await.is_empty(),
            "a refusal leaves no line"
        );
    }

    #[tokio::test]
    async fn another_admin_who_can_sign_in_frees_the_first_to_go_and_a_suspended_one_does_not_count()
     {
        let test = TestDb::fresh().await;
        let mut conn = test.pool.acquire().await.unwrap();
        let first = person(&mut conn, "first", true).await;
        let second = person(&mut conn, "second", true).await;
        let origin = Origin::default();
        let acting = by(Actor::System, &origin);
        suspend(&mut conn, second, &acting).await.unwrap();

        // The second cannot sign in, so the first is still the last.
        let refused = revoke_admin(&mut conn, first, &acting).await;
        assert!(matches!(refused, Err(Refused::LastAdmin { .. })));

        resume(&mut conn, second, &acting).await.unwrap();
        let done = revoke_admin(&mut conn, first, &acting).await.unwrap();
        assert!(done.changed);
        // And now the second is the last.
        let refused = suspend(&mut conn, second, &acting).await;
        assert!(matches!(refused, Err(Refused::LastAdmin { .. })));
    }

    #[tokio::test]
    async fn making_and_unmaking_a_system_admin_leaves_a_line_each_by_the_actor_given() {
        let test = TestDb::fresh().await;
        let mut conn = test.pool.acquire().await.unwrap();
        let _boss = person(&mut conn, "boss", true).await;
        let ada = person(&mut conn, "ada", false).await;
        let origin = Origin::default();
        let acting = by(Actor::System, &origin);

        grant_admin(&mut conn, ada, &acting).await.unwrap();
        revoke_admin(&mut conn, ada, &acting).await.unwrap();

        assert_eq!(
            lines(&mut conn).await,
            vec![
                ("user.admin_granted".to_owned(), "system".to_owned(), None),
                ("user.admin_revoked".to_owned(), "system".to_owned(), None),
            ]
        );
    }

    #[tokio::test]
    async fn nobody_is_found_where_there_is_no_such_human_user() {
        let test = TestDb::fresh().await;
        let mut conn = test.pool.acquire().await.unwrap();
        let missing = new_id();
        let origin = Origin::default();

        let refused = suspend(&mut conn, missing, &by(Actor::System, &origin)).await;

        assert!(matches!(refused, Err(Refused::NoSuchUser(id)) if id == missing));
    }
}
