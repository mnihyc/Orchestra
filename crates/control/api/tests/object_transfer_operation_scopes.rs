//! Native individual projections and metadata retain their ordinary ownership outcomes.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "tests fail by panicking"
)]

mod desk;
mod places;
mod support;

use desk::Desk;
use http::StatusCode;
use orch_access_contract::decisions::ObjectRef;
use orch_api_contract::document_tree::{
    DocumentContainerCreate, DocumentContainerCreateBody, DocumentTreeItem,
};
use orch_api_contract::ownership::{ObjectTransfer, TransferBody};
use orch_api_core::middleware::authentication::{API_TOKEN_PREFIX, new_token, token_hash};
use orch_api_core::testing::RawReply;
use orch_authz::ownership::{NewAccess, start_resource};
use orch_db::queries::{identity, resources::access_fixtures};
use orch_types::{Id, Json, Time, new_id};
use serde_json::json;

struct Scene {
    desk: Desk,
    receiver: places::Who,
}
impl Scene {
    async fn new() -> Self {
        orch_api_core::routes::resources::sealing::install_master_key(
            orch_db::secrets::MasterKey::new([36; 32]),
        );
        let desk = Desk::new().await;
        let receiver = desk.member("transfer-recipient", "viewer").await;
        Self { desk, receiver }
    }
    async fn resource(&self, kind: &str) -> Id {
        let mut conn = self.desk.world.conn().await;
        let id = access_fixtures::insert_resource(
            &mut conn,
            kind,
            Some(self.desk.ada.id()),
            Some(self.desk.org),
            if matches!(kind, "mcp_server" | "mount") {
                "native-single-view"
            } else {
                "Native single view"
            },
        )
        .await
        .unwrap();
        start_resource(
            &mut conn,
            ObjectRef {
                kind: kind.parse().unwrap(),
                id,
            },
            self.desk.ada.id(),
            NewAccess::OnlyMe,
            Time::new(self.desk.world.now()),
        )
        .await
        .unwrap();
        id
    }
    async fn target(&self, kind: &str) -> Id {
        match kind {
            "document" => {
                self.desk
                    .make(&self.desk.ada, "Native metadata")
                    .await
                    .base
                    .id
            }
            "document_container" => {
                let made = self
                    .desk
                    .world
                    .app
                    .call(DocumentContainerCreate, &[&self.desk.topic])
                    .token(&self.desk.ada.token)
                    .body(&DocumentContainerCreateBody {
                        parent_container_id: None,
                        name: "Native folder metadata".into(),
                        client_id: new_id().to_string(),
                    })
                    .send()
                    .await
                    .ok();
                let Some(DocumentTreeItem::Container(id)) =
                    made.entries.first().map(|entry| entry.item)
                else {
                    panic!("native folder identity")
                };
                id
            }
            _ => self.resource(kind).await,
        }
    }
    async fn key(&self, kind: &str, id: Id, read: bool) -> String {
        let mut rules = vec![
            json!({"target":{"kind":kind,"id":id,"reach":"self"},"allow":[{"operation":"ObjectTransfer","actions":if read { vec!["manage","read"] } else { vec!["manage"] }}]}),
        ];
        if read {
            rules.push(json!({"target":{"kind":"org","id":self.desk.org,"reach":"subtree"},"allow":[{"operation":"ObjectTransfer","actions":["read"]}]}));
            if kind == "environment" {
                let template: Option<Id> =
                    sqlx::query_scalar("SELECT template_id FROM environments WHERE id=$1")
                        .bind(id)
                        .fetch_one(&self.desk.world.db.pool)
                        .await
                        .unwrap();
                if let Some(template) = template {
                    rules.push(json!({"target":{"kind":"template","id":template,"reach":"self"},"allow":[{"operation":"ObjectTransfer","actions":["read"]}]}));
                }
            }
        }
        let scope: orch_access_contract::key_scope::KeyScope =
            serde_json::from_value(json!({"rules":rules})).unwrap();
        assert!(scope.validate().is_empty());
        let secret = new_token(API_TOKEN_PREFIX);
        identity::create_api_token_scoped(
            &mut *self.desk.world.conn().await,
            &identity::NewApiToken {
                id: new_id(),
                user_id: self.desk.ada.id(),
                name: "Native transfer scope",
                token_hash: &token_hash(&secret),
                scopes: &[],
                node_id: None,
                at: self.desk.world.now(),
                expires_at: Some(self.desk.world.now() + chrono::Duration::hours(1)),
            },
            Some(&serde_json::to_value(scope).unwrap()),
        )
        .await
        .unwrap();
        secret
    }
}

