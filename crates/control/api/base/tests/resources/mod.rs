//! Helpers for the tests of resources over HTTP: naming people on a resource's page, and the shapes tests of one kind of resource share.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a test fails by panicking"
)]
#![allow(dead_code, reason = "each test file uses some of what is here")]

use crate::places::{Who, join, make_org, person};
use crate::support::World;
use orch_db::queries::access::PageKey;
use orch_db::queries::access::testing::add_entry;
use orch_types::Id;

/// An org of `boss`, with an admin of the org, a member who is only a viewer, and a stranger.
pub struct Company {
    pub boss: Who,
    pub admin: Who,
    pub viewer: Who,
    pub stranger: Who,
    pub acme: Id,
}

/// Makes the company: `Acme`, its people, and one who is none of them.
pub async fn company(world: &World) -> Company {
    let boss = person(world, "boss").await;
    let admin = person(world, "admin").await;
    let viewer = person(world, "viewer").await;
    let stranger = person(world, "stranger").await;
    let acme = make_org(world, &boss, "Acme", "acme").await.base.id;
    join(world, acme, acme, &admin, "admin", &boss).await;
    join(world, acme, acme, &viewer, "viewer", &boss).await;
    Company {
        boss,
        admin,
        viewer,
        stranger,
        acme,
    }
}

/// Names a holder in a row of the page of a resource, as the page would have it written: `kind`
/// is the kind of resource in the words of its page (`provider`, `agent`, ...), the holder is
/// `(kind, id, role)` as an entry keeps it.
pub async fn name_on_page(
    world: &World,
    kind: &str,
    id: Id,
    permission: &str,
    holder: (&str, Option<Id>, Option<&str>),
) -> Id {
    let mut conn = world.conn().await;
    add_entry(
        &mut conn,
        &PageKey::new(kind, id, "live"),
        "any",
        permission,
        holder,
        None,
        world.now(),
    )
    .await
    .unwrap()
}

/// Names a person in a row of the page of a resource.
pub async fn name_person(world: &World, kind: &str, id: Id, permission: &str, person: Id) -> Id {
    name_on_page(world, kind, id, permission, ("user", Some(person), None)).await
}

/// Plays the machine: when it is sent a command it answers with `result`. The commands wait in
/// the database; the answer is written in their place.
pub fn answer(world: &World, machine: Id, result: orch_types::Json) -> tokio::task::JoinHandle<()> {
    let pool = world.db.pool.clone();
    tokio::spawn(async move {
        for _ in 0..400 {
            let answered = sqlx::query(
                "UPDATE commands SET state = 'succeeded', result = $2
                  WHERE machine_id = $1 AND state IN ('queued', 'sent', 'accepted')",
            )
            .bind(machine)
            .bind(&result)
            .execute(&pool)
            .await
            .unwrap()
            .rows_affected();
            if answered > 0 {
                return;
            }
            tokio::time::sleep(std::time::Duration::from_millis(25)).await;
        }
    })
}

/// Plays the machine that keeps a provider when it is asked what the provider lists: an API provider's
/// role is started there before it is asked (engine `discover_provider_models`), so that start is
/// acknowledged and the models source read alone gets `result`, within ten seconds.
pub fn answer_discovery(
    world: &World,
    machine: Id,
    result: orch_types::Json,
) -> tokio::task::JoinHandle<()> {
    let pool = world.db.pool.clone();
    tokio::spawn(async move {
        tokio::time::timeout(std::time::Duration::from_secs(10), async {
            loop {
                sqlx::query(
                    "UPDATE commands SET state = 'succeeded', result = '{}'
                      WHERE machine_id = $1 AND kind = 'provider.start'
                        AND state IN ('queued', 'sent', 'accepted')",
                )
                .bind(machine)
                .execute(&pool)
                .await
                .unwrap();
                let answered = sqlx::query(
                    "UPDATE commands SET state = 'succeeded', result = $2
                      WHERE machine_id = $1 AND kind = 'provider.discover_models'
                        AND state IN ('queued', 'sent', 'accepted')",
                )
                .bind(machine)
                .bind(&result)
                .execute(&pool)
                .await
                .unwrap()
                .rows_affected();
                if answered > 0 {
                    return;
                }
                tokio::time::sleep(std::time::Duration::from_millis(25)).await;
            }
        })
        .await
        .expect("the provider did not ask its model source within ten seconds");
    })
}

/// Says that the machine has a session.
pub async fn online(world: &World, machine: Id) {
    let mut conn = world.conn().await;
    orch_db::queries::resources::set_presence(&mut conn, machine, true, None, world.now())
        .await
        .unwrap();
}

/// A machine of `owner`'s whose local policy lets it forward model calls to `upstreams`.
pub async fn machine_forwarding_to(world: &World, owner: Id, upstreams: &[&str]) -> Id {
    let mut conn = world.conn().await;
    let machine = orch_db::queries::resources::access_fixtures::insert_persons_machine(
        &mut conn, owner, "laptop",
    )
    .await
    .unwrap();
    let policy = orch_link_contract::policy::LocalPolicy {
        provider_upstreams: upstreams.iter().map(|url| (*url).to_owned()).collect(),
        ..orch_link_contract::policy::LocalPolicy::default()
    };
    // A machine that keeps a key and forwards model calls plays the provider role, and declares the
    // directory runtime service its providers run in, as a connector with provider homes does.
    let descriptor = orch_driver_contract::descriptor::DriverDescriptor::registered_directories(
        "test",
        orch_driver_contract::descriptor::EnvironmentDataStorage {
            made_by_orch: Some(orch_driver_contract::descriptor::MadeDataKind::Directory),
            existing_directory: true,
            reason: None,
        },
    );
    sqlx::query(
        "UPDATE machines SET roles = ARRAY['provider'],
                facts = jsonb_build_object('adapters', '[]'::jsonb, 'policy', $2::jsonb,
                                           'provider_runtime_descriptor', $3::jsonb) WHERE id = $1",
    )
    .bind(machine)
    .bind(serde_json::to_value(&policy).unwrap())
    .bind(serde_json::to_value(&descriptor).unwrap())
    .execute(&mut *conn)
    .await
    .unwrap();
    machine
}
