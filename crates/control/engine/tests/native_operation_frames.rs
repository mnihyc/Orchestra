//! Native captured stages bind current real operands and never borrow a new subject or Session.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "test assertions fail by panicking"
)]

mod common;

#[path = "native_operation_frames/preparation.rs"]
mod preparation;

use common::World;
use futures::future::BoxFuture;
use orch_access_contract::key_scope::{KeyScope, KeyTargetKind};
use orch_access_contract::permissions::{ObjectKind, Permission};
use orch_api_contract::admin::AdminOrgUpdate;
use orch_api_contract::directory::TagUpdate;
use orch_api_contract::directory_sources::{self, Source as TagSource};
use orch_api_contract::operation_sources::{Binding, Cardinality, Declaration};
use orch_api_contract::operations::{FacetUsage, TargetResolver};
use orch_api_contract::routes::{Endpoint, Route};
use orch_authz::{Act, Caller, CapturedOperation, Policy, Principal, TokenGrant, nodes};
use orch_db::queries::{hierarchy, identity};
use orch_db::{Conn, Db};
use orch_engine::EngineError;
use orch_engine::operation_scope::{
    continuing::{self, Error, Source},
    sources::{self, Facts},
    targets,
};
use orch_types::{Clock, Hash, Id, ManualClock, Time, new_id};
use serde_json::json;
use std::sync::Arc;

struct TagValue {
    row: hierarchy::TagRow,
    kind: KeyTargetKind,
}
struct SelectedTag;
impl Source for SelectedTag {
    type Role = TagSource;
    type Input = Id;
    type Value = TagValue;
    const ROUTE: Route = TagUpdate::ROUTE;
    const DECLARATION: &'static Declaration<TagSource> = &directory_sources::TAG_UPDATE;

    fn prepare<'a>(
        conn: &'a mut Conn,
        _caller: &'a Caller,
        node: &'a Id,
        _now: Time,
    ) -> BoxFuture<'a, Result<(TagValue, Vec<Facts<TagSource>>), Error>> {
        Box::pin(async move {
            // The actual native ordered selector models a stored worker slot; its chosen row
            // may disappear while a later stage waits. Scope alone sees only its real governor.
            let row = hierarchy::tags_of(conn, &[*node])
                .await?
                .into_iter()
                .next()
                .ok_or(Error::Refused)?;
            let target = targets::node(conn, row.node_id)
                .await
                .map_err(EngineError::unreadable)?;
            let kind = target.object.kind;
            Ok((
                TagValue { row, kind },
                vec![Facts::one(TagSource::TagNode, kind, target)],
            ))
        })
    }
    fn require<'a>(
        conn: &'a mut Conn,
        principal: &'a Principal,
        _input: &'a Id,
        value: &'a TagValue,
    ) -> BoxFuture<'a, Result<(), Error>> {
        Box::pin(async move {
            let permission = match value.kind {
                KeyTargetKind::Object(ObjectKind::Org) => Permission::OrgManage,
                KeyTargetKind::Object(ObjectKind::Project) => Permission::ProjectManage,
                KeyTargetKind::Object(ObjectKind::Topic) => Permission::TopicManage,
                KeyTargetKind::Object(ObjectKind::Thread) => Permission::ThreadManage,
                _ => return Err(Error::Refused),
            };
            if nodes::check(
                conn,
                principal,
                value.row.node_id,
                permission,
                Act::Use,
                Policy::default(),
            )
            .await?
            .is_allowed()
            {
                Ok(())
            } else {
                Err(Error::Refused)
            }
        })
    }
    fn locks<'a>(
        conn: &'a mut Conn,
        _principal: &'a Principal,
        _input: &'a Id,
        value: &'a TagValue,
    ) -> BoxFuture<'a, Result<(), Error>> {
        Box::pin(async move {
            let id: Option<Id> = sqlx::query_scalar("SELECT id FROM tags WHERE id=$1 FOR UPDATE")
                .bind(value.row.id)
                .fetch_optional(conn)
                .await
                .map_err(orch_db::DbError::from)?;
            if id.is_some() {
                Ok(())
            } else {
                Err(Error::Refused)
            }
        })
    }
    fn check_opened<'a>(
        _conn: &'a mut Conn,
        _principal: &'a Principal,
        _input: &'a Id,
        old: &'a TagValue,
        fresh: &'a TagValue,
    ) -> BoxFuture<'a, Result<bool, Error>> {
        Box::pin(async move {
            Ok(old.row.id == fresh.row.id
                && old.row.node_id == fresh.row.node_id
                && old.kind == fresh.kind)
        })
    }
}