fn plural(kind: &str) -> &'static str {
    match kind {
        "provider" => "providers",
        "agent" => "agents",
        "environment" => "environments",
        "host" => "hosts",
        "machine" => "machines",
        "template" => "templates",
        "mcp_server" => "mcp-servers",
        "mount" => "mounts",
        "skill" => "skills",
        "document" => "documents",
        "document_container" => "document-containers",
        _ => panic!("unknown fixture kind"),
    }
}
async fn transfer(s: &Scene, token: &str, kind: &str, id: Id, to: Id) -> RawReply {
    s.desk
        .world
        .app
        .call(ObjectTransfer, &[&plural(kind), &id])
        .token(token)
        .body(&TransferBody {
            user_id: to,
            keep_previous: Some(true),
        })
        .send_bytes()
        .await
}
fn full(reply: RawReply) -> Json {
    assert_eq!(
        reply.status,
        StatusCode::OK,
        "native ownership response: {}",
        String::from_utf8_lossy(&reply.raw)
    );
    let value: Json = serde_json::from_slice(&reply.raw).unwrap();
    assert_ne!(
        value["projection"], "receipt",
        "this control requires the complete native union"
    );
    let _: orch_api_contract::resources::Resource = serde_json::from_value(value.clone()).unwrap();
    value
}
async fn legacy_skill_origin(s: &Scene, id: Id) {
    let origin = json!({"kind":"url","url":"https://archive.example.invalid/skill.tar.gz?token=fixture-source-secret","folder":null,"host_id":new_id()});
    sqlx::query("UPDATE skills SET origin=$2,source_url_secret_id=NULL WHERE id=$1")
        .bind(id)
        .bind(origin)
        .execute(&s.desk.world.db.pool)
        .await
        .unwrap();
}
async fn style(kind: &str) {
    let s = Scene::new().await;
    let id = s.target(kind).await;
    if kind == "provider" {
        let facts = json!({"windows":[{"id":"fixture-window","label":"Fixture window","duration_minutes":60,"used_percent":20,"resets_at":s.desk.world.now()+chrono::Duration::hours(1)}]});
        orch_db::queries::resources::keeper::set_facts(
            &mut *s.desk.world.conn().await,
            id,
            &facts,
            s.desk.world.now(),
        )
        .await
        .unwrap();
    }
    if kind == "environment" {
        let host = s.resource("host").await;
        sqlx::query("INSERT INTO runtimes(id,kind,host_id,unit_name,environment_id,driver,template_id,spec_hash,state) SELECT $1,'environment',$2,'native-cut-unit',e.id,'test',e.template_id,$3,'running' FROM environments e WHERE e.id=$4")
            .bind(new_id()).bind(host).bind(orch_types::Hash::of(b"native singleton runtime").as_bytes().to_vec()).bind(id).execute(&s.desk.world.db.pool).await.unwrap();
    }
    if kind == "host" {
        let environment = s.resource("environment").await;
        // Nondefault retained bytes and a real environment reference distinguish this native style.
        sqlx::query("UPDATE hosts SET kept_images=$2 WHERE id=$1").bind(id).bind(json!({"epoch":1,"images":[{"system":"fixture","environment":environment,"generation":1,"capture":new_id(),"reference":"native-kept-image","bytes":13,"total_bytes":17,"layers":1}]})).execute(&s.desk.world.db.pool).await.unwrap();
    }
    if kind == "machine" {
        sqlx::query("UPDATE machines SET roles=ARRAY['node','environment'] WHERE id=$1")
            .bind(id)
            .execute(&s.desk.world.db.pool)
            .await
            .unwrap();
    }
    if kind == "mcp_server" {
        sqlx::query("UPDATE mcp_servers SET spec=$2 WHERE id=$1").bind(id)
            .bind(json!({"transport":"stdio","command":["tool-server"],"secret_env":[],"env":{"NATIVE_STYLE":"individual"}}))
            .execute(&s.desk.world.db.pool).await.unwrap();
    }
    if kind == "skill" {
        legacy_skill_origin(&s, id).await;
    }
    let ordinary = full(transfer(&s, &s.desk.ada.token, kind, id, s.receiver.id()).await);
    match kind {
        "provider" => assert!(
            ordinary["plan_forecasts"].is_object(),
            "the native individual forecast finisher is exercised"
        ),
        "environment" => assert!(
            ordinary["runtime_changes"].is_object(),
            "the native whole-view runtime finisher is exercised"
        ),
        "host" => assert_eq!(
            ordinary["kept_layers"][0]["bytes"], 13,
            "the retained layer is real native content"
        ),
        _ => {}
    }
    if kind == "skill" {
        legacy_skill_origin(&s, id).await;
    }
    let token = s.key(kind, id, true).await;
    let scoped = full(transfer(&s, &token, kind, id, s.desk.ada.id()).await);
    // Ordinary readback must not be able to repair a missing scoped maintenance prefix.
    if kind == "skill" {
        let (origin, pointer): (Json, Option<Id>) =
            sqlx::query_as("SELECT origin,source_url_secret_id FROM skills WHERE id=$1")
                .bind(id)
                .fetch_one(&s.desk.world.db.pool)
                .await
                .unwrap();
        assert!(
            pointer.is_some(),
            "native source maintenance keeps sealed custody"
        );
        assert!(!origin.to_string().contains("fixture-source-secret"));
        assert!(!scoped.to_string().contains("fixture-source-secret"));
    }
    let native = full(transfer(&s, &s.desk.ada.token, kind, id, s.desk.ada.id()).await);
    assert_eq!(
        scoped, native,
        "the scoped factory preserves the native individual result exactly"
    );
    let (_, stored) = orch_authz::things::site(
        &mut *s.desk.world.conn().await,
        orch_authz::things::Thing::Other(ObjectRef {
            kind: kind.parse().unwrap(),
            id,
        }),
    )
    .await
    .unwrap();
    assert_eq!(
        stored.first_owner,
        Some(s.desk.ada.id()),
        "the scoped request performed the real transfer"
    );
}
macro_rules! native_style {
    ($name:ident, $kind:literal) => {
        #[tokio::test]
        async fn $name() {
            style($kind).await;
        }
    };
}
native_style!(
    provider_individual_finishing_keeps_the_native_result,
    "provider"
);
native_style!(agent_single_rows_keep_the_native_result, "agent");
native_style!(
    environment_whole_view_keeps_the_native_result,
    "environment"
);
native_style!(host_retained_layers_keep_the_native_result, "host");
#[tokio::test]
async fn machine_canonical_subject_keeps_the_native_result() {
    style("machine").await;
    host_facade_ownership().await;
}
native_style!(template_single_rows_keep_the_native_result, "template");
#[tokio::test]
async fn mcp_individual_environment_style_keeps_the_native_result() {
    style("mcp_server").await;
    environment_bound_mcp_ownership().await;
}
native_style!(mount_single_projection_keeps_the_native_result, "mount");
native_style!(skill_single_maintenance_keeps_the_native_result, "skill");
native_style!(document_metadata_cut_keeps_the_native_result, "document");
native_style!(
    folder_metadata_cut_keeps_the_native_result,
    "document_container"
);

