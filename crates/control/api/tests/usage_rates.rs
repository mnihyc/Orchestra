//! Rates over HTTP: what each model usable at a node is counted at and where that rate comes from, the shipped catalogue of estimates for anyone signed in, and its replacement by a system admin from a file, from an address or from the release, checked whole and audited.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a test fails by panicking"
)]

mod places;
mod resources;
mod support;
mod usage_world;

use axum::Router;
use axum::http::StatusCode;
use axum::response::IntoResponse;
use axum::routing::get;
use orch_api_contract::common::ErrorCode;
use orch_api_contract::resources::{
    ModelParameters, ProviderModelCreate, ProviderModelCreateBody, Rate,
};
use orch_api_contract::usage::{
    AdminUsageCatalogueSet, AdminUsageCatalogueSetBody, CatalogueFetch, CatalogueOrigin,
    CatalogueShipped, RateCatalogue, RateCatalogueFile, RateFamily, RateSource, RateTable,
    UsageCatalogueGet, UsageCatalogueGetQuery, UsageRateTableGet, UsageRateTableGetQuery,
};
use orch_api_core::testing::Reply;
use orch_config::ServerConfig;
use orch_db::seeds;
use places::{Who, missing_permissions};
use serde_json::{Value, json};
use std::sync::{Arc, Mutex};
use support::World;
use usage_world::{Team, Use, spend, team};

/// A catalogue of figures as of `as_of`, with the families `catalogue-model*` at `input` and `other-*`.
fn catalogue(as_of: &str, input: &str) -> Value {
    json!({
        "format": 1, "as_of": as_of, "unit": "USD",
        "families": [
            {
                "match": "catalogue-model*", "vendor": "Example", "source": "https://example.test/pricing", "as_of": as_of,
                "rate": { "input_per_mtok": input, "cached_input_per_mtok": "0.4", "output_per_mtok": "20" }
            },
            {
                "match": "other-*", "vendor": "Example", "source": "https://example.test/other", "as_of": as_of,
                "rate": { "input_per_mtok": "1", "cached_input_per_mtok": "0.1", "output_per_mtok": "5" }
            }
        ]
    })
}

/// Each fault of a refusal, as the member it places and what it says. A refusal's detail is its
/// code's sentence; what is wrong, and with which entry, is in its faults.
fn faults(problem: &orch_api_contract::common::Problem) -> Vec<(String, String)> {
    problem
        .extra
        .get("errors")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .map(|fault| {
            let text = |member: &str| fault[member].as_str().unwrap_or_default().to_owned();
            (text("path"), text("message"))
        })
        .collect()
}

async fn shipped_catalogue(world: &World, doc: &Value) {
    let mut conn = world.conn().await;
    seeds::load_shipped_catalogue(&mut conn, &doc.to_string(), world.now())
        .await
        .unwrap();
}

fn rate(input: &str) -> Rate {
    Rate {
        input_per_mtok: input.to_owned(),
        cached_input_per_mtok: "0.1".to_owned(),
        output_per_mtok: "10".to_owned(),
        cache_write_per_mtok: None,
        tiers: None,
        above: None,
    }
}

async fn add_model(world: &World, t: &Team, name: &str, rate: Option<Rate>) {
    world
        .app
        .call(ProviderModelCreate, &[&t.provider])
        .token(&t.boss.token)
        .body(&ProviderModelCreateBody {
            model_name: name.to_owned(),
            display_name: None,
            parameters: rate.map(|rate| ModelParameters {
                rate: Some(rate),
                ..ModelParameters::default()
            }),
            groups: None,
        })
        .send()
        .await
        .ok();
}

async fn table(world: &World, by: &Who, node: orch_types::Id) -> RateTable {
    world
        .app
        .call(UsageRateTableGet, &[])
        .token(&by.token)
        .query(&UsageRateTableGetQuery {
            node_id: Some(node),
            ..UsageRateTableGetQuery::default()
        })
        .send()
        .await
        .ok()
}

