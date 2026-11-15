//! Group directory reads keep their existing visibility, current payload scope and live counts.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "test assertions fail by panicking"
)]

mod places;
mod resources;
mod support;

use crate::support::FullPermissionPageReply;
use http::StatusCode;
use orch_access_contract::key_scope::{KeyPlaceExclusion, KeyScope};
use orch_access_contract::roles::PlaceRef;
use orch_api_contract::access::{HolderInput, PageChange, PageChangeWhole, PermissionPageUpdate};
use orch_api_contract::directory::{
    Group, GroupHolderAdd, GroupMemberAdd, GroupPage, NodeGroupList, OrgGroupCreate,
    OrgGroupCreateBody, OrgGroupList, OrgGroupListQuery,
};
use orch_api_core::middleware::authentication::{API_TOKEN_PREFIX, new_token, token_hash};
use orch_api_core::testing::RawReply;
use orch_authz::{Caller, Principal};
use orch_db::queries::{access, identity};
use orch_types::{Id, Json, Time, new_id};
use places::Who;
use serde_json::json;
use support::World;

#[derive(Clone, Copy, Debug)]
enum Reader {
    Org,
    Node,
}
const READERS: [Reader; 2] = [Reader::Org, Reader::Node];
impl Reader {
    fn operation(self) -> &'static str {
        match self {
            Self::Org => "OrgGroupList",
            Self::Node => "NodeGroupList",
        }
    }
    async fn read(self, world: &World, secret: &str, id: Id, under: bool) -> RawReply {
        match self {
            Self::Org => {
                world
                    .app
                    .call(OrgGroupList, &[&id])
                    .token(secret)
                    .query(&OrgGroupListQuery {
                        under: Some(under),
                        ..OrgGroupListQuery::default()
                    })
                    .send_bytes()
                    .await
            }
            Self::Node => {
                world
                    .app
                    .call(NodeGroupList, &[&id])
                    .token(secret)
                    .send_bytes()
                    .await
            }
        }
    }
}

struct Key {
    id: Id,
    secret: String,
}
fn rule(
    kind: &str,
    id: Id,
    reach: &str,
    resource: &str,
    operation: &str,
    actions: &[&str],
) -> Json {
    json!({"target":{"kind":kind,"id":id,"reach":reach},"resources":[resource],
        "allow":[{"operation":operation,"actions":actions}]})
}
fn group_rule(group: Id, reader: Reader) -> Json {
    rule(
        "group",
        group,
        "self",
        "group",
        reader.operation(),
        &["read"],
    )
}
async fn key(world: &World, actor: Id, rules: Vec<Json>, excluded: Option<Id>) -> Key {
    let mut policy: KeyScope = serde_json::from_value(json!({"rules":rules})).unwrap();
    if let Some(id) = excluded {
        policy.excluded_places.push(KeyPlaceExclusion::Refused {
            place: PlaceRef::Node(id),
        });
    }
    assert!(policy.validate().is_empty());
    let secret = new_token(API_TOKEN_PREFIX);
    let id = new_id();
    identity::create_api_token_scoped(
        &mut *world.conn().await,
        &identity::NewApiToken {
            id,
            user_id: actor,
            name: "Group directory scope control",
            token_hash: &token_hash(&secret),
            scopes: &[],
            node_id: None,
            at: world.now(),
            expires_at: Some(world.now() + chrono::Duration::hours(1)),
        },
        Some(&serde_json::to_value(policy).unwrap()),
    )
    .await
    .unwrap();
    Key { id, secret }
}
async fn legacy_key(world: &World, actor: Id) -> String {
    let secret = new_token(API_TOKEN_PREFIX);
    identity::create_api_token(
        &mut *world.conn().await,
        &identity::NewApiToken {
            id: new_id(),
            user_id: actor,
            name: "Group legacy full-account control",
            token_hash: &token_hash(&secret),
            scopes: &["*".to_owned()],
            node_id: None,
            at: world.now(),
            expires_at: Some(world.now() + chrono::Duration::hours(1)),
        },
    )
    .await
    .unwrap();
    secret
}
fn value(reply: &RawReply) -> Json {
    serde_json::from_slice(&reply.raw).unwrap()
}
fn success(reply: RawReply) -> Json {
    assert_eq!(
        reply.status,
        StatusCode::OK,
        "{}",
        String::from_utf8_lossy(&reply.raw)
    );
    let page: GroupPage = serde_json::from_slice(&reply.raw).unwrap();
    assert!(
        page.next_cursor.is_none(),
        "this fixture fits one native page"
    );
    serde_json::to_value(page.items).unwrap()
}
async fn no_dispatch(world: &World) {
    let count: i64 =
        sqlx::query_scalar("SELECT (SELECT count(*) FROM jobs)+(SELECT count(*) FROM commands)")
            .fetch_one(&world.db.pool)
            .await
            .unwrap();
    assert_eq!(
        count, 0,
        "directory reads do not start native or deferred work"
    );
}
async fn group(world: &World, owner: &Who, org: Id, name: &str) -> Group {
    world
        .app
        .call(OrgGroupCreate, &[&org])
        .token(&owner.token)
        .body(&OrgGroupCreateBody {
            name: name.into(),
            description: Some("Acme is literal group text".into()),
        })
        .send()
        .await
        .ok()
        .into_full()
        .expect("this ordinary control requires the full native creation response")
}
async fn member(world: &World, owner: &Who, group: Id, person: Id) {
    world
        .app
        .call(GroupMemberAdd, &[&group, &person])
        .token(&owner.token)
        .send()
        .await
        .ok();
}
async fn nested(world: &World, owner: &Who, group: Id, child: Id) {
    world
        .app
        .call(GroupHolderAdd, &[&group])
        .token(&owner.token)
        .body(&orch_api_contract::directory::GroupHolderAddBody {
            holder: HolderInput::Group { id: child },
            expected_revision: None,
        })
        .send()
        .await
        .ok();
}

