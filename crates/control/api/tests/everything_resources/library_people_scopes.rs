//! Native captured people, exact version membership and current address/profile sources.

use super::*;
use crate::support::FullSkillVersionReply as _;
use futures::FutureExt as _;
use http::StatusCode;
use orch_access_contract::key_scope::KeyScope;
use orch_access_contract::permissions::ObjectKind;
use orch_api_contract::library::{
    LibraryPeopleBody, LibraryPeopleReply, LibraryTarget, ResourceLibraryPeopleRead,
};
use orch_api_core::middleware::authentication::{API_TOKEN_PREFIX, new_token, token_hash};
use orch_api_core::testing::RawReply;
use orch_db::queries::identity::{self, NewApiToken};
use orch_types::{Json, new_id};
use serde_json::json;

const OP: &str = "ResourceLibraryPeopleRead";

fn rule(operation: &str, kind: &str, id: Id, reach: &str) -> Json {
    json!({"target":{"kind":kind,"id":id,"reach":reach},"allow":[{"operation":operation,"actions":["read"]}]})
}

async fn key(world: &World, actor: Id, rules: Vec<Json>) -> String {
    let scope: KeyScope = serde_json::from_value(json!({"rules":rules})).unwrap();
    assert!(scope.validate().is_empty());
    let secret = new_token(API_TOKEN_PREFIX);
    identity::create_api_token_scoped(
        &mut *world.conn().await,
        &NewApiToken {
            id: new_id(),
            user_id: actor,
            name: "Native library people scopes",
            token_hash: &token_hash(&secret),
            scopes: &[],
            node_id: None,
            at: world.now(),
            expires_at: Some(world.now() + chrono::Duration::hours(1)),
        },
        Some(&json!(scope)),
    )
    .await
    .unwrap();
    secret
}

fn body(kind: ObjectKind, id: Id) -> LibraryPeopleBody {
    LibraryPeopleBody {
        target: LibraryTarget {
            object: ObjectRef { kind, id },
            version_id: None,
        },
        cursor: None,
        recent: false,
    }
}

async fn people(world: &World, token: &str, body: &LibraryPeopleBody) -> RawReply {
    world
        .app
        .call(ResourceLibraryPeopleRead, &[])
        .token(token)
        .body(body)
        .send_bytes()
        .await
}

fn shown(reply: RawReply) -> LibraryPeopleReply {
    assert_eq!(
        reply.status,
        StatusCode::OK,
        "{}",
        String::from_utf8_lossy(&reply.raw)
    );
    serde_json::from_slice(&reply.raw).unwrap()
}

async fn record(world: &World, template: Id, person: Id) {
    orch_engine::resource_facts::library::created(
        &mut *world.conn().await,
        template,
        None,
        new_id(),
        false,
        Some(person),
        Time::new(world.now()),
    )
    .await
    .unwrap();
}

async fn private_profile(world: &World, user: Id) {
    sqlx::query("INSERT INTO user_profiles(user_id,audience) VALUES($1,'private') ON CONFLICT(user_id) DO UPDATE SET audience='private'")
        .bind(user).execute(&world.db.pool).await.unwrap();
}

