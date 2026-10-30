//! How the system stands in Management: its parts and queues, its blob store, its certificates and OpenAPI document, its hosts and runtimes, what it stored about itself, and its plugins.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a test fails by panicking"
)]

mod admin_support;
mod places;
mod support;

use admin_support::{Cast, api_token, assert_no_system_admin, cast, refused_to};
use chrono::{DateTime, Duration, Utc};
use orch_api_contract::admin::{
    AdminHostList, AdminMetricList, AdminPkiGet, AdminPlugin, AdminPluginList, AdminRuntimeList,
    AdminRuntimeListQuery, AdminStorageGet, AdminSystemGet, MetricResolution, MetricSeries,
    OpenapiGet, PkiCaGet, PluginState, Runtime, SystemComponent, SystemComponentName,
};
use orch_api_contract::common::ErrorCode;
use orch_api_contract::resources::{Host, UnitKind};
use orch_config::ServerConfig;
use orch_config::server::Plugin;
use orch_db::queries::resources::access_fixtures as fixtures;
use orch_db::queries::resources::testing::insert_machine;
use orch_db::{heartbeat, migrations};
use orch_pki::ca::Holder;
use orch_pki::certificates::{Certificate, ServiceName};
use orch_types::{Id, Time};
use places::Who;
use serde_json::json;
use support::World;

async fn sql(world: &World, statement: &str) {
    sqlx::query(sqlx::AssertSqlSafe(statement.to_owned()))
        .execute(&world.db.pool)
        .await
        .unwrap();
}

fn component(parts: &[SystemComponent], name: SystemComponentName) -> &SystemComponent {
    parts.iter().find(|part| part.name == name).unwrap()
}

#[tokio::test]
async fn the_state_of_the_system_says_how_its_parts_fare_and_what_waits_in_its_queues() {
    let world = World::new().await;
    let c = cast(&world).await;
    let now = world.now();
    {
        let mut conn = world.conn().await;
        let run = orch_types::new_id();
        heartbeat::beat(
            &mut conn,
            "gateway",
            run,
            "0.1.0",
            now - Duration::hours(1),
            now,
        )
        .await
        .unwrap();
        heartbeat::beat(
            &mut conn,
            "worker",
            run,
            "0.1.0",
            now - Duration::hours(1),
            now - Duration::minutes(2),
        )
        .await
        .unwrap();
    }
    sql(&world, "INSERT INTO outbox (id, channel, payload) VALUES (gen_random_uuid(), 'email', '{}'), (gen_random_uuid(), 'email', '{}')").await;
    sql(&world, "INSERT INTO outbox (id, channel, payload, state) VALUES (gen_random_uuid(), 'email', '{}', 'sent')").await;
    let online = {
        let mut conn = world.conn().await;
        let online = insert_machine(&mut conn, "online one", "connected")
            .await
            .unwrap();
        insert_machine(&mut conn, "offline one", "connected")
            .await
            .unwrap();
        online
    };
    sql(
        &world,
        &format!("UPDATE machines SET online = true WHERE id = '{online}'"),
    )
    .await;
    sql(
        &world,
        &format!(
            "INSERT INTO commands (id, machine_id, kind, body, state, deadline_at) VALUES
             (gen_random_uuid(), '{online}', 'x', '{{}}', 'queued', now() + interval '1 hour'),
             (gen_random_uuid(), '{online}', 'x', '{{}}', 'succeeded', now() + interval '1 hour')"
        ),
    )
    .await;
    sql(&world, "INSERT INTO jobs (id, kind, state) VALUES (gen_random_uuid(), 'snapshot', 'running'), (gen_random_uuid(), 'snapshot', 'queued')").await;
    let project = places::make_project(&world, &c.boss, c.acme, "Web")
        .await
        .base
        .id;
    let topic = places::make_topic(&world, &c.boss, project, "Kickoff")
        .await
        .base
        .id;
    {
        let mut conn = world.conn().await;
        for (name, state) in [("running one", "running"), ("idle one", "idle")] {
            let id = orch_types::new_id();
            orch_db::queries::hierarchy::testing::insert_node(
                &mut conn,
                id,
                "thread",
                Some(topic),
                name,
                c.boss.id(),
            )
            .await
            .unwrap();
            sqlx::query("UPDATE threads SET state = $2 WHERE node_id = $1")
                .bind(id)
                .bind(state)
                .execute(&mut *conn)
                .await
                .unwrap();
        }
    }

    let info = world
        .app
        .call(AdminSystemGet, &[])
        .token(&c.root.token)
        .send()
        .await
        .ok();

    assert_eq!(info.version, "0.0.0-test");
    assert_eq!(info.build, "1");
    assert!(info.started_at.get() <= now);
    let names: Vec<_> = info.components.iter().map(|part| part.name).collect();
    assert_eq!(
        names,
        [
            SystemComponentName::Api,
            SystemComponentName::Gateway,
            SystemComponentName::Worker,
            SystemComponentName::Database
        ]
    );
    assert!(component(&info.components, SystemComponentName::Api).ok);
    assert!(component(&info.components, SystemComponentName::Database).ok);
    let gateway = component(&info.components, SystemComponentName::Gateway);
    assert!(gateway.ok && gateway.detail.is_none());
    assert_eq!(gateway.seen_at.unwrap().get(), now);
    let worker = component(&info.components, SystemComponentName::Worker);
    assert!(
        !worker.ok,
        "its last report is older than three of its beats"
    );
    assert_eq!(
        worker.detail.as_deref(),
        Some("it reported last 120 seconds ago")
    );
    assert_eq!(info.machines_online, 1);
    assert_eq!(info.threads_running, 1);
    assert_eq!(info.queues.outbox_pending, 2);
    assert_eq!(info.queues.commands_open, 1);
    assert_eq!(info.queues.jobs_running, 1);
}