#[tokio::test]
async fn the_rate_table_says_where_the_rate_of_each_model_comes_from_and_which_have_none() {
    let world = World::new().await;
    let t = team(&world).await;
    shipped_catalogue(&world, &catalogue("2026-10-10", "4")).await;
    world
        .set(
            "usage.rates",
            json!({ "card-model*": { "input_per_mtok": "3", "cached_input_per_mtok": "0.3", "output_per_mtok": "9" } }),
        )
        .await;
    add_model(&world, &t, "own-model", Some(rate("2"))).await;
    add_model(&world, &t, "card-model-1", None).await;
    add_model(&world, &t, "catalogue-model-1", None).await;
    add_model(&world, &t, "mystery-model", None).await;
    // The mystery model was used, and tokens without a rate are counted apart.
    spend(
        &world,
        &t,
        &Use::of(&world, t.admin.id(), 700).model("mystery-model"),
    )
    .await;
    spend(
        &world,
        &t,
        &Use::of(&world, t.admin.id(), 300).model("mystery-model"),
    )
    .await;

    let answer = table(&world, &t.admin, t.project).await;

    let by_name = |name: &str| {
        answer
            .models
            .iter()
            .find(|model| model.model_name == name)
            .unwrap_or_else(|| panic!("no {name}"))
    };
    assert_eq!(
        by_name("own-model").source,
        Some(RateSource::Provider),
        "the provider's own entry"
    );
    assert_eq!(
        by_name("own-model").rate.as_ref().unwrap().input_per_mtok,
        "2"
    );
    assert_eq!(by_name("card-model-1").source, Some(RateSource::RateCard));
    assert_eq!(
        by_name("card-model-1").pattern.as_deref(),
        Some("card-model*")
    );
    assert_eq!(
        by_name("catalogue-model-1").source,
        Some(RateSource::Catalogue)
    );
    assert_eq!(
        by_name("catalogue-model-1").pattern.as_deref(),
        Some("catalogue-model*")
    );
    assert_eq!(
        by_name("catalogue-model-1")
            .rate
            .as_ref()
            .unwrap()
            .input_per_mtok,
        "4"
    );
    let none = by_name("mystery-model");
    assert_eq!(
        (none.source, none.rate.clone(), none.pattern.clone()),
        (None, None, None)
    );
    assert_eq!(
        none.unrated_tokens, 1000,
        "what was used of it without a rate, counted"
    );
    assert_eq!(by_name("own-model").unrated_tokens, 0);
    assert!(answer.catalogue.on);
    assert_eq!(answer.catalogue.scale, "1");
    assert_eq!(answer.catalogue.as_of, "2026-10-10");
}

#[tokio::test]
async fn the_rate_table_is_for_whoever_holds_usage_view_on_the_node() {
    let world = World::new().await;
    let t = team(&world).await;
    for by in [&t.viewer, &t.stranger] {
        let refused = world
            .app
            .call(UsageRateTableGet, &[])
            .token(&by.token)
            .query(&UsageRateTableGetQuery {
                node_id: Some(t.project),
                ..UsageRateTableGetQuery::default()
            })
            .send()
            .await
            .problem();
        assert_eq!(refused.code, ErrorCode::AuthzDenied);
        assert_eq!(missing_permissions(&refused), ["usage.view"]);
    }
    let missing = world
        .app
        .call(UsageRateTableGet, &[])
        .token(&t.admin.token)
        .query(&UsageRateTableGetQuery {
            node_id: Some(orch_types::new_id()),
            ..UsageRateTableGetQuery::default()
        })
        .send()
        .await
        .problem();
    assert_eq!(missing.code, ErrorCode::NotFound);
}

async fn read_catalogue(world: &World, by: &Who, q: Option<&str>) -> RateCatalogue {
    world
        .app
        .call(UsageCatalogueGet, &[])
        .token(&by.token)
        .query(&UsageCatalogueGetQuery {
            q: q.map(str::to_owned),
        })
        .send()
        .await
        .ok()
        .expect("a catalogue was loaded for this read")
}

#[tokio::test]
async fn no_loaded_catalogue_is_a_successful_null_read_for_every_signed_in_viewer() {
    let world = World::new().await;
    let t = team(&world).await;
    // A development installation can have no catalogue. A read must not manufacture estimates
    // from a file that is not in force or turn this ordinary empty state into a 404.
    for by in [&t.boss, &t.viewer, &t.stranger] {
        for q in [None, Some("catalogue-model-1")] {
            let answer = world
                .app
                .call(UsageCatalogueGet, &[])
                .token(&by.token)
                .query(&UsageCatalogueGetQuery {
                    q: q.map(str::to_owned),
                })
                .send()
                .await;
            assert_eq!(answer.status, StatusCode::OK);
            assert_eq!(answer.raw.as_ref(), b"null");
            assert!(answer.ok().is_none());
        }
    }
    let nobody = world
        .app
        .call(UsageCatalogueGet, &[])
        .query(&UsageCatalogueGetQuery::default())
        .send()
        .await
        .problem();
    assert_eq!(nobody.code, ErrorCode::AuthRequired);
}

