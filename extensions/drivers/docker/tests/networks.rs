//! The networks of units on a real Docker daemon: every unit stands on a network of its own cut from the pool, reaches
//! the gateway and no other unit, and a unit without a network reaches the gateway and nothing else.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a test fails by panicking"
)]

mod common;

use std::collections::{BTreeSet, HashMap};
use std::io::Read;
use std::net::{Ipv4Addr, SocketAddr, TcpStream};
use std::time::Duration;

use common::{Gateway, Lab, address_on, bridge_address};
use orch_driver_contract::driver::{DriverError, EnvironmentDriver};
use orch_driver_contract::network::UnitNetworkRole;
use orch_driver_contract::units::UnitRef;
use serde_json::json;

/// Historical and foreign fixture bridges use the configured explicit pool too.
/// A developer's unrelated networks must not consume this fixture's address capacity.
async fn fixture_network(lab: &Lab, mut request: bollard::models::NetworkCreateRequest) {
    use bollard::models::{Ipam, IpamConfig};
    let pool = lab.config.networks.pool().unwrap();
    let seed = orch_types::new_id().as_u128();
    // Exhaustion is bounded by the configured pool's actual number of subnets.
    for attempt in 0..pool.subnets(lab.config.networks.prefix).unwrap().count() {
        let networks = lab
            .docker
            .list_networks(None::<bollard::query_parameters::ListNetworksOptions>)
            .await
            .unwrap();
        let taken: Vec<ipnet::IpNet> = networks
            .iter()
            .filter_map(|network| network.ipam.as_ref())
            .filter_map(|ipam| ipam.config.as_ref())
            .flatten()
            .filter_map(|config| config.subnet.as_deref())
            .filter_map(|subnet| subnet.parse().ok())
            .collect();
        let Some(subnet) = orch_driver_docker::restricted::subnet_choice(
            pool,
            lab.config.networks.prefix,
            seed,
            &taken,
            attempt,
        ) else {
            break;
        };
        request.ipam = Some(Ipam {
            driver: Some("default".into()),
            config: Some(vec![IpamConfig {
                subnet: Some(subnet.to_string()),
                gateway: subnet.hosts().next().map(|address| address.to_string()),
                ..Default::default()
            }]),
            ..Default::default()
        });
        match lab.docker.create_network(request.clone()).await {
            Ok(_) => return,
            // Another client may reserve the chosen subnet after the read.
            Err(error) if error.to_string().to_ascii_lowercase().contains("overlap") => {}
            Err(error) => panic!("fixture network {}: {error}", request.name),
        }
    }
    panic!("no fixture subnet remains in {pool}");
}

fn reach(address: &str, port: u16) -> Option<String> {
    let target: SocketAddr = format!("{address}:{port}").parse().unwrap();
    let mut stream = TcpStream::connect_timeout(&target, Duration::from_secs(3)).ok()?;
    stream
        .set_read_timeout(Some(Duration::from_secs(3)))
        .unwrap();
    let mut text = String::new();
    stream.read_to_string(&mut text).ok()?;
    Some(text)
}

#[tokio::test]
async fn two_units_cannot_reach_each_other_and_each_reaches_the_address_given_as_the_gateway() {
    let Some(lab) = Lab::start().await else {
        return;
    };
    let Some(bridge) = bridge_address(&lab.docker).await else {
        eprintln!("skipped: the daemon's default bridge has no address");
        return;
    };
    let gateway = Gateway::listen(&bridge);
    // A peer that listens.
    let peer = lab.request("environment", "peer", |request| {
        request["env"] = json!({ "LAB_RUN": "nc -lk -p 8080 -e echo peer-reply &" });
    });
    let peer_unit = lab.run(&peer).await;
    let peer_network = format!("{}-net", peer.name);
    let peer_address = address_on(&lab.docker, &peer_unit.reference, &peer_network).await;
    // The host reaches it, so that the unit failing to is the network's doing.
    let mut told = None;
    for _ in 0..20 {
        told = reach(&peer_address, 8080);
        if told.is_some() {
            break;
        }
        std::thread::sleep(Duration::from_millis(200));
    }
    assert_eq!(told.as_deref(), Some("peer-reply\n"), "the peer listens");
    let visitor = lab.request("environment", "visitor", |request| {
        request["env"] = json!({
            "GATEWAY_PORT": gateway.port.to_string(), "PEER": peer_address,
            "LAB_RUN": "nc -w 3 gateway.orch.internal $GATEWAY_PORT > $ORCH_DATA_DIR/gateway.txt 2>&1; nc -w 3 $PEER 8080 > $ORCH_DATA_DIR/peer.txt 2>&1; echo \"rc=$?\" >> $ORCH_DATA_DIR/peer.txt"
        });
    });
    let visitor_unit = lab.run(&visitor).await;
    let from_gateway = lab
        .read(&visitor_unit, "/var/lib/orch/gateway.txt")
        .await
        .unwrap();
    assert!(
        from_gateway.starts_with("gateway-hello from "),
        "{from_gateway:?}"
    );
    let from_peer = lab
        .read(&visitor_unit, "/var/lib/orch/peer.txt")
        .await
        .unwrap();
    assert!(
        !from_peer.contains("peer-reply"),
        "the units reached each other: {from_peer:?}"
    );
    assert!(!from_peer.contains("rc=0"), "{from_peer:?}");
    // Each network is the driver's, the unit's own, and joins nobody to anybody.
    let network = lab
        .docker
        .inspect_network(&peer_network, None)
        .await
        .unwrap();
    assert_eq!(network.internal, Some(false));
    assert_eq!(
        network.options.unwrap()["com.docker.network.bridge.enable_icc"],
        "false"
    );
    assert_eq!(network.labels.unwrap()["orch.network"], "unit");
    assert!(
        !lab.has_network(&lab.config.networks.env).await,
        "no common bridge is made any more"
    );
}