#[tokio::test]
async fn library_people_bind_the_actual_requested_resource_and_keep_public_people_and_hidden_native_counts()
 {
    let world = World::new().await;
    let reader = person(&world, "people_scope_reader").await;
    let visible = person(&world, "people_scope_visible").await;
    let hidden = person(&world, "people_scope_hidden").await;
    let outsider = person(&world, "people_scope_outsider").await;
    let home = make_org(&world, &reader, "People sources", "people-sources")
        .await
        .base
        .id;
    let template = resource(&world, &reader, home, "template", "people-source-template").await;
    record(&world, template, visible.id()).await;
    record(&world, template, hidden.id()).await;
    private_profile(&world, hidden.id()).await;
    let request = body(ObjectKind::Template, template);
    let token = key(
        &world,
        reader.id(),
        vec![rule(OP, "template", template, "self")],
    )
    .await;
    let result = shown(people(&world, &token, &request).await);
    assert_eq!(
        result.items.iter().map(|user| user.id).collect::<Vec<_>>(),
        vec![visible.id()]
    );
    assert_eq!(result.hidden, 1);
    assert!(result.items[0].address.is_none());
    assert!(
        !serde_json::to_string(&result)
            .unwrap()
            .contains(&hidden.id().to_string())
    );
    for (actor, row, expected) in [
        (
            reader.id(),
            rule("ResourceLibraryRead", "template", template, "self"),
            StatusCode::FORBIDDEN,
        ),
        (
            reader.id(),
            rule(OP, "provider", template, "self"),
            StatusCode::FORBIDDEN,
        ),
        (
            outsider.id(),
            rule(OP, "template", template, "self"),
            StatusCode::NOT_FOUND,
        ),
    ] {
        let token = key(&world, actor, vec![row]).await;
        let refused = people(&world, &token, &request).await;
        assert_eq!(refused.status, expected);
        assert!(!String::from_utf8_lossy(&refused.raw).contains("people_scope_visible"));
    }
    let empty = resource(&world, &reader, home, "template", "empty-people-template").await;
    let token = key(
        &world,
        reader.id(),
        vec![rule(OP, "template", empty, "self")],
    )
    .await;
    let result = shown(people(&world, &token, &body(ObjectKind::Template, empty)).await);
    assert!(result.items.is_empty());
    assert_eq!(result.hidden, 0);
    assert!(result.next_cursor.is_none());
}

#[tokio::test]
async fn library_people_keep_each_native_resource_kind_and_the_real_owned_mcp_parent() {
    let world = World::new().await;
    let reader = person(&world, "people_kinds_reader").await;
    let home = make_org(
        &world,
        &reader,
        "Typed people sources",
        "typed-people-sources",
    )
    .await
    .base
    .id;
    for kind in [
        "provider",
        "agent",
        "environment",
        "host",
        "machine",
        "template",
        "mcp_server",
        "mount",
        "skill",
    ] {
        let id = resource(&world, &reader, home, kind, &format!("people-kind-{kind}")).await;
        let token = key(&world, reader.id(), vec![rule(OP, kind, id, "self")]).await;
        let result = shown(people(&world, &token, &body(kind.parse().unwrap(), id)).await);
        assert!(
            result.items.is_empty(),
            "the actual empty {kind} history stays empty"
        );
        assert_eq!(result.hidden, 0);
    }
    let env = resource(&world, &reader, home, "environment", "people-parent-env").await;
    let id = new_id();
    orch_db::queries::resources::mcp::insert(
        &mut *world.conn().await,
        &orch_db::queries::resources::mcp::McpRow {
            id,
            name: "people-owned-server".to_owned(),
            description: String::new(),
            org_id: Some(home),
            home_id: Some(home),
            owner_id: Some(reader.id()),
            machine_id: None,
            environment_id: Some(env),
            imported: false,
            spec: json!({"transport":"stdio","command":["fixture-owned-server"],"secret_env":[]}),
            enabled: true,
            revision: 1,
        },
        world.now(),
    )
    .await
    .unwrap();
    let request = body(ObjectKind::McpServer, id);
    assert!(
        shown(people(&world, &reader.token, &request).await)
            .items
            .is_empty()
    );
    for family in ["mcp_server", "environment"] {
        let mut row = rule(OP, "environment", env, "self");
        row["resources"] = json!([family]);
        let token = key(&world, reader.id(), vec![row]).await;
        assert_eq!(
            people(&world, &token, &request).await.status,
            StatusCode::FORBIDDEN
        );
    }
    let token = key(
        &world,
        reader.id(),
        vec![rule(OP, "environment", env, "self")],
    )
    .await;
    assert!(
        shown(people(&world, &token, &request).await)
            .items
            .is_empty()
    );
    let missing = new_id();
    let token = key(
        &world,
        reader.id(),
        vec![rule(OP, "template", missing, "self")],
    )
    .await;
    assert_eq!(
        people(&world, &token, &body(ObjectKind::Template, missing))
            .await
            .status,
        StatusCode::NOT_FOUND
    );
}

