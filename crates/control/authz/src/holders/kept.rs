//! The identities a person holds (their groups and places of page-only viewing among them), kept between requests
//! while they still hold.
//!
//! Working them out (`access_holders`) reads every place and every page of the system; every request and every
//! delivery of a live feed needs them. A set is kept by this process with what it was worked out under, and used
//! again only when the database says that nothing it was worked out from has changed in a way the asking statement
//! can see, and that no role or entry begins or stops counting between the two moments
//! ([`orch_db::queries::access::kept`]). Otherwise it is worked out again, so a change is seen by the next request,
//! as when nothing was kept. A set is kept only when the transaction that worked it out had written nothing: one that
//! had could see its own changes, which other requests must not see before it commits.

use crate::Error;
use orch_db::Conn;
use orch_db::queries::access::kept::{self, HolderSets};
use orch_types::{Id, Time};
use std::collections::HashMap;
use std::sync::{Arc, LazyLock, Mutex, PoisonError};
use std::time::{Duration, Instant};

/// The most sets this process keeps. Past it, the sets older than [`LONGEST`] go, then all of them.
const MOST: usize = 4096;
/// How long a set is kept at most. Neither bound decides what is right (a set is used only while it holds); they
/// bound the memory kept for people who stopped asking, and what a change written past the triggers that record
/// changes (a restore with triggers switched off) could leave unseen.
const LONGEST: Duration = Duration::from_secs(600);

struct Kept {
    sets: Arc<HolderSets>,
    made: Instant,
}

type HolderCache = HashMap<(Id, bool), Arc<Kept>>;

/// The sets kept, by person and whether their flag of system admin counted.
static KEPT: LazyLock<Mutex<HolderCache>> = LazyLock::new(Mutex::default);

/// The holder sets of `user` at `now`, `system` saying whether their flag of system admin counts: the kept ones when
/// they still hold, else worked out now. One statement either way.
pub(super) async fn holder_sets(
    conn: &mut Conn,
    user: Id,
    now: Time,
    system: bool,
) -> Result<Arc<HolderSets>, Error> {
    let key = (user, system);
    let found = KEPT
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .get(&key)
        .filter(|found| found.made.elapsed() < LONGEST && found.sets.stamp.covers(now.get()))
        .cloned();
    let Some(sets) = kept::holder_sets(
        conn,
        user,
        now.get(),
        system,
        found.as_ref().map(|found| &found.sets.stamp),
    )
    .await?
    else {
        // The database says the kept sets still hold; there are kept sets, since only they can hold.
        return match found {
            Some(found) => Ok(Arc::clone(&found.sets)),
            None => Err(Error::Db(orch_db::DbError::Rule {
                constraint: "kept_holder_sets".into(),
            })),
        };
    };
    let sets = Arc::new(sets);
    if sets.stamp.keepable() {
        let mut all = KEPT.lock().unwrap_or_else(PoisonError::into_inner);
        if all.len() >= MOST && !all.contains_key(&key) {
            all.retain(|_, kept| kept.made.elapsed() < LONGEST);
            if all.len() >= MOST {
                all.clear();
            }
        }
        all.insert(
            key,
            Arc::new(Kept {
                sets: Arc::clone(&sets),
                made: Instant::now(),
            }),
        );
    }
    Ok(sets)
}

#[cfg(test)]
mod tests {
    use super::*;
    use orch_db::queries::access::testing::{add_to_group, insert_group, insert_member};
    use orch_db::queries::hierarchy::testing::insert_node;
    use orch_db::queries::identity::testing::insert_user;
    use orch_testkit::{clock, database::TestDb};
    use orch_types::new_id;

    fn kept_of(user: Id) -> Option<Arc<Kept>> {
        KEPT.lock()
            .unwrap_or_else(PoisonError::into_inner)
            .get(&(user, false))
            .cloned()
    }

    #[tokio::test]
    async fn a_set_is_used_again_until_what_it_comes_from_changes_and_never_kept_from_a_writing_transaction()
     {
        let test = TestDb::fresh().await;
        let mut conn = test.pool.acquire().await.unwrap();
        let (ada, bo) = (new_id(), new_id());
        insert_user(&mut conn, ada, "ada").await.unwrap();
        insert_user(&mut conn, bo, "bob").await.unwrap();
        let root = new_id();
        insert_node(&mut conn, root, "org", None, "Acme", ada)
            .await
            .unwrap();
        insert_member(&mut conn, root, bo, root).await.unwrap();
        let now = Time::new(clock::start());

        let none = holder_sets(&mut conn, bo, now, false).await.unwrap();
        assert!(none.groups.is_empty() && none.viewers.is_empty());
        let first = kept_of(bo).expect("a set worked out outside a writing transaction is kept");
        holder_sets(&mut conn, bo, now, false).await.unwrap();
        assert!(
            Arc::ptr_eq(&first, &kept_of(bo).unwrap()),
            "nothing changed: the kept set is used"
        );
        sqlx::query("UPDATE nodes SET name = 'Acme Inc' WHERE id = $1")
            .bind(root)
            .execute(&mut *conn)
            .await
            .unwrap();
        holder_sets(&mut conn, bo, now, false).await.unwrap();
        assert!(
            Arc::ptr_eq(&first, &kept_of(bo).unwrap()),
            "a new name changes nothing it comes from"
        );

        let group = insert_group(&mut conn, root, "Ops", ada).await.unwrap();
        add_to_group(&mut conn, group, bo).await.unwrap();
        assert_eq!(
            holder_sets(&mut conn, bo, now, false).await.unwrap().groups,
            [group]
        );
        let second = kept_of(bo).unwrap();
        assert!(
            !Arc::ptr_eq(&first, &second),
            "a membership of a group is a change"
        );

        let other = insert_group(&mut conn, root, "Dev", ada).await.unwrap();
        let mut tx = sqlx::Connection::begin(&mut *conn).await.unwrap();
        add_to_group(&mut tx, other, bo).await.unwrap();
        let inside = holder_sets(&mut tx, bo, now, false)
            .await
            .unwrap()
            .groups
            .clone();
        assert_eq!(inside.len(), 2, "the transaction sees its own change");
        assert!(
            Arc::ptr_eq(&second, &kept_of(bo).unwrap()),
            "what a writing transaction worked out is not kept"
        );
        tx.rollback().await.unwrap();
        assert_eq!(
            holder_sets(&mut conn, bo, now, false).await.unwrap().groups,
            [group]
        );
    }
}
