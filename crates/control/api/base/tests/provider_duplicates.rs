//! Provider duplicates use native creation, owner-only secret transfer and independent initial Make authority.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "tests fail by panicking"
)]
#[path = "provider_duplicates/configuration.rs"]
mod configuration;
mod places;
mod resources;
#[path = "provider_duplicates/subscriptions.rs"]
mod subscriptions;
mod support;
mod thread_history_scope;
use orch_api_contract::{
    access::NewResourceAccess,
    duplicates::{DuplicateBody, DuplicateDestination, ProviderDuplicate},
    resources::{CreateProvider, ProviderCreate, Upstream},
};
use orch_api_core::{
    routes::resources::sealing::{binding, header_secret, install_master_key},
    testing::RawReply,
};
use orch_db::secrets::MasterKey;
use orch_types::{Id, Json, new_id};
use serde_json::json;
use support::World;
use thread_history_scope::{Proof, key, ok, rule};
const API_SECRET: &str = "synthetic-provider-copy-key";
const HEADER_SECRET: &str = "synthetic-provider-copy-header";
struct Scene {
    world: World,
    company: resources::Company,
    source: Id,
}

impl Scene {
    async fn new() -> Proof<Self> {
        Self::with_sources(None).await
    }
    async fn with_sources(
        sources: Option<orch_api_contract::resources::NewProviderSources>,
    ) -> Proof<Self> {
        Self::with_location(sources, false).await
    }
    async fn with_location(
        sources: Option<orch_api_contract::resources::NewProviderSources>,
        program: bool,
    ) -> Proof<Self> {
        if std::env::var_os("ORCH_TEST_NATIVE_OPERATIONS").is_some() {
            let _ = support::Statements::now();
        }
        install_master_key(MasterKey::new([7; 32]));
        let world = World::new().await;
        world.app.app.engine.stop();
        world.seed_release().await;
        let company = resources::company(&world).await;
        let machine = if program {
            let machine = resources::machine_forwarding_to(
                &world,
                company.boss.id(),
                &["https://api.example.test/v1"],
            )
            .await;
            let mut descriptor = orch_provider_contract::SourceDescriptor::new(
                "copy-plan",
                "Copy plan",
                "A registered native source used by the fixture.",
            );
            descriptor.facts = Some(Default::default());
            sqlx::query("UPDATE machines SET facts=jsonb_set(facts,'{plugins}',$2) WHERE id=$1")
                .bind(machine)
                .bind(json!([{"kind":"provider","name":"copy-plan","descriptor":descriptor}]))
                .execute(&world.db.pool)
                .await?;
            Some(machine)
        } else {
            None
        };
        let source = world
            .app
            .call(ProviderCreate, &[])
            .token(&company.boss.token)
            .body(&CreateProvider::Api {
                name: "Original API".into(),
                home_id: company.acme,
                upstreams: vec![Upstream {
                    protocol: "openai-responses".into(),
                    base_url: "https://api.example.test/v1".into(),
                    options: json!({"secret_headers":["x-tenant-secret"]}),
                }],
                api_key: Some(API_SECRET.into()),
                secrets: Some([("x-tenant-secret".into(), HEADER_SECRET.into())].into()),
                access: Some(NewResourceAccess::OnlyMe),
                runtime_machine_id: machine,
                key_machine_id: machine,
                arrangements: None,
                runtime_config: None,
                sources,
            })
            .send()
            .await
            .ok()
            .into_full()
            .map_err(|_| "actual native source provider missing")?
            .provider
            .base
            .id;
        Ok(Self {
            world,
            company,
            source,
        })
    }
    async fn copy(&self, token: &str, body: &DuplicateBody) -> RawReply {
        self.world
            .app
            .call(ProviderDuplicate, &[&self.source])
            .token(token)
            .body(body)
            .send_bytes()
            .await
    }
    async fn count(&self) -> Proof<i64> {
        Ok(sqlx::query_scalar(
            "SELECT count(*) FROM providers WHERE org_id=$1 AND deleted_at IS NULL",
        )
        .bind(self.company.acme)
        .fetch_one(&self.world.db.pool)
        .await?)
    }
    async fn secret(&self, id: Id, name: &str) -> Proof<Option<Vec<u8>>> {
        Ok(sqlx::query_scalar(
            "SELECT sealed FROM secrets WHERE owner_kind='provider' AND owner_id=$1 AND name=$2",
        )
        .bind(id)
        .bind(name)
        .fetch_optional(&self.world.db.pool)
        .await?)
    }
}
fn id(reply: &RawReply) -> Proof<Id> {
    let value = ok(reply)?;
    let receipt: orch_api_contract::duplicates::DuplicateReceipt =
        serde_json::from_value(value.clone())?;
    assert_eq!(
        receipt.projection,
        orch_api_contract::common::ReceiptProjection::Receipt
    );
    assert_eq!(
        receipt.kind,
        orch_api_contract::duplicates::DuplicateKind::Provider
    );
    assert_eq!(value, serde_json::to_value(&receipt)?);
    assert!(!String::from_utf8_lossy(&reply.raw).contains(API_SECRET));
    assert!(!String::from_utf8_lossy(&reply.raw).contains(HEADER_SECRET));
    Ok(receipt.id)
}
#[tokio::test]
async fn provider_copy_requires_the_current_seeded_template_and_its_real_use_grant() -> Proof {
    use orch_db::queries::access::{self, PageKey};
    let s = Scene::new().await?;
    let body = DuplicateBody {
        destination: Some(DuplicateDestination::Node { id: s.company.acme }),
        ..Default::default()
    };
    let _made = id(&s.copy(&s.company.boss.token, &body).await)?;
    let providers = s.count().await?;
    let jobs: i64 = sqlx::query_scalar("SELECT count(*) FROM jobs WHERE kind='provider_provision'")
        .fetch_one(&s.world.db.pool)
        .await?;
    let template = orch_db::seeds::PROVIDER_RUNTIME_TEMPLATE_ID;
    sqlx::query("UPDATE env_templates SET enabled=false WHERE id=$1")
        .bind(template)
        .execute(&s.world.db.pool)
        .await?;
    let disabled = s.copy(&s.company.boss.token, &body).await;
    assert_eq!(disabled.status, http::StatusCode::FORBIDDEN);
    assert_eq!(s.count().await?, providers);
    sqlx::query("UPDATE env_templates SET enabled=true WHERE id=$1")
        .bind(template)
        .execute(&s.world.db.pool)
        .await?;
    {
        let mut conn = s.world.conn().await;
        let page = PageKey::new("template", template, "live");
        let use_entry = access::find_entry(
            &mut conn,
            &page,
            "any",
            "template.use",
            "everyone",
            None,
            None,
        )
        .await?
        .ok_or("the shipped template Use row is absent")?;
        assert_eq!(use_entry.source, "seed");
        sqlx::query("UPDATE permission_entries SET expires_at=$2 WHERE id=$1")
            .bind(use_entry.id)
            .bind(s.world.now())
            .execute(&mut *conn)
            .await?;
        orch_authz::epoch::bump(&mut conn).await?;
    }
    let expired = s.copy(&s.company.boss.token, &body).await;
    assert_eq!(expired.status, http::StatusCode::FORBIDDEN);
    assert_eq!(s.count().await?, providers);
    let current_jobs: i64 =
        sqlx::query_scalar("SELECT count(*) FROM jobs WHERE kind='provider_provision'")
            .fetch_one(&s.world.db.pool)
            .await?;
    assert_eq!(current_jobs, jobs, "a refused copy commits no initial Make");
    Ok(())
}
#[tokio::test]
async fn api_provider_duplicates_use_native_creation_and_reseal_only_actual_owners_secrets() -> Proof
{
    let s = Scene::new().await?;
    let department = places::make_department(
        &s.world,
        &s.company.boss,
        s.company.acme,
        "API copies",
        "api-copies",
    )
    .await
    .base
    .id;
    let original = s
        .secret(s.source, "api_key")
        .await?
        .ok_or("source API key missing")?;
    for home in [s.company.acme, department] {
        let made = id(&s
            .copy(
                &s.company.boss.token,
                &DuplicateBody {
                    destination: Some(DuplicateDestination::Node { id: home }),
                    ..Default::default()
                },
            )
            .await)?;
        assert_ne!(made, s.source);
        let row: (String,Id,Id,Json,Option<Id>,Option<Id>,Json,Json)=sqlx::query_as("SELECT name,home_id,owner_id,upstreams,machine_id,key_machine_id,runtime_config,sources FROM providers WHERE id=$1").bind(made).fetch_one(&s.world.db.pool).await?;
        assert_eq!(
            (row.0.as_str(), row.1, row.2),
            ("Original API copy", home, s.company.boss.id())
        );
        assert_eq!(row.3[0]["base_url"], "https://api.example.test/v1");
        assert_eq!((row.4, row.5), (None, None));
        assert_eq!(
            row.6,
            json!({}),
            "destination runtime values and locks are selected afresh"
        );
        let sealed = s
            .secret(made, "api_key")
            .await?
            .ok_or("owner API key not copied")?;
        assert_ne!(sealed, original);
        assert_eq!(
            MasterKey::new([7; 32]).open(&sealed, &binding("provider", made, "api_key"))?,
            API_SECRET.as_bytes()
        );
        let header = header_secret("x-tenant-secret");
        let sealed = s
            .secret(made, &header)
            .await?
            .ok_or("declared header missing")?;
        assert_eq!(
            MasterKey::new([7; 32]).open(&sealed, &binding("provider", made, &header))?,
            HEADER_SECRET.as_bytes()
        );
        let (job, plan): (Id, Json) = sqlx::query_as(
            "SELECT id,plan FROM jobs WHERE provider_id=$1 AND kind='provider_provision'",
        )
        .bind(made)
        .fetch_one(&s.world.db.pool)
        .await?;
        assert_eq!(
            plan["provider_creator"]["origin"]["operation"],
            "ProviderDuplicate"
        );
        assert_eq!(
            plan["provider_creator"]["duplicate"]["source"],
            json!(s.source)
        );
        assert_eq!(plan["provider_creator"]["job"], json!(job));
        assert!(plan["provider_creator"]["duplicate"]["raw"].is_object());
        let runtimes: i64 =
            sqlx::query_scalar("SELECT count(*) FROM runtimes WHERE provider_id=$1")
                .bind(made)
                .fetch_one(&s.world.db.pool)
                .await?;
        assert_eq!(
            runtimes, 0,
            "the new initial Make has not borrowed the old runtime"
        );
    }
    resources::name_person(
        &s.world,
        "provider",
        s.source,
        "provider.view",
        s.company.admin.id(),
    )
    .await;
    resources::name_person(
        &s.world,
        "provider",
        s.source,
        "provider.manage",
        s.company.admin.id(),
    )
    .await;
    let copied = s
        .copy(&s.company.admin.token, &DuplicateBody::default())
        .await;
    let made = id(&copied)?;
    assert_eq!(
        serde_json::from_slice::<Json>(&copied.raw)?["setup"],
        json!(["credentials_not_copied"])
    );
    assert!(
        s.secret(made, "api_key").await?.is_none(),
        "a manager or org admin is not an owner"
    );
    assert!(
        s.secret(made, &header_secret("x-tenant-secret"))
            .await?
            .is_none()
    );
    let owner: Id = sqlx::query_scalar("SELECT owner_id FROM providers WHERE id=$1")
        .bind(made)
        .fetch_one(&s.world.db.pool)
        .await?;
    assert_eq!(owner, s.company.admin.id());
    Ok(())
}
#[tokio::test]
async fn provider_copy_keeps_current_runtime_template_availability_and_use_gates() -> Proof {
    for gate in ["enabled", "use"] {
        let s = Scene::new().await?;
        let template = orch_db::seeds::PROVIDER_RUNTIME_TEMPLATE_ID;
        let mut conn = s.world.conn().await;
        let changed = if gate == "enabled" {
            sqlx::query("UPDATE env_templates SET enabled=false WHERE id=$1")
                .bind(template)
                .execute(&mut *conn)
                .await?
                .rows_affected()
        } else {
            sqlx::query("UPDATE permission_entries SET removed_at=$2 WHERE object_kind='template' AND object_id=$1 AND page='live' AND permission='template.use' AND removed_at IS NULL")
                .bind(template)
                .bind(s.world.now())
                .execute(&mut *conn)
                .await?
                .rows_affected()
        };
        assert!(
            changed > 0,
            "the fixture withdraws a real {gate} dependency"
        );
        orch_authz::epoch::bump(&mut conn).await?;
        drop(conn);
        let reply = s
            .copy(&s.company.boss.token, &DuplicateBody::default())
            .await;
        assert_eq!(
            reply.status,
            http::StatusCode::FORBIDDEN,
            "the native {gate} gate remains mandatory: {}",
            String::from_utf8_lossy(&reply.raw)
        );
        let problem: Json = serde_json::from_slice(&reply.raw)?;
        assert_eq!(problem["code"], "authz.denied");
        assert_eq!(s.count().await?, 1, "a refused copy creates no provider");
        let copies: i64 = sqlx::query_scalar(
            "SELECT count(*) FROM jobs WHERE kind='provider_provision' AND provider_id<>$1",
        )
        .bind(s.source)
        .fetch_one(&s.world.db.pool)
        .await?;
        assert_eq!(copies, 0, "a refused copy leaves no first-Make job");
    }
    Ok(())
}