#[tokio::test]
async fn library_people_protected_addresses_need_the_exact_operation_read_from_the_native_selected_domain()
 {
    let world = World::new().await;
    let reader = person(&world, "people_address_reader").await;
    let user = person(&world, "people_address_member").await;
    let home = make_org(&world, &reader, "Address source", "address-source")
        .await
        .base
        .id;
    join(&world, home, home, &user, "viewer", &reader).await;
    let template = resource(&world, &reader, home, "template", "address-source-template").await;
    record(&world, template, user.id()).await;
    let request = body(ObjectKind::Template, template);
    let ordinary = shown(people(&world, &reader.token, &request).await);
    assert!(
        ordinary.items[0]
            .address
            .as_ref()
            .unwrap()
            .contains("address-source")
    );
    for operation in [None, Some("ResourceLibraryRead"), Some(OP)] {
        let mut rules = vec![rule(OP, "template", template, "self")];
        if let Some(operation) = operation {
            rules.push(rule(operation, "org", home, "self"));
        }
        let token = key(&world, reader.id(), rules).await;
        let result = shown(people(&world, &token, &request).await);
        assert_eq!(result.items[0].id, user.id());
        assert_eq!(
            result.items[0].address,
            if operation == Some(OP) {
                ordinary.items[0].address.clone()
            } else {
                None
            }
        );
    }
}

#[tokio::test]
async fn library_people_keep_the_native_visible_window_cursor_recent_cut_and_hidden_counts() {
    let world = World::new().await;
    let reader = person(&world, "people_window_reader").await;
    let home = make_org(&world, &reader, "Window source", "window-source")
        .await
        .base
        .id;
    let template = resource(&world, &reader, home, "template", "window-source-template").await;
    let old = person(&world, "people_window_old").await;
    record(&world, template, old.id()).await;
    world.advance(chrono::Duration::days(31));
    let mut all = vec![old.id()];
    let mut recent = Vec::new();
    for index in 0..50 {
        let user = person(&world, &format!("people_window_{index}")).await;
        record(&world, template, user.id()).await;
        all.push(user.id());
        recent.push(user.id());
    }
    let hidden = person(&world, "people_window_hidden").await;
    private_profile(&world, hidden.id()).await;
    record(&world, template, hidden.id()).await;
    all.sort();
    recent.sort();
    let token = key(
        &world,
        reader.id(),
        vec![rule(OP, "template", template, "self")],
    )
    .await;
    let mut request = body(ObjectKind::Template, template);
    let first = shown(people(&world, &token, &request).await);
    assert_eq!(
        first.items.iter().map(|user| user.id).collect::<Vec<_>>(),
        all[..50]
    );
    assert_eq!(first.hidden, 1);
    assert_eq!(first.next_cursor, Some(all[49]));
    request.cursor = first.next_cursor;
    let last = shown(people(&world, &token, &request).await);
    assert_eq!(
        last.items.iter().map(|user| user.id).collect::<Vec<_>>(),
        all[50..]
    );
    assert_eq!(last.hidden, 1);
    assert!(last.next_cursor.is_none());
    request.cursor = None;
    request.recent = true;
    let result = shown(people(&world, &token, &request).await);
    assert_eq!(
        result.items.iter().map(|user| user.id).collect::<Vec<_>>(),
        recent
    );
    assert_eq!(result.hidden, 1);
    assert!(result.next_cursor.is_none());
}

pub(super) async fn skill_version(world: &World, owner: &Who, home: Id, name: &str) -> (Id, Id) {
    use orch_api_contract::skills::*;
    let skill = world
        .app
        .call(SkillCreate, &[])
        .token(&owner.token)
        .body(&SkillCreateBody {
            home_id: home,
            from: SkillSource::New {
                name: name.to_owned(),
                description: "Native pinned people fixture".to_owned(),
            },
            access: Some(orch_api_contract::access::NewResourceAccess::OnlyMe),
        })
        .send()
        .await
        .ok()
        .into_full()
        .expect("this native creation control requires its full response");
    let draft = world
        .app
        .call(SkillDraftGet, &[&skill.id])
        .token(&owner.token)
        .send()
        .await
        .ok();
    let published = world
        .app
        .call(SkillDraftPublish, &[&skill.id])
        .token(&owner.token)
        .body(&SkillDraftPublishBody {
            expected_revision: draft.info.revision,
        })
        .send()
        .map(|reply| reply.full_skill_version())
        .await
        .ok();
    (skill.id, published.version.id)
}