struct AdminOrg;
impl Source for AdminOrg {
    type Role = ();
    type Input = Id;
    type Value = Id;
    const ROUTE: Route = AdminOrgUpdate::ROUTE;
    const DECLARATION: &'static Declaration<()> = &Declaration {
        operation: "AdminOrgUpdate",
        bindings: &[Binding {
            source: (),
            target: TargetResolver::Path("id"),
            usage: FacetUsage::Required,
            cardinality: Cardinality::One,
        }],
    };
    fn prepare<'a>(
        conn: &'a mut Conn,
        _caller: &'a Caller,
        org: &'a Id,
        _now: Time,
    ) -> BoxFuture<'a, Result<(Id, Vec<Facts<()>>), Error>> {
        Box::pin(async move {
            let target = targets::node(conn, *org)
                .await
                .map_err(EngineError::unreadable)?;
            if target.object.kind != KeyTargetKind::Object(ObjectKind::Org) {
                return Err(Error::Refused);
            }
            Ok((*org, vec![Facts::one((), target.object.kind, target)]))
        })
    }
    fn require<'a>(
        _conn: &'a mut Conn,
        principal: &'a Principal,
        _input: &'a Id,
        _value: &'a Id,
    ) -> BoxFuture<'a, Result<(), Error>> {
        Box::pin(async move {
            if principal.has_system_admin_standing()
                && (principal.caller().grant().is_some() || principal.is_system_admin())
            {
                Ok(())
            } else {
                Err(Error::Refused)
            }
        })
    }
    fn locks<'a>(
        conn: &'a mut Conn,
        _principal: &'a Principal,
        _input: &'a Id,
        value: &'a Id,
    ) -> BoxFuture<'a, Result<(), Error>> {
        Box::pin(async move {
            sqlx::query("SELECT id FROM nodes WHERE id=$1 FOR UPDATE")
                .bind(value)
                .fetch_one(conn)
                .await
                .map_err(orch_db::DbError::from)?;
            Ok(())
        })
    }
    fn check_opened<'a>(
        _conn: &'a mut Conn,
        _principal: &'a Principal,
        _input: &'a Id,
        old: &'a Id,
        fresh: &'a Id,
    ) -> BoxFuture<'a, Result<bool, Error>> {
        Box::pin(async move { Ok(old == fresh) })
    }
}