struct Scene {
    company: resources::Company,
    project: Id,
    department: Id,
    department_project: Id,
    selected: Group,
    department_group: Group,
}
impl Scene {
    async fn new(world: &World) -> Self {
        let company = resources::company(world).await;
        let project = places::make_project(world, &company.boss, company.acme, "Web")
            .await
            .base
            .id;
        let department =
            places::make_department(world, &company.boss, company.acme, "Research", "research")
                .await
                .base
                .id;
        let department_project = places::make_project(world, &company.boss, department, "Lab")
            .await
            .base
            .id;
        // The name intentionally equals the keeping-org label. It is literal Group
        // content and must not be scrubbed when the separate place label is hidden.
        let selected = group(world, &company.boss, company.acme, "Acme").await;
        group(
            world,
            &company.boss,
            company.acme,
            "Unselected roster sentinel",
        )
        .await;
        let inner = group(world, &company.boss, company.acme, "Nested roster sentinel").await;
        let duplicate = group(
            world,
            &company.boss,
            company.acme,
            "Duplicate roster sentinel",
        )
        .await;
        member(world, &company.boss, inner.id, company.viewer.id()).await;
        member(world, &company.boss, inner.id, company.admin.id()).await;
        member(world, &company.boss, duplicate.id, company.viewer.id()).await;
        member(world, &company.boss, selected.id, company.viewer.id()).await;
        nested(world, &company.boss, selected.id, inner.id).await;
        nested(world, &company.boss, selected.id, duplicate.id).await;
        let department_group = group(
            world,
            &company.boss,
            department,
            "Department roster sentinel",
        )
        .await;
        Self {
            company,
            project,
            department,
            department_project,
            selected,
            department_group,
        }
    }
    fn place(&self, reader: Reader) -> Id {
        match reader {
            Reader::Org => self.company.acme,
            Reader::Node => self.project,
        }
    }
}

async fn exact_payload(reader: Reader) {
    let world = World::new().await;
    let s = Scene::new(&world).await;
    let ordinary = success(
        reader
            .read(&world, &s.company.viewer.token, s.place(reader), false)
            .await,
    );
    let legacy = legacy_key(&world, s.company.viewer.id()).await;
    assert_eq!(
        success(reader.read(&world, &legacy, s.place(reader), false).await),
        ordinary,
        "legacy full-account keys keep the ordinary browser projection"
    );
    let expected = ordinary
        .as_array()
        .unwrap()
        .iter()
        .find(|row| row["id"] == json!(s.selected.id))
        .unwrap();
    assert_eq!(
        expected["member_count"], 2,
        "nested and direct membership counts are DISTINCT current people"
    );
    let limited = key(
        &world,
        s.company.viewer.id(),
        vec![group_rule(s.selected.id, reader)],
        None,
    )
    .await;
    let rows = success(
        reader
            .read(&world, &limited.secret, s.place(reader), false)
            .await,
    );
    assert_eq!(rows.as_array().unwrap().len(), 1);
    assert_eq!(rows[0]["id"], json!(s.selected.id));
    assert_eq!(rows[0]["name"], "Acme");
    assert_eq!(rows[0]["description"], "Acme is literal group text");
    assert_eq!(rows[0]["member_count"], 2);
    assert!(
        rows[0]["place"].is_null(),
        "keeper qualification needs its own operation Read"
    );
    assert!(
        !rows.to_string().contains("roster sentinel"),
        "unread Group payloads are omitted as whole rows"
    );
    let mut without_place = expected.clone();
    without_place.as_object_mut().unwrap().remove("place");
    assert_eq!(
        rows[0], without_place,
        "the rest of the ordinary Group payload is preserved"
    );
    member(&world, &s.company.boss, s.selected.id, s.company.boss.id()).await;
    let fresh = success(
        reader
            .read(&world, &limited.secret, s.place(reader), false)
            .await,
    );
    assert_eq!(
        fresh[0]["member_count"], 3,
        "a later read uses the live holder graph"
    );
    no_dispatch(&world).await;
}