#[tokio::test]
async fn library_people_pin_an_actual_skill_version_to_its_own_readable_resource() {
    let world = World::new().await;
    let reader = person(&world, "people_version_reader").await;
    let user = person(&world, "people_version_actor").await;
    let home = make_org(&world, &reader, "Version source", "version-source")
        .await
        .base
        .id;
    let (skill, version) = skill_version(&world, &reader, home, "people-version-one").await;
    let (_, other) = skill_version(&world, &reader, home, "people-version-two").await;
    // Seed an already captured native version membership. A tool-use Report with
    // no active turn is deliberately ignored by its writer, unlike creation uses.
    sqlx::query("INSERT INTO resource_use_people(subject_kind,subject_id,version_id,use_kind,person_id,last_at) VALUES('skill',$1,$2,'skill_invocation',$3,$4)")
        .bind(skill)
        .bind(version)
        .bind(user.id())
        .bind(world.now())
        .execute(&world.db.pool)
        .await
        .unwrap();
    let token = key(&world, reader.id(), vec![rule(OP, "skill", skill, "self")]).await;
    let mut request = body(ObjectKind::Skill, skill);
    request.target.version_id = Some(version);
    let result = shown(people(&world, &token, &request).await);
    assert_eq!(
        result.items.iter().map(|user| user.id).collect::<Vec<_>>(),
        vec![user.id()]
    );
    request.target.version_id = Some(other);
    let refused = people(&world, &token, &request).await;
    assert_eq!(refused.status, StatusCode::NOT_FOUND);
    assert!(!String::from_utf8_lossy(&refused.raw).contains("people_version_actor"));
}

async fn activity_wait(world: &World, pid: i32, request: &tokio::task::JoinHandle<RawReply>) {
    tokio::time::timeout(std::time::Duration::from_secs(30), async {
        loop {
            assert!(!request.is_finished(), "the native people result must reach actual activity bookkeeping");
            let blocked: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM pg_stat_activity WHERE datname=current_database()
                AND wait_event_type='Lock' AND $1=ANY(pg_blocking_pids(pid)) AND query LIKE '%INSERT INTO api_token_activity%')")
                .bind(pid).fetch_one(&world.db.pool).await.unwrap();
            if blocked { break; } tokio::task::yield_now().await;
        }
    }).await.expect("the actual final people activity INSERT must wait on the held backend");
}

#[tokio::test]
async fn library_people_publication_rechecks_original_capture_profile_and_named_address_membership_after_activity_wait()
 {
    for change in [
        "unchanged",
        "expired",
        "private_profile",
        "capture_removed",
        "address_removed",
    ] {
        let world = World::new().await;
        let reader = person(&world, "people_held_reader").await;
        let user = person(&world, "people_held_member").await;
        let home = make_org(&world, &reader, "Held people source", "held-people-source")
            .await
            .base
            .id;
        join(&world, home, home, &user, "viewer", &reader).await;
        let template = resource(&world, &reader, home, "template", "held-people-template").await;
        record(&world, template, user.id()).await;
        let token = key(
            &world,
            reader.id(),
            vec![
                rule(OP, "template", template, "self"),
                rule(OP, "org", home, "self"),
            ],
        )
        .await;
        let input = body(ObjectKind::Template, template);
        let control = shown(people(&world, &token, &input).await);
        assert!(control.items[0].address.is_some());
        let mut blocker = world.db.pool.begin().await.unwrap();
        sqlx::query("LOCK TABLE api_token_activity IN SHARE MODE")
            .execute(&mut *blocker)
            .await
            .unwrap();
        let pid: i32 = sqlx::query_scalar("SELECT pg_backend_pid()")
            .fetch_one(&mut *blocker)
            .await
            .unwrap();
        let app = world.app.clone();
        let request_token = token.clone();
        let request_input = input.clone();
        let request = tokio::spawn(async move {
            let request = app
                .call(ResourceLibraryPeopleRead, &[])
                .token(&request_token)
                .body(&request_input);
            request.send_bytes().await
        });
        activity_wait(&world, pid, &request).await;
        match change {
            "expired" => world.advance(chrono::Duration::hours(2)),
            "private_profile" => private_profile(&world, user.id()).await,
            "capture_removed" => {
                sqlx::query("DELETE FROM resource_use_people WHERE subject_kind='template' AND subject_id=$1 AND person_id=$2")
                .bind(template).bind(user.id()).execute(&world.db.pool).await.unwrap();
            }
            "address_removed" => {
                sqlx::query("DELETE FROM org_members WHERE user_id=$1 AND org_id=$2")
                    .bind(user.id())
                    .bind(home)
                    .execute(&world.db.pool)
                    .await
                    .unwrap();
            }
            _ => {}
        }
        blocker.commit().await.unwrap();
        let reply = tokio::time::timeout(std::time::Duration::from_secs(30), request)
            .await
            .unwrap()
            .unwrap();
        if change == "unchanged" {
            assert_eq!(shown(reply), control);
        } else {
            assert_eq!(
                reply.status,
                if change == "expired" {
                    StatusCode::UNAUTHORIZED
                } else {
                    StatusCode::FORBIDDEN
                }
            );
            assert!(!String::from_utf8_lossy(&reply.raw).contains("people_held_member"));
        }
        if change == "address_removed" {
            let current = shown(people(&world, &token, &input).await);
            assert_eq!(current.items[0].id, user.id());
            assert!(current.items[0].address.is_none());
        }
    }
}