struct Fixture {
    world: World,
    clock: ManualClock,
    tag: Id,
}
struct Key {
    id: Id,
    origin: CapturedOperation,
}
impl Fixture {
    async fn new() -> Self {
        let clock = ManualClock::new(chrono::Utc::now());
        let world = World::with_clock(Arc::new(clock.clone())).await;
        world.engine().stop();
        let tag = new_id();
        hierarchy::create_tag(
            &mut *world.pool.acquire().await.unwrap(),
            tag,
            world.root,
            "Original selected tag",
            1,
            clock.now(),
        )
        .await
        .unwrap();
        Self { world, clock, tag }
    }
    fn policy(&self, operation: &str, action: &str) -> KeyScope {
        serde_json::from_value(
            json!({"rules":[{"target":{"kind":"org","id":self.world.root,"reach":"self"},
            "resources":["org"],"allow":[{"operation":operation,"actions":[action]}]}]}),
        )
        .unwrap()
    }
    async fn key(&self, actor: Id, administrative: bool) -> Key {
        let id = new_id();
        let operation = if administrative {
            "AdminOrgUpdate"
        } else {
            "TagUpdate"
        };
        let policy = self.policy(operation, "manage");
        let expires = self.clock.now() + chrono::Duration::hours(1);
        let mut tx = self.world.pool.begin().await.unwrap();
        identity::create_api_token_scoped(
            &mut tx,
            &identity::NewApiToken {
                id,
                user_id: actor,
                name: "Native operation frame fixture",
                token_hash: Hash::of(id.as_bytes()).as_bytes(),
                scopes: &[],
                node_id: None,
                at: self.clock.now(),
                expires_at: Some(expires),
            },
            Some(&json!(policy)),
        )
        .await
        .unwrap();
        let caller = Caller::token(
            actor,
            TokenGrant::from_stored_scope(&[], None, Some(policy), Some(Time::new(expires)))
                .unwrap()
                .for_token(id),
        );
        let target = targets::node(&mut tx, self.world.root).await.unwrap();
        let caller = if administrative {
            sources::bind_required(
                AdminOrg::ROUTE,
                AdminOrg::DECLARATION,
                &[Facts::one((), target.object.kind, target)],
            )
            .unwrap()
            .admit(&caller, false)
            .unwrap()
        } else {
            sources::bind_required(
                SelectedTag::ROUTE,
                SelectedTag::DECLARATION,
                &[Facts::one(TagSource::TagNode, target.object.kind, target)],
            )
            .unwrap()
            .admit(&caller, false)
            .unwrap()
        };
        let origin = CapturedOperation::capture(&caller).unwrap();
        tx.commit().await.unwrap();
        Key { id, origin }
    }
    async fn name(&self) -> String {
        sqlx::query_scalar("SELECT name FROM tags WHERE id=$1")
            .bind(self.tag)
            .fetch_one(self.world.pool.pool())
            .await
            .unwrap()
    }
}

async fn apply(
    db: Db,
    clock: ManualClock,
    origin: CapturedOperation,
    node: Id,
) -> Result<(), Error> {
    let mut tx = db.begin().await?;
    let frame = continuing::prepare::<SelectedTag>(&mut tx, &clock, &origin, node).await?;
    let parts = frame.into_parts();
    parts.first_effect.check(&clock)?;
    hierarchy::change_tag(
        &mut tx,
        parts.value.row.id,
        Some("Changed by actual native work"),
        None,
    )
    .await?;
    tx.commit().await.map_err(orch_db::DbError::from)?;
    Ok(())
}

#[tokio::test]
async fn native_transport_ceiling_is_retained_restored_and_cannot_lend_a_wider_key() {
    let s = Fixture::new().await;
    let original = s.key(s.world.ada, false).await;
    let extra = s.key(s.world.ada, false).await;
    let mut tx = s.world.pool.begin().await.unwrap();
    let current = extra
        .origin
        .restore_current(&mut tx, &s.clock)
        .await
        .unwrap()
        .unwrap();
    let frame = continuing::prepare_with_caller::<SelectedTag>(
        &mut tx,
        &s.clock,
        &original.origin,
        &current,
        s.world.root,
    )
    .await
    .unwrap();
    let keys = frame
        .principal()
        .caller()
        .grant()
        .unwrap()
        .token_ids()
        .collect::<Vec<_>>();
    assert_eq!(keys.len(), 2);
    assert!(keys.contains(&original.id) && keys.contains(&extra.id));
    let opened = frame.into_parts().opened;
    assert_eq!(opened.origin().operation(), "TagUpdate");
    tx.commit().await.unwrap();
    sqlx::query("UPDATE api_tokens SET operation_scope=$2 WHERE id=$1")
        .bind(extra.id)
        .bind(json!(s.policy("TagDelete", "manage")))
        .execute(s.world.pool.pool())
        .await
        .unwrap();
    let mut tx = s.world.pool.begin().await.unwrap();
    let still_wide = original
        .origin
        .restore_current(&mut tx, &s.clock)
        .await
        .unwrap()
        .unwrap();
    assert!(
        opened
            .refresh_with_caller(&mut tx, &s.clock, &still_wide)
            .await
            .is_err(),
        "the still-wide original cannot replace the current retained extra-key ceiling"
    );
    assert_eq!(s.name().await, "Original selected tag");
}