#[tokio::test]
async fn provider_duplicate_requires_its_own_source_read_and_destination_write_and_native_creation()
-> Proof {
    let s = Scene::new().await?;
    let read = rule("ProviderDuplicate", "provider", s.source, "self", "read");
    let write = rule("ProviderDuplicate", "org", s.company.acme, "self", "write");
    for rules in [
        vec![read.clone()],
        vec![write.clone()],
        vec![
            rule("ProviderGet", "provider", s.source, "self", "read"),
            write.clone(),
        ],
        vec![
            rule("ProviderDuplicate", "provider", new_id(), "self", "read"),
            write.clone(),
        ],
    ] {
        let token = key(&s.world, s.company.boss.id(), rules).await?;
        assert!(
            !s.copy(&token.secret, &DuplicateBody::default())
                .await
                .status
                .is_success()
        );
        assert_eq!(s.count().await?, 1);
    }
    let token = key(
        &s.world,
        s.company.boss.id(),
        vec![read.clone(), write.clone()],
    )
    .await?;
    id(&s.copy(&token.secret, &DuplicateBody::default()).await)?;
    let token = key(&s.world, s.company.stranger.id(), vec![read, write]).await?;
    assert!(
        !s.copy(&token.secret, &DuplicateBody::default())
            .await
            .status
            .is_success(),
        "a scope never supplies ordinary View or MayMake"
    );
    assert_eq!(s.count().await?, 2);
    let stale = s
        .copy(
            &s.company.boss.token,
            &DuplicateBody {
                expected_revision: Some(i64::MAX),
                ..Default::default()
            },
        )
        .await;
    assert_eq!(stale.status.as_u16(), 409);
    assert_eq!(s.count().await?, 2);
    Ok(())
}
#[tokio::test]
async fn portable_subscription_copies_start_signed_out_with_no_login_secret_or_old_runtime() -> Proof
{
    use orch_api_contract::resources::{OrgAgentCreate, OrgAgentCreateBody};
    let s = Scene::new().await?;
    let agent = s
        .world
        .app
        .call(OrgAgentCreate, &[&s.company.acme])
        .token(&s.company.boss.token)
        .body(&OrgAgentCreateBody {
            name: "Portable subscription profile".into(),
            harness_kind: "fake".into(),
            profile: json!({}),
        })
        .send()
        .await
        .ok()
        .into_full()
        .map_err(|_| "native subscription profile missing")?
        .base
        .id;
    let source = s
        .world
        .app
        .call(ProviderCreate, &[])
        .token(&s.company.boss.token)
        .body(&CreateProvider::Subscription {
            name: "Original subscription".into(),
            home_id: s.company.acme,
            agent_id: agent,
            config: None,
            sign_in: None,
            runtime_machine_id: None,
            key_machine_id: None,
            access: Some(NewResourceAccess::OnlyMe),
            arrangements: None,
            runtime_config: None,
            sources: None,
        })
        .send()
        .await
        .ok()
        .into_full()
        .map_err(|_| "native subscription missing")?
        .provider
        .base
        .id;
    sqlx::query("UPDATE providers SET login_state=$2,status='ready' WHERE id=$1")
        .bind(source)
        .bind(json!({"state":"completed","account":"synthetic login"}))
        .execute(&s.world.db.pool)
        .await?;
    let sealed = MasterKey::new([7; 32]).seal(
        b"synthetic rotating refresh",
        &binding("provider", source, "login:refresh"),
    )?;
    orch_db::queries::settings::put_secret(
        &mut *s.world.conn().await,
        "provider",
        source,
        "login:refresh",
        &sealed,
        Some(s.company.boss.id()),
        s.world.now(),
    )
    .await?;
    let reply = s
        .world
        .app
        .call(ProviderDuplicate, &[&source])
        .token(&s.company.boss.token)
        .body(&DuplicateBody::default())
        .send_bytes()
        .await;
    let made = id(&reply)?;
    type CopiedLogin = (
        String,
        Option<Json>,
        Option<Json>,
        Option<Id>,
        Option<Id>,
        Option<Id>,
    );
    let row: CopiedLogin = sqlx::query_as("SELECT status,login_state,login_pending,machine_id,key_machine_id,held_by_environment_id FROM providers WHERE id=$1").bind(made).fetch_one(&s.world.db.pool).await?;
    assert_eq!(row, ("needs_setup".into(), None, None, None, None, None));
    let secrets: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM secrets WHERE owner_kind='provider' AND owner_id=$1",
    )
    .bind(made)
    .fetch_one(&s.world.db.pool)
    .await?;
    assert_eq!(secrets, 0);
    assert!(
        s.secret(source, "login:refresh").await?.is_some(),
        "the source's rotating sign-in was not moved or consumed"
    );
    Ok(())
}
#[tokio::test]
async fn completed_provider_copy_replays_its_sparse_identity_without_repeating_the_make_or_credentials()
-> Proof {
    let s = Scene::new().await?;
    let token = key(
        &s.world,
        s.company.boss.id(),
        vec![
            rule("ProviderDuplicate", "provider", s.source, "self", "read"),
            rule("ProviderDuplicate", "org", s.company.acme, "self", "write"),
        ],
    )
    .await?;
    let retry = new_id().to_string();
    let body = DuplicateBody::default();
    let first = s
        .world
        .app
        .call(ProviderDuplicate, &[&s.source])
        .token(&token.secret)
        .header("Idempotency-Key", &retry)
        .body(&body)
        .send_bytes()
        .await;
    let made = id(&first)?;
    let reader = key(
        &s.world,
        s.company.boss.id(),
        vec![rule("ProviderGet", "provider", s.source, "self", "read")],
    )
    .await?;
    let replay = s
        .world
        .app
        .call(ProviderDuplicate, &[&s.source])
        .token(&reader.secret)
        .header("Idempotency-Key", &retry)
        .body(&body)
        .send_bytes()
        .await;
    assert_eq!(id(&replay)?, made);
    assert_eq!(s.count().await?, 2);
    let jobs: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM jobs WHERE provider_id=$1 AND kind='provider_provision'",
    )
    .bind(made)
    .fetch_one(&s.world.db.pool)
    .await?;
    assert_eq!(jobs, 1);
    assert!(!s.copy(&reader.secret, &body).await.status.is_success());
    Ok(())
}
#[tokio::test]
async fn provider_copy_uses_the_original_raw_deadline_after_the_real_source_row_wait() -> Proof {
    for expire in [false, true] {
        let s = Scene::new().await?;
        let token = key(
            &s.world,
            s.company.boss.id(),
            vec![
                rule("ProviderDuplicate", "provider", s.source, "self", "read"),
                rule("ProviderDuplicate", "org", s.company.acme, "self", "write"),
            ],
        )
        .await?;
        let mut blocker = s.world.db.pool.begin().await?;
        sqlx::query("SELECT id FROM providers WHERE id=$1 FOR UPDATE")
            .bind(s.source)
            .execute(&mut *blocker)
            .await?;
        let pid: i32 = sqlx::query_scalar("SELECT pg_backend_pid()")
            .fetch_one(&mut *blocker)
            .await?;
        let app = s.world.app.clone();
        let source = s.source;
        let task = tokio::spawn(async move {
            let request = app
                .call(ProviderDuplicate, &[&source])
                .token(&token.secret)
                .body(&DuplicateBody::default());
            request.send_bytes().await
        });
        tokio::time::timeout(std::time::Duration::from_secs(30),async {
            loop {
                assert!(!task.is_finished());
                let waits:bool=sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM pg_stat_activity WHERE datname=current_database() AND wait_event_type='Lock' AND $1=ANY(pg_blocking_pids(pid)))").bind(pid).fetch_one(&s.world.db.pool).await.unwrap();
                if waits {break;} tokio::task::yield_now().await;
            }
        }).await?;
        if expire {
            s.world.advance(chrono::Duration::hours(2));
        }
        blocker.commit().await?;
        let reply = tokio::time::timeout(std::time::Duration::from_secs(30), task).await??;
        if expire {
            assert!(!reply.status.is_success());
            assert_eq!(s.count().await?, 1);
        } else {
            id(&reply)?;
            assert_eq!(s.count().await?, 2);
        }
    }
    Ok(())
}

