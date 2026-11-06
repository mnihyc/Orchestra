//! Actual queued input producers retain independent Create/Update credentials through execution.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a control fails by panicking"
)]

mod common;

use common::{World, queued, turn_ended};
use orch_access_contract::key_scope::{
    KeyAction, KeyOperation, KeyReach, KeyRule, KeyScope, KeyTarget, KeyTargetKind,
};
use orch_access_contract::permissions::{ObjectKind, Permission};
use orch_api_contract::operations::{self, TargetResolver};
use orch_api_contract::routes::{Endpoint, Route};
use orch_authz::{Caller, TokenGrant};
use orch_db::queries::{identity, threads, tokens};
use orch_engine::inputs::InputError;
use orch_engine::operation_scope::{Prepared, targets};
use orch_link_contract::events::TurnOutcome;
use orch_types::{Hash, Id, Time, new_id};

const THREAD: KeyTargetKind = KeyTargetKind::Object(ObjectKind::Thread);

fn plain(route: Route) -> String {
    let operation = operations::for_route(&route).unwrap();
    operation
        .variants
        .into_iter()
        .find(|variant| {
            variant.name.is_empty() || matches!(variant.name.as_str(), "submit" | "edit")
        })
        .expect("the real operation declares its ordinary text intent")
        .operation
}

fn scope(thread: Id, operation: &str, action: KeyAction) -> KeyScope {
    KeyScope {
        rules: vec![KeyRule {
            target: KeyTarget {
                kind: THREAD,
                id: Some(thread),
                reach: KeyReach::SelfOnly,
            },
            resources: Some(vec![THREAD]),
            allow: vec![KeyOperation {
                operation: operation.into(),
                actions: vec![action],
            }],
        }],
        ..KeyScope::default()
    }
}

async fn key(world: &World, user: Id, thread: Id, route: Route) -> (Id, Caller, String) {
    let operation = plain(route);
    let policy = scope(thread, &operation, KeyAction::Write);
    let id = new_id();
    let expires = world.clock.now() + chrono::Duration::hours(1);
    let digest = Hash::of(id.as_bytes());
    let mut conn = world.pool.acquire().await.unwrap();
    identity::create_api_token_scoped(
        &mut conn,
        &identity::NewApiToken {
            id,
            user_id: user,
            name: "input boundary control",
            token_hash: digest.as_bytes(),
            scopes: &[],
            node_id: None,
            at: world.clock.now() - chrono::Duration::hours(1),
            expires_at: Some(expires),
        },
        Some(&serde_json::to_value(&policy).unwrap()),
    )
    .await
    .unwrap();
    let caller = Caller::token(
        user,
        TokenGrant::from_stored_scope(&[], None, Some(policy), Some(Time::new(expires)))
            .unwrap()
            .for_token(id),
    );
    let target = targets::node(&mut conn, thread).await.unwrap();
    let mut plan = Prepared::new(route, &operation).unwrap();
    for (index, facet) in plan.variant().facets.clone().iter().enumerate() {
        match facet.target {
            TargetResolver::Path("id") | TargetResolver::PermissionObject => plan
                .bind(index, THREAD, std::slice::from_ref(&target))
                .unwrap(),
            TargetResolver::ResolvedCollection(_) => plan.bind_empty(index).unwrap(),
            _ => panic!("the text control has no invented singleton target"),
        }
    }
    (id, plan.admit(&caller, false).unwrap(), operation)
}

