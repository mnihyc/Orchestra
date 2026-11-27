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
    // The reader's rows under the Resources group are exactly the resources of that kind the whole inventory lets the
    // reader see or find. (A machine row is decided by its host, so its row and its decision differ in identity.)
    let principal = Principal::load(
        &mut conn,
        Caller::session(reader.id()),
        Time::new(world.now()),
    )
    .await
    .unwrap();
    let whole = orch_authz::everything::project(&mut conn, &principal, None)
        .await
        .unwrap();
    let mut expected = BTreeMap::new();
    for (kind, _) in LISTS.iter().filter(|(kind, _)| *kind != "machine") {
        let seeds =
            orch_db::queries::everything::resource_access_seeds(&mut conn, &[(*kind).to_owned()])
                .await
                .unwrap();
        let mut ids: Vec<String> = seeds
            .iter()
            .filter(|seed| seed.kind == *kind)
            .map(|seed| seed.id.to_string())
            .filter(|id| {
                let grant = &whole.grants["objects"][id];
                grant["view"] == serde_json::json!(true)
                    || grant["listed"] == serde_json::json!(true)
            })
            .collect();
        ids.sort();
        expected.insert(*kind, ids);
    }
    drop(conn);
    for (kind, ids) in expected {
        let reply = world
            .app
            .call(EverythingList, &[])
            .token(&reader.token)
            .raw_query(&format!(
                "scope=mine&parent=resources&kinds={kind}&sort=name&direction=asc&limit=200"
            ))
            .send_bytes()
            .await;
        assert_eq!(reply.status.as_u16(), 200, "{kind}");
        let page: serde_json::Value = serde_json::from_slice(&reply.raw).unwrap();
        let mut listed: Vec<String> = page["items"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|row| row["kind"] == serde_json::json!(kind))
            .map(|row| row["id"].as_str().unwrap().to_owned())
            .collect();
        listed.sort();
        assert_eq!(listed, ids, "{kind} rows");
    }
}

#[tokio::test]
async fn provider_and_machine_rows_show_the_same_bindings_and_usage_as_their_resource_facts() {
    use orch_access_contract::{decisions::ObjectRef, permissions::ObjectKind};
    use orch_api_contract::resource_facts::{DependentRelation, FactSubject};
    use orch_engine::resource_facts::{self, library};

    let (world, reader, _) = fixture(1, true).await;
    let mut conn = world.conn().await;
    let at = Time::new(world.now());
    let principal = Principal::load(&mut conn, Caller::session(reader.id()), at)
        .await
        .unwrap();
    let (provider, machine): (Id, Id) = sqlx::query_as(
        "SELECT p.id,p.machine_id FROM providers p WHERE EXISTS (SELECT 1 FROM threads t WHERE t.provider_id=p.id) AND p.machine_id IS NOT NULL ORDER BY p.id LIMIT 1"
    ).fetch_one(&mut *conn).await.unwrap();
    let subjects = [
        FactSubject::Resource {
            object: ObjectRef {
                kind: ObjectKind::Provider,
                id: provider,
            },
        },
        FactSubject::Resource {
            object: ObjectRef {
                kind: ObjectKind::Machine,
                id: machine,
            },
        },
    ];
    let mut full =
        resource_facts::read_with(&mut conn, &subjects, at, Some(&library::RecordedUses))
            .await
            .unwrap();
    full[0]
        .dependents
        .retain(|edge| edge.relation == DependentRelation::Binding);
    assert!(!full[0].dependents.is_empty());
    assert!(full[0].uses.is_some());
    full[1].dependents.clear();
    let listed = library::list_rows(&mut conn, &subjects, at).await.unwrap();
    let mut expected = resource_facts::project(&mut conn, &principal, Default::default(), &full)
        .await
        .unwrap();
    let mut actual = resource_facts::project(&mut conn, &principal, Default::default(), &listed)
        .await
        .unwrap();
    for row in expected.iter_mut().chain(&mut actual) {
        for group in &mut row.dependents {
            group
                .visible
                .sort_by_key(|edge| edge.object.map(|object| object.id));
        }
    }
    assert_eq!(actual, expected);
}