#[tokio::test]
async fn org_group_read_preserves_the_selected_payload_and_live_distinct_members() {
    exact_payload(Reader::Org).await;
}
#[tokio::test]
async fn node_group_read_preserves_the_selected_payload_and_live_distinct_members() {
    exact_payload(Reader::Node).await;
}

#[tokio::test]
async fn optional_keeper_qualification_needs_this_operations_node_read_without_scrubbing_literals()
{
    let world = World::new().await;
    let s = Scene::new(&world).await;
    for reader in READERS {
        let unrelated = key(
            &world,
            s.company.viewer.id(),
            vec![
                group_rule(s.selected.id, reader),
                rule(
                    "org",
                    s.company.acme,
                    "self",
                    "org",
                    "NodeGet.read",
                    &["read"],
                ),
            ],
            None,
        )
        .await;
        let hidden = success(
            reader
                .read(&world, &unrelated.secret, s.place(reader), false)
                .await,
        );
        assert_eq!(hidden[0]["name"], "Acme");
        assert_eq!(hidden[0]["member_count"], 2);
        assert!(hidden[0]["place"].is_null());
        let complete = key(
            &world,
            s.company.viewer.id(),
            vec![
                group_rule(s.selected.id, reader),
                rule(
                    "org",
                    s.company.acme,
                    "self",
                    "org",
                    reader.operation(),
                    &["read"],
                ),
            ],
            None,
        )
        .await;
        let shown = success(
            reader
                .read(&world, &complete.secret, s.place(reader), false)
                .await,
        );
        let ordinary = success(
            reader
                .read(&world, &s.company.viewer.token, s.place(reader), false)
                .await,
        );
        let expected = ordinary
            .as_array()
            .unwrap()
            .iter()
            .find(|row| row["id"] == json!(s.selected.id))
            .unwrap();
        assert_eq!(&shown[0], expected);
        assert_eq!(shown[0]["place"], "Acme");
    }
    no_dispatch(&world).await;
}

#[tokio::test]
async fn under_and_nearest_keeper_selection_remain_the_original_directory_rules() {
    let world = World::new().await;
    let s = Scene::new(&world).await;
    let org = key(
        &world,
        s.company.viewer.id(),
        vec![
            group_rule(s.selected.id, Reader::Org),
            group_rule(s.department_group.id, Reader::Org),
        ],
        None,
    )
    .await;
    let direct = success(
        Reader::Org
            .read(&world, &org.secret, s.company.acme, false)
            .await,
    );
    assert_eq!(direct.as_array().unwrap().len(), 1);
    assert_eq!(direct[0]["id"], json!(s.selected.id));
    let under = success(
        Reader::Org
            .read(&world, &org.secret, s.company.acme, true)
            .await,
    );
    let ids: Vec<Json> = under
        .as_array()
        .unwrap()
        .iter()
        .map(|row| row["id"].clone())
        .collect();
    assert_eq!(
        ids,
        vec![json!(s.selected.id), json!(s.department_group.id)],
        "existing name/id order is retained after scope filtering"
    );
    let excluded = key(
        &world,
        s.company.viewer.id(),
        vec![
            group_rule(s.selected.id, Reader::Org),
            group_rule(s.department_group.id, Reader::Org),
        ],
        Some(s.department),
    )
    .await;
    let cut = success(
        Reader::Org
            .read(&world, &excluded.secret, s.company.acme, true)
            .await,
    );
    assert_eq!(cut.as_array().unwrap().len(), 1);
    assert_eq!(cut[0]["id"], json!(s.selected.id));
    assert!(!cut.to_string().contains("Department roster sentinel"));
    let node = key(
        &world,
        s.company.viewer.id(),
        vec![
            group_rule(s.selected.id, Reader::Node),
            group_rule(s.department_group.id, Reader::Node),
        ],
        None,
    )
    .await;
    let nearest = success(
        Reader::Node
            .read(&world, &node.secret, s.department_project, false)
            .await,
    );
    assert_eq!(nearest.as_array().unwrap().len(), 1);
    assert_eq!(nearest[0]["id"], json!(s.department_group.id));
    no_dispatch(&world).await;
}