#[tokio::test]
async fn a_part_that_has_never_reported_is_not_well_and_the_api_row_gives_its_start() {
    let world = World::new().await;
    let c = cast(&world).await;

    let silent = world
        .app
        .call(AdminSystemGet, &[])
        .token(&c.root.token)
        .send()
        .await
        .ok();
    let started = world.now() - Duration::days(3);
    {
        let mut conn = world.conn().await;
        heartbeat::beat(
            &mut conn,
            "api",
            orch_types::new_id(),
            "0.1.0",
            started,
            world.now(),
        )
        .await
        .unwrap();
    }
    let reported = world
        .app
        .call(AdminSystemGet, &[])
        .token(&c.root.token)
        .send()
        .await
        .ok();

    let worker = component(&silent.components, SystemComponentName::Worker);
    assert!(!worker.ok);
    assert_eq!(worker.detail.as_deref(), Some("it has not reported yet"));
    assert!(worker.seen_at.is_none());
    assert_eq!(reported.started_at.get(), started);
}

#[tokio::test]
async fn storage_says_what_the_blob_store_holds_and_which_environments_hold_the_most() {
    let directory = tempfile::tempdir().unwrap();
    let mut config = ServerConfig::default();
    directory
        .path()
        .to_str()
        .unwrap()
        .clone_into(&mut config.storage.root);
    let world = World::with(config).await;
    let c = cast(&world).await;
    let (first, second) = {
        let mut conn = world.conn().await;
        let first = fixtures::insert_resource(
            &mut conn,
            "environment",
            Some(c.boss.id()),
            Some(c.acme),
            "Big",
        )
        .await
        .unwrap();
        let second = fixtures::insert_resource(
            &mut conn,
            "environment",
            Some(c.boss.id()),
            Some(c.acme),
            "Small",
        )
        .await
        .unwrap();
        (first, second)
    };
    for (hash, size, pins) in [(1_u8, 1000, 1), (2, 300, 0), (3, 200, 0)] {
        sqlx::query("INSERT INTO blobs (hash, size, pin_count) VALUES ($1, $2, $3)")
            .bind([hash; 32].as_slice())
            .bind(size)
            .bind(pins)
            .execute(&world.db.pool)
            .await
            .unwrap();
    }
    for (environment, path, hash) in [
        (first, "a", 1_u8),
        (first, "b", 2),
        (first, "c", 2),
        (second, "d", 3),
    ] {
        sqlx::query("INSERT INTO file_cache (environment_id, path, size, mtime_ns, hash) VALUES ($1, $2, 1, 1, $3)")
            .bind(environment)
            .bind(path)
            .bind([hash; 32].as_slice())
            .execute(&world.db.pool)
            .await
            .unwrap();
    }

    let stats = world
        .app
        .call(AdminStorageGet, &[])
        .token(&c.root.token)
        .send()
        .await
        .ok();

    assert_eq!(stats.blob_count, 3);
    assert_eq!(stats.blob_bytes, 1500);
    assert_eq!(stats.pinned_bytes, 1000, "what something refers to is kept");
    assert_eq!(stats.cache_bytes, 500, "the rest may be evicted");
    assert!(
        stats.disk_free_bytes > 0,
        "the disk the store is on has room"
    );
    let top: Vec<_> = stats
        .top_environments
        .iter()
        .map(|each| {
            (
                each.environment.name.as_str(),
                each.cache_bytes,
                each.pinned_bytes,
            )
        })
        .collect();
    assert_eq!(
        top,
        [("Big", 300, 1000), ("Small", 200, 0)],
        "a blob shared by two files of one environment counts once"
    );
}