/// The IPv4 subnet of a network of the daemon.
async fn subnet_of(lab: &Lab, network: &str) -> ipnet::Ipv4Net {
    lab.docker
        .inspect_network(network, None)
        .await
        .unwrap()
        .ipam
        .and_then(|ipam| ipam.config)
        .and_then(|configs| configs.into_iter().find_map(|config| config.subnet))
        .expect("a subnet")
        .parse()
        .unwrap()
}

#[tokio::test]
async fn two_units_get_two_subnets_of_the_pool_and_nothing_is_taken_from_the_daemons_own_pools() {
    let Some(lab) = Lab::start().await else {
        return;
    };
    let pool = lab.config.networks.pool().unwrap();
    let environment = lab.request("environment", "first", |_| {});
    let provider = lab.request("provider", "second", |_| {});
    let running = lab.run(&environment).await;
    let made = lab.create(&provider).await;
    let mut subnets = Vec::new();
    for (spec, unit) in [(&environment, &running), (&provider, &made)] {
        let own = format!("{}-net", spec.name);
        let inspected = lab.inspect(unit).await;
        assert_eq!(
            inspected.host_config.unwrap().network_mode.as_deref(),
            Some(own.as_str())
        );
        let subnet = subnet_of(&lab, &own).await;
        assert!(pool.contains(&subnet), "{subnet} is cut from {pool}");
        assert_eq!(subnet.prefix_len(), lab.config.networks.prefix);
        subnets.push(subnet);
    }
    assert_ne!(subnets[0], subnets[1], "two units never share a subnet");
    // Everything the driver made for the two is their two networks, both from the pool: no common bridge, and
    // nothing from the daemon's own pools.
    let filters: HashMap<String, Vec<String>> =
        HashMap::from([("label".to_owned(), vec![format!("orch.test={}", lab.token)])]);
    let ours = lab
        .docker
        .list_networks(Some(
            bollard::query_parameters::ListNetworksOptionsBuilder::default()
                .filters(&filters)
                .build(),
        ))
        .await
        .unwrap();
    let names: BTreeSet<String> = ours
        .iter()
        .filter_map(|network| network.name.clone())
        .collect();
    assert_eq!(
        names,
        BTreeSet::from([
            format!("{}-net", environment.name),
            format!("{}-net", provider.name)
        ])
    );
    // What the facts and the hooks are told: the subnet, the host's address on it and the unit's own.
    let facts = lab.driver.inspect_network(&running).await.unwrap();
    assert_eq!(facts.networks.len(), 1, "{:?}", facts.networks);
    let own = &facts.networks[0];
    assert_eq!(own.role, UnitNetworkRole::Unit);
    assert_eq!(own.name, format!("{}-net", environment.name));
    assert_eq!(own.subnet, subnets[0].to_string());
    assert_eq!(
        own.gateway,
        subnets[0].hosts().next().map(|address| address.to_string())
    );
    let address: Ipv4Addr = own
        .address
        .as_deref()
        .expect("a running unit's address")
        .parse()
        .unwrap();
    assert!(subnets[0].contains(&address));
    // A unit's network goes with it.
    lab.driver.remove(&made).await.unwrap();
    assert!(!lab.has_network(&format!("{}-net", provider.name)).await);
}

