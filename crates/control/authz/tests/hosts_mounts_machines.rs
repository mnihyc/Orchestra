//! A host and a template are shared alike and give nothing inside an environment; a mount is used for a scope and its data is placed per machine; a machine's local policy comes first, and a key handed over is changed by whoever manages what it bounds.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a test fails by panicking"
)]

mod support;

use orch_access_contract::pages::Holder;
use orch_access_contract::permissions::{ObjectKind, Permission as P};
use orch_access_contract::roles::Role;
use orch_authz::resources::{self, KeyAbout, Keying, Locating, LocationAsk, LocationKind};
use orch_authz::usable::{Policies, Usable};
use orch_authz::{Caller, Policy};
use orch_db::queries::resources::access_fixtures;
use orch_types::Id;
use serde_json::json;
use support::World;

async fn locating(w: &World, who: Id, ask: &LocationAsk) -> Locating {
    let principal = w.principal(Caller::session(who)).await;
    let mut conn = w.conn().await;
    resources::may_add_location(&mut conn, &principal, ask, &Policies::default())
        .await
        .unwrap()
}

/// The machine a host runs its node on.
async fn machine_of_host(w: &World, host: Id) -> Id {
    let mut conn = w.conn().await;
    sqlx::query_scalar("SELECT machine_id FROM hosts WHERE id = $1")
        .bind(host)
        .fetch_one(&mut *conn)
        .await
        .unwrap()
}

#[tokio::test]
async fn a_mount_is_used_for_a_scope_by_whoever_last_wrote_the_setting_when_they_hold_its_use() {
    let w = World::new().await;
    let (mia, writer) = (w.member("mia").await, w.member("writer").await);
    let mount = w.thing("mount", mia, w.root, "data").await;
    // The topic is the node and whoever wrote the setting is the person.
    assert!(matches!(
        w.usable(Caller::session(writer), mount, Some(w.topic))
            .await,
        Usable::No
    ));
    w.grant("mount", mount, P::MountUse, Holder::User { id: writer })
        .await;
    assert!(matches!(
        w.usable(Caller::session(writer), mount, Some(w.topic))
            .await,
        Usable::Yes(_)
    ));
    assert!(
        matches!(
            w.usable(Caller::session(mia), mount, Some(w.topic)).await,
            Usable::Yes(_)
        ),
        "its owner"
    );
    // The use of the mount says nothing of managing it.
    assert!(!w.can_on(writer, "mount", mount, P::MountManage).await);
    // A disabled mount is used by nobody.
    {
        let mut conn = w.conn().await;
        access_fixtures::set_disabled(&mut conn, "mount", mount, true)
            .await
            .unwrap();
    }
    assert!(matches!(
        w.usable(Caller::session(mia), mount, Some(w.topic)).await,
        Usable::Unavailable(_)
    ));
}