#[tokio::test]
async fn the_authority_names_the_machines_whose_certificates_end_soon_in_expiry_order() {
    let world = World::new().await;
    let c = cast(&world).await;
    let authority = world
        .app
        .app
        .link
        .certificates()
        .authority_certificate()
        .clone();
    let (soon, later, lapsed, revoked, boundary, beyond, deleted) = {
        let mut conn = world.conn().await;
        (
            insert_machine(&mut conn, "soon", "connected")
                .await
                .unwrap(),
            insert_machine(&mut conn, "later", "connected")
                .await
                .unwrap(),
            insert_machine(&mut conn, "lapsed", "connected")
                .await
                .unwrap(),
            insert_machine(&mut conn, "revoked", "connected")
                .await
                .unwrap(),
            insert_machine(&mut conn, "at thirty days", "managed")
                .await
                .unwrap(),
            insert_machine(&mut conn, "past thirty days", "connected")
                .await
                .unwrap(),
            insert_machine(&mut conn, "deleted", "connected")
                .await
                .unwrap(),
        )
    };
    let end = |id: Id, at: DateTime<Utc>| {
        let world = &world;
        async move {
            sqlx::query("UPDATE machines SET cert_not_after = $2 WHERE id = $1")
                .bind(id)
                .bind(at)
                .execute(&world.db.pool)
                .await
                .unwrap();
        }
    };
    end(soon, world.now() + Duration::days(10)).await;
    end(later, world.now() + Duration::days(90)).await;
    end(lapsed, world.now() - Duration::days(2)).await;
    end(revoked, world.now() + Duration::days(5)).await;
    end(boundary, world.now() + Duration::days(30)).await;
    end(
        beyond,
        world.now() + Duration::days(30) + Duration::seconds(1),
    )
    .await;
    end(deleted, world.now() + Duration::days(1)).await;
    sql(
        &world,
        &format!("UPDATE machines SET status = 'revoked' WHERE id = '{revoked}'"),
    )
    .await;
    sql(
        &world,
        &format!("UPDATE machines SET deleted_at = now() WHERE id = '{deleted}'"),
    )
    .await;

    let pki = world
        .app
        .call(AdminPkiGet, &[])
        .token(&c.root.token)
        .send()
        .await
        .ok();

    assert_eq!(pki.ca.fingerprint_sha256, authority.fingerprint());
    assert_eq!(pki.ca.fingerprint_sha256.len(), 64);
    assert_eq!(pki.ca.not_after, authority.not_after());
    assert_eq!(pki.ca.not_before, authority.not_before());
    assert_eq!(
        Some(pki.ca.subject.as_str()),
        authority.subject_common_name()
    );
    assert_eq!(
        pki.machines_expiring_30d, 3,
        "lapsed, soon and exactly thirty days; not revoked, deleted or beyond the window"
    );
    assert_eq!(
        pki.machines_expiring
            .iter()
            .map(|entry| (entry.machine.id, entry.not_after))
            .collect::<Vec<_>>(),
        [
            (lapsed, Time::new(world.now() - Duration::days(2))),
            (soon, Time::new(world.now() + Duration::days(10))),
            (boundary, Time::new(world.now() + Duration::days(30))),
        ]
    );
    assert_eq!(pki.machines_expiring[0].machine.name, "lapsed");
    assert_eq!(
        pki.machines_expiring[0].machine.platform_os,
        orch_api_contract::resources::MachineOs::Linux
    );
    assert_eq!(
        pki.machines_expiring[2].machine.placement,
        orch_api_contract::environment::Placement::Managed
    );
}

