//! A native facts report retains independent sample presence through the stored public view.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    reason = "test assertions"
)]

use super::*;

use crate::support::World;

use orch_api_contract::resources::MachineGet;

use orch_connector_core::facts::FactsDraft;

use orch_connector_core::facts::Host;

use orch_connector_core::facts::machine_facts;

use orch_link::streams::Received;

use orch_link::streams::StreamReceiver;

use orch_link_contract::session::MachineFactsRates;

use orch_link_contract::streams::Ev;

use serde_json::json;
#[cfg(any(test, feature = "test-support"))]
async fn an_unmeasured_or_partial_link_stays_absent_through_report_and_machine_api() {
    let world = World::new().await;
    let owner = crate::places::person(&world, "rate-presence-owner").await;
    let id = resources::access_fixtures::insert_persons_machine(
        &mut *world.conn().await,
        owner.id(),
        "rate-presence-machine",
    )
    .await
    .unwrap();
    for (i, (rates, expected)) in [
        (MachineFactsRates::default(), json!({})),
        (
            MachineFactsRates {
                up_bps: Some(0),
                down_bps: None,
                round_trip_ms: Some(16),
            },
            json!({"up_bps":0,"round_trip_ms":16}),
        ),
    ]
    .into_iter()
    .enumerate()
    {
        let (processors, ram, disk) = [
            (6, 42_764_201_984_i64, 267_000_000_000_i64),
            (4, 41_696_751_616_i64, 300_000_000_000_i64),
        ][i];
        let mut facts = machine_facts(
            &Host {
                hostname: "rate-presence".into(),
                os_version: "fixture".into(),
                cpus: processors,
                memory_bytes: ram,
            },
            FactsDraft::default(),
            None,
            rates,
            0,
        );
        facts.data_volume_bytes = Some(disk);
        let body = serde_json::to_value(facts).unwrap();
        assert_eq!(body["rates"], expected);
        let mut receiver = StreamReceiver::new(BTreeMap::new(), vec![Role::Environment]);
        let delivery = match receiver.on_ev(Ev {
            native_opening_id: None,
            stream: "machine".into(),
            seq: 1,
            at: world.app.app.clock.time(),
            kind: "machine.facts".into(),
            body,
        }) {
            Received::Apply(delivery) => delivery,
            other => panic!("no facts delivery: {other:?}"),
        };
        let mut tx = world.db.pool.begin().await.unwrap();
        world
            .app
            .app
            .link
            .apply_machine_facts(&mut tx, id, &delivery, world.now())
            .await
            .unwrap();
        tx.commit().await.unwrap();
        let shown = world
            .app
            .call(MachineGet, &[&id])
            .token(&owner.token)
            .send()
            .await
            .ok();
        assert_eq!(shown.facts["rates"], expected);
        assert_eq!(shown.facts["cpus"], json!(processors));
        assert_eq!(shown.facts["memory_bytes"], json!(ram));
        assert_eq!(shown.facts["data_volume_bytes"], json!(disk));
    }
}
#[cfg(any(test, feature = "test-support"))]
#[doc(hidden)]
pub fn __api_run_an_unmeasured_or_partial_link_stays_absent_through_report_and_machine_api()
-> ::futures::future::LocalBoxFuture<'static, ()> {
    ::std::boxed::Box::pin(
        self::an_unmeasured_or_partial_link_stays_absent_through_report_and_machine_api(),
    )
}