#[tokio::test]
async fn mount_locations_use_the_same_machine_management_and_declared_data_support() {
    let w = World::new().await;
    let (mia, hal, vic, owen) = (
        w.member("mia").await,
        w.member("hal").await,
        w.member("vic").await,
        w.member("owen").await,
    );
    let mount = w.thing("mount", mia, w.root, "data").await;
    let host = w.thing("host", hal, w.root, "gpu").await;
    let host_machine = machine_of_host(&w, host).await;
    w.grant("mount", mount, P::MountUse, Holder::User { id: hal })
        .await;
    let path = |machine: Id| LocationAsk {
        mount,
        machine,
        kind: LocationKind::Path,
        storage: None,
    };
    let volume = |storage: Option<&str>| LocationAsk {
        mount,
        machine: host_machine,
        kind: LocationKind::Volume,
        storage: storage.map(str::to_owned),
    };
    // A path on a host exposes a directory of it: its manager says so, and the mount's manager
    // who does not manage the host is refused, and told who can.
    assert_eq!(
        locating(&w, hal, &path(host_machine)).await,
        Locating::Allowed
    );
    let Locating::Refused(refusal) = locating(&w, mia, &path(host_machine)).await else {
        panic!("the mount's manager does not manage the host");
    };
    assert_eq!(refusal.missing[0].object.kind, ObjectKind::Host);
    assert_eq!(refusal.missing[0].permissions, [P::HostManage]);
    assert!(refusal.missing[0].ask.contains(&hal));
    // The data kind does not grant machine management.
    assert!(
        matches!(locating(&w, mia, &volume(None)).await, Locating::Refused(_)),
        "no use of the host"
    );
    w.put(
        "host",
        host,
        P::HostUse,
        Holder::User { id: mia },
        json!({ "storages": ["fast"] }),
    )
    .await;
    {
        let mut conn = w.conn().await;
        sqlx::query("UPDATE hosts SET storages = $2 WHERE id = $1")
            .bind(host)
            .bind(json!([{ "name": "fast", "default": true }, { "name": "bulk" }]))
            .execute(&mut *conn)
            .await
            .unwrap();
        sqlx::query("UPDATE hosts SET drivers=$2 WHERE id=$1").bind(host)
            .bind(json!({"driver":{"data_storage":{"made_by_orch":"volume","existing_directory":true,"reason":null}}}))
            .execute(&mut *conn).await.unwrap();
    }
    assert!(
        matches!(locating(&w, mia, &volume(None)).await, Locating::Refused(_)),
        "host.use alone cannot add a location"
    );
    w.grant("host", host, P::HostManage, Holder::User { id: mia })
        .await;
    assert_eq!(
        locating(&w, mia, &volume(None)).await,
        Locating::Allowed,
        "the machine manager chooses its declared storage"
    );
    assert_eq!(
        locating(&w, mia, &volume(Some("fast"))).await,
        Locating::Allowed
    );
    assert_eq!(
        locating(&w, mia, &volume(Some("bulk"))).await,
        Locating::Allowed,
        "management is independent of a narrower Use entry"
    );
    assert_eq!(
        locating(&w, hal, &volume(None)).await,
        Locating::Allowed,
        "the machine manager holding mount.use may place either data kind"
    );
    // A path on a person's own machine is added by that machine's owner, holding the use of the mount.
    // These bare identities have no personal home: choose the real home explicitly and
    // start its canonical Owners row, as the machine registration workflow does.
    let laptop = w.thing("machine", owen, w.root, "laptop").await;
    assert!(
        matches!(
            locating(&w, owen, &path(laptop)).await,
            Locating::Refused(_)
        ),
        "owning the machine is not using the mount"
    );
    w.grant("mount", mount, P::MountUse, Holder::User { id: owen })
        .await;
    assert_eq!(locating(&w, owen, &path(laptop)).await, Locating::Allowed);
    for person in [vic, mia] {
        let Locating::Refused(refusal) = locating(&w, person, &path(laptop)).await else {
            panic!("neither an unrelated person nor the mount owner manages this machine");
        };
        assert_eq!(refusal.required, P::MachineManage);
        assert_eq!(refusal.object.kind, ObjectKind::Machine);
        assert_eq!(refusal.object.id, laptop);
        assert_eq!(refusal.missing[0].permissions, [P::MachineManage]);
        assert!(refusal.missing[0].ask.contains(&owen));
    }
    // Missing data-making facts, rather than a machine kind, refuse owned data.
    let on_laptop = LocationAsk {
        kind: LocationKind::Volume,
        machine: laptop,
        ..path(laptop)
    };
    assert_eq!(
        locating(&w, owen, &on_laptop).await,
        Locating::NoVolumeThere
    );
    sqlx::query("UPDATE machines SET facts=jsonb_build_object('environment_descriptor',$2::jsonb) WHERE id=$1").bind(laptop)
        .bind(json!({"name":"directory","data_storage":{"made_by_orch":"directory"}})).execute(&mut *w.conn().await).await.unwrap();
    assert_eq!(locating(&w, owen, &on_laptop).await, Locating::Allowed);
    assert_eq!(
        locating(&w, owen, &path(orch_types::new_id())).await,
        Locating::NoSuchMachine
    );
}

