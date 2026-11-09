//! MCP preset duplication carries secrets only for a current native owner and reseals under the new identity.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "tests fail by panicking"
)]
mod places;
mod resources;
mod support;
mod thread_history_scope;
use orch_api_contract::{
    access::NewResourceAccess,
    duplicates::{DuplicateBody, DuplicateDestination, McpDuplicateBody, McpServerDuplicate},
    resources::{McpServerCreate, McpServerCreateBody},
};
use orch_api_core::{
    routes::resources::sealing::{binding, install_master_key},
    testing::RawReply,
};
use orch_db::{queries::resources::mcp as rows, secrets::MasterKey};
use orch_types::{Id, Json};
use serde_json::json;
use support::World;
use thread_history_scope::{Proof, key, ok, rule};
const SECRET: &str = "fixture-duplicate-secret-value";
struct Scene {
    world: World,
    company: resources::Company,
    source: Id,
}
impl Scene {
    async fn new() -> Proof<Self> {
        install_master_key(MasterKey::new([7; 32]));
        let world = World::new().await;
        let company = resources::company(&world).await;
        let source=world.app.call(McpServerCreate,&[]).token(&company.boss.token).body(&McpServerCreateBody {
            home_id:Some(company.acme),name:"source-tools".into(),description:Some("Actual preset configuration".into()),
            spec:json!({"transport":"http","url":"https://tools.example.test/mcp","authentication":{"kind":"bearer","secret":"TOKEN"},"secret_env":["TOKEN"],"tools_allow":["search"]}),
            secrets:Some([("TOKEN".into(),SECRET.into())].into()),access:Some(NewResourceAccess::OnlyMe),
        }).send().await.ok().into_full().map_err(|_|"actual native preset missing")?.id;
        Ok(Self {
            world,
            company,
            source,
        })
    }
    async fn copy(&self, token: &str, body: &DuplicateBody) -> RawReply {
        self.world
            .app
            .call(McpServerDuplicate, &[&self.source])
            .token(token)
            .body(&McpDuplicateBody::from(body.clone()))
            .send_bytes()
            .await
    }
    async fn sealed(&self, id: Id) -> Proof<Option<Vec<u8>>> {
        Ok(sqlx::query_scalar("SELECT sealed FROM secrets WHERE owner_kind='mcp_server' AND owner_id=$1 AND name='TOKEN'").bind(id).fetch_optional(&self.world.db.pool).await?)
    }
    async fn count(&self) -> Proof<i64> {
        Ok(sqlx::query_scalar("SELECT count(*) FROM mcp_servers")
            .fetch_one(&self.world.db.pool)
            .await?)
    }
}
fn id(reply: &RawReply) -> Proof<Id> {
    let value = ok(reply)?;
    let id = serde_json::from_value(value["id"].clone())?;
    assert_eq!(
        value,
        json!({"projection":"receipt","kind":"mcp_server","id":id})
    );
    assert!(!String::from_utf8_lossy(&reply.raw).contains(SECRET));
    Ok(id)
}
#[tokio::test]
async fn preset_duplicate_reseals_an_owners_secret_and_gives_other_readers_and_managers_a_signed_out_copy()
-> Proof {
    let s = Scene::new().await?;
    let original = s
        .sealed(s.source)
        .await?
        .ok_or("source credential missing")?;
    let own = id(&s
        .copy(&s.company.boss.token, &DuplicateBody::default())
        .await)?;
    let sealed = s
        .sealed(own)
        .await?
        .ok_or("actual owner's credential not copied")?;
    assert_ne!(sealed, original);
    assert_eq!(
        MasterKey::new([7; 32]).open(&sealed, &binding("mcp_server", own, "TOKEN"))?,
        SECRET.as_bytes()
    );
    assert!(
        MasterKey::new([7; 32])
            .open(&sealed, &binding("mcp_server", s.source, "TOKEN"))
            .is_err()
    );
    for (viewer, manage) in [(&s.company.viewer, false), (&s.company.admin, true)] {
        resources::name_person(&s.world, "mcp_server", s.source, "mcp.view", viewer.id()).await;
        if manage {
            resources::name_person(&s.world, "mcp_server", s.source, "mcp.manage", viewer.id())
                .await;
            places::admin_mode(&s.world, viewer, true).await;
        }
        let made = id(&s.copy(&viewer.token, &DuplicateBody::default()).await)?;
        assert!(s.sealed(made).await?.is_none());
        let row = rows::servers(&mut *s.world.conn().await, &[made])
            .await?
            .pop()
            .ok_or("new preset missing")?;
        assert_eq!(row.owner_id, Some(viewer.id()));
        assert_eq!(row.spec["secret_env"], json!(["TOKEN"]));
        assert_eq!(row.spec["tools_allow"], json!(["search"]));
        assert_eq!(row.spec["url"], "https://tools.example.test/mcp");
        let audit: Vec<Json> =
            sqlx::query_scalar("SELECT details FROM audit_log WHERE object_id=$1")
                .bind(made)
                .fetch_all(&s.world.db.pool)
                .await?;
        assert!(!json!(audit).to_string().contains(SECRET));
    }
    assert_eq!(s.count().await?, 4);
    assert_eq!(s.sealed(s.source).await?, Some(original));
    Ok(())
}
#[tokio::test]
async fn preset_duplicate_uses_exact_own_operation_scopes_native_destination_and_source_revision()
-> Proof {
    let s = Scene::new().await?;
    let read = || rule("McpServerDuplicate", "mcp_server", s.source, "self", "read");
    let write = || rule("McpServerDuplicate", "org", s.company.acme, "self", "write");
    for rules in [
        vec![read()],
        vec![write()],
        vec![
            rule("McpServerGet", "mcp_server", s.source, "self", "read"),
            write(),
        ],
    ] {
        let token = key(&s.world, s.company.boss.id(), rules).await?;
        assert_eq!(
            s.copy(&token.secret, &DuplicateBody::default())
                .await
                .status
                .as_u16(),
            403
        );
    }
    let department = places::make_department(
        &s.world,
        &s.company.boss,
        s.company.acme,
        "MCP home",
        "mcp-home",
    )
    .await
    .base
    .id;
    let token = key(&s.world, s.company.boss.id(), vec![read(), write()]).await?;
    let refused = s
        .copy(
            &token.secret,
            &DuplicateBody {
                destination: Some(DuplicateDestination::Node { id: department }),
                ..Default::default()
            },
        )
        .await;
    assert_eq!(
        refused.status.as_u16(),
        403,
        "a source-root scope cannot assert a different creation target"
    );
    let full = key(
        &s.world,
        s.company.boss.id(),
        vec![
            read(),
            rule("McpServerDuplicate", "org", department, "self", "write"),
        ],
    )
    .await?;
    let made = id(&s
        .copy(
            &full.secret,
            &DuplicateBody {
                destination: Some(DuplicateDestination::Node { id: department }),
                ..Default::default()
            },
        )
        .await)?;
    let row = rows::servers(&mut *s.world.conn().await, &[made])
        .await?
        .pop()
        .ok_or("new department preset missing")?;
    assert_eq!(row.home_id, Some(department));
    assert_eq!(row.description, "Actual preset configuration");
    assert!(s.sealed(made).await?.is_some());
    let stale = s
        .copy(
            &s.company.boss.token,
            &DuplicateBody {
                expected_revision: Some(0),
                ..Default::default()
            },
        )
        .await;
    assert_eq!(
        stale.problem().code,
        orch_api_contract::common::ErrorCode::ConflictStaleRevision
    );
    resources::name_person(
        &s.world,
        "mcp_server",
        s.source,
        "mcp.view",
        s.company.viewer.id(),
    )
    .await;
    s.world
        .set_at(
            orch_settings::Target::Node(s.company.acme),
            "resources.created_by_members",
            json!([]),
        )
        .await;
    assert_eq!(
        s.copy(&s.company.viewer.token, &DuplicateBody::default())
            .await
            .status
            .as_u16(),
        403
    );
    assert_eq!(s.count().await?, 2);
    Ok(())
}
#[tokio::test]
async fn preset_secret_copy_rechecks_current_ownership_after_the_real_native_creation_name_wait()
-> Proof {
    for lose_owner in [false, true] {
        let s = Scene::new().await?;
        resources::name_person(
            &s.world,
            "mcp_server",
            s.source,
            "mcp.view",
            s.company.boss.id(),
        )
        .await;
        let mut blocker = s.world.db.pool.begin().await?;
        rows::lock_preset_name(&mut blocker, Some(s.company.acme), None, "waiting-copy").await?;
        let pid: i32 = sqlx::query_scalar("SELECT pg_backend_pid()")
            .fetch_one(&mut *blocker)
            .await?;
        let app = s.world.app.clone();
        let source = s.source;
        let actor = s.company.boss.token.clone();
        let request = tokio::spawn(async move {
            let request =
                app.call(McpServerDuplicate, &[&source])
                    .token(&actor)
                    .body(&McpDuplicateBody {
                        name: Some("waiting-copy".into()),
                        ..Default::default()
                    });
            request.send_bytes().await
        });
        tokio::time::timeout(std::time::Duration::from_secs(30),async {loop {assert!(!request.is_finished());let waiting:bool=sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM pg_stat_activity WHERE datname=current_database() AND wait_event_type='Lock' AND $1=ANY(pg_blocking_pids(pid)))").bind(pid).fetch_one(&s.world.db.pool).await.unwrap();if waiting{break;}tokio::task::yield_now().await;}}).await?;
        if lose_owner {
            resources::name_person(
                &s.world,
                "mcp_server",
                s.source,
                "access.own",
                s.company.viewer.id(),
            )
            .await;
            sqlx::query("UPDATE permission_entries SET removed_at=$3 WHERE object_kind='mcp_server' AND object_id=$1 AND permission='access.own' AND holder_id=$2 AND removed_at IS NULL").bind(s.source).bind(s.company.boss.id()).bind(s.world.now()).execute(&s.world.db.pool).await?;
        }
        blocker.commit().await?;
        let reply = tokio::time::timeout(std::time::Duration::from_secs(30), request).await??;
        if lose_owner {
            assert!(!reply.status.is_success());
            assert_eq!(s.count().await?, 1);
        } else {
            let made = id(&reply)?;
            assert!(s.sealed(made).await?.is_some());
            assert_eq!(s.count().await?, 2);
        }
    }
    Ok(())
}

async fn own_source(s: &Scene) -> Proof<(Id, Id, Id)> {
    let machine = orch_db::queries::resources::access_fixtures::insert_persons_machine(
        &mut *s.world.conn().await,
        s.company.admin.id(),
        "Native tools machine",
    )
    .await?;
    // This fixture registers the machine in the team's home. Imported definitions follow
    // that registered home, independently of the environment from which import is opened.
    sqlx::query("UPDATE machines SET home_id=$2,org_id=$2 WHERE id=$1")
        .bind(machine)
        .bind(s.company.acme)
        .execute(&s.world.db.pool)
        .await?;
    let policy = orch_link_contract::policy::LocalPolicy {
        environment_roots: vec!["/srv/mcp".into()],
        exec_offers: vec!["plain".into()],
        mcp_add: true,
        ..Default::default()
    };
    sqlx::query(
        "UPDATE machines SET online=true,facts=jsonb_build_object('policy',$2::jsonb) WHERE id=$1",
    )
    .bind(machine)
    .bind(json!(policy))
    .execute(&s.world.db.pool)
    .await?;
    let env = s
        .world
        .app
        .call(
            orch_api_contract::environment::MachineEnvironmentCreate,
            &[&machine],
        )
        .token(&s.company.admin.token)
        .body(
            &orch_api_contract::environment::MachineEnvironmentCreateBody {
                data_sources: None,
                home_id: Some(s.company.acme),
                name: "Tools environment".into(),
                root: "/srv/mcp/work".into(),
                home_node_id: None,
                remote_write: None,
                terminal: None,
                access: None,
            },
        )
        .send()
        .await
        .ok()
        .into_full()
        .map_err(|_| "native parent missing")?
        .base
        .id;
    let source=s.world.app.call(orch_api_contract::environment::EnvironmentMcpServerCreate,&[&env]).token(&s.company.admin.token).body(&orch_api_contract::environment::EnvironmentMcpServerCreateBody{name:"own-tools".into(),description:Some("Owned native definition".into()),spec:json!({"transport":"stdio","command":["fixture-tool"],"secret_env":["TOKEN"]}),secrets:Some([("TOKEN".into(),SECRET.into())].into()),enable:Some(true)}).send().await.ok().into_full().map_err(|_|"native environment definition missing")?.id;
    Ok((source, env, machine))
}
#[tokio::test]
async fn owned_server_duplicates_use_the_same_parent_writer_or_explicit_preset_home_without_bypassing_machine_policy()
-> Proof {
    let mut s = Scene::new().await?;
    let (source, env, machine) = own_source(&s).await?;
    s.source = source;
    let first = id(&s
        .copy(&s.company.admin.token, &DuplicateBody::default())
        .await)?;
    let row = rows::servers(&mut *s.world.conn().await, &[first])
        .await?
        .pop()
        .ok_or("owned copy missing")?;
    assert_eq!(row.environment_id, Some(env));
    assert_eq!(row.name, "own-tools-copy");
    assert_eq!(row.owner_id, Some(s.company.admin.id()));
    assert!(s.sealed(first).await?.is_some());
    let enabled: bool = sqlx::query_scalar(
        "SELECT COALESCE((config->'mcp.enabled'->>$2)::boolean,false) FROM environments WHERE id=$1",
    )
    .bind(env)
    .bind(first.to_string())
    .fetch_one(&s.world.db.pool)
    .await?;
    assert!(enabled);
    let preset = id(&s
        .copy(
            &s.company.admin.token,
            &DuplicateBody {
                destination: Some(DuplicateDestination::Node { id: s.company.acme }),
                ..Default::default()
            },
        )
        .await)?;
    let row = rows::servers(&mut *s.world.conn().await, &[preset])
        .await?
        .pop()
        .ok_or("preset copy missing")?;
    assert_eq!(row.environment_id, None);
    assert_eq!(row.home_id, Some(s.company.acme));
    assert!(s.sealed(preset).await?.is_some());
    let source_read = json!({"target":{"kind":"environment","id":env,"reach":"self"},"resources":["mcp_server"],"allow":[{"operation":"McpServerDuplicate","actions":["read","write"]}]});
    let partial = key(&s.world, s.company.admin.id(), vec![source_read.clone()]).await?;
    assert_eq!(
        s.copy(&partial.secret, &DuplicateBody::default())
            .await
            .status
            .as_u16(),
        403
    );
    let environment_manage = json!({"target":{"kind":"environment","id":env,"reach":"self"},"resources":["environment"],"allow":[{"operation":"McpServerDuplicate","actions":["manage"]}]});
    let full = key(
        &s.world,
        s.company.admin.id(),
        vec![source_read, environment_manage],
    )
    .await?;
    id(&s.copy(&full.secret, &DuplicateBody::default()).await)?;
    for permission in ["environment.view", "environment.manage"] {
        resources::name_person(
            &s.world,
            "environment",
            env,
            permission,
            s.company.viewer.id(),
        )
        .await;
    }
    let reader_copy = id(&s
        .copy(&s.company.viewer.token, &DuplicateBody::default())
        .await)?;
    assert!(
        s.sealed(reader_copy).await?.is_none(),
        "a parent manager does not acquire its Owners row's credential values"
    );
    sqlx::query(
        "UPDATE machines SET facts=jsonb_set(facts,'{policy,mcp.add}','false') WHERE id=$1",
    )
    .bind(machine)
    .execute(&s.world.db.pool)
    .await?;
    let before = s.count().await?;
    let refused = s.copy(&full.secret, &DuplicateBody::default()).await;
    assert_eq!(
        refused.problem().code,
        orch_api_contract::common::ErrorCode::PolicyMcpAddDisabled
    );
    assert_eq!(s.count().await?, before);
    Ok(())
}

#[path = "mcp_duplicates/imported.rs"]
mod imported;

#[tokio::test]
async fn cross_org_mcp_copy_uses_native_creation_and_records_the_original_source_namespace() -> Proof
{
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
            rule("McpServerDuplicate", "mcp_server", s.source, "self", "read"),
            rule("McpServerDuplicate", "org", destination, "self", "write"),
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
    let row: serde_json::Value = sqlx::query_scalar("SELECT jsonb_build_object('home',home_id,'org',org_id,'owner',owner_id) FROM mcp_servers WHERE id=$1")
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
async fn preset_move_uses_one_native_copy_and_delete_transaction_with_fresh_ownership_and_secrets()
-> Proof {
    use orch_api_contract::duplicates::{
        McpMoveOutBody, McpServerMoveOut, McpServerMoveOutPreview, MoveOutConfirmation,
    };
    let s = Scene::new().await?;
    let target = places::make_org(
        &s.world,
        &s.company.boss,
        "Move recipient",
        "move-recipient",
    )
    .await
    .base
    .id;
    let copy = McpDuplicateBody {
        destination: Some(DuplicateDestination::Node { id: target }),
        ..Default::default()
    };
    let reviewer=key(&s.world,s.company.boss.id(),vec![json!({"target":{"kind":"mcp_server","id":s.source,"reach":"self"},"resources":["mcp_server"],"allow":[{"operation":"McpServerMoveOutPreview","actions":["manage","read"]}]}),rule("McpServerMoveOutPreview","org",target,"self","write")]).await?;
    let plan = s
        .world
        .app
        .call(McpServerMoveOutPreview, &[&s.source])
        .token(&reviewer.secret)
        .body(&copy)
        .send()
        .await
        .ok();
    let actor = key(
        &s.world,
        s.company.boss.id(),
        vec![
            rule("McpServerMoveOut", "mcp_server", s.source, "self", "manage"),
            rule("McpServerMoveOut", "org", target, "self", "write"),
        ],
    )
    .await?;
    let old = s
        .sealed(s.source)
        .await?
        .ok_or("original source secret absent")?;
    let moved = s
        .world
        .app
        .call(McpServerMoveOut, &[&s.source])
        .token(&actor.secret)
        .body(&McpMoveOutBody {
            copy,
            delete_plan: MoveOutConfirmation {
                revision: plan.revision,
                stamp: plan.stamp,
            },
        })
        .send_bytes()
        .await;
    let made = id(&moved)?;
    assert!(
        rows::servers(&mut *s.world.conn().await, &[s.source])
            .await?
            .is_empty()
    );
    let row = rows::servers(&mut *s.world.conn().await, &[made])
        .await?
        .pop()
        .ok_or("moved native definition missing")?;
    assert_eq!(
        (row.home_id, row.org_id, row.owner_id),
        (Some(target), Some(target), Some(s.company.boss.id()))
    );
    let sealed = s
        .sealed(made)
        .await?
        .ok_or("moved native owner secret absent")?;
    assert_ne!(sealed, old);
    assert_eq!(
        MasterKey::new([7; 32]).open(&sealed, &binding("mcp_server", made, "TOKEN"))?,
        SECRET.as_bytes()
    );
    let actions:Vec<String>=sqlx::query_scalar("SELECT action FROM audit_log WHERE (object_id=$1 AND action IN ('content.copied_out','mcp.deleted')) OR (object_id=$2 AND action='mcp.created') ORDER BY action").bind(s.source).bind(made).fetch_all(&s.world.db.pool).await?;
    assert_eq!(
        actions,
        vec!["content.copied_out", "mcp.created", "mcp.deleted"]
    );
    Ok(())
}
#[tokio::test]
async fn mcp_move_refuses_nonowners_and_a_changed_plan_without_duplicating_or_deleting() -> Proof {
    use orch_api_contract::duplicates::{
        McpMoveOutBody, McpServerMoveOut, McpServerMoveOutPreview, MoveOutConfirmation,
    };
    let s = Scene::new().await?;
    let target = places::make_org(
        &s.world,
        &s.company.boss,
        "Move recipient",
        "move-recipient",
    )
    .await
    .base
    .id;
    let copy = McpDuplicateBody {
        destination: Some(DuplicateDestination::Node { id: target }),
        ..Default::default()
    };
    for actor in [&s.company.viewer, &s.company.admin] {
        resources::name_person(&s.world, "mcp_server", s.source, "mcp.manage", actor.id()).await;
        places::join(&s.world, target, target, actor, "admin", &s.company.boss).await;
        if actor.id() == s.company.admin.id() {
            places::admin_mode(&s.world, actor, true).await;
        }
        let refused = s
            .world
            .app
            .call(McpServerMoveOutPreview, &[&s.source])
            .token(&actor.token)
            .body(&copy)
            .send_bytes()
            .await;
        assert_eq!(refused.status.as_u16(), 403, "{:?}", refused);
    }
    let plan = s
        .world
        .app
        .call(McpServerMoveOutPreview, &[&s.source])
        .token(&s.company.boss.token)
        .body(&copy)
        .send()
        .await
        .ok();
    sqlx::query("UPDATE mcp_servers SET revision=revision+1 WHERE id=$1")
        .bind(s.source)
        .execute(&s.world.db.pool)
        .await?;
    let refused = s
        .world
        .app
        .call(McpServerMoveOut, &[&s.source])
        .token(&s.company.boss.token)
        .body(&McpMoveOutBody {
            copy,
            delete_plan: MoveOutConfirmation {
                revision: plan.revision,
                stamp: plan.stamp,
            },
        })
        .send_bytes()
        .await;
    assert_eq!(
        refused.problem().code,
        orch_api_contract::common::ErrorCode::ConflictStaleRevision
    );
    assert_eq!(s.count().await?, 1);
    assert!(
        !rows::servers(&mut *s.world.conn().await, &[s.source])
            .await?
            .is_empty()
    );
    Ok(())
}
#[tokio::test]
async fn mcp_move_rolls_the_native_creation_and_secret_pins_back_if_original_deletion_fails()
-> Proof {
    use orch_api_contract::duplicates::{
        McpMoveOutBody, McpServerMoveOut, McpServerMoveOutPreview, MoveOutConfirmation,
    };
    let s = Scene::new().await?;
    let target = places::make_org(
        &s.world,
        &s.company.boss,
        "Move recipient",
        "move-recipient",
    )
    .await
    .base
    .id;
    let copy = McpDuplicateBody {
        destination: Some(DuplicateDestination::Node { id: target }),
        ..Default::default()
    };
    let plan = s
        .world
        .app
        .call(McpServerMoveOutPreview, &[&s.source])
        .token(&s.company.boss.token)
        .body(&copy)
        .send()
        .await
        .ok();
    sqlx::query("CREATE FUNCTION hold_mcp_move_delete() RETURNS trigger LANGUAGE plpgsql AS $$ BEGIN IF NEW.deleted_at IS NOT NULL THEN RAISE EXCEPTION 'held MCP deletion'; END IF; RETURN NEW; END $$").execute(&s.world.db.pool).await?;
    sqlx::query("CREATE TRIGGER hold_mcp_move_delete BEFORE UPDATE OF deleted_at ON mcp_servers FOR EACH ROW EXECUTE FUNCTION hold_mcp_move_delete()").execute(&s.world.db.pool).await?;
    let refused = s
        .world
        .app
        .call(McpServerMoveOut, &[&s.source])
        .token(&s.company.boss.token)
        .body(&McpMoveOutBody {
            copy,
            delete_plan: MoveOutConfirmation {
                revision: plan.revision,
                stamp: plan.stamp,
            },
        })
        .send_bytes()
        .await;
    assert_eq!(refused.status.as_u16(), 500);
    assert_eq!(s.count().await?, 1);
    let secrets: i64 = sqlx::query_scalar(
        "SELECT count(DISTINCT owner_id) FROM secrets WHERE owner_kind='mcp_server'",
    )
    .fetch_one(&s.world.db.pool)
    .await?;
    assert_eq!(secrets, 1);
    let audits: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM audit_log WHERE action IN ('content.copied_out','mcp.deleted')",
    )
    .fetch_one(&s.world.db.pool)
    .await?;
    assert_eq!(audits, 0);
    Ok(())
}

#[tokio::test]
async fn an_owned_mcp_move_binds_its_real_source_parent_and_uses_the_same_native_source_ending()
-> Proof {
    use orch_api_contract::duplicates::{
        McpMoveOutBody, McpServerMoveOut, McpServerMoveOutPreview, MoveOutConfirmation,
    };
    let mut s = Scene::new().await?;
    let (source, environment, _) = own_source(&s).await?;
    s.source = source;
    let target = places::make_org(
        &s.world,
        &s.company.admin,
        "Owned move recipient",
        "owned-move-recipient",
    )
    .await
    .base
    .id;
    let copy = McpDuplicateBody {
        destination: Some(DuplicateDestination::Node { id: target }),
        ..Default::default()
    };
    let plan = s
        .world
        .app
        .call(McpServerMoveOutPreview, &[&source])
        .token(&s.company.admin.token)
        .body(&copy)
        .send()
        .await
        .ok();
    let base = vec![
        json!({"target":{"kind":"environment","id":environment,"reach":"self"},"resources":["mcp_server"],"allow":[{"operation":"McpServerMoveOut","actions":["manage"]}]}),
        rule("McpServerMoveOut", "org", target, "self", "write"),
    ];
    let request = McpMoveOutBody {
        copy,
        delete_plan: MoveOutConfirmation {
            revision: plan.revision,
            stamp: plan.stamp,
        },
    };
    let partial = key(&s.world, s.company.admin.id(), base.clone()).await?;
    assert_eq!(
        s.world
            .app
            .call(McpServerMoveOut, &[&source])
            .token(&partial.secret)
            .body(&request)
            .send_bytes()
            .await
            .status
            .as_u16(),
        403
    );
    let mut rules = base;
    rules.push(rule(
        "McpServerMoveOut",
        "environment",
        environment,
        "self",
        "manage",
    ));
    let full = key(&s.world, s.company.admin.id(), rules).await?;
    let moved = s
        .world
        .app
        .call(McpServerMoveOut, &[&source])
        .token(&full.secret)
        .body(&request)
        .send_bytes()
        .await;
    let made = id(&moved)?;
    assert!(
        rows::servers(&mut *s.world.conn().await, &[source])
            .await?
            .is_empty()
    );
    let row = rows::servers(&mut *s.world.conn().await, &[made])
        .await?
        .pop()
        .ok_or("moved own definition is missing")?;
    assert_eq!(
        (row.environment_id, row.home_id, row.owner_id),
        (None, Some(target), Some(s.company.admin.id()))
    );
    assert!(s.sealed(made).await?.is_some());
    Ok(())
}