async fn held_pair(world: &World) -> (Id, Id, Id, Id, String, String) {
    let thread = world.thread("Independent input credentials").await;
    world
        .exec("UPDATE environments SET status='failed',status_detail='input authority control'")
        .await;
    let (create_id, creator, create) = key(
        world,
        world.ada,
        thread,
        orch_api_contract::threads::ThreadInputCreate::ROUTE,
    )
    .await;
    let (edit_id, editor, edit) = key(
        world,
        world.ada,
        thread,
        orch_api_contract::threads::ThreadInputUpdate::ROUTE,
    )
    .await;
    let sent = world
        .engine()
        .submit_input(&creator, thread, queued("Original accepted words"))
        .await
        .unwrap();
    world
        .engine()
        .edit_input(
            &editor,
            thread,
            sent.input.id,
            Some(queued("Edited accepted words").content),
            None,
        )
        .await
        .unwrap();
    assert!(world.turn_starts(thread).is_empty());
    let stored = threads::input(&mut world.pool.acquire().await.unwrap(), sent.input.id)
        .await
        .unwrap()
        .unwrap();
    let authority = stored.submission_authority.unwrap();
    let origins = authority["origins"]
        .as_array()
        .expect("actual operation origins are retained");
    assert_eq!(origins.len(), 2);
    assert_eq!(origins[0]["operation"], create);
    assert_eq!(origins[1]["operation"], edit);
    assert!(!authority.to_string().contains("node_ancestors"));
    (thread, sent.input.id, create_id, edit_id, create, edit)
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn create_only_and_edit_only_origins_execute_the_same_final_input() {
    let world = World::new().await;
    let (thread, input, _, _, _, _) = held_pair(&world).await;
    world
        .exec("UPDATE environments SET status='running',status_detail=NULL")
        .await;
    world.engine().preflight(thread).await.unwrap();
    let start = world.turn_start(thread, 0).await;
    assert_eq!(
        common::words_after_where(&start.text),
        "Edited accepted words"
    );
    assert_eq!(world.input_state(input).await.0, "delivered");
    world
        .answer(thread, &start, "authority-final", "Done.")
        .await;
    world
        .say(thread, turn_ended(start.turn_id, TurnOutcome::Completed))
        .await;
    world.wait_state(thread, "idle").await;
    assert_eq!(world.turn_starts(thread).len(), 1);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn each_origin_still_binds_current_action_target_revocation_expiry_and_place() {
    for case in ["action", "target", "revoked", "expired", "place", "user"] {
        let world = World::new().await;
        let (thread, input, create_id, edit_id, _, edit) = held_pair(&world).await;
        let mut conn = world.pool.acquire().await.unwrap();
        match case {
            "action" | "target" => {
                let policy = scope(
                    if case == "target" { new_id() } else { thread },
                    &edit,
                    if case == "action" {
                        KeyAction::Read
                    } else {
                        KeyAction::Write
                    },
                );
                sqlx::query("UPDATE api_tokens SET operation_scope=$2 WHERE id=$1")
                    .bind(edit_id)
                    .bind(serde_json::to_value(policy).unwrap())
                    .execute(&mut *conn)
                    .await
                    .unwrap();
            }
            "revoked" => {
                identity::revoke_api_token(&mut conn, create_id, world.ada, world.clock.now())
                    .await
                    .unwrap();
            }
            "expired" => {
                sqlx::query("UPDATE api_tokens SET expires_at=$2 WHERE id=$1")
                    .bind(edit_id)
                    .bind(world.clock.now() - chrono::Duration::minutes(1))
                    .execute(&mut *conn)
                    .await
                    .unwrap();
            }
            "user" => {
                sqlx::query("UPDATE users SET status='suspended' WHERE id=$1")
                    .bind(world.ada)
                    .execute(&mut *conn)
                    .await
                    .unwrap();
            }
            "place" => {
                tokens::refuse(
                    &mut conn,
                    create_id,
                    world.root,
                    world.ada,
                    world.clock.now(),
                )
                .await
                .unwrap();
            }
            _ => unreachable!(),
        }
        drop(conn);
        world
            .exec("UPDATE environments SET status='running',status_detail=NULL")
            .await;
        world.engine().preflight(thread).await.unwrap();
        assert!(
            world.turn_starts(thread).is_empty(),
            "{case} must not execute the input"
        );
        assert_ne!(world.input_state(input).await.0, "delivered", "{case}");
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn another_person_may_reorder_but_cannot_edit_or_replace_author_origins() {
    let world = World::new().await;
    let (thread, input, _, _, _, _) = held_pair(&world).await;
    let other = world.person("other-controller").await;
    let mut conn = world.pool.acquire().await.unwrap();
    orch_db::queries::access::testing::add_entry(
        &mut conn,
        &orch_db::queries::access::PageKey::new("node", thread, "live"),
        "any",
        Permission::ThreadInteract.as_str(),
        ("user", Some(other), None),
        None,
        world.clock.now(),
    )
    .await
    .unwrap();
    let before = threads::input(&mut conn, input)
        .await
        .unwrap()
        .unwrap()
        .submission_authority;
    drop(conn);
    let (_, controller, _) = key(
        &world,
        other,
        thread,
        orch_api_contract::threads::ThreadInputUpdate::ROUTE,
    )
    .await;
    world
        .engine()
        .edit_input(&controller, thread, input, None, Some(None))
        .await
        .unwrap();
    assert!(matches!(
        world
            .engine()
            .edit_input(
                &controller,
                thread,
                input,
                Some(queued("Not the author's words").content),
                None
            )
            .await,
        Err(InputError::NotTheAuthor)
    ));
    let after = threads::input(&mut world.pool.acquire().await.unwrap(), input)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(after.author_id, Some(world.ada));
    assert_eq!(after.submission_authority, before);
    assert!(world.turn_starts(thread).is_empty());
}
