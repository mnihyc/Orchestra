//! Operator catalogue and legacy configuration behavior.
#![allow(clippy::unwrap_used, clippy::expect_used, reason = "test assertions")]

use orch_config::{ServerConfig, load_text};

#[test]
fn legacy_configuration_synthesizes_one_gateway_without_changing_entrances() {
    let config: ServerConfig = load_text(
        "[gateway]\ngateway_id='local'\nendpoints=[{url='wss://local.test:9443',trust='pinned'},{url='wss://front.test',trust='public'}]\n",
        "orch.toml", |_| None,
    ).unwrap();
    let catalogue = config.gateway.advertised_gateways();
    assert_eq!(catalogue.len(), 1);
    assert_eq!(catalogue[0].id, "local");
    assert_eq!(catalogue[0].label, "local");
    assert_eq!(catalogue[0].region, "Unspecified");
    assert_eq!(
        catalogue[0].endpoints,
        config.gateway.advertised_endpoints()
    );
    assert_eq!(catalogue[0].endpoints[1].url, "wss://front.test");
}

#[test]
fn labelled_gateways_project_the_same_ordered_legacy_endpoint_list() {
    let config: ServerConfig = load_text(
        "[[gateway.instances]]\nid='eu'\nlabel='Europe'\nregion='Europe'\nendpoints=[{url='wss://eu.test:9443',trust='pinned'},{url='https://eu.test',trust='public'}]\n[[gateway.instances]]\nid='asia'\nlabel='Asia tools'\nregion='Asia'\nendpoints=[{url='wss://asia.test:9443',trust='pinned'}]\n",
        "orch.toml", |_| None,
    ).unwrap();
    let catalogue = config.gateway.advertised_gateways();
    assert_eq!(catalogue.len(), 2);
    assert_eq!(
        (
            &*catalogue[1].id,
            &*catalogue[1].label,
            &*catalogue[1].region
        ),
        ("asia", "Asia tools", "Asia")
    );
    assert_eq!(
        config
            .gateway
            .advertised_endpoints()
            .into_iter()
            .map(|endpoint| endpoint.url)
            .collect::<Vec<_>>(),
        [
            "wss://eu.test:9443",
            "https://eu.test",
            "wss://asia.test:9443"
        ]
    );
}

#[test]
fn ambiguous_or_unverified_gateway_catalogues_are_refused() {
    let first = "[[gateway.instances]]\nid='eu'\nlabel='Europe'\nregion='Europe'\nendpoints=[{url='wss://eu.test',trust='pinned'}]\n";
    for second in [
        "id='eu'\nlabel='Other'\nregion='Other'\nendpoints=[{url='wss://other.test',trust='pinned'}]",
        "id='asia'\nlabel='Other'\nregion='Other'\nendpoints=[{url='https://eu.test:443/link',trust='public'}]",
        "id='asia'\nlabel=''\nregion='Asia'\nendpoints=[{url='wss://asia.test',trust='pinned'}]",
        "id='asia'\nlabel='Asia'\nregion='Asia'\nendpoints=[]",
        "id='asia'\nlabel='Asia'\nregion='Asia'\nendpoints=[{url='http://asia.test',trust='public'}]",
        "id='asia'\nlabel='Asia'\nregion='Asia'\nendpoints=[{url='wss://user:secret@asia.test',trust='pinned'}]",
        "id='asia'\nlabel='Asia'\nregion='Asia'\nendpoints=[{url='wss://asia.test?secret=private',trust='pinned'}]",
        "id='asia'\nlabel='Asia'\nregion='Asia'\nendpoints=[{url='wss://asia.test',trust='unknown'}]",
    ] {
        assert!(
            load_text::<ServerConfig>(
                &format!("{first}[[gateway.instances]]\n{second}\n"),
                "orch.toml",
                |_| None
            )
            .is_err(),
            "an invalid or ambiguous catalogue was accepted"
        );
    }
}

#[test]
fn connector_gateway_selection_has_one_explicit_persisted_meaning() {
    use orch_config::ConnectorConfig;
    use orch_link_contract::gateways::GatewaySelection;
    let selected: ConnectorConfig = load_text(
        "[connector.gateway]\nselection='selected'\ngateway_id='asia'\n",
        "config.toml",
        |_| None,
    )
    .unwrap();
    assert_eq!(
        selected.connector.gateway.selection,
        GatewaySelection::Selected
    );
    assert_eq!(
        selected.connector.gateway.gateway_id.as_deref(),
        Some("asia")
    );
    let automatic: ConnectorConfig = load_text(
        "[connector.gateway]\nselection='automatic'\n",
        "config.toml",
        |_| None,
    )
    .unwrap();
    assert_eq!(
        automatic.connector.gateway.selection,
        GatewaySelection::Automatic
    );
    for invalid in [
        "selection='selected'",
        "selection='automatic'\ngateway_id='asia'",
        "selection='nearby'",
    ] {
        assert!(
            load_text::<ConnectorConfig>(
                &format!("[connector.gateway]\n{invalid}\n"),
                "config.toml",
                |_| None
            )
            .is_err()
        );
    }
}