#[tokio::test]
async fn a_hosts_manager_stops_what_runs_on_it_and_is_nothing_inside_an_environment_made_there() {
    let w = World::new().await;
    let (hal, cal, admin) = (
        w.member("hal").await,
        w.member("cal").await,
        w.member("admin").await,
    );
    w.give(w.root, admin, Role::Admin).await;
    let host = w.thing("host", hal, w.root, "gpu").await;
    let template = w.thing("template", hal, w.root, "recipe").await;
    let environment = w.thing("environment", cal, w.topic, "work").await;
    // Managing the host is managing the units on it, whoever's environment they are; not what is in them.
    assert!(w.can_on(hal, "host", host, P::HostManage).await);
    assert!(
        !w.can_on(hal, "environment", environment, P::EnvironmentManage)
            .await
    );
    assert!(
        !w.can_on(hal, "environment", environment, P::EnvironmentView)
            .await
    );
    // A host and a template are shared alike: View, Use and Manage with the same meaning.
    let dee = w.member("dee").await;
    for (kind, id, use_row) in [
        ("host", host, P::HostUse),
        ("template", template, P::TemplateUse),
    ] {
        assert!(!w.can_on(dee, kind, id, use_row).await);
        w.grant(kind, id, use_row, Holder::User { id: dee }).await;
        assert!(w.can_on(dee, kind, id, use_row).await);
        let view = if kind == "host" {
            P::HostView
        } else {
            P::TemplateView
        };
        assert!(w.can_on(dee, kind, id, view).await, "use holds looking");
        assert!(
            !w.can_on(
                dee,
                kind,
                id,
                if kind == "host" {
                    P::HostManage
                } else {
                    P::TemplateManage
                }
            )
            .await
        );
    }
    // Off keeps the ordinary rights the Admin role's matching page rows already give.
    // A private page removes those rights; explicit On supplies native administrative reach.
    assert!(
        w.can_on(admin, "environment", environment, P::EnvironmentManage)
            .await
    );
    w.thing_row("environment", environment, P::EnvironmentManage, false)
        .await;
    assert!(
        !w.can_on(admin, "environment", environment, P::EnvironmentManage)
            .await
    );
    assert!(
        !w.decide_on(
            Caller::session(admin).with_admin_mode(false),
            "environment",
            environment,
            P::AccessManage
        )
        .await
        .is_allowed()
    );
    for permission in [P::AccessManage, P::EnvironmentManage] {
        assert!(
            w.decide_on(
                Caller::session(admin).with_admin_mode(true),
                "environment",
                environment,
                permission
            )
            .await
            .is_allowed(),
            "On administers the private environment: {permission}"
        );
    }
    w.grant(
        "environment",
        environment,
        P::EnvironmentManage,
        Holder::User { id: admin },
    )
    .await;
    assert!(
        w.can_on(admin, "environment", environment, P::EnvironmentManage)
            .await
    );
}