#[tokio::test]
async fn captured_native_frame_keeps_exact_scope_actual_operands_and_the_ordinary_gate() {
    let s = Fixture::new().await;
    let key = s.key(s.world.ada, false).await;
    let mut tx = s.world.pool.begin().await.unwrap();
    let frame = continuing::prepare::<SelectedTag>(&mut tx, &s.clock, &key.origin, s.world.root)
        .await
        .unwrap();
    assert_eq!(frame.value().row.id, s.tag);
    assert_eq!(
        frame.principal().caller().admitted_operation(),
        Some("TagUpdate")
    );
    assert_eq!(
        frame
            .principal()
            .caller()
            .grant()
            .unwrap()
            .token_ids()
            .collect::<Vec<_>>(),
        vec![key.id]
    );
    assert!(!frame.principal().caller().carries_system_admin());
    assert!(!frame.principal().caller().as_admin());
    tx.commit().await.unwrap();
    apply(
        s.world.pool.clone(),
        s.clock.clone(),
        key.origin.clone(),
        s.world.root,
    )
    .await
    .unwrap();
    assert_eq!(s.name().await, "Changed by actual native work");
    let outsider = s.world.person("outsider").await;
    let foreign = s.key(outsider, false).await;
    let mut tx = s.world.pool.begin().await.unwrap();
    assert!(
        continuing::prepare::<SelectedTag>(&mut tx, &s.clock, &foreign.origin, s.world.root)
            .await
            .is_err()
    );
    assert!(
        continuing::prepare::<AdminOrg>(&mut tx, &s.clock, &key.origin, s.world.root)
            .await
            .is_err()
    );
}

#[tokio::test]
async fn captured_native_frame_rechecks_after_the_actual_native_operand_lock_wait() {
    for expire in [false, true] {
        let s = Fixture::new().await;
        let key = s.key(s.world.ada, false).await;
        let mut blocker = s.world.pool.begin().await.unwrap();
        sqlx::query("SELECT id FROM tags WHERE id=$1 FOR UPDATE")
            .bind(s.tag)
            .fetch_one(&mut *blocker)
            .await
            .unwrap();
        let pid: i32 = sqlx::query_scalar("SELECT pg_backend_pid()")
            .fetch_one(&mut *blocker)
            .await
            .unwrap();
        let request = tokio::spawn(apply(
            s.world.pool.clone(),
            s.clock.clone(),
            key.origin,
            s.world.root,
        ));
        tokio::time::timeout(std::time::Duration::from_secs(30), async {
            loop {
                assert!(!request.is_finished(), "the native operand wait must be reached");
                let blocked: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM pg_stat_activity WHERE datname=current_database() AND wait_event_type='Lock' AND $1=ANY(pg_blocking_pids(pid)) AND query LIKE 'SELECT id FROM tags%FOR UPDATE%')")
                    .bind(pid).fetch_one(s.world.pool.pool()).await.unwrap();
                if blocked { break; }
                tokio::task::yield_now().await;
            }
        }).await.expect("the request reaches its actual native SQL lock");
        assert_eq!(s.name().await, "Original selected tag");
        if expire {
            s.clock.advance(chrono::Duration::hours(2));
        }
        blocker.commit().await.unwrap();
        let result = tokio::time::timeout(std::time::Duration::from_secs(30), request)
            .await
            .unwrap()
            .unwrap();
        if expire {
            assert!(result.is_err());
            assert_eq!(s.name().await, "Original selected tag");
        } else {
            result.unwrap();
            assert_eq!(s.name().await, "Changed by actual native work");
        }
    }
}