#[tokio::test]
async fn the_pki_list_stops_at_fifty_soonest_certificates_but_counts_every_machine_due() {
    let world = World::new().await;
    let c = cast(&world).await;
    let mut expected = Vec::new();
    {
        let mut conn = world.conn().await;
        // Insert latest first so insertion order cannot stand in for expiry order.
        for hour in (1_i64..=53).rev() {
            let id = insert_machine(&mut conn, &format!("due-{hour}"), "connected")
                .await
                .unwrap();
            let not_after = world.now() + Duration::hours(hour);
            sqlx::query("UPDATE machines SET cert_not_after = $2 WHERE id = $1")
                .bind(id)
                .bind(not_after)
                .execute(&mut *conn)
                .await
                .unwrap();
            expected.push((id, not_after));
        }
    }
    expected.sort_unstable_by_key(|(id, at)| (*at, *id));
    let pki = world
        .app
        .call(AdminPkiGet, &[])
        .token(&c.root.token)
        .send()
        .await
        .ok();
    assert_eq!(
        pki.machines_expiring_30d, 53,
        "the total is not capped with the list"
    );
    assert_eq!(pki.machines_expiring.len(), 50);
    assert_eq!(
        pki.machines_expiring
            .iter()
            .map(|entry| (entry.machine.id, entry.not_after))
            .collect::<Vec<_>>(),
        expected
            .into_iter()
            .take(50)
            .map(|(id, at)| (id, Time::new(at)))
            .collect::<Vec<_>>()
    );
}

#[tokio::test]
async fn the_pki_list_is_empty_when_no_machine_certificate_needs_attention() {
    let world = World::new().await;
    let c = cast(&world).await;
    let pki = world
        .app
        .call(AdminPkiGet, &[])
        .token(&c.root.token)
        .send()
        .await
        .ok();
    assert_eq!(pki.machines_expiring_30d, 0);
    assert!(pki.machines_expiring.is_empty());
}

#[tokio::test]
async fn the_certificates_of_the_services_this_process_can_read_are_listed_with_their_end() {
    let world = World::new().await;
    let c = cast(&world).await;
    let directory = tempfile::tempdir().unwrap();
    let authority = world.app.app.link.certificates().authority();
    let (_, issued) = authority
        .issue_with_new_key(
            &Holder::Service {
                service: ServiceName::Gateway,
                names: vec!["gateway".to_owned()],
            },
            200,
            &world.app.clock,
        )
        .unwrap();
    std::fs::write(
        directory.path().join("service_gateway_cert.pem"),
        issued.pem(),
    )
    .unwrap();
    std::fs::write(
        directory.path().join("service_worker_cert.pem"),
        "not a certificate",
    )
    .unwrap();
    assert!(orch_api_core::routes::admin::system::install_secrets_dir(
        directory.path()
    ));

    let pki = world
        .app
        .call(AdminPkiGet, &[])
        .token(&c.root.token)
        .send()
        .await
        .ok();

    assert_eq!(
        pki.services.len(),
        1,
        "the worker's file is no certificate and the API's is not here"
    );
    assert_eq!(pki.services[0].name, "gateway");
    assert_eq!(pki.services[0].not_after, issued.not_after());
    assert_eq!(
        Certificate::from_pem(&issued.pem()).unwrap().not_after(),
        issued.not_after()
    );
}

#[tokio::test]
async fn the_certificate_of_the_authority_is_public_and_is_the_one_the_system_issues_under() {
    let world = World::new().await;
    cast(&world).await;
    let authority = world
        .app
        .app
        .link
        .certificates()
        .authority_certificate()
        .clone();

    let served = world.app.call(PkiCaGet, &[]).send_bytes().await;

    assert_eq!(served.status, 200, "no sign-in is needed");
    assert_eq!(served.headers["content-type"], "application/x-pem-file");
    let text = std::str::from_utf8(&served.raw).unwrap();
    assert_eq!(text, authority.pem());
    let read = Certificate::from_pem(text).unwrap();
    assert!(read.is_ca());
    assert_eq!(read.fingerprint(), authority.fingerprint());
}

#[tokio::test]
async fn the_openapi_document_is_for_any_signed_in_person_and_lists_every_request_of_the_contract()
{
    let world = World::new().await;
    let c = cast(&world).await;
    let token = api_token(&world, &c.ada).await;

    let by_session = world
        .app
        .call(OpenapiGet, &[])
        .token(&c.ada.token)
        .send_bytes()
        .await;
    let by_token = world
        .app
        .call(OpenapiGet, &[])
        .token(&token)
        .send_bytes()
        .await;
    let nobody = world.app.call(OpenapiGet, &[]).send_bytes().await;

    assert_eq!(by_session.status, 200);
    assert_eq!(by_token.status, 200, "a token may read it");
    assert_eq!(nobody.problem().code, ErrorCode::AuthRequired);
    let document: serde_json::Value = serde_json::from_slice(&by_session.raw).unwrap();
    assert_eq!(document["openapi"], json!("3.1.0"));
    assert_eq!(document["info"]["version"], json!("0.0.0-test"));
    let operations: usize = document["paths"]
        .as_object()
        .unwrap()
        .values()
        .map(|path| path.as_object().unwrap().len())
        .sum();
    assert_eq!(operations, orch_api_contract::routes::all().len());
    assert_eq!(
        document["paths"]["/admin/users"]["get"]["x-orch-access"],
        json!("session")
    );
    assert!(document["components"]["schemas"]["AdminUser"].is_object());
    assert_eq!(
        by_session.raw, by_token.raw,
        "the same document for everybody"
    );
}