#[tokio::test]
async fn an_open_unit_made_on_a_common_bridge_is_moved_to_a_network_of_its_own_at_its_next_start_in_the_same_container()
 {
    use bollard::models::{ContainerCreateBody, NetworkCreateRequest};
    use bollard::query_parameters::{CreateContainerOptionsBuilder, RemoveContainerOptionsBuilder};
    let Some(lab) = Lab::start().await else {
        return;
    };
    // A unit as the driver once made one: on the common bridge of environments.
    let spec = lab.request("environment", "old-open", |_| {});
    let unit = lab.create(&spec).await;
    let bridge = lab.config.networks.env.clone();
    fixture_network(
        &lab,
        NetworkCreateRequest {
            name: bridge.clone(),
            driver: Some("bridge".to_owned()),
            labels: Some(
                [
                    ("orch.test".to_owned(), lab.token.clone()),
                    ("orch.network".to_owned(), "env".to_owned()),
                ]
                .into(),
            ),
            ..NetworkCreateRequest::default()
        },
    )
    .await;
    let made = lab.inspect(&unit).await;
    let config = made.config.clone().unwrap();
    let mut host = made.host_config.clone().unwrap();
    host.network_mode = Some(bridge.clone());
    lab.docker
        .remove_container(
            &unit.reference,
            Some(RemoveContainerOptionsBuilder::default().force(true).build()),
        )
        .await
        .unwrap();
    let old = lab
        .docker
        .create_container(
            Some(
                CreateContainerOptionsBuilder::default()
                    .name(&spec.name)
                    .build(),
            ),
            ContainerCreateBody {
                image: config.image,
                env: config.env,
                entrypoint: config.entrypoint,
                cmd: config.cmd,
                user: config.user,
                labels: config.labels,
                working_dir: config.working_dir,
                host_config: Some(host),
                ..ContainerCreateBody::default()
            },
        )
        .await
        .unwrap();
    let unit = UnitRef {
        reference: old.id.clone(),
        ..unit
    };
    // Its next start puts it on a network of its own and takes it off the bridge: the same container, so whatever
    // it holds outside its volumes stays.
    lab.driver.start(&unit).await.unwrap();
    lab.wait_done(&unit).await;
    let moved = lab
        .docker
        .inspect_container(&spec.name, None)
        .await
        .unwrap();
    assert_eq!(
        moved.id.as_deref(),
        Some(old.id.as_str()),
        "the unit was not made again"
    );
    let on: BTreeSet<String> = moved
        .network_settings
        .unwrap()
        .networks
        .unwrap()
        .into_keys()
        .collect();
    assert_eq!(on, BTreeSet::from([format!("{}-net", spec.name)]));
    let address: Ipv4Addr = address_on(&lab.docker, &old.id, &format!("{}-net", spec.name))
        .await
        .parse()
        .unwrap();
    assert!(lab.config.networks.pool().unwrap().contains(&address));
}

/// What a unit's stand-in connector wrote into the file `file` of its data directory; empty when nothing.
async fn written(lab: &Lab, unit: &UnitRef, file: &str) -> String {
    lab.read(unit, &format!("/var/lib/orch/{file}"))
        .await
        .unwrap_or_default()
}

/// A lab whose driver keeps restricted units' proxies' files in the lab's own directory, and whether this daemon
/// was found able to make their networks.
async fn restricted_lab() -> Option<Lab> {
    let lab = Lab::with(|config, dir| {
        config.egress.dir = Some(dir.join("egress"));
    })
    .await?;
    if !lab.driver.facts().restricted_egress {
        eprintln!(
            "skipped: this daemon cannot make a unit's own network whose bridge has no address on the host"
        );
        return None;
    }
    Some(lab)
}