#[tokio::test]
async fn only_machine_managers_change_delegated_policy_even_when_the_machine_has_no_registrations()
{
    let w = World::new().await;
    let owner = w.member("owner").await;
    let manager = w.member("manager").await;
    let worker = w.member("worker").await;
    let machine = w.thing("machine", owner, w.root, "desk").await;
    assert!(
        w.can_on(owner, "machine", machine, P::MachineManage).await,
        "the policy fixture has a real canonical machine owner"
    );
    let environment = w.thing("environment", owner, w.topic, "work").await;
    {
        let mut conn = w.conn().await;
        access_fixtures::register_on(&mut conn, "environment", environment, machine)
            .await
            .unwrap();
    }
    w.grant(
        "environment",
        environment,
        P::EnvironmentManage,
        Holder::User { id: worker },
    )
    .await;
    let answer = |who, delegated| {
        let w = &w;
        async move {
            let principal = w.principal(Caller::session(who)).await;
            let mut conn = w.conn().await;
            resources::may_change_policy_key(
                &mut conn,
                &principal,
                machine,
                KeyAbout::Environments,
                delegated,
                Policy::default(),
            )
            .await
            .unwrap()
        }
    };
    assert!(matches!(answer(worker, true).await, Keying::Refused(_)));
    w.grant(
        "machine",
        machine,
        P::MachineManage,
        Holder::User { id: manager },
    )
    .await;
    assert_eq!(answer(manager, true).await, Keying::Allowed);
    assert_eq!(answer(manager, false).await, Keying::NotDelegated);
    assert_eq!(answer(owner, true).await, Keying::Allowed);
    let host = w.thing("host", owner, w.root, "server").await;
    let node = machine_of_host(&w, host).await;
    w.grant("host", host, P::HostManage, Holder::User { id: manager })
        .await;
    let principal = w.principal(Caller::session(manager)).await;
    let mut conn = w.conn().await;
    assert_eq!(
        resources::may_change_policy_key(
            &mut conn,
            &principal,
            node,
            KeyAbout::TheMachine,
            true,
            Policy::default()
        )
        .await
        .unwrap(),
        Keying::Allowed
    );
}

#[tokio::test]
async fn declared_data_makers_and_host_capabilities_follow_their_actual_machine_liveness() {
    let w = World::new().await;
    let owner = w.member("data-owner").await;
    let host = w.thing("host", owner, w.root, "data-host").await;
    let node = machine_of_host(&w, host).await;
    let directory = w.thing("machine", owner, w.root, "data-directory").await;
    let storages = json!([{"name":"fast","default":true}]);
    let drivers = json!({"native-volume":{"data_storage":{"made_by_orch":"volume"}}});
    let mut conn = w.conn().await;
    sqlx::query("UPDATE hosts SET drivers=$2,storages=$3 WHERE id=$1")
        .bind(host)
        .bind(&drivers)
        .bind(&storages)
        .execute(&mut *conn)
        .await
        .unwrap();
    sqlx::query("UPDATE machines SET facts=$2 WHERE id=$1")
        .bind(directory)
        .bind(json!({"environment_descriptor":{"name":"native-directory",
            "data_storage":{"made_by_orch":"directory"}},"storages":storages}))
        .execute(&mut *conn)
        .await
        .unwrap();
    let data = orch_db::queries::resources::mounts::data_driver(&mut conn, node)
        .await
        .unwrap();
    assert_eq!(data, Some(("native-volume".to_owned(), storages.clone())));
    let data = orch_db::queries::resources::mounts::data_driver(&mut conn, directory)
        .await
        .unwrap();
    assert_eq!(
        data,
        Some(("native-directory".to_owned(), storages.clone()))
    );
    assert_eq!(
        orch_db::queries::resources::mounts::volume_host(&mut conn, host)
            .await
            .unwrap(),
        Some((node, drivers.clone(), storages.clone()))
    );
    assert_eq!(
        orch_db::queries::resources::mounts::capabilities(&mut conn, node)
            .await
            .unwrap(),
        Some((drivers, storages))
    );
    // Host liveness is its machine's deletion state; hosts has no deleted_at column.
    sqlx::query("UPDATE machines SET deleted_at=$2 WHERE id=ANY($1)")
        .bind(vec![node, directory])
        .bind(w.now.get())
        .execute(&mut *conn)
        .await
        .unwrap();
    assert_eq!(
        orch_db::queries::resources::mounts::data_driver(&mut conn, node)
            .await
            .unwrap(),
        None
    );
    assert_eq!(
        orch_db::queries::resources::mounts::data_driver(&mut conn, directory)
            .await
            .unwrap(),
        None
    );
    assert_eq!(
        orch_db::queries::resources::mounts::volume_host(&mut conn, host)
            .await
            .unwrap(),
        None
    );
    assert_eq!(
        orch_db::queries::resources::mounts::capabilities(&mut conn, node)
            .await
            .unwrap(),
        None
    );
}