#[tokio::test]
async fn manage_only_transfer_returns_actual_ownership_without_native_content() {
    let s = Scene::new().await;
    let id = s.target("provider").await;
    let token = s.key("provider", id, false).await;
    let reply = transfer(&s, &token, "provider", id, s.receiver.id()).await;
    assert_eq!(reply.status, StatusCode::OK);
    let value: Json = serde_json::from_slice(&reply.raw).unwrap();
    assert_eq!(
        value,
        json!({"projection":"receipt","object_id":id,"kind":"provider","owner_id":s.receiver.id(),"changed":true})
    );
}

#[tokio::test]
async fn object_transfer_target_wait_checks_the_original_deadline_before_ownership_changes() {
    let s = Scene::new().await;
    let id = s.target("document").await;
    let token = new_token(API_TOKEN_PREFIX);
    identity::create_api_token_scoped(
        &mut *s.desk.world.conn().await,
        &identity::NewApiToken {
            id: new_id(),
            user_id: s.desk.ada.id(),
            name: "Expiring native ownership control",
            token_hash: &token_hash(&token),
            scopes: &["*".into()],
            node_id: None,
            at: s.desk.world.now(),
            expires_at: Some(s.desk.world.now() + chrono::Duration::seconds(1)),
        },
        None,
    )
    .await
    .unwrap();
    let state = || async {
        sqlx::query_scalar::<_,Json>("SELECT jsonb_build_object('owner',owner_id,'entries',(SELECT COALESCE(jsonb_agg(to_jsonb(e) ORDER BY id),'[]'::jsonb) FROM permission_entries e WHERE object_id=$1),'audit',(SELECT count(*) FROM audit_log WHERE object_id=$1)) FROM documents WHERE id=$1")
            .bind(id).fetch_one(&s.desk.world.db.pool).await.unwrap()
    };
    let before = state().await;
    let mut held = s.desk.world.db.pool.begin().await.unwrap();
    sqlx::query("SELECT id FROM documents WHERE id=$1 FOR UPDATE")
        .bind(id)
        .fetch_one(&mut *held)
        .await
        .unwrap();
    let locker: i32 = sqlx::query_scalar("SELECT pg_backend_pid()")
        .fetch_one(&mut *held)
        .await
        .unwrap();
    let app = s.desk.world.app.clone();
    let receiver = s.receiver.id();
    let mut pending = tokio::spawn(async move {
        let plural = "documents".to_owned();
        let object = id.to_string();
        let body = TransferBody {
            user_id: receiver,
            keep_previous: Some(true),
        };
        let call = app
            .call(ObjectTransfer, &[&plural, &object])
            .token(&token)
            .body(&body);
        call.send_bytes().await
    });
    // Reuse the page cohort's finite rendezvous bound and polling cadence. Actual PostgreSQL
    // blocking establishes the cut; the one-second deadline is crossed only by the manual clock.
    let reached = tokio::time::timeout(std::time::Duration::from_secs(20), async {
        loop {
            let waiting: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM pg_stat_activity WHERE $1=ANY(pg_blocking_pids(pid)) AND wait_event_type='Lock' AND query LIKE '%documents%')")
                .bind(locker).fetch_one(&s.desk.world.db.pool).await.unwrap();
            if waiting { break true; }
            if pending.is_finished() { break false; }
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
    }).await;
    if !matches!(reached, Ok(true)) {
        pending.abort();
        let drained = pending.await;
        held.rollback().await.unwrap();
        panic!("the native ownership target wait was not reached: {drained:?}");
    }
    s.desk.world.advance(chrono::Duration::seconds(2));
    held.rollback().await.unwrap();
    let reply = match tokio::time::timeout(std::time::Duration::from_secs(20), &mut pending).await {
        Ok(reply) => reply.unwrap(),
        Err(error) => {
            pending.abort();
            let drained = pending.await;
            panic!("the released ownership request did not finish: {error}; {drained:?}");
        }
    };
    assert_eq!(
        state().await,
        before,
        "an expired original credential changes no ownership, entry or audit row"
    );
    assert_eq!(
        reply.problem().code,
        orch_api_contract::common::ErrorCode::AuthInvalidToken
    );
}