#[tokio::test]
async fn a_unit_without_a_way_out_reaches_the_gateway_through_its_proxy_and_no_other_host_service_unit_or_internet()
 {
    let Some(lab) = restricted_lab().await else {
        return;
    };
    let Some(bridge) = bridge_address(&lab.docker).await else {
        eprintln!("skipped: the daemon's default bridge has no address");
        return;
    };
    // The gateway, where `gateway.orch.internal` takes the proxy, and a second service of the host beside it.
    let gateway = Gateway::listen(&bridge);
    let other = Gateway::listen(&bridge);
    // A unit on the pool's network, which listens.
    let peer = lab.request("environment", "peer", |request| {
        request["env"] = json!({ "LAB_RUN": "nc -lk -p 8080 -e echo peer-reply &" });
    });
    let peer_unit = lab.run(&peer).await;
    let peer_address = address_on(
        &lab.docker,
        &peer_unit.reference,
        &format!("{}-net", peer.name),
    )
    .await;
    let spec = lab.request("environment", "sealed", |request| {
        request["spec"]["network"]["mode"] = json!("none");
        request["gateway_url"] = json!(format!("wss://gateway.orch.internal:{}", gateway.port));
        request["env"] = json!({
            "GATEWAY_PORT": gateway.port.to_string(), "BRIDGE": bridge, "OTHER_PORT": other.port.to_string(),
            "PEER": peer_address,
            "LAB_RUN": "p=${HTTPS_PROXY#http://}; ph=${p%:*}; pp=${p##*:}; for i in 1 2 3 4 5 6 7 8 9 10; do printf 'CONNECT gateway.orch.internal:%s HTTP/1.1\\r\\n\\r\\n' \"$GATEWAY_PORT\" | nc -w 5 \"$ph\" \"$pp\" > $ORCH_DATA_DIR/gateway.txt 2>&1; grep -q ' 200 ' $ORCH_DATA_DIR/gateway.txt && break; sleep 1; done; printf 'CONNECT example.com:443 HTTP/1.1\\r\\n\\r\\n' | nc -w 5 \"$ph\" \"$pp\" > $ORCH_DATA_DIR/denied.txt 2>&1; nc -w 3 gateway.orch.internal $GATEWAY_PORT > $ORCH_DATA_DIR/direct.txt 2>&1; echo \"rc=$?\" >> $ORCH_DATA_DIR/direct.txt; nc -w 3 $BRIDGE $OTHER_PORT > $ORCH_DATA_DIR/other.txt 2>&1; echo \"rc=$?\" >> $ORCH_DATA_DIR/other.txt; nc -w 3 $PEER 8080 > $ORCH_DATA_DIR/peer.txt 2>&1; echo \"rc=$?\" >> $ORCH_DATA_DIR/peer.txt; nc -w 3 1.1.1.1 80 > $ORCH_DATA_DIR/outside.txt 2>&1; echo \"rc=$?\" >> $ORCH_DATA_DIR/outside.txt; ip route | grep -c '^default' > $ORCH_DATA_DIR/routes.txt"
        });
    });
    let unit = lab.run(&spec).await;
    // Through its proxy, the gateway answers; another host's name is refused there.
    let through = written(&lab, &unit, "gateway.txt").await;
    assert!(
        through.contains("200 Connection established"),
        "{through:?}"
    );
    assert!(through.contains("gateway-hello from "), "{through:?}");
    assert!(written(&lab, &unit, "denied.txt").await.contains("403"));
    // Directly, nothing answers: not the gateway by its name, not a second service of the host, not another unit,
    // not the internet; and there is no route to try.
    for file in ["direct.txt", "other.txt", "peer.txt", "outside.txt"] {
        let text = written(&lab, &unit, file).await;
        assert!(
            !text.contains("hello") && !text.contains("peer-reply") && !text.contains("rc=0"),
            "{file}: {text:?}"
        );
    }
    assert_eq!(
        other.accepted.load(std::sync::atomic::Ordering::SeqCst),
        0,
        "the second service of the host saw no connection"
    );
    assert_eq!(
        written(&lab, &unit, "routes.txt").await.trim(),
        "0",
        "there is no default route"
    );
    // The unit's network is its own: internal, its bridge without an address on the host; the unit has no host
    // entry for the gateway and the proxy as its name server.
    let network_name = format!("{}-net", spec.name);
    let inspected = lab.inspect(&unit).await;
    let host = inspected.host_config.unwrap();
    assert_eq!(host.network_mode.as_deref(), Some(network_name.as_str()));
    assert_eq!(host.extra_hosts, None);
    let network = lab
        .docker
        .inspect_network(&network_name, None)
        .await
        .unwrap();
    assert_eq!(network.internal, Some(true));
    assert_eq!(
        network.options.unwrap()["com.docker.network.bridge.gateway_mode_ipv4"],
        "isolated"
    );
    assert!(lab.has_container(&format!("{}-proxy", spec.name)).await);
    assert!(!lab.launch(&unit).await["hosts"].contains("gateway.orch.internal"));
    // The proxy goes out on a network of its own from the pool, which the unit is not on; the facts say both.
    let outward = format!("{}-out", spec.name);
    let pool = lab.config.networks.pool().unwrap();
    assert!(pool.contains(&subnet_of(&lab, &outward).await));
    assert!(pool.contains(&subnet_of(&lab, &network_name).await));
    let facts = lab.driver.inspect_network(&unit).await.unwrap();
    let roles: Vec<(String, UnitNetworkRole, bool)> = facts
        .networks
        .iter()
        .map(|network| {
            (
                network.name.clone(),
                network.role,
                network.gateway.is_some(),
            )
        })
        .collect();
    assert!(
        roles.contains(&(network_name.clone(), UnitNetworkRole::Unit, false)),
        "the unit's own network has no host address: {roles:?}"
    );
    assert!(
        roles.contains(&(outward.clone(), UnitNetworkRole::Egress, true)),
        "{roles:?}"
    );
}