#[tokio::test]
async fn the_catalogue_is_read_by_anyone_signed_in_and_with_a_model_only_the_family_that_matches_it()
 {
    let world = World::new().await;
    let t = team(&world).await;
    shipped_catalogue(&world, &catalogue("2026-10-10", "4")).await;

    let all = read_catalogue(&world, &t.stranger, None).await;
    assert_eq!(all.as_of, "2026-10-10");
    assert_eq!(all.unit, "USD");
    assert_eq!(all.origin, CatalogueOrigin::Shipped);
    assert_eq!(all.loaded_by, None, "a start loaded it");
    assert_eq!(all.families.len(), 2);

    let one = read_catalogue(&world, &t.viewer, Some("Vendor/Catalogue-Model-9")).await;
    assert_eq!(
        one.families
            .iter()
            .map(|family| family.pattern.as_str())
            .collect::<Vec<_>>(),
        ["catalogue-model*"],
        "the name is read as the rates read it: lower case, from its last /"
    );
    assert!(
        read_catalogue(&world, &t.viewer, Some("no-such-model"))
            .await
            .families
            .is_empty()
    );

    let nobody = world
        .app
        .call(UsageCatalogueGet, &[])
        .query(&UsageCatalogueGetQuery::default())
        .send()
        .await
        .problem();
    assert_eq!(nobody.code, ErrorCode::AuthRequired);
}

fn family(pattern: &str, input: &str) -> RateFamily {
    RateFamily {
        pattern: pattern.to_owned(),
        vendor: "Example".to_owned(),
        source: "https://example.test/pricing".to_owned(),
        as_of: "2026-11-01".to_owned(),
        note: None,
        rate: rate(input),
    }
}

async fn upload(world: &World, by: &Who, body: AdminUsageCatalogueSetBody) -> Reply<RateCatalogue> {
    world
        .app
        .call(AdminUsageCatalogueSet, &[])
        .token(&by.token)
        .body(&body)
        .send()
        .await
}

fn file(as_of: &str, families: Vec<RateFamily>) -> AdminUsageCatalogueSetBody {
    AdminUsageCatalogueSetBody::File(RateCatalogueFile {
        format: 1,
        as_of: as_of.to_owned(),
        unit: "USD".to_owned(),
        families,
    })
}

async fn in_force(world: &World) -> (String, String) {
    sqlx::query_as("SELECT origin, as_of::text FROM rate_catalogue")
        .fetch_one(&world.db.pool)
        .await
        .unwrap()
}

async fn trail(world: &World) -> Vec<(String, String)> {
    sqlx::query_as(
        "SELECT action, actor_kind FROM audit_log WHERE action = 'rate_catalogue.loaded' ORDER BY at, id",
    )
    .fetch_all(&world.db.pool)
    .await
    .unwrap()
}

#[tokio::test]
async fn a_system_admin_replaces_the_catalogue_by_a_file_that_is_in_force_at_once_and_audited() {
    let world = World::new().await;
    let t = team(&world).await;
    shipped_catalogue(&world, &catalogue("2026-10-10", "4")).await;

    let answer = upload(
        &world,
        &t.sysadmin,
        file("2026-01-01", vec![family("fresh-*", "7")]),
    )
    .await
    .ok();

    assert_eq!(answer.origin, CatalogueOrigin::Uploaded);
    assert_eq!(answer.as_of, "2026-01-01", "whatever its date");
    assert_eq!(answer.loaded_by.unwrap().username, "root");
    assert_eq!(answer.families.len(), 1);
    assert_eq!(
        in_force(&world).await,
        ("uploaded".to_owned(), "2026-01-01".to_owned())
    );
    let seen = read_catalogue(&world, &t.viewer, None).await;
    assert_eq!(
        seen.families[0].pattern, "fresh-*",
        "everybody reads the one in force"
    );
    assert_eq!(
        trail(&world).await,
        [
            ("rate_catalogue.loaded".to_owned(), "system".to_owned()),
            ("rate_catalogue.loaded".to_owned(), "user".to_owned()),
        ],
        "the start's line and the admin's"
    );
}