async fn a_host(world: &World, owner: Option<Id>, home: Option<Id>, name: &str) -> Id {
    let mut conn = world.conn().await;
    fixtures::insert_resource(&mut conn, "host", owner, home, name)
        .await
        .unwrap()
}

#[tokio::test]
async fn every_host_of_the_system_is_listed_to_a_system_admin_those_of_orgs_and_the_systems_own() {
    let world = World::new().await;
    let c = cast(&world).await;
    let _theirs = a_host(&world, Some(c.boss.id()), Some(c.acme), "Build box").await;
    let _system = a_host(&world, None, None, "Edge").await;

    let hosts: Vec<Host> = world
        .app
        .call(AdminHostList, &[])
        .token(&c.root.token)
        .send()
        .await
        .ok();

    let names: Vec<_> = hosts.iter().map(|host| host.base.name.as_str()).collect();
    assert_eq!(
        names,
        ["Build box", "Edge"],
        "in the order they were enrolled"
    );
    let theirs = &hosts[0];
    assert_eq!(theirs.owner.as_ref().unwrap().username, "boss");
    assert_eq!(theirs.org_id, Some(c.acme));
    assert!(
        theirs.access.administers,
        "the system's admin administers it"
    );
    let edge = &hosts[1];
    assert!(edge.owner.is_none() && edge.home.is_none());
    assert!(edge.access.administers);
}

/// Puts a unit of each pool on two hosts, one of them gone.
struct Units {
    first: Id,
    second: Id,
    environment: Id,
    provider: Id,
}

async fn units(world: &World, c: &Cast) -> Units {
    let first = a_host(world, None, None, "First").await;
    let second = a_host(world, None, None, "Second").await;
    let (template, environment, provider) = {
        let mut conn = world.conn().await;
        let template = fixtures::insert_resource(
            &mut conn,
            "template",
            Some(c.boss.id()),
            Some(c.acme),
            "Ubuntu",
        )
        .await
        .unwrap();
        let environment = fixtures::insert_resource(
            &mut conn,
            "environment",
            Some(c.boss.id()),
            Some(c.acme),
            "Work",
        )
        .await
        .unwrap();
        let provider = fixtures::insert_resource(
            &mut conn,
            "provider",
            Some(c.boss.id()),
            Some(c.acme),
            "Sub",
        )
        .await
        .unwrap();
        (template, environment, provider)
    };
    for (kind, host, name, state, owner_column, owner) in [
        (
            "environment",
            first,
            "unit-env",
            "running",
            "environment_id",
            environment,
        ),
        (
            "provider",
            second,
            "unit-provider",
            "stopped",
            "provider_id",
            provider,
        ),
        (
            "environment",
            second,
            "unit-gone",
            "removed",
            "environment_id",
            environment,
        ),
    ] {
        sqlx::query(sqlx::AssertSqlSafe(format!(
            "INSERT INTO runtimes (id, kind, host_id, unit_name, {owner_column}, driver, template_id, spec_hash, state, generation)
             VALUES ($1, $2, $3, $4, $5, 'docker', $6, '\\x00'::bytea, $7, 2)"
        )))
        .bind(orch_types::new_id())
        .bind(kind)
        .bind(host)
        .bind(name)
        .bind(owner)
        .bind(template)
        .bind(state)
        .execute(&world.db.pool)
        .await
        .unwrap();
    }
    Units {
        first,
        second,
        environment,
        provider,
    }
}

async fn runtimes(world: &World, by: &Who, query: &AdminRuntimeListQuery) -> Vec<Runtime> {
    world
        .app
        .call(AdminRuntimeList, &[])
        .token(&by.token)
        .query(query)
        .send()
        .await
        .ok()
}