#[tokio::test]
async fn library_people_publication_does_not_repage_or_drop_a_readable_source_when_new_captures_change_the_window()
 {
    let world = World::new().await;
    let reader = person(&world, "people_churn_reader").await;
    let late_capture = person(&world, "people_churn_new_capture").await;
    let home = make_org(
        &world,
        &reader,
        "Churn people source",
        "churn-people-source",
    )
    .await
    .base
    .id;
    let template = resource(&world, &reader, home, "template", "churn-people-template").await;
    let mut users = Vec::new();
    for index in 0..50 {
        let user = person(&world, &format!("people_churn_{index}")).await;
        record(&world, template, user.id()).await;
        users.push(user);
    }
    users.sort_by_key(Who::id);
    let last_user = users.last().unwrap();
    let last = last_user.id();
    let first = users.first().unwrap().id();
    // A real membership gives only the original last person's page its named address source.
    join(&world, home, home, last_user, "viewer", &reader).await;
    let token = key(
        &world,
        reader.id(),
        vec![
            rule(OP, "template", template, "self"),
            rule(OP, "org", home, "self"),
        ],
    )
    .await;
    let input = body(ObjectKind::Template, template);
    let control = shown(people(&world, &token, &input).await);
    assert_eq!(control.items.len(), 50);
    assert_eq!(control.items.last().unwrap().id, last);
    assert!(control.items.last().unwrap().address.is_some());
    let mut blocker = world.db.pool.begin().await.unwrap();
    sqlx::query("LOCK TABLE api_token_activity IN SHARE MODE")
        .execute(&mut *blocker)
        .await
        .unwrap();
    let pid: i32 = sqlx::query_scalar("SELECT pg_backend_pid()")
        .fetch_one(&mut *blocker)
        .await
        .unwrap();
    let app = world.app.clone();
    let request_token = token.clone();
    let request_input = input.clone();
    let request = tokio::spawn(async move {
        let request = app
            .call(ResourceLibraryPeopleRead, &[])
            .token(&request_token)
            .body(&request_input);
        request.send_bytes().await
    });
    activity_wait(&world, pid, &request).await;
    assert!(
        late_capture.id() < first,
        "the native earlier identity must displace the last visible row"
    );
    record(&world, template, late_capture.id()).await;
    blocker.commit().await.unwrap();
    let reply = tokio::time::timeout(std::time::Duration::from_secs(30), request)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(shown(reply), control);
    let current = shown(people(&world, &token, &input).await);
    assert_eq!(current.items.len(), 50);
    assert!(!current.items.iter().any(|user| user.id == last));
}