#[tokio::test]
async fn the_system_rate_form_needs_a_browser_system_admin_and_lists_no_private_provider_models() {
    let world = World::new().await;
    let t = team(&world).await;
    let query = UsageRateTableGetQuery {
        node_id: None,
        system: Some(true),
    };
    let rates = world
        .app
        .call(UsageRateTableGet, &[])
        .token(&t.sysadmin.token)
        .query(&query)
        .send()
        .await
        .ok();
    assert!(rates.models.is_empty());
    assert_eq!(rates.catalogue.scale, "1");
    let denied = world
        .app
        .call(UsageRateTableGet, &[])
        .token(&t.viewer.token)
        .query(&query)
        .send()
        .await
        .problem();
    assert_eq!(denied.code, ErrorCode::AuthzDenied);
    let both = UsageRateTableGetQuery {
        node_id: Some(t.project),
        system: Some(true),
    };
    assert_eq!(
        world
            .app
            .call(UsageRateTableGet, &[])
            .token(&t.sysadmin.token)
            .query(&both)
            .send()
            .await
            .problem()
            .code,
        ErrorCode::RequestInvalid
    );
}

#[tokio::test]
async fn a_catalogue_with_one_wrong_entry_is_refused_naming_it_and_the_one_in_force_stays() {
    let world = World::new().await;
    let t = team(&world).await;
    shipped_catalogue(&world, &catalogue("2026-10-10", "4")).await;
    let audited = trail(&world).await.len();

    let broken = upload(
        &world,
        &t.sysadmin,
        file(
            "2026-11-01",
            vec![family("fine-*", "2"), family("broken-*", "plenty")],
        ),
    )
    .await
    .problem();

    assert_eq!(broken.code, ErrorCode::RequestInvalid);
    let named = faults(&broken);
    assert!(
        named
            .iter()
            .any(|(path, message)| path == "families[1].rate.input_per_mtok"
                && message.contains("broken-*")),
        "the entry is named: {named:?}"
    );
    assert_eq!(
        in_force(&world).await,
        ("shipped".to_owned(), "2026-10-10".to_owned())
    );
    assert_eq!(
        trail(&world).await.len(),
        audited,
        "a refusal leaves no line"
    );
    let wrong_format = world
        .app
        .call(AdminUsageCatalogueSet, &[])
        .token(&t.sysadmin.token)
        .raw_body(&json!({ "format": 2, "as_of": "2026-11-01", "unit": "USD", "families": [] }))
        .send()
        .await
        .problem();
    assert_eq!(wrong_format.code, ErrorCode::RequestInvalid);
    let not_a_catalogue = world
        .app
        .call(AdminUsageCatalogueSet, &[])
        .token(&t.sysadmin.token)
        .raw_body(&json!({ "hello": "world" }))
        .send()
        .await
        .problem();
    assert_eq!(not_a_catalogue.code, ErrorCode::RequestInvalid);
}

#[tokio::test]
async fn nobody_but_a_system_admin_replaces_the_catalogue() {
    let world = World::new().await;
    let t = team(&world).await;
    shipped_catalogue(&world, &catalogue("2026-10-10", "4")).await;
    for by in [&t.boss, &t.admin, &t.viewer, &t.stranger] {
        let refused = upload(&world, by, file("2026-11-01", vec![family("fresh-*", "7")]))
            .await
            .problem();
        assert_eq!(
            refused.code,
            ErrorCode::AuthzDenied,
            "{}",
            by.person.username
        );
    }
    let nobody = world
        .app
        .call(AdminUsageCatalogueSet, &[])
        .body(&file("2026-11-01", vec![family("fresh-*", "7")]))
        .send()
        .await
        .problem();
    assert_eq!(nobody.code, ErrorCode::AuthRequired);
    assert_eq!(in_force(&world).await.0, "shipped", "nothing changed");
}

#[tokio::test]
async fn a_catalogue_larger_than_two_mebibytes_is_refused_before_it_is_read() {
    let world = World::new().await;
    let t = team(&world).await;
    let families: Vec<RateFamily> = (0..20_000)
        .map(|n| family(&format!("padding-model-with-a-long-name-{n}-*"), "1"))
        .collect();
    let huge = file("2026-11-01", families);
    assert!(serde_json::to_vec(&huge).unwrap().len() > 2 * 1024 * 1024);

    let refused = upload(&world, &t.sysadmin, huge).await.problem();

    assert_eq!(refused.code, ErrorCode::RequestTooLarge);
}

/// An address on this machine that answers with what it is told to.
struct Address {
    url: String,
    answer: Arc<Mutex<(StatusCode, String)>>,
}