#[tokio::test]
async fn captured_native_work_cannot_replace_its_original_subject_at_the_same_governor() {
    let s = Fixture::new().await;
    let key = s.key(s.world.ada, false).await;
    let mut tx = s.world.pool.begin().await.unwrap();
    let old = continuing::prepare::<SelectedTag>(&mut tx, &s.clock, &key.origin, s.world.root)
        .await
        .unwrap()
        .into_parts()
        .opened;
    tx.commit().await.unwrap();
    let replacement = new_id();
    let mut tx = s.world.pool.begin().await.unwrap();
    hierarchy::delete_tag(&mut tx, s.tag).await.unwrap();
    hierarchy::create_tag(
        &mut tx,
        replacement,
        s.world.root,
        "Another native subject",
        1,
        s.clock.now(),
    )
    .await
    .unwrap();
    tx.commit().await.unwrap();
    let mut tx = s.world.pool.begin().await.unwrap();
    let fresh = continuing::prepare::<SelectedTag>(&mut tx, &s.clock, &key.origin, s.world.root)
        .await
        .unwrap();
    assert_eq!(
        fresh.value().row.id,
        replacement,
        "a new request legitimately sees the replacement"
    );
    assert!(
        old.refresh(&mut tx, &s.clock).await.is_err(),
        "same governor does not preserve the old native row"
    );
}

#[tokio::test]
async fn native_original_keys_current_scope_revocation_and_final_deadline_never_become_sessions() {
    for revoke in [false, true] {
        let s = Fixture::new().await;
        let key = s.key(s.world.ada, false).await;
        if revoke {
            sqlx::query("UPDATE api_tokens SET revoked_at=$2 WHERE id=$1")
                .bind(key.id)
                .bind(s.clock.now())
                .execute(s.world.pool.pool())
                .await
                .unwrap();
        } else {
            sqlx::query("UPDATE api_tokens SET operation_scope=$2 WHERE id=$1")
                .bind(key.id)
                .bind(json!(s.policy("TagUpdate", "read")))
                .execute(s.world.pool.pool())
                .await
                .unwrap();
        }
        assert!(
            apply(
                s.world.pool.clone(),
                s.clock.clone(),
                key.origin,
                s.world.root
            )
            .await
            .is_err()
        );
        assert_eq!(s.name().await, "Original selected tag");
    }
    let s = Fixture::new().await;
    let key = s.key(s.world.ada, false).await;
    let mut tx = s.world.pool.begin().await.unwrap();
    let parts = continuing::prepare::<SelectedTag>(&mut tx, &s.clock, &key.origin, s.world.root)
        .await
        .unwrap()
        .into_parts();
    s.clock.advance(chrono::Duration::hours(2));
    assert!(parts.first_effect.check(&s.clock).is_err());
    tx.rollback().await.unwrap();
    assert_eq!(s.name().await, "Original selected tag");
}

#[tokio::test]
async fn native_administrative_stages_recheck_real_current_standing_without_private_override() {
    let s = Fixture::new().await;
    sqlx::query("UPDATE users SET is_system_admin=true WHERE id=$1")
        .bind(s.world.ada)
        .execute(s.world.pool.pool())
        .await
        .unwrap();
    let key = s.key(s.world.ada, true).await;
    let mut tx = s.world.pool.begin().await.unwrap();
    let frame = continuing::prepare::<AdminOrg>(&mut tx, &s.clock, &key.origin, s.world.root)
        .await
        .unwrap();
    assert!(frame.principal().has_system_admin_standing());
    assert!(!frame.principal().caller().as_admin());
    assert!(!frame.principal().caller().carries_system_admin());
    tx.commit().await.unwrap();
    sqlx::query("UPDATE users SET is_system_admin=false WHERE id=$1")
        .bind(s.world.ada)
        .execute(s.world.pool.pool())
        .await
        .unwrap();
    let mut tx = s.world.pool.begin().await.unwrap();
    assert!(
        continuing::prepare::<AdminOrg>(&mut tx, &s.clock, &key.origin, s.world.root)
            .await
            .is_err()
    );
}