#[tokio::test]
async fn cross_org_provider_copy_uses_native_creation_and_records_the_original_source_namespace()
-> Proof {
    let s = Scene::new().await?;
    let destination = places::make_org(
        &s.world,
        &s.company.boss,
        "Cross-root recipient",
        "cross-root-recipient",
    )
    .await
    .base
    .id;
    let token = key(
        &s.world,
        s.company.boss.id(),
        vec![
            rule("ProviderDuplicate", "provider", s.source, "self", "read"),
            rule("ProviderDuplicate", "org", destination, "self", "write"),
        ],
    )
    .await?;
    let made = id(&s
        .copy(
            &token.secret,
            &DuplicateBody {
                destination: Some(DuplicateDestination::Node { id: destination }),
                ..Default::default()
            },
        )
        .await)?;
    assert_ne!(made, s.source);
    let row: serde_json::Value = sqlx::query_scalar("SELECT jsonb_build_object('home',home_id,'org',org_id,'owner',owner_id) FROM providers WHERE id=$1")
        .bind(made).fetch_one(&s.world.db.pool).await?;
    assert_eq!(
        row,
        json!({"home":destination,"org":destination,"owner":s.company.boss.id()})
    );
    let audit: (Option<Id>, serde_json::Value) = sqlx::query_as(
        "SELECT org_id,details FROM audit_log WHERE object_id=$1 AND action='content.copied_out'",
    )
    .bind(s.source)
    .fetch_one(&s.world.db.pool)
    .await?;
    assert_eq!(audit.0, Some(s.company.acme));
    assert_eq!(audit.1["created_id"], json!(made));
    assert_eq!(audit.1["destination_org_id"], json!(destination));
    Ok(())
}