#[tokio::test]
async fn private_keeper_visibility_is_not_donated_by_a_full_key_scope() {
    let world = World::new().await;
    let s = Scene::new(&world).await;
    world
        .app
        .call(PermissionPageUpdate, &[&"nodes", &s.department])
        .token(&s.company.boss.token)
        .body(&PageChange {
            whole: Some(PageChangeWhole::Private),
            ..Default::default()
        })
        .send()
        .await
        .full_permission_page()
        .ok();
    access::testing::add_entry(
        &mut *world.conn().await,
        &access::PageKey::new("node", s.department_project, "live"),
        "any",
        "project.view",
        ("user", Some(s.company.viewer.id()), None),
        None,
        world.now(),
    )
    .await
    .unwrap();
    let mut conn = world.conn().await;
    let principal = Principal::load(
        &mut conn,
        Caller::session(s.company.viewer.id()),
        Time::new(world.now()),
    )
    .await
    .unwrap();
    assert!(
        orch_authz::nodes::check_named(
            &mut conn,
            &principal,
            s.department_project,
            orch_api_core::middleware::authorization::VIEW,
            orch_authz::Act::Use,
            orch_authz::Policy::default()
        )
        .await
        .unwrap()
        .is_allowed()
    );
    assert!(
        !orch_authz::nodes::check_named(
            &mut conn,
            &principal,
            s.department,
            orch_api_core::middleware::authorization::VIEW,
            orch_authz::Act::Use,
            orch_authz::Policy::default()
        )
        .await
        .unwrap()
        .is_allowed()
    );
    drop(conn);
    assert_eq!(
        success(
            Reader::Node
                .read(&world, &s.company.viewer.token, s.department_project, false)
                .await
        ),
        json!([])
    );
    let token = key(
        &world,
        s.company.viewer.id(),
        vec![
            group_rule(s.department_group.id, Reader::Node),
            rule(
                "org",
                s.department,
                "subtree",
                "org",
                Reader::Node.operation(),
                &["read"],
            ),
        ],
        None,
    )
    .await;
    assert_eq!(
        success(
            Reader::Node
                .read(&world, &token.secret, s.department_project, false)
                .await
        ),
        json!([])
    );
    no_dispatch(&world).await;
}

#[tokio::test]
async fn checked_empty_and_filtered_empty_still_require_the_exact_group_read_operation() {
    let world = World::new().await;
    let s = Scene::new(&world).await;
    let empty = places::make_department(&world, &s.company.boss, s.company.acme, "Empty", "empty")
        .await
        .base
        .id;
    let empty_project = places::make_project(&world, &s.company.boss, empty, "Blank")
        .await
        .base
        .id;
    for reader in READERS {
        let target = match reader {
            Reader::Org => empty,
            Reader::Node => empty_project,
        };
        let allowed = key(
            &world,
            s.company.viewer.id(),
            vec![group_rule(s.selected.id, reader)],
            None,
        )
        .await;
        assert_eq!(
            success(reader.read(&world, &allowed.secret, target, false).await),
            json!([])
        );
        let filtered = key(
            &world,
            s.company.viewer.id(),
            vec![group_rule(s.department_group.id, reader)],
            None,
        )
        .await;
        assert_eq!(
            success(
                reader
                    .read(&world, &filtered.secret, s.place(reader), false)
                    .await
            ),
            json!([])
        );
        for bad in [
            rule(
                "group",
                s.selected.id,
                "self",
                "group",
                reader.operation(),
                &["manage"],
            ),
            rule(
                "group",
                s.selected.id,
                "self",
                "group",
                "GroupMemberList",
                &["read"],
            ),
            rule(
                "org",
                s.company.acme,
                "subtree",
                "org",
                reader.operation(),
                &["read"],
            ),
        ] {
            let denied = key(&world, s.company.viewer.id(), vec![bad], None).await;
            let reply = reader.read(&world, &denied.secret, target, false).await;
            assert_eq!(
                reply.status,
                StatusCode::FORBIDDEN,
                "{}",
                String::from_utf8_lossy(&reply.raw)
            );
        }
    }
    no_dispatch(&world).await;
}