#[tokio::test]
async fn native_work_keeps_every_captured_key_and_their_current_earliest_deadline() {
    let s = Fixture::new().await;
    let first = s.key(s.world.ada, false).await;
    let second = s.key(s.world.ada, false).await;
    let mut tx = s.world.pool.begin().await.unwrap();
    let left = first
        .origin
        .restore_current(&mut tx, &s.clock)
        .await
        .unwrap()
        .unwrap();
    let right = second
        .origin
        .restore_current(&mut tx, &s.clock)
        .await
        .unwrap()
        .unwrap();
    let both = left.intersect_restrictions(&right).unwrap();
    let target = targets::node(&mut tx, s.world.root).await.unwrap();
    let plan = sources::bind_required(
        SelectedTag::ROUTE,
        SelectedTag::DECLARATION,
        &[Facts::one(TagSource::TagNode, target.object.kind, target)],
    )
    .unwrap();
    let origin = CapturedOperation::capture(&plan.admit(&both, false).unwrap()).unwrap();
    tx.commit().await.unwrap();
    sqlx::query("UPDATE api_tokens SET expires_at=$2 WHERE id=$1")
        .bind(second.id)
        .bind(s.clock.now() + chrono::Duration::minutes(5))
        .execute(s.world.pool.pool())
        .await
        .unwrap();
    let mut tx = s.world.pool.begin().await.unwrap();
    let parts = continuing::prepare::<SelectedTag>(&mut tx, &s.clock, &origin, s.world.root)
        .await
        .unwrap()
        .into_parts();
    let mut actual = parts
        .principal
        .caller()
        .grant()
        .unwrap()
        .token_ids()
        .collect::<Vec<_>>();
    let mut expected = vec![first.id, second.id];
    actual.sort();
    expected.sort();
    assert_eq!(actual, expected);
    assert!(!parts.principal.caller().as_admin());
    s.clock.advance(chrono::Duration::minutes(6));
    assert!(parts.first_effect.check(&s.clock).is_err());
    tx.rollback().await.unwrap();
    assert_eq!(s.name().await, "Original selected tag");
}

#[tokio::test]
async fn captured_native_work_uses_the_same_required_team_second_step_setup_hold() {
    let s = Fixture::new().await;
    let member = s.world.person("native-member").await;
    let mut tx = s.world.pool.begin().await.unwrap();
    orch_db::queries::access::testing::insert_member(&mut tx, s.world.root, member, s.world.root)
        .await
        .unwrap();
    orch_db::queries::access::testing::give_role(
        &mut tx,
        s.world.root,
        "user",
        member,
        "admin",
        s.world.ada,
        None,
    )
    .await
    .unwrap();
    tx.commit().await.unwrap();
    let key = s.key(member, false).await;
    // Establish the native ordinary gate before adding the existing account hold.
    let mut tx = s.world.pool.begin().await.unwrap();
    let _frame = continuing::prepare::<SelectedTag>(&mut tx, &s.clock, &key.origin, s.world.root)
        .await
        .unwrap();
    tx.commit().await.unwrap();
    s.world
        .set(s.world.root, "auth.second_step", json!("required"))
        .await;
    let mut tx = s.world.pool.begin().await.unwrap();
    let user = identity::user_by_id(&mut tx, member)
        .await
        .unwrap()
        .unwrap();
    let required = orch_engine::operation_scope::account_policy::requirement(&mut tx, &user)
        .await
        .unwrap();
    assert!(
        required.orgs.contains(&s.world.root),
        "the fixture must have a real required team root"
    );
    assert!(!orch_engine::operation_scope::account_policy::configured(
        &user
    ));
    assert!(
        continuing::prepare::<SelectedTag>(&mut tx, &s.clock, &key.origin, s.world.root)
            .await
            .is_err()
    );
    tx.rollback().await.unwrap();
    // Native keys wait for stored setup, not a new browser session/given-proof lease.
    sqlx::query("UPDATE users SET totp_secret=$2,totp_enabled_at=$3 WHERE id=$1")
        .bind(member)
        .bind(vec![7_u8; 64])
        .bind(s.clock.now())
        .execute(s.world.pool.pool())
        .await
        .unwrap();
    let mut tx = s.world.pool.begin().await.unwrap();
    let frame = continuing::prepare::<SelectedTag>(&mut tx, &s.clock, &key.origin, s.world.root)
        .await
        .unwrap();
    assert_eq!(frame.principal().user(), member);
    assert!(frame.principal().second_step_hold().is_none());
    assert!(frame.principal().caller().grant().is_some());
    assert!(!frame.principal().caller().as_admin());
}
