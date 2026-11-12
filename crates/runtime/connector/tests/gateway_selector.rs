//! The shared installer/console selector, with no real service or credential use.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    reason = "test assertions"
)]

use async_trait::async_trait;
use orch_connector_core::enrollment::{DataDir, EnrollError, EnrollTransport};
use orch_connector_core::gateway_selector::{
    configure_enrollment, from_argument, options, prompt, retain_current,
};
use orch_link_contract::enrollment::{
    Endpoint, EnrollReply, EnrollRequest, EnrollmentToken, RenewReply, RenewRequest, Trust,
};
use orch_link_contract::gateways::{
    GatewayChoice, GatewayInstance, GatewayMeasurement, GatewaySelection,
};
use orch_pki::enrollment::MachineIdentity;
use std::io::Cursor;

fn choices() -> Vec<orch_connector_core::gateway_selector::GatewayOption> {
    let catalogue = [
        ("eu", "Europe", "Europe", 94),
        ("asia", "Asia tools", "Asia", 188),
    ]
    .into_iter()
    .map(|(id, label, region, ms)| {
        let endpoint = Endpoint {
            url: format!("wss://{id}.test:9443"),
            trust: Trust::Pinned,
        };
        (
            GatewayInstance {
                id: id.into(),
                label: label.into(),
                region: region.into(),
                endpoints: vec![endpoint.clone()],
            },
            GatewayMeasurement {
                gateway_id: id.into(),
                endpoint,
                round_trip_ms: ms,
            },
        )
    })
    .collect::<Vec<_>>();
    options(
        &catalogue
            .iter()
            .map(|(gateway, _)| gateway.clone())
            .collect::<Vec<_>>(),
        &catalogue
            .iter()
            .map(|(_, sample)| sample.clone())
            .collect::<Vec<_>>(),
    )
}

#[test]
fn the_selector_shows_operator_labels_regions_and_measurements_and_saves_the_exact_id() {
    let rows = choices();
    let mut output = Vec::new();
    let selected = prompt(
        &rows,
        &GatewayChoice::default(),
        &mut Cursor::new("3\n"),
        &mut output,
    )
    .unwrap();
    assert_eq!(
        selected,
        GatewayChoice {
            selection: GatewaySelection::Selected,
            gateway_id: Some("asia".into())
        }
    );
    let shown = String::from_utf8(output).unwrap();
    assert!(shown.contains("Gateway for this machine"));
    assert!(
        shown.contains("Automatic        Lowest measured round trip; measured again on reconnect")
    );
    assert!(shown.contains("Europe") && shown.contains("94 ms"));
    assert!(shown.contains("Asia tools") && shown.contains("188 ms"));
    assert!(from_argument("unknown", &rows).is_err());
}

#[test]
fn enter_retains_a_selected_gateway_even_when_it_is_no_longer_advertised() {
    let current = GatewayChoice {
        selection: GatewaySelection::Selected,
        gateway_id: Some("retired".into()),
    };
    let mut rows = choices();
    retain_current(&mut rows, &current);
    let selected = prompt(&rows, &current, &mut Cursor::new("\n"), &mut Vec::new()).unwrap();
    assert_eq!(selected, current);
    assert!(rows.iter().any(|row| row.label.contains("unavailable")));
}

#[test]
fn invalid_pasted_input_is_not_echoed_and_cancellation_is_not_an_automatic_choice() {
    let mut output = Vec::new();
    let selected = prompt(
        &choices(),
        &GatewayChoice::default(),
        &mut Cursor::new("oenr_fixture_private\n2\n"),
        &mut output,
    )
    .unwrap();
    assert_eq!(selected.gateway_id.as_deref(), Some("eu"));
    assert!(
        !String::from_utf8(output)
            .unwrap()
            .contains("oenr_fixture_private")
    );
    assert!(
        prompt(
            &choices(),
            &GatewayChoice::default(),
            &mut Cursor::new(""),
            &mut Vec::new()
        )
        .is_err()
    );
}

struct NoNetwork;
#[async_trait]
impl EnrollTransport for NoNetwork {
    async fn gateways(
        &self,
        _: &str,
        _: &EnrollmentToken,
    ) -> Result<Option<orch_link_contract::gateways::GatewayDiscovery>, EnrollError> {
        panic!("retaining an unattended choice must not discover gateways")
    }
    async fn enroll(
        &self,
        _: &str,
        _: &EnrollmentToken,
        _: &EnrollRequest,
    ) -> Result<EnrollReply, EnrollError> {
        panic!("choosing a gateway must not enroll")
    }
    async fn renew(
        &self,
        _: &Endpoint,
        _: &MachineIdentity,
        _: &RenewRequest,
    ) -> Result<RenewReply, EnrollError> {
        panic!("choosing a gateway must not renew")
    }
}

#[tokio::test]
async fn unattended_setup_without_a_flag_preserves_the_existing_choice_and_configuration() {
    let directory = tempfile::tempdir().unwrap();
    let data = DataDir::open(directory.path()).unwrap();
    let selected = GatewayChoice {
        selection: GatewaySelection::Selected,
        gateway_id: Some("asia".into()),
    };
    data.update_config(|config| config.connector.gateway = selected.clone())
        .unwrap();
    let before = std::fs::read(data.config_path()).unwrap();
    let retained = configure_enrollment(
        &data,
        "https://orch.test",
        "not-observed",
        &NoNetwork,
        None,
        false,
    )
    .await
    .unwrap();
    assert_eq!(retained, selected);
    assert_eq!(std::fs::read(data.config_path()).unwrap(), before);
}