impl Address {
    async fn start() -> Self {
        let answer = Arc::new(Mutex::new((StatusCode::NOT_FOUND, String::new())));
        let served = Arc::clone(&answer);
        let app = Router::new().route(
            "/rates.json",
            get(move || {
                let served = Arc::clone(&served);
                async move {
                    let (status, text) = served.lock().unwrap().clone();
                    (status, text).into_response()
                }
            }),
        );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}/rates.json", listener.local_addr().unwrap());
        tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        Self { url, answer }
    }

    fn says(&self, status: StatusCode, text: &str) {
        *self.answer.lock().unwrap() = (status, text.to_owned());
    }
}

async fn fetch(world: &World, by: &Who) -> Reply<RateCatalogue> {
    upload(
        world,
        by,
        AdminUsageCatalogueSetBody::Fetch(CatalogueFetch { fetch: true }),
    )
    .await
}

#[tokio::test]
async fn the_catalogue_is_fetched_from_the_address_an_admin_set_and_from_nowhere_else() {
    let world = World::new().await;
    let t = team(&world).await;
    shipped_catalogue(&world, &catalogue("2026-10-10", "4")).await;

    let nowhere = fetch(&world, &t.sysadmin).await.problem();
    assert_eq!(
        nowhere.code,
        ErrorCode::RequestInvalid,
        "nothing is fetched unless somebody set an address"
    );
    let named = faults(&nowhere);
    assert!(named.iter().any(|(path, _)| path == "fetch"), "{named:?}");

    let address = Address::start().await;
    sqlx::query(
        "INSERT INTO settings (node_id, key, value) VALUES (NULL, 'usage.catalogue_url', $1)",
    )
    .bind(json!(address.url))
    .execute(&world.db.pool)
    .await
    .unwrap();

    address.says(StatusCode::INTERNAL_SERVER_ERROR, "");
    let down = fetch(&world, &t.sysadmin).await.problem();
    assert_eq!(
        down.code,
        ErrorCode::RequestInvalid,
        "an address that does not answer gives no catalogue"
    );
    address.says(StatusCode::OK, &catalogue("2026-12-01", "9").to_string());

    let answer = fetch(&world, &t.sysadmin).await.ok();

    assert_eq!(answer.origin, CatalogueOrigin::Fetched);
    assert_eq!(answer.source.as_deref(), Some(address.url.as_str()));
    assert_eq!(answer.as_of, "2026-12-01");
    assert_eq!(answer.loaded_by.unwrap().username, "root");
    let refused = fetch(&world, &t.viewer).await.problem();
    assert_eq!(refused.code, ErrorCode::AuthzDenied);

    let mut broken = catalogue("2027-01-01", "9");
    broken["families"][1]["rate"]["output_per_mtok"] = json!("plenty");
    address.says(StatusCode::OK, &broken.to_string());
    let refused = fetch(&world, &t.sysadmin).await.problem();
    assert_eq!(refused.code, ErrorCode::RequestInvalid);
    let named = faults(&refused);
    assert!(
        named
            .iter()
            .any(|(path, message)| path == "families[1].rate.output_per_mtok"
                && message.contains("other-*")),
        "the entry is named: {named:?}"
    );
    assert_eq!(
        in_force(&world).await,
        ("fetched".to_owned(), "2026-12-01".to_owned())
    );
}

#[tokio::test]
async fn the_catalogue_of_the_release_is_put_back_where_the_release_keeps_it() {
    let dir = tempfile::tempdir().unwrap();
    let release = dir.path().join("release");
    std::fs::create_dir_all(release.join("share")).unwrap();
    std::fs::write(
        release.join("share/model-rates.json"),
        catalogue("2026-10-10", "1").to_string(),
    )
    .unwrap();
    let mut config = ServerConfig::default();
    config.database.migrations_dir = release.join("migrations").display().to_string();
    let world = World::with(config).await;
    let t = team(&world).await;
    upload(
        &world,
        &t.sysadmin,
        file("2026-12-31", vec![family("fresh-*", "9")]),
    )
    .await
    .ok();
    assert_eq!(in_force(&world).await.0, "uploaded");

    let answer = upload(
        &world,
        &t.sysadmin,
        AdminUsageCatalogueSetBody::Shipped(CatalogueShipped { shipped: true }),
    )
    .await
    .ok();

    assert_eq!(answer.origin, CatalogueOrigin::Shipped);
    assert_eq!(
        answer.as_of, "2026-10-10",
        "an admin's file and a newer date do not stand in its way"
    );
    assert_eq!(answer.shipped_as_of, "2026-10-10");
    let refused = upload(
        &world,
        &t.boss,
        AdminUsageCatalogueSetBody::Shipped(CatalogueShipped { shipped: true }),
    )
    .await
    .problem();
    assert_eq!(refused.code, ErrorCode::AuthzDenied);
}
