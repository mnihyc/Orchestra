//! A list under the Resources group decides its resources from their own sites, as the whole inventory decides them.
use super::page_budgets::fixture;
use orch_api_contract::everything::EverythingList;
use orch_authz::{Caller, Principal};
use orch_types::{Id, Time};
use std::{collections::BTreeMap, fmt::Debug};

/// Each kind of resource a list can show, with the kinds its access is decided with.
const LISTS: [(&str, &[&str]); 10] = [
    ("provider", &["provider"]),
    ("agent", &["agent"]),
    ("environment", &["environment"]),
    ("machine", &["machine", "host"]),
    ("host", &["machine", "host"]),
    ("template", &["template"]),
    ("mcp_server", &["mcp_server"]),
    ("mount", &["mount"]),
    ("skill", &["skill"]),
    ("pool", &["pool"]),
];

fn keyed<T: Debug>(items: impl IntoIterator<Item = (Id, T)>) -> BTreeMap<Id, String> {
    items
        .into_iter()
        .map(|(id, item)| (id, format!("{item:?}")))
        .collect()
}

#[tokio::test]
async fn a_resource_list_decides_each_resource_and_its_places_as_the_whole_inventory_does() {
    let (world, reader, [org, ..]) = fixture(1, true).await;
    let mut conn = world.conn().await;
    // A tag on one provider, defined at an org: its place's name follows that org's own decision.
    let tag = orch_types::new_id();
    sqlx::query("INSERT INTO tags(id,node_id,name,color) VALUES($1,$2,'Shared',3)")
        .bind(tag)
        .bind(org)
        .execute(&mut *conn)
        .await
        .unwrap();
    sqlx::query(
        "INSERT INTO resource_tags(resource_kind,resource_id,tag_id) SELECT 'provider',id,$1 FROM providers ORDER BY id LIMIT 1",
    )
    .bind(tag)
    .execute(&mut *conn)
    .await
    .unwrap();
    let owner: Id = sqlx::query_scalar("SELECT id FROM users WHERE username='page-owner'")
        .fetch_one(&mut *conn)
        .await
        .unwrap();
    for user in [reader.id(), owner] {
        let principal = Principal::load(&mut conn, Caller::session(user), Time::new(world.now()))
            .await
            .unwrap();
        let whole = orch_authz::everything::project(&mut conn, &principal, None)
            .await
            .unwrap();
        for (kind, decided) in LISTS {
            let decided: Vec<String> = decided.iter().map(|each| (*each).to_owned()).collect();
            let narrow = orch_authz::everything::project_resources(&mut conn, &principal, &decided)
                .await
                .unwrap();
            let seen = |projection: &orch_authz::everything::Projection| {
                keyed(
                    projection
                        .resources
                        .iter()
                        .filter(|each| {
                            decided
                                .iter()
                                .any(|kind| kind == each.resource.kind.as_str())
                        })
                        .map(|each| (each.resource.id, each)),
                )
            };
            assert_eq!(seen(&narrow), seen(&whole), "{kind} standings for {user}");
            let seeds = orch_db::queries::everything::resource_access_seeds(&mut conn, &decided)
                .await
                .unwrap();
            for seed in seeds.iter().filter(|seed| seed.kind != "node") {
                let id = seed.id.to_string();
                assert_eq!(
                    narrow.grants["objects"].get(&id),
                    whole.grants["objects"].get(&id),
                    "{kind} {id} decision for {user}"
                );
            }
            for (id, step) in &narrow.names {
                assert_eq!(
                    Some(format!("{step:?}")),
                    whole.names.get(id).map(|step| format!("{step:?}")),
                    "{kind} place {id} for {user}"
                );
            }
        }
    }
    drop(conn);
    // The reader's rows under the Resources group are the rows the whole inventory lists for the same kind.
    for (kind, _) in LISTS {
        let read = |query: String| {
            let world = &world;
            let token = &reader.token;
            async move {
                let reply = world
                    .app
                    .call(EverythingList, &[])
                    .token(token)
                    .raw_query(&query)
                    .send_bytes()
                    .await;
                assert_eq!(reply.status.as_u16(), 200, "{query}");
                let page: serde_json::Value = serde_json::from_slice(&reply.raw).unwrap();
                page["items"].clone()
            }
        };
        let listed = read(format!(
            "scope=mine&parent=resources&kinds={kind}&sort=name&direction=asc&limit=200"
        ))
        .await;
        let everywhere = read(format!(
            "scope=mine&kinds={kind}&sort=name&direction=asc&limit=200"
        ))
        .await;
        assert_eq!(listed, everywhere, "{kind} rows");
    }
}