#[tokio::test]
async fn native_machine_host_facade_has_one_ownership_page_and_cannot_transfer_its_physical_identity_as_a_second_resource()
 {
    host_facade_ownership().await;
}
async fn host_facade_ownership() {
    let s = Scene::new().await;
    let machine = s.resource("machine").await;
    let host = s.resource("host").await;
    sqlx::query("UPDATE hosts SET machine_id=$2 WHERE id=$1")
        .bind(host)
        .bind(machine)
        .execute(&s.desk.world.db.pool)
        .await
        .unwrap();
    let key = s.key("machine", machine, true).await;
    for token in [&s.desk.ada.token, &key] {
        let refused = transfer(&s, token, "machine", machine, s.receiver.id()).await;
        assert_eq!(
            refused.status,
            StatusCode::NOT_FOUND,
            "{}",
            String::from_utf8_lossy(&refused.raw)
        );
    }
    let owners: (Option<Id>, Option<Id>) = sqlx::query_as("SELECT m.owner_id,h.owner_id FROM machines m JOIN hosts h ON h.machine_id=m.id WHERE m.id=$1")
        .bind(machine).fetch_one(&s.desk.world.db.pool).await.unwrap();
    assert_eq!(owners, (Some(s.desk.ada.id()), Some(s.desk.ada.id())));
}

#[tokio::test]
async fn native_environment_bound_mcp_has_no_independent_ownership_page_and_does_not_transfer_its_environment()
 {
    environment_bound_mcp_ownership().await;
}
async fn environment_bound_mcp_ownership() {
    let s = Scene::new().await;
    let mcp = s.resource("mcp_server").await;
    let environment = s.resource("environment").await;
    sqlx::query("UPDATE mcp_servers SET environment_id=$2 WHERE id=$1")
        .bind(mcp)
        .bind(environment)
        .execute(&s.desk.world.db.pool)
        .await
        .unwrap();
    let key = s.key("mcp_server", mcp, true).await;
    for token in [&s.desk.ada.token, &key] {
        let refused = transfer(&s, token, "mcp_server", mcp, s.receiver.id()).await;
        assert_eq!(
            refused.status,
            StatusCode::NOT_FOUND,
            "{}",
            String::from_utf8_lossy(&refused.raw)
        );
    }
    let owners: (Option<Id>, Option<Id>) = sqlx::query_as("SELECT s.owner_id,e.owner_id FROM mcp_servers s JOIN environments e ON e.id=s.environment_id WHERE s.id=$1")
        .bind(mcp).fetch_one(&s.desk.world.db.pool).await.unwrap();
    assert_eq!(owners, (Some(s.desk.ada.id()), Some(s.desk.ada.id())));
}