#[tokio::test]
async fn a_unit_made_on_the_old_shared_network_is_made_again_on_a_network_of_its_own_when_it_next_starts()
 {
    use bollard::models::{ContainerCreateBody, NetworkCreateRequest};
    use bollard::query_parameters::{CreateContainerOptionsBuilder, RemoveContainerOptionsBuilder};
    let Some(lab) = restricted_lab().await else {
        return;
    };
    // A unit as the driver once made one without a way out: on the shared internal network.
    let spec = lab.request("environment", "old-sealed", |_| {});
    let unit = lab.create(&spec).await;
    let old_network = format!("{}-internal", lab.config.networks.env);
    fixture_network(
        &lab,
        NetworkCreateRequest {
            name: old_network.clone(),
            driver: Some("bridge".to_owned()),
            internal: Some(true),
            labels: Some(
                [
                    ("orch.test".to_owned(), lab.token.clone()),
                    ("orch.network".to_owned(), "env-internal".to_owned()),
                ]
                .into(),
            ),
            ..NetworkCreateRequest::default()
        },
    )
    .await;
    let made = lab.inspect(&unit).await;
    let config = made.config.clone().unwrap();
    let mut host = made.host_config.clone().unwrap();
    host.network_mode = Some(old_network.clone());
    lab.docker
        .remove_container(
            &unit.reference,
            Some(RemoveContainerOptionsBuilder::default().force(true).build()),
        )
        .await
        .unwrap();
    // The simulated old shared unit predates per-unit networks. Do not leave the fresh open
    // fixture's unused network at the name its isolated successor must allocate.
    lab.docker
        .remove_network(&format!("{}-net", spec.name))
        .await
        .unwrap();
    lab.docker
        .create_container(
            Some(
                CreateContainerOptionsBuilder::default()
                    .name(&spec.name)
                    .build(),
            ),
            ContainerCreateBody {
                image: config.image,
                env: config.env,
                entrypoint: config.entrypoint,
                cmd: config.cmd,
                user: config.user,
                labels: config.labels,
                working_dir: config.working_dir,
                host_config: Some(host),
                ..ContainerCreateBody::default()
            },
        )
        .await
        .unwrap();
    // Its next start makes it again on its own network, with its proxy, and never on the old one.
    lab.driver.start(&unit).await.unwrap();
    let again = lab
        .docker
        .inspect_container(&spec.name, None)
        .await
        .unwrap();
    assert_eq!(
        again.host_config.unwrap().network_mode,
        Some(format!("{}-net", spec.name))
    );
    assert_eq!(again.config.unwrap().labels.unwrap()["orch.egress"], "none");
    assert!(lab.has_container(&format!("{}-proxy", spec.name)).await);
}

#[tokio::test]
async fn a_network_of_the_name_that_is_not_the_drivers_is_never_used() {
    let Some(lab) = Lab::start().await else {
        return;
    };
    // Somebody else's network with the name the driver would give the unit's own (labelled for the lab only so
    // that it is removed).
    let spec = lab.request("environment", "foreign-network", |_| {});
    lab.track(&spec);
    let foreign = bollard::models::NetworkCreateRequest {
        name: format!("{}-net", spec.name),
        driver: Some("bridge".to_owned()),
        labels: Some([("orch.test".to_owned(), lab.token.clone())].into()),
        ..bollard::models::NetworkCreateRequest::default()
    };
    fixture_network(&lab, foreign).await;
    let error = lab.driver.create(&spec).await.unwrap_err();
    assert!(
        matches!(error, DriverError::Internal(ref why) if why.contains("is not one of the driver's")),
        "{error:?}"
    );
    assert!(!lab.has_container(&spec.name).await);
}