#[tokio::test]
async fn a_scoped_owner_provider_move_reviews_the_native_delete_and_queues_only_its_actual_first_make()
-> Proof {
    use orch_api_contract::duplicates::{
        MoveOutBody, MoveOutConfirmation, MoveOutDeletePlan, ProviderMoveOut,
        ProviderMoveOutPreview,
    };
    let s = Scene::new().await?;
    let destination = places::make_org(
        &s.world,
        &s.company.boss,
        "Move recipient",
        "move-recipient",
    )
    .await
    .base
    .id;
    let copy = DuplicateBody {
        destination: Some(DuplicateDestination::Node { id: destination }),
        ..Default::default()
    };
    let token = key(
        &s.world,
        s.company.boss.id(),
        vec![
            rule(
                "ProviderMoveOutPreview",
                "provider",
                s.source,
                "self",
                "manage",
            ),
            rule(
                "ProviderMoveOutPreview",
                "provider",
                s.source,
                "self",
                "read",
            ),
            rule(
                "ProviderMoveOutPreview",
                "org",
                destination,
                "self",
                "write",
            ),
            rule("ProviderMoveOut", "provider", s.source, "self", "manage"),
            rule("ProviderMoveOut", "org", destination, "self", "write"),
        ],
    )
    .await?;
    let preview = s
        .world
        .app
        .call(ProviderMoveOutPreview, &[&s.source])
        .token(&token.secret)
        .body(&copy)
        .send_bytes()
        .await;
    ok(&preview)?;
    let plan: MoveOutDeletePlan = serde_json::from_slice(&preview.raw)?;
    let reply = s
        .world
        .app
        .call(ProviderMoveOut, &[&s.source])
        .token(&token.secret)
        .body(&MoveOutBody {
            copy: copy.clone(),
            delete_plan: MoveOutConfirmation {
                revision: plan.revision,
                stamp: plan.stamp,
            },
        })
        .send_bytes()
        .await;
    let made = id(&reply)?;
    assert_ne!(made, s.source);
    let source_live: bool =
        sqlx::query_scalar("SELECT deleted_at IS NULL FROM providers WHERE id=$1")
            .bind(s.source)
            .fetch_one(&s.world.db.pool)
            .await?;
    assert!(
        source_live,
        "native Make has not succeeded yet; source Delete cannot run at queue time"
    );
    let origin:Json=sqlx::query_scalar("SELECT plan->'provider_creator' FROM jobs WHERE provider_id=$1 AND kind='provider_provision' AND state='queued'").bind(made).fetch_one(&s.world.db.pool).await?;
    assert_eq!(origin["origin"]["operation"], "ProviderMoveOut");
    assert_eq!(origin["duplicate"]["source"], json!(s.source));
    assert!(origin["duplicate"]["moving"].is_string());
    let new:Json=sqlx::query_scalar("SELECT jsonb_build_object('owner',owner_id,'home',home_id,'machine',machine_id) FROM providers WHERE id=$1").bind(made).fetch_one(&s.world.db.pool).await?;
    assert_eq!(
        new,
        json!({"owner":s.company.boss.id(),"home":destination,"machine":null})
    );
    let sealed = s
        .secret(made, "api_key")
        .await?
        .ok_or("copied key absent")?;
    assert_eq!(
        MasterKey::new([7; 32]).open(&sealed, &binding("provider", made, "api_key"))?,
        API_SECRET.as_bytes()
    );
    Ok(())
}
#[tokio::test]
async fn a_provider_move_requires_the_actual_owner_and_a_current_native_delete_confirmation()
-> Proof {
    use orch_api_contract::duplicates::{
        MoveOutBody, MoveOutConfirmation, MoveOutDeletePlan, ProviderMoveOut,
        ProviderMoveOutPreview,
    };
    let s = Scene::new().await?;
    let destination = places::make_org(
        &s.world,
        &s.company.boss,
        "Reviewed recipient",
        "reviewed-recipient",
    )
    .await
    .base
    .id;
    let copy = DuplicateBody {
        destination: Some(DuplicateDestination::Node { id: destination }),
        ..Default::default()
    };
    let preview = s
        .world
        .app
        .call(ProviderMoveOutPreview, &[&s.source])
        .token(&s.company.boss.token)
        .body(&copy)
        .send_bytes()
        .await;
    ok(&preview)?;
    let plan: MoveOutDeletePlan = serde_json::from_slice(&preview.raw)?;
    sqlx::query("UPDATE providers SET revision=revision+1 WHERE id=$1")
        .bind(s.source)
        .execute(&s.world.db.pool)
        .await?;
    let reply = s
        .world
        .app
        .call(ProviderMoveOut, &[&s.source])
        .token(&s.company.boss.token)
        .body(&MoveOutBody {
            copy,
            delete_plan: MoveOutConfirmation {
                revision: plan.revision,
                stamp: plan.stamp,
            },
        })
        .send_bytes()
        .await;
    assert_eq!(reply.status, 409, "{}", String::from_utf8_lossy(&reply.raw));
    assert_eq!(s.count().await?, 1);
    resources::name_person(
        &s.world,
        "provider",
        s.source,
        "provider.view",
        s.company.admin.id(),
    )
    .await;
    resources::name_person(
        &s.world,
        "provider",
        s.source,
        "provider.manage",
        s.company.admin.id(),
    )
    .await;
    let manager_home = places::make_org(
        &s.world,
        &s.company.admin,
        "Manager destination",
        "manager-destination",
    )
    .await
    .base
    .id;
    let reply = s
        .world
        .app
        .call(ProviderMoveOutPreview, &[&s.source])
        .token(&s.company.admin.token)
        .body(&DuplicateBody {
            destination: Some(DuplicateDestination::Node { id: manager_home }),
            ..Default::default()
        })
        .send_bytes()
        .await;
    assert_eq!(
        reply.status, 403,
        "a native manager with creation rights is still not an owner"
    );
    Ok(())
}