#[tokio::test]
async fn the_runtimes_of_both_pools_are_listed_whoever_s_they_are_and_those_removed_are_not() {
    let world = World::new().await;
    let c = cast(&world).await;
    let on = units(&world, &c).await;

    let all = runtimes(&world, &c.root, &AdminRuntimeListQuery::default()).await;
    let here = runtimes(
        &world,
        &c.root,
        &AdminRuntimeListQuery {
            host_id: Some(on.first),
            kind: None,
        },
    )
    .await;
    let providers = runtimes(
        &world,
        &c.root,
        &AdminRuntimeListQuery {
            host_id: None,
            kind: Some(UnitKind::Provider),
        },
    )
    .await;

    let units_named: Vec<_> = all.iter().map(|each| each.unit_name.as_str()).collect();
    assert_eq!(
        units_named,
        ["unit-env", "unit-provider"],
        "the removed one is only a note for the node"
    );
    let environment = &all[0];
    assert_eq!(environment.kind, UnitKind::Environment);
    assert_eq!(environment.template.name, "Ubuntu");
    assert_eq!(environment.environment.as_ref().unwrap().id, on.environment);
    assert_eq!(environment.environment.as_ref().unwrap().name, "Work");
    assert!(environment.provider.is_none());
    // The unit was made from another template than the one its environment is on now, so one update
    // waits for it: the list says what each unit is behind in, whoever's it is.
    assert_eq!(
        (
            environment.state.as_str(),
            environment.generation,
            environment.updates
        ),
        ("running", 2, 1)
    );
    let provider = &all[1];
    assert_eq!(provider.provider.as_ref().unwrap().id, on.provider);
    assert_eq!(provider.host_id, on.second);
    assert_eq!(here.len(), 1);
    assert_eq!(here[0].unit_name, "unit-env");
    assert_eq!(providers.len(), 1);
    assert_eq!(providers[0].unit_name, "unit-provider");
}

/// Writes a minute of a measurement.
async fn minute(
    world: &World,
    at: DateTime<Utc>,
    name: &str,
    labels: serde_json::Value,
    value: f64,
) {
    sqlx::query("INSERT INTO metric_samples (at, name, labels, value) VALUES ($1, $2, $3, $4)")
        .bind(at)
        .bind(name)
        .bind(labels)
        .bind(value)
        .execute(&world.db.pool)
        .await
        .unwrap();
}

async fn series(world: &World, by: &Who, query: &str) -> Vec<MetricSeries> {
    world
        .app
        .call(AdminMetricList, &[])
        .token(&by.token)
        .raw_query(query)
        .send()
        .await
        .ok()
}

fn span(from: DateTime<Utc>, to: DateTime<Utc>) -> String {
    format!("from={}&to={}", Time::new(from), Time::new(to))
}

#[tokio::test]
async fn a_span_inside_the_days_minutes_are_kept_is_answered_from_the_minutes_with_a_mean_for_a_gauge_and_a_sum_for_a_counter()
 {
    let world = World::new().await;
    let c = cast(&world).await;
    {
        let mut conn = world.conn().await;
        migrations::ensure_partitions(&mut conn, world.now())
            .await
            .unwrap();
    }
    let hour = world.now() - Duration::hours(1);
    for index in 0..60_i64 {
        let at = hour + Duration::minutes(index);
        #[expect(clippy::cast_precision_loss, reason = "a small test figure")]
        let figure = index as f64;
        minute(
            &world,
            at,
            "outbox_pending",
            json!({ "channel": "email" }),
            figure,
        )
        .await;
        minute(
            &world,
            at,
            "route_messages_total",
            json!({ "channel": "email", "form": "single", "result": "sent" }),
            1.0,
        )
        .await;
    }
    let window = span(hour, world.now());

    let by_minute = series(
        &world,
        &c.root,
        &format!("name=outbox_pending&name=route_messages_total&{window}"),
    )
    .await;
    let by_hour = series(
        &world,
        &c.root,
        &format!("name=outbox_pending&name=route_messages_total&step=60&{window}"),
    )
    .await;
    let by_five = series(
        &world,
        &c.root,
        &format!("name=outbox_pending&step=5&{window}"),
    )
    .await;

    assert_eq!(by_minute.len(), 2);
    let gauge = by_minute
        .iter()
        .find(|each| each.name == "outbox_pending")
        .unwrap();
    assert_eq!(gauge.resolution, MetricResolution::Minute);
    assert_eq!(gauge.labels["channel"], "email");
    assert_eq!(
        gauge.points.len(),
        60,
        "one point for each minute, the finest step that fits"
    );
    assert!(gauge.peaks.is_none(), "peaks are for the hours");
    assert_eq!(gauge.points[7].1, 7.0);
    let gauge = by_hour
        .iter()
        .find(|each| each.name == "outbox_pending")
        .unwrap();
    let counter = by_hour
        .iter()
        .find(|each| each.name == "route_messages_total")
        .unwrap();
    assert_eq!(gauge.points.len(), 1);
    assert_eq!(
        gauge.points[0].1, 29.5,
        "a gauge is the mean of its minutes"
    );
    assert_eq!(
        counter.points[0].1, 60.0,
        "a counter is the sum of what it grew by"
    );
    assert_eq!(by_five[0].points.len(), 12);
    assert_eq!(by_five[0].points[0].1, 2.0, "the mean of minutes 0 to 4");
}