#[tokio::test]
async fn current_key_narrowing_revocation_and_real_ordinary_view_loss_do_not_reuse_old_rows() {
    let world = World::new().await;
    let s = Scene::new(&world).await;
    let view = access::testing::add_entry(
        &mut *world.conn().await,
        &access::PageKey::new("node", s.company.acme, "live"),
        "any",
        "org.view",
        ("user", Some(s.company.stranger.id()), None),
        None,
        world.now(),
    )
    .await
    .unwrap();
    // This fixture exercises a project request too. Its existing Project View
    // prerequisite is independent of the keeping org's Org View.
    access::testing::add_entry(
        &mut *world.conn().await,
        &access::PageKey::new("node", s.project, "live"),
        "any",
        "project.view",
        ("user", Some(s.company.stranger.id()), None),
        None,
        world.now(),
    )
    .await
    .unwrap();
    for reader in READERS {
        let ordinary = success(
            reader
                .read(&world, &s.company.stranger.token, s.place(reader), false)
                .await,
        );
        assert!(
            ordinary
                .as_array()
                .unwrap()
                .iter()
                .any(|row| row["id"] == json!(s.selected.id))
        );
        let token = key(
            &world,
            s.company.stranger.id(),
            vec![group_rule(s.selected.id, reader)],
            None,
        )
        .await;
        let initial = success(
            reader
                .read(&world, &token.secret, s.place(reader), false)
                .await,
        );
        assert_eq!(initial[0]["id"], json!(s.selected.id));
        let narrowed: KeyScope =
            serde_json::from_value(json!({"rules":[group_rule(s.department_group.id,reader)]}))
                .unwrap();
        sqlx::query("UPDATE api_tokens SET operation_scope=$2 WHERE id=$1")
            .bind(token.id)
            .bind(serde_json::to_value(narrowed).unwrap())
            .execute(&world.db.pool)
            .await
            .unwrap();
        assert_eq!(
            success(
                reader
                    .read(&world, &token.secret, s.place(reader), false)
                    .await
            ),
            json!([])
        );
        assert!(
            identity::revoke_api_token(
                &mut *world.conn().await,
                token.id,
                s.company.stranger.id(),
                world.now()
            )
            .await
            .unwrap()
        );
        assert_eq!(
            reader
                .read(&world, &token.secret, s.place(reader), false)
                .await
                .status,
            StatusCode::UNAUTHORIZED
        );
    }
    let token = key(
        &world,
        s.company.stranger.id(),
        vec![group_rule(s.selected.id, Reader::Org)],
        None,
    )
    .await;
    assert_eq!(
        success(
            Reader::Org
                .read(&world, &token.secret, s.company.acme, false)
                .await
        )[0]["id"],
        json!(s.selected.id)
    );
    assert!(
        access::remove_entry(
            &mut *world.conn().await,
            view,
            s.company.boss.id(),
            world.now()
        )
        .await
        .unwrap()
    );
    orch_authz::epoch::bump(&mut *world.conn().await)
        .await
        .unwrap();
    assert_eq!(
        Reader::Org
            .read(&world, &token.secret, s.company.acme, false)
            .await
            .status,
        StatusCode::FORBIDDEN
    );
    no_dispatch(&world).await;
}

#[tokio::test]
async fn a_group_after_a_thousand_is_found_without_scanning_browser_pages() {
    let world = World::new().await;
    let c = resources::company(&world).await;
    sqlx::query("INSERT INTO groups(id,org_id,name,description)
        SELECT gen_random_uuid(),$1,'Group '||lpad(n::text,4,'0'),'A directory group' FROM generate_series(0,1000) n")
        .bind(c.acme).execute(&world.db.pool).await.unwrap();
    let found = world
        .app
        .call(OrgGroupList, &[&c.acme])
        .token(&c.boss.token)
        .query(&OrgGroupListQuery {
            q: Some("Group 1000".into()),
            limit: Some(37),
            ..Default::default()
        })
        .send()
        .await
        .ok();
    assert_eq!(found.items.len(), 1);
    assert_eq!(found.items[0].name, "Group 1000");
    let mut cursor = None;
    let mut seen = std::collections::HashSet::new();
    loop {
        let page = world
            .app
            .call(OrgGroupList, &[&c.acme])
            .token(&c.boss.token)
            .query(&OrgGroupListQuery {
                limit: Some(137),
                cursor,
                ..Default::default()
            })
            .send()
            .await
            .ok();
        for row in page.items {
            assert!(seen.insert(row.id));
        }
        cursor = page.next_cursor;
        if cursor.is_none() {
            break;
        }
    }
    assert_eq!(seen.len(), 1001);
}
