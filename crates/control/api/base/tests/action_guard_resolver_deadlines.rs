//! Real resolver and holder-expiry queries wait inside Guard acquisition before an authority decision.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "tests fail by panicking"
)]

mod places;
mod support;

use chrono::Duration;
use http::StatusCode;
use orch_access_contract::key_scope::KeyScope;
use orch_api_contract::common::ErrorCode;
use orch_api_contract::notifications::DestinationDelete;
use orch_api_core::middleware::authentication::{
    ACCESS_TOKEN_PREFIX, API_TOKEN_PREFIX, new_token, token_hash,
};
use orch_api_core::testing::RawReply;
use orch_db::queries::notifications::channels::{self as rows, NewDestination};
use orch_db::queries::{access, identity};
use orch_types::{Id, new_id};
use places::{Who, make_org, person};
use serde_json::json;
use support::World;

// Existing request/guard rendezvous watchdog only. Authority crosses one exact
// logical second after BOTH actual PostgreSQL lock edges have been observed.
const RENDEZVOUS: std::time::Duration = std::time::Duration::from_secs(30);

#[derive(Clone, Copy, Debug)]
enum Case {
    LegacyKey,
    ModernKey,
    BrowserSession,
    OrdinaryEntry,
    OrdinaryRole,
    OrdinaryFrameEntry,
}
impl Case {
    fn ordinary(self) -> bool {
        matches!(
            self,
            Self::OrdinaryEntry | Self::OrdinaryRole | Self::OrdinaryFrameEntry
        )
    }
}
struct Credential {
    id: Id,
    secret: String,
    session: bool,
}
async fn credential(world: &World, user: Id, org: Id, case: Case) -> Credential {
    let id = new_id();
    let session = matches!(case, Case::BrowserSession);
    let secret = new_token(if session {
        ACCESS_TOKEN_PREFIX
    } else {
        API_TOKEN_PREFIX
    });
    let until = world.now()
        + if case.ordinary() {
            Duration::hours(1)
        } else {
            Duration::seconds(1)
        };
    if session {
        identity::testing::insert_session(
            &mut *world.conn().await,
            id,
            user,
            &token_hash(&secret),
            until,
        )
        .await
        .unwrap();
    } else {
        let modern = matches!(case, Case::ModernKey);
        let policy = modern.then(|| {
            let scope: KeyScope = serde_json::from_value(json!({"rules":[{
                "target":{"kind":"org","id":org,"reach":"self"},
                "allow":[{"operation":"DestinationDelete","actions":["manage"]}]
            }]}))
            .unwrap();
            assert!(scope.validate().is_empty());
            serde_json::to_value(scope).unwrap()
        });
        let scopes = if modern {
            Vec::new()
        } else {
            vec!["*".to_owned()]
        };
        identity::create_api_token_scoped(
            &mut *world.conn().await,
            &identity::NewApiToken {
                id,
                user_id: user,
                name: "Resolver deadline control",
                token_hash: &token_hash(&secret),
                scopes: &scopes,
                node_id: None,
                at: world.now(),
                expires_at: Some(until),
            },
            policy.as_ref(),
        )
        .await
        .unwrap();
    }
    Credential {
        id,
        secret,
        session,
    }
}
async fn destination(world: &World, by: &Who, node: Id) -> Id {
    let id = new_id();
    rows::insert_destination(
        &mut *world.conn().await,
        &NewDestination {
            id,
            node_id: node,
            channel: "webhook",
            name: "Existing resolver deadline destination",
            config: &json!({"url":"https://resolver-deadline.invalid/hook"}),
            templates: &json!({}),
            kinds: &[],
            level: "important",
            enabled: true,
            created_by: by.id(),
            at: world.now(),
        },
    )
    .await
    .unwrap();
    id
}
async fn blocked(
    world: &World,
    blocker: i32,
    pending: &tokio::task::JoinHandle<RawReply>,
    identity: bool,
) -> bool {
    tokio::time::timeout(RENDEZVOUS, async {
        loop {
            let reached: bool = sqlx::query_scalar(
                "SELECT EXISTS(SELECT 1 FROM pg_stat_activity
                WHERE datname=current_database() AND pg_blocking_pids(pid) @> ARRAY[$1]::integer[]
                  AND wait_event_type='Lock'
                  AND CASE WHEN $2 THEN lower(query) LIKE '%orch_authz_saw%'
                           ELSE lower(query) LIKE '%select node_id from destinations%' END)",
            )
            .bind(blocker)
            .bind(identity)
            .fetch_one(&world.db.pool)
            .await
            .unwrap();
            if reached {
                return true;
            }
            if pending.is_finished() {
                return false;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap_or(false)
}

async fn blocked_frame(
    world: &World,
    blocker: i32,
    request_backend: i32,
    pending: &tokio::task::JoinHandle<RawReply>,
    pattern: &str,
) -> bool {
    tokio::time::timeout(RENDEZVOUS, async {
        loop {
            let reached: bool = sqlx::query_scalar(
                "SELECT EXISTS(SELECT 1 FROM pg_stat_activity
                WHERE datname=current_database() AND pg_blocking_pids(pid) @> ARRAY[$1]::integer[]
                  AND wait_event_type='Lock' AND lower(query) LIKE $2 AND pid=$3)",
            )
            .bind(blocker)
            .bind(pattern)
            .bind(request_backend)
            .fetch_one(&world.db.pool)
            .await
            .unwrap();
            if reached {
                return true;
            }
            if pending.is_finished() {
                return false;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap_or(false)
}

async fn run(case: Case) {
    let world = World::new().await;
    let owner = person(&world, "resolver-owner").await;
    let org = make_org(&world, &owner, "Resolver office", "resolver-office")
        .await
        .base
        .id;
    let actor = if case.ordinary() {
        let actor = person(&world, "resolver-manager").await;
        let role = matches!(case, Case::OrdinaryRole);
        if role {
            access::testing::insert_member(&mut *world.conn().await, org, actor.id(), org)
                .await
                .unwrap();
            access::testing::give_role(
                &mut *world.conn().await,
                org,
                "user",
                actor.id(),
                "developer",
                owner.id(),
                Some(world.now() + Duration::seconds(1)),
            )
            .await
            .unwrap();
        }
        access::testing::add_entry(
            &mut *world.conn().await,
            &access::PageKey::new("node", org, "live"),
            "any",
            "settings.manage",
            if role {
                ("role", None, Some("developer"))
            } else {
                ("user", Some(actor.id()), None)
            },
            if role {
                None
            } else {
                Some(world.now() + Duration::seconds(1))
            },
            world.now(),
        )
        .await
        .unwrap();
        actor
    } else {
        owner.clone()
    };
    // Actual native endpoint mirror, with the same ordinary actor and real stored target.
    let mirror = destination(&world, &owner, org).await;
    let ordinary = world
        .app
        .call(DestinationDelete, &[&mirror])
        .token(&actor.token)
        .send_bytes()
        .await;
    assert_eq!(
        ordinary.status,
        StatusCode::NO_CONTENT,
        "{case:?} ordinary native premise: {}",
        String::from_utf8_lossy(&ordinary.raw)
    );
    let id = destination(&world, &owner, org).await;
    let audit_before: i64 = sqlx::query_scalar("SELECT count(*) FROM audit_log WHERE object_id=$1")
        .bind(id)
        .fetch_one(&world.db.pool)
        .await
        .unwrap();
    let credential = credential(&world, actor.id(), org, case).await;
    let mut identity_lock = world.db.pool.begin().await.unwrap();
    if credential.session {
        sqlx::query("SELECT id FROM sessions WHERE id=$1 FOR UPDATE")
            .bind(credential.id)
            .fetch_one(&mut *identity_lock)
            .await
            .unwrap();
    } else {
        sqlx::query("SELECT id FROM api_tokens WHERE id=$1 FOR UPDATE")
            .bind(credential.id)
            .fetch_one(&mut *identity_lock)
            .await
            .unwrap();
    }
    let identity_pid: i32 = sqlx::query_scalar("SELECT pg_backend_pid()")
        .fetch_one(&mut *identity_lock)
        .await
        .unwrap();
    let app = world.app.clone();
    let secret = credential.secret;
    let pending = tokio::spawn(async move {
        let request = app.call(DestinationDelete, &[&id]).token(&secret);
        request.send_bytes().await
    });
    if !blocked(&world, identity_pid, &pending, true).await {
        pending.abort();
        let result = pending.await;
        identity_lock.rollback().await.unwrap();
        panic!("{case:?}: actual identity action guard was not reached: {result:?}");
    }
    if matches!(case, Case::OrdinaryRole | Case::OrdinaryFrameEntry) {
        // Current Principal preparation reads holder expiry before resolving the destination.
        // Install the real authority-table cut while that SAME action connection is still
        // waiting at identity, before it can acquire reentrant relation read locks.
        let backends: Vec<i32> = sqlx::query_scalar(
            "SELECT pid FROM pg_stat_activity
             WHERE datname=current_database() AND pg_blocking_pids(pid) @> ARRAY[$1]::integer[]
               AND wait_event_type='Lock' AND lower(query) LIKE '%orch_authz_saw%'",
        )
        .bind(identity_pid)
        .fetch_all(&world.db.pool)
        .await
        .unwrap();
        if backends.len() != 1 {
            pending.abort();
            let _ = pending.await;
            identity_lock.rollback().await.unwrap();
            panic!("{case:?}: actual action backend was not uniquely observed: {backends:?}");
        }
        let request_backend = backends[0];
        let mut frame_lock = world.db.pool.begin().await.unwrap();
        let locked = if matches!(case, Case::OrdinaryRole) {
            sqlx::query("LOCK TABLE node_roles IN ACCESS EXCLUSIVE MODE NOWAIT")
                .execute(&mut *frame_lock)
                .await
        } else {
            sqlx::query("LOCK TABLE permission_entries IN ACCESS EXCLUSIVE MODE NOWAIT")
                .execute(&mut *frame_lock)
                .await
        };
        if let Err(error) = locked {
            pending.abort();
            let _ = pending.await;
            identity_lock.rollback().await.unwrap();
            frame_lock.rollback().await.unwrap();
            panic!("{case:?}: facts were already held before the intended authority cut: {error}");
        }
        let frame_pid: i32 = sqlx::query_scalar("SELECT pg_backend_pid()")
            .fetch_one(&mut *frame_lock)
            .await
            .unwrap();
        identity_lock.rollback().await.unwrap();
        // This native holder-frame query compares current role AND page expiry in its
        // valid-until stamp, even when the holder identities themselves are cached.
        let reached = blocked_frame(
            &world,
            frame_pid,
            request_backend,
            &pending,
            "%with kept as materialized%pg_snapshot_xmin%access_changes_floor%",
        )
        .await;
        if !reached {
            pending.abort();
            let result = pending.await;
            frame_lock.rollback().await.unwrap();
            panic!("{case:?}: actual ordinary expiry-frame lookup was not reached: {result:?}");
        }
        let before_epoch = access::epoch(&mut *world.conn().await).await.unwrap();
        world.advance(Duration::seconds(1));
        let credential_live: bool = sqlx::query_scalar(
            "SELECT revoked_at IS NULL AND expires_at > $2 FROM api_tokens WHERE id=$1",
        )
        .bind(credential.id)
        .bind(world.now())
        .fetch_one(&world.db.pool)
        .await
        .unwrap();
        assert!(
            credential_live,
            "only ordinary role or page authority expires"
        );
        frame_lock.rollback().await.unwrap();
        let after_epoch = access::epoch(&mut *world.conn().await).await.unwrap();
        eprintln!(
            "ordinary Frame expiry case={case:?} epoch_before={before_epoch} epoch_after={after_epoch}"
        );
    } else {
        // The credential/resolver controls keep their original real destination-row cut.
        let mut target_lock = world.db.pool.begin().await.unwrap();
        sqlx::query("SELECT id FROM destinations WHERE id=$1 FOR NO KEY UPDATE")
            .bind(id)
            .fetch_one(&mut *target_lock)
            .await
            .unwrap();
        let target_pid: i32 = sqlx::query_scalar("SELECT pg_backend_pid()")
            .fetch_one(&mut *target_lock)
            .await
            .unwrap();
        identity_lock.rollback().await.unwrap();
        if !blocked(&world, target_pid, &pending, false).await {
            pending.abort();
            let result = pending.await;
            target_lock.rollback().await.unwrap();
            panic!("{case:?}: actual same-operation resolver row wait was not reached: {result:?}");
        }
        world.advance(Duration::seconds(1));
        target_lock.rollback().await.unwrap();
    }
    let reply = tokio::time::timeout(RENDEZVOUS, pending)
        .await
        .unwrap()
        .unwrap();
    let retained = rows::destination(&mut *world.conn().await, id)
        .await
        .unwrap()
        .is_some();
    assert!(
        retained,
        "{case:?}: expired pre-effect authority removed the actual destination; HTTP{} {}",
        reply.status,
        String::from_utf8_lossy(&reply.raw)
    );
    let audit_after: i64 = sqlx::query_scalar("SELECT count(*) FROM audit_log WHERE object_id=$1")
        .bind(id)
        .fetch_one(&world.db.pool)
        .await
        .unwrap();
    assert_eq!(
        audit_after, audit_before,
        "refused pre-effect authority writes no destination audit"
    );
    if case.ordinary() {
        assert_eq!(
            reply.status,
            StatusCode::FORBIDDEN,
            "fresh ordinary authority is checked at post-wait time"
        );
        assert_eq!(reply.problem().code, ErrorCode::AuthzDenied);
    } else {
        assert_eq!(
            reply.status,
            StatusCode::UNAUTHORIZED,
            "{case:?}: current original credential expires before native effect"
        );
        assert_eq!(reply.problem().code, ErrorCode::AuthInvalidToken);
    }
}
#[tokio::test]
async fn legacy_key_expiry_inside_real_guard_resolver_wait_prevents_effect() {
    run(Case::LegacyKey).await;
}
#[tokio::test]
async fn modern_key_expiry_inside_real_guard_resolver_wait_prevents_effect() {
    run(Case::ModernKey).await;
}
#[tokio::test]
async fn browser_session_expiry_inside_real_guard_resolver_wait_prevents_effect() {
    run(Case::BrowserSession).await;
}
#[tokio::test]
async fn ordinary_permission_expiry_inside_real_guard_resolver_wait_reloads_principal() {
    run(Case::OrdinaryEntry).await;
}

#[tokio::test]
async fn ordinary_role_expiry_inside_actual_frame_lookup_prevents_effect() {
    run(Case::OrdinaryRole).await;
}

#[tokio::test]
async fn ordinary_page_entry_expiry_inside_actual_frame_lookup_prevents_effect() {
    run(Case::OrdinaryFrameEntry).await;
}