#[tokio::test]
async fn without_a_name_the_answer_lists_the_names_there_are_and_an_unknown_name_has_no_series() {
    let world = World::new().await;
    let c = cast(&world).await;
    {
        let mut conn = world.conn().await;
        migrations::ensure_partitions(&mut conn, world.now())
            .await
            .unwrap();
    }
    let at = world.now() - Duration::minutes(5);
    minute(
        &world,
        at,
        "outbox_pending",
        json!({ "channel": "email" }),
        1.0,
    )
    .await;
    minute(
        &world,
        at,
        "outbox_pending",
        json!({ "channel": "push" }),
        2.0,
    )
    .await;
    minute(&world, at, "threads", json!({ "state": "idle" }), 3.0).await;
    let window = span(world.now() - Duration::hours(1), world.now());

    let names = series(&world, &c.root, &window).await;
    let unknown = series(&world, &c.root, &format!("name=nothing_there&{window}")).await;

    let listed: Vec<_> = names.iter().map(|each| each.name.as_str()).collect();
    assert_eq!(listed, ["outbox_pending", "threads"], "each name once");
    assert!(names.iter().all(|each| each.points.is_empty()));
    assert!(unknown.is_empty());
}

#[tokio::test]
async fn an_older_span_is_answered_from_the_hours_with_their_peaks_and_a_finer_step_is_refused() {
    let world = World::new().await;
    let c = cast(&world).await;
    {
        let mut conn = world.conn().await;
        migrations::ensure_partitions(&mut conn, world.now())
            .await
            .unwrap();
    }
    let day: DateTime<Utc> = "2026-08-01T00:00:00Z".parse().unwrap();
    for hour in 0..24_i64 {
        #[expect(clippy::cast_precision_loss, reason = "a small test figure")]
        let figure = hour as f64;
        sqlx::query("INSERT INTO metric_hours (at, name, labels, mean, peak, total, samples) VALUES ($1, $2, $3, $4, $5, $6, 60)")
            .bind(day + Duration::hours(hour))
            .bind("outbox_pending")
            .bind(json!({ "channel": "email" }))
            .bind(figure)
            .bind(figure + 10.0)
            .bind(figure * 60.0)
            .execute(&world.db.pool)
            .await
            .unwrap();
        sqlx::query("INSERT INTO metric_hours (at, name, labels, mean, peak, total, samples) VALUES ($1, 'route_messages_total', '{}', 1, 1, 60, 60)")
            .bind(day + Duration::hours(hour))
            .execute(&world.db.pool)
            .await
            .unwrap();
    }
    let window = span(day, day + Duration::hours(24));

    let hourly = series(&world, &c.root, &format!("name=outbox_pending&{window}")).await;
    let daily = series(
        &world,
        &c.root,
        &format!("name=outbox_pending&name=route_messages_total&step=1440&{window}"),
    )
    .await;
    let finer = world
        .app
        .call(AdminMetricList, &[])
        .token(&c.root.token)
        .raw_query(&format!("name=outbox_pending&step=5&{window}"))
        .send()
        .await
        .problem();

    assert_eq!(hourly[0].resolution, MetricResolution::Hour);
    assert_eq!(hourly[0].points.len(), 24);
    assert_eq!(hourly[0].peaks.as_ref().unwrap().len(), 24);
    assert_eq!(hourly[0].peaks.as_ref().unwrap()[3].1, 13.0);
    let gauge = daily
        .iter()
        .find(|each| each.name == "outbox_pending")
        .unwrap();
    let counter = daily
        .iter()
        .find(|each| each.name == "route_messages_total")
        .unwrap();
    assert_eq!(gauge.points.len(), 1);
    assert_eq!(gauge.points[0].1, 11.5, "the mean of the day's hours");
    assert_eq!(
        gauge.peaks.as_ref().unwrap()[0].1,
        33.0,
        "the largest minute of the day"
    );
    assert_eq!(
        counter.points[0].1, 1440.0,
        "what a counter grew by in a day"
    );
    assert_eq!(finer.code, ErrorCode::RequestInvalid);
    assert_eq!(finer.extra["errors"][0]["path"], json!("step"));
}

