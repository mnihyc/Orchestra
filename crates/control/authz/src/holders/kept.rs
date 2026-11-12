//! The identities a person holds (their groups and places of page-only viewing among them), kept between requests
//! while they still hold.
//!
//! Working them out (`access_holders`) reads every place and every page of the system; every request and every
//! delivery of a live feed needs them. A set is kept by this process with what it was worked out under, and used
//! again only when the database says that nothing it was worked out from has changed in a way the asking statement
//! can see, and that no role or entry begins or stops counting between the two moments
//! ([`orch_db::queries::access::kept`], which also lists the triggers that record every change a set comes from).
//! Otherwise it is worked out again, so a request sees every change its snapshot sees, as when nothing was kept. A set
//! is kept only when the transaction that worked it out had recorded no change of a row it comes from: one that had
//! could see its own changes, which other requests must not see before it commits. A transaction that only locked
//! rows (a request locks its own credential before it reads anything else), or wrote rows nothing is read from, has
//! changed nothing, and its set is kept. One whose identifier lies before the horizon of its snapshot (a read
//! committed transaction that another passed after its first lock or write) keeps nothing: a change it made later
//! would go unseen by the check on its set.

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
    use orch_db::queries::access::PageKey;
    use orch_db::queries::access::testing::{add_entry, add_to_group, insert_group, insert_member};
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

    #[tokio::test]
    async fn a_set_worked_out_after_a_row_lock_is_kept_and_one_worked_out_after_a_change_of_its_own_is_not()
     {
        let test = TestDb::fresh().await;
        let mut conn = test.pool.acquire().await.unwrap();
        let (ada, bo, cy) = (new_id(), new_id(), new_id());
        for (id, name) in [(ada, "ada"), (bo, "bob"), (cy, "cyd")] {
            insert_user(&mut conn, id, name).await.unwrap();
        }
        let root = new_id();
        insert_node(&mut conn, root, "org", None, "Acme", ada)
            .await
            .unwrap();
        insert_member(&mut conn, root, bo, root).await.unwrap();
        insert_member(&mut conn, root, cy, root).await.unwrap();
        let now = Time::new(clock::start());

        // A request's read: a repeatable snapshot whose first statement locks the row of the caller. The lock
        // gives the transaction an identifier and changes nothing the set comes from.
        let mut tx = sqlx::Connection::begin(&mut *conn).await.unwrap();
        sqlx::query("SET TRANSACTION ISOLATION LEVEL REPEATABLE READ")
            .execute(&mut *tx)
            .await
            .unwrap();
        sqlx::query("SELECT id FROM users WHERE id = $1 FOR KEY SHARE")
            .bind(bo)
            .fetch_one(&mut *tx)
            .await
            .unwrap();
        holder_sets(&mut tx, bo, now, false).await.unwrap();
        tx.rollback().await.unwrap();
        assert!(
            kept_of(bo).is_some(),
            "a set worked out by a transaction that only locked a row is kept"
        );

        // The same transaction after it changed what the set comes from sees its own change: nothing is kept.
        let mut tx = sqlx::Connection::begin(&mut *conn).await.unwrap();
        sqlx::query("SET TRANSACTION ISOLATION LEVEL REPEATABLE READ")
            .execute(&mut *tx)
            .await
            .unwrap();
        sqlx::query("SELECT id FROM users WHERE id = $1 FOR KEY SHARE")
            .bind(cy)
            .fetch_one(&mut *tx)
            .await
            .unwrap();
        let group = insert_group(&mut tx, root, "Ops", ada).await.unwrap();
        add_to_group(&mut tx, group, cy).await.unwrap();
        assert_eq!(
            holder_sets(&mut tx, cy, now, false).await.unwrap().groups,
            [group],
            "the transaction sees its own change"
        );
        tx.rollback().await.unwrap();
        assert!(
            kept_of(cy).is_none(),
            "what a transaction worked out after a change of its own is not kept"
        );
    }

    /// A world for the cases that follow: an org with a topic everyone may view, two people in the org, two groups of
    /// the org, and the connections that cases hold open together.
    struct Scene {
        test: TestDb,
        bo: Id,
        cy: Id,
        topic: Id,
        group: Id,
        other: Id,
        now: Time,
    }

    async fn scene() -> Scene {
        let test = TestDb::fresh().await;
        let mut conn = test.pool.acquire().await.unwrap();
        let (ada, bo, cy) = (new_id(), new_id(), new_id());
        for (id, name) in [(ada, "ada"), (bo, "bob"), (cy, "cyd")] {
            insert_user(&mut conn, id, name).await.unwrap();
        }
        let (root, topic) = (new_id(), new_id());
        insert_node(&mut conn, root, "org", None, "Acme", ada)
            .await
            .unwrap();
        insert_node(&mut conn, topic, "topic", Some(root), "Plans", ada)
            .await
            .unwrap();
        insert_member(&mut conn, root, bo, root).await.unwrap();
        insert_member(&mut conn, root, cy, root).await.unwrap();
        let group = insert_group(&mut conn, root, "Ops", ada).await.unwrap();
        let other = insert_group(&mut conn, root, "Dev", ada).await.unwrap();
        let now = Time::new(clock::start());
        // Everyone who is a person may view the topic: a person who exists has a place of page-only viewing.
        add_entry(
            &mut conn,
            &PageKey::new("node", topic, "live"),
            "any",
            "topic.view",
            ("everyone", None, None),
            None,
            now.get(),
        )
        .await
        .unwrap();
        drop(conn);
        Scene {
            test,
            bo,
            cy,
            topic,
            group,
            other,
            now,
        }
    }

    /// The groups and the places of page-only viewing worked out afresh, with nothing kept.
    async fn afresh(conn: &mut Conn, person: Id, now: Time) -> (Vec<Id>, Vec<Id>) {
        let rows = orch_db::queries::access::effective_holder_sets(conn, person, now.get(), false)
            .await
            .unwrap();
        let pick = |kind: &str| {
            rows.iter()
                .filter(|(each, _)| each == kind)
                .map(|(_, id)| *id)
                .collect::<Vec<_>>()
        };
        (pick("group"), pick("visible"))
    }

    /// The sets the cache gives, which must be those worked out afresh by the same connection.
    async fn same(conn: &mut Conn, person: Id, now: Time, step: &str) -> Arc<HolderSets> {
        let sets = holder_sets(conn, person, now, false).await.unwrap();
        let (groups, viewers) = afresh(conn, person, now).await;
        assert_eq!(sets.groups, groups, "{step}: groups");
        assert_eq!(sets.viewers, viewers, "{step}: places of viewing");
        sets
    }

    /// What a request does first on its snapshot: a repeatable read whose first statement locks the caller's row.
    async fn lock_first(tx: &mut Conn, person: Id) {
        sqlx::query("SET TRANSACTION ISOLATION LEVEL REPEATABLE READ")
            .execute(&mut *tx)
            .await
            .unwrap();
        lock_row(tx, person).await;
    }

    async fn lock_row(tx: &mut Conn, person: Id) {
        sqlx::query("SELECT id FROM users WHERE id = $1 FOR KEY SHARE")
            .bind(person)
            .fetch_one(tx)
            .await
            .unwrap();
    }

    async fn compute_after_a_lock_then_write_then_end(commit: bool) {
        let s = scene().await;
        let mut held = s.test.pool.acquire().await.unwrap();
        let mut elsewhere = s.test.pool.acquire().await.unwrap();
        let mut tx = sqlx::Connection::begin(&mut *held).await.unwrap();
        lock_first(&mut tx, s.bo).await;
        same(&mut tx, s.bo, s.now, "before its change").await;
        let kept = kept_of(s.bo).expect("a set worked out after only a lock is kept");
        add_to_group(&mut tx, s.group, s.bo).await.unwrap();
        let outside = same(&mut elsewhere, s.bo, s.now, "elsewhere, the change open").await;
        assert!(outside.groups.is_empty(), "nothing uncommitted is seen");
        let inside = same(&mut tx, s.bo, s.now, "inside, after its change").await;
        assert_eq!(inside.groups, [s.group]);
        assert!(
            Arc::ptr_eq(&kept, &kept_of(s.bo).unwrap()),
            "what it worked out after its own change is not kept"
        );
        let expected = if commit {
            tx.commit().await.unwrap();
            vec![s.group]
        } else {
            tx.rollback().await.unwrap();
            Vec::new()
        };
        let after = same(&mut elsewhere, s.bo, s.now, "after the end").await;
        assert_eq!(after.groups, expected, "commit: {commit}");
    }

    #[tokio::test]
    async fn a_set_worked_out_after_a_lock_and_before_a_change_gives_way_to_that_change_when_it_commits()
     {
        compute_after_a_lock_then_write_then_end(true).await;
    }

    #[tokio::test]
    async fn a_set_worked_out_after_a_lock_and_before_a_change_stays_right_when_that_change_rolls_back()
     {
        compute_after_a_lock_then_write_then_end(false).await;
    }

    #[tokio::test]
    async fn a_writer_that_commits_between_the_snapshot_of_a_set_and_its_keeping_is_seen_by_the_next_reader()
     {
        let s = scene().await;
        let mut held = s.test.pool.acquire().await.unwrap();
        let mut writer = s.test.pool.acquire().await.unwrap();
        let mut reader = s.test.pool.acquire().await.unwrap();
        let mut tx = sqlx::Connection::begin(&mut *held).await.unwrap();
        lock_first(&mut tx, s.bo).await;
        // The snapshot is taken; a writer commits after it and before the set is worked out and kept under it.
        add_to_group(&mut writer, s.group, s.bo).await.unwrap();
        let old = holder_sets(&mut tx, s.bo, s.now, false).await.unwrap();
        assert!(
            old.groups.is_empty(),
            "the snapshot is older than the writer"
        );
        tx.commit().await.unwrap();
        assert!(
            kept_of(s.bo).is_some(),
            "the set of the old snapshot is kept"
        );
        let next = same(&mut reader, s.bo, s.now, "the next reader").await;
        assert_eq!(next.groups, [s.group], "the kept set does not hold for it");
    }

    #[tokio::test]
    async fn a_read_committed_transaction_that_another_passes_before_its_holder_statement_keeps_nothing()
     {
        let s = scene().await;
        let mut held = s.test.pool.acquire().await.unwrap();
        let mut passing = s.test.pool.acquire().await.unwrap();
        let mut reader = s.test.pool.acquire().await.unwrap();
        let mut tx = sqlx::Connection::begin(&mut *held).await.unwrap();
        lock_row(&mut tx, s.bo).await;
        // Another transaction takes a later identifier and completes before the holder statement.
        sqlx::query("UPDATE users SET display_name = 'Passing' WHERE id = $1")
            .bind(s.cy)
            .execute(&mut *passing)
            .await
            .unwrap();
        same(&mut tx, s.bo, s.now, "inside").await;
        assert!(
            kept_of(s.bo).is_none(),
            "a change this transaction makes next would not be seen by the check on a kept set"
        );
        add_to_group(&mut tx, s.group, s.bo).await.unwrap();
        tx.commit().await.unwrap();
        let after = same(&mut reader, s.bo, s.now, "after its commit").await;
        assert_eq!(after.groups, [s.group]);
    }

    #[tokio::test]
    async fn a_change_rolled_back_to_a_savepoint_is_no_change_and_one_released_into_the_transaction_is()
     {
        let s = scene().await;
        let mut held = s.test.pool.acquire().await.unwrap();
        let mut reader = s.test.pool.acquire().await.unwrap();
        let mut tx = sqlx::Connection::begin(&mut *held).await.unwrap();
        lock_first(&mut tx, s.bo).await;
        sqlx::query("SAVEPOINT a").execute(&mut *tx).await.unwrap();
        add_to_group(&mut tx, s.group, s.bo).await.unwrap();
        sqlx::query("ROLLBACK TO a")
            .execute(&mut *tx)
            .await
            .unwrap();
        let rolled = same(&mut tx, s.bo, s.now, "after the rollback to the savepoint").await;
        assert!(rolled.groups.is_empty());
        let kept =
            kept_of(s.bo).expect("a change that was rolled back left no record: the set is kept");
        sqlx::query("SAVEPOINT b").execute(&mut *tx).await.unwrap();
        add_to_group(&mut tx, s.other, s.bo).await.unwrap();
        sqlx::query("RELEASE b").execute(&mut *tx).await.unwrap();
        let released = same(&mut tx, s.bo, s.now, "after the release").await;
        assert_eq!(released.groups, [s.other]);
        assert!(
            Arc::ptr_eq(&kept, &kept_of(s.bo).unwrap()),
            "what it worked out after a change it kept is not kept"
        );
        tx.commit().await.unwrap();
        let after = same(&mut reader, s.bo, s.now, "after the commit").await;
        assert_eq!(after.groups, [s.other]);
    }

    #[tokio::test]
    async fn a_subtransaction_that_aborted_leaves_no_record_of_what_it_wrote() {
        let s = scene().await;
        let mut held = s.test.pool.acquire().await.unwrap();
        let mut reader = s.test.pool.acquire().await.unwrap();
        let mut tx = sqlx::Connection::begin(&mut *held).await.unwrap();
        lock_first(&mut tx, s.bo).await;
        sqlx::query("SAVEPOINT a").execute(&mut *tx).await.unwrap();
        add_to_group(&mut tx, s.group, s.bo).await.unwrap();
        assert!(
            add_to_group(&mut tx, s.group, s.bo).await.is_err(),
            "the second membership is refused and aborts the subtransaction"
        );
        sqlx::query("ROLLBACK TO a")
            .execute(&mut *tx)
            .await
            .unwrap();
        let inside = same(&mut tx, s.bo, s.now, "after the aborted subtransaction").await;
        assert!(inside.groups.is_empty());
        assert!(
            kept_of(s.bo).is_some(),
            "nothing it wrote remains: the set is kept"
        );
        add_to_group(&mut tx, s.other, s.bo).await.unwrap();
        tx.commit().await.unwrap();
        let after = same(&mut reader, s.bo, s.now, "after the commit").await;
        assert_eq!(after.groups, [s.other], "the kept set does not hold for it");
    }

    #[tokio::test]
    async fn a_write_to_a_column_nothing_is_read_from_does_not_stop_a_set_being_kept() {
        let s = scene().await;
        let mut held = s.test.pool.acquire().await.unwrap();
        let mut tx = sqlx::Connection::begin(&mut *held).await.unwrap();
        lock_first(&mut tx, s.bo).await;
        sqlx::query("UPDATE users SET display_name = 'Renamed' WHERE id = $1")
            .bind(s.bo)
            .execute(&mut *tx)
            .await
            .unwrap();
        same(&mut tx, s.bo, s.now, "after a rename").await;
        assert!(
            kept_of(s.bo).is_some(),
            "a rename is no change of what a set comes from"
        );
        tx.rollback().await.unwrap();
    }

    async fn a_person_made_in_a_locking_transaction_then_computed_then_ended(commit: bool) {
        let s = scene().await;
        let ghost = new_id();
        let mut held = s.test.pool.acquire().await.unwrap();
        let mut reader = s.test.pool.acquire().await.unwrap();
        let mut tx = sqlx::Connection::begin(&mut *held).await.unwrap();
        lock_first(&mut tx, s.bo).await;
        insert_user(&mut tx, ghost, "ghost").await.unwrap();
        let inside = same(&mut tx, ghost, s.now, "inside, the person made").await;
        assert!(
            inside.viewers.contains(&s.topic),
            "a person who exists may view the topic"
        );
        assert!(
            kept_of(ghost).is_none(),
            "making a person is a change of what a set comes from: nothing is kept"
        );
        if commit {
            tx.commit().await.unwrap();
        } else {
            tx.rollback().await.unwrap();
        }
        let after = same(&mut reader, ghost, s.now, "after the end").await;
        assert_eq!(
            after.viewers.contains(&s.topic),
            commit,
            "a person who was never made has no place; one who was has the topic (commit: {commit})"
        );
        assert!(commit || after.viewers.is_empty());
    }

    #[tokio::test]
    async fn a_person_made_and_computed_in_a_transaction_that_commits_has_the_places_of_a_person() {
        a_person_made_in_a_locking_transaction_then_computed_then_ended(true).await;
    }

    #[tokio::test]
    async fn a_person_made_and_computed_in_a_transaction_that_rolls_back_leaves_no_set_for_an_id_that_never_was()
     {
        a_person_made_in_a_locking_transaction_then_computed_then_ended(false).await;
    }

    #[tokio::test]
    async fn a_set_kept_for_an_id_before_its_person_exists_is_not_used_once_the_person_is_made() {
        let s = scene().await;
        let late = new_id();
        let mut reader = s.test.pool.acquire().await.unwrap();
        let mut writer = s.test.pool.acquire().await.unwrap();
        let before = same(&mut reader, late, s.now, "before the person exists").await;
        assert!(before.viewers.is_empty(), "no person: no place");
        assert!(
            kept_of(late).is_some(),
            "a plain read keeps what it worked out"
        );
        insert_user(&mut writer, late, "late").await.unwrap();
        let after = same(&mut reader, late, s.now, "after the person is made").await;
        assert!(
            after.viewers.contains(&s.topic),
            "the kept set does not hold for it"
        );
    }
}