#[tokio::test]
async fn a_span_that_would_give_more_than_720_points_or_runs_backwards_is_refused() {
    let world = World::new().await;
    let c = cast(&world).await;
    let now = world.now();
    let ask = |query: String| {
        let world = &world;
        let root = &c.root;
        async move {
            world
                .app
                .call(AdminMetricList, &[])
                .token(&root.token)
                .raw_query(&query)
                .send()
                .await
                .problem()
        }
    };

    let too_fine = ask(format!(
        "name=x&step=1&{}",
        span(now - Duration::days(2), now)
    ))
    .await;
    let backwards = ask(format!("name=x&{}", span(now, now - Duration::hours(1)))).await;
    let odd = ask(format!(
        "name=x&step=7&{}",
        span(now - Duration::hours(1), now)
    ))
    .await;

    assert_eq!(too_fine.extra["errors"][0]["path"], json!("step"));
    assert_eq!(backwards.extra["errors"][0]["path"], json!("to"));
    assert_eq!(odd.extra["errors"][0]["path"], json!("step"));
}

#[tokio::test]
async fn the_plugins_the_configuration_names_and_those_machines_report_are_listed() {
    let mut config = ServerConfig::default();
    config.plugin.push(Plugin {
        kind: "driver".into(),
        name: "firecracker".into(),
        command: vec!["/opt/orch-plugins/firecracker".into()],
        ..Plugin::default()
    });
    let world = World::with(config).await;
    let c = cast(&world).await;
    let (laptop, host) = {
        let mut conn = world.conn().await;
        let laptop = insert_machine(&mut conn, "laptop", "connected")
            .await
            .unwrap();
        let host = fixtures::insert_resource(&mut conn, "host", None, None, "Edge")
            .await
            .unwrap();
        (laptop, host)
    };
    sql(
        &world,
        &format!(
            "UPDATE machines SET facts = '{{\"plugins\":[{{\"kind\":\"adapter\",\"name\":\"grok\",\"version\":\"0.2.0\",\"state\":\"running\"}}]}}'::jsonb WHERE id = '{laptop}'"
        ),
    )
    .await;
    sql(
        &world,
        &format!(
            "UPDATE machines SET facts = '{{\"plugins\":[{{\"kind\":\"driver\",\"name\":\"kata\",\"version\":\"1.0.0\",\"state\":\"failed\"}}]}}'::jsonb WHERE id = (SELECT machine_id FROM hosts WHERE id = '{host}')"
        ),
    )
    .await;

    let listed: Vec<AdminPlugin> = world
        .app
        .call(AdminPluginList, &[])
        .token(&c.root.token)
        .send()
        .await
        .ok();

    assert_eq!(listed.len(), 3);
    let configured = listed
        .iter()
        .find(|each| each.name == "firecracker")
        .unwrap();
    assert_eq!(configured.component, "api");
    assert!(configured.machine.is_none());
    assert_eq!(configured.state, PluginState::Restarting);
    assert!(configured.detail.contains("configuration"));
    let on_a_laptop = listed.iter().find(|each| each.name == "grok").unwrap();
    assert_eq!(on_a_laptop.component, "connector");
    assert_eq!(on_a_laptop.machine.as_ref().unwrap().name, "laptop");
    assert_eq!(
        (on_a_laptop.state, on_a_laptop.version.as_str()),
        (PluginState::Running, "0.2.0")
    );
    let on_a_host = listed.iter().find(|each| each.name == "kata").unwrap();
    assert_eq!(on_a_host.component, "node");
    assert_eq!(on_a_host.state, PluginState::Failed);
}

#[tokio::test]
async fn somebody_who_is_no_system_admin_is_refused_how_the_system_stands_and_so_is_an_orgs_admin()
{
    let world = World::new().await;
    let c: Cast = cast(&world).await;
    let window = span(world.now() - Duration::hours(1), world.now());

    for (who, name) in [(&c.boss, "an org's admin"), (&c.ada, "an ordinary person")] {
        let problems = [
            refused_to(&world, AdminSystemGet, &[], who, None, "").await,
            refused_to(&world, AdminStorageGet, &[], who, None, "").await,
            refused_to(&world, AdminPkiGet, &[], who, None, "").await,
            refused_to(&world, AdminHostList, &[], who, None, "").await,
            refused_to(&world, AdminRuntimeList, &[], who, None, "").await,
            refused_to(&world, AdminMetricList, &[], who, None, &window).await,
            refused_to(&world, AdminPluginList, &[], who, None, "").await,
        ];
        for problem in &problems {
            assert_no_system_admin(problem, name);
        }
    }
}
