//! One gateway selector for direct enrollment, installers and the local console.

use crate::enrollment::{DataDir, EnrollError, EnrollTransport, EnrollmentProbe, endpoints_of};
use orch_link::gateways::PrimaryGateway;
use orch_link_contract::enrollment::EnrollmentToken;
use orch_link_contract::gateways::{
    GatewayChoice, GatewayInstance, GatewayMeasurement, GatewaySelection,
};
use serde::{Deserialize, Serialize};
use std::io::{self, BufRead, IsTerminal, Write};

/// One visible choice; the stable ID is carried by `choice`, never inferred from a label.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GatewayOption {
    /// Automatic or the exact logical gateway.
    pub choice: GatewayChoice,
    /// Operator label, or Automatic.
    pub label: String,
    /// Operator region; empty for Automatic.
    pub region: String,
    /// Best verified entrance's median, when measured.
    pub round_trip_ms: Option<u64>,
}

/// Render the same options from fresh enrollment or cached authenticated advertisements.
#[must_use]
pub fn options(
    catalogue: &[GatewayInstance],
    measurements: &[GatewayMeasurement],
) -> Vec<GatewayOption> {
    let mut rows = vec![GatewayOption {
        choice: GatewayChoice::default(),
        label: "Automatic".into(),
        region: String::new(),
        round_trip_ms: None,
    }];
    rows.extend(catalogue.iter().map(|gateway| {
        GatewayOption {
            choice: GatewayChoice {
                selection: GatewaySelection::Selected,
                gateway_id: Some(gateway.id.clone()),
            },
            label: gateway.label.clone(),
            region: gateway.region.clone(),
            round_trip_ms: measurements
                .iter()
                .filter(|sample| sample.gateway_id == gateway.id)
                .map(|sample| sample.round_trip_ms)
                .min(),
        }
    }));
    rows
}

/// A removed selected gateway stays visible and selected; advertisements never select Automatic.
pub fn retain_current(rows: &mut Vec<GatewayOption>, choice: &GatewayChoice) {
    if choice.selection == GatewaySelection::Selected
        && !rows.iter().any(|row| row.choice == *choice)
    {
        rows.push(GatewayOption {
            choice: choice.clone(),
            label: format!(
                "{} (unavailable)",
                choice.gateway_id.as_deref().unwrap_or("Selected gateway")
            ),
            region: String::new(),
            round_trip_ms: None,
        });
    }
}

/// A safe line used by both the install prompt and the console's list.
#[must_use]
pub fn option_line(row: &GatewayOption) -> String {
    if row.choice.selection == GatewaySelection::Automatic {
        return "Automatic        Lowest measured round trip; measured again on reconnect".into();
    }
    let timing = row
        .round_trip_ms
        .map_or_else(|| "Not measured".into(), |ms| format!("{ms} ms"));
    format!("{:<18} {:<12} {timing}", row.label, row.region)
}

/// Explain the persisted choice without claiming that a session already stands.
#[must_use]
pub fn choice_name(choice: &GatewayChoice, catalogue: &[GatewayInstance]) -> String {
    if choice.selection == GatewaySelection::Automatic {
        return "Automatic".into();
    }
    catalogue
        .iter()
        .find(|gateway| Some(gateway.id.as_str()) == choice.gateway_id.as_deref())
        .map(|gateway| format!("{} ({})", gateway.label, gateway.region))
        .unwrap_or_else(|| {
            choice
                .gateway_id
                .clone()
                .unwrap_or_else(|| "Selected gateway".into())
        })
}

/// The install flag accepts Automatic or one advertised gateway's exact ID.
pub fn from_argument(value: &str, rows: &[GatewayOption]) -> Result<GatewayChoice, String> {
    if value == "automatic" {
        return Ok(GatewayChoice::default());
    }
    rows.iter()
        .find(|row| row.choice.gateway_id.as_deref() == Some(value))
        .map(|row| row.choice.clone())
        .ok_or_else(|| "Choose Automatic or an advertised gateway ID.".to_owned())
}

/// Read-only discovery/probes for an installation prompt; no token is consumed.
pub async fn enrollment_options(
    data: &DataDir,
    address: &str,
    token_text: &str,
    transport: &dyn EnrollTransport,
    measure: bool,
) -> Result<Vec<GatewayOption>, EnrollError> {
    let token: EnrollmentToken = token_text
        .trim()
        .parse()
        .map_err(|_| EnrollError::BadToken)?;
    let Some(discovery) = transport.gateways(address, &token).await? else {
        let record = data.record().ok();
        return Ok(options(
            record
                .as_ref()
                .map_or(&[], |record| record.gateways.as_slice()),
            record
                .as_ref()
                .map_or(&[], |record| record.gateway_measurements.as_slice()),
        ));
    };
    let authority = orch_pki::certificates::Certificate::from_pem(&discovery.ca_pem)?;
    if !token.pins(&authority.spki_sha256()) {
        return Err(EnrollError::Unreachable(
            "the gateway catalogue belongs to another installation".into(),
        ));
    }
    let measurements = if measure {
        let config = data.config(|_| None)?;
        let local = endpoints_of(&config);
        let endpoints = if local.is_empty() {
            discovery
                .gateways
                .iter()
                .flat_map(|gateway| gateway.endpoints.clone())
                .collect()
        } else {
            local
        };
        let policy = PrimaryGateway {
            choice: GatewayChoice::default(),
            catalogue: discovery.gateways.clone(),
        };
        let prober = EnrollmentProbe {
            transport,
            authority: &authority,
        };
        // Missing measurements stay visible; a hand-picked gateway is never overruled by a probe.
        policy
            .candidates(&prober, &endpoints, None)
            .await
            .map(|(_, measurements)| measurements)
            .unwrap_or_default()
    } else {
        Vec::new()
    };
    Ok(options(&discovery.gateways, &measurements))
}

/// Read a choice through any ordinary reader/writer; installers use the controlling terminal.
pub fn prompt<R: BufRead, W: Write>(
    rows: &[GatewayOption],
    current: &GatewayChoice,
    input: &mut R,
    output: &mut W,
) -> io::Result<GatewayChoice> {
    let default = rows
        .iter()
        .position(|row| row.choice == *current)
        .unwrap_or(0);
    writeln!(output, "Gateway for this machine")?;
    for (index, row) in rows.iter().enumerate() {
        writeln!(output, "  {}  {}", index + 1, option_line(row))?;
    }
    loop {
        write!(output, "Gateway [{}]: ", default + 1)?;
        output.flush()?;
        let mut line = String::new();
        let limit = crate::console::enrollment::MAX_TEXT_BYTES;
        let read = std::io::Read::take(&mut *input, limit as u64).read_line(&mut line)?;
        if line.len() >= limit && !line.ends_with('\n') {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "gateway selection input is too long",
            ));
        }
        if read == 0 {
            return Err(io::Error::new(
                io::ErrorKind::Interrupted,
                "gateway selection cancelled",
            ));
        }
        let value = line.trim();
        if value.is_empty() {
            return rows
                .get(default)
                .map(|row| row.choice.clone())
                .ok_or_else(|| io::Error::other("no gateway choice is available"));
        }
        if let Some(row) = value
            .parse::<usize>()
            .ok()
            .and_then(|number| number.checked_sub(1))
            .and_then(|index| rows.get(index))
        {
            return Ok(row.choice.clone());
        }
        if let Ok(choice) = from_argument(value, rows) {
            return Ok(choice);
        }
        writeln!(
            output,
            "Choose a listed gateway number or ID, or automatic."
        )?;
    }
}

fn terminal_input() -> Option<std::fs::File> {
    if !io::stderr().is_terminal() {
        return None;
    }
    #[cfg(unix)]
    {
        std::fs::OpenOptions::new().read(true).open("/dev/tty").ok()
    }
    #[cfg(windows)]
    {
        std::fs::OpenOptions::new().read(true).open("CONIN$").ok()
    }
    #[cfg(not(any(unix, windows)))]
    {
        None
    }
}

/// Configure only an explicit flag or first-install interactive choice. Re-enrollment
/// and unattended startup retain the local setting; fresh unattended defaults are Automatic.
pub async fn configure_enrollment(
    data: &DataDir,
    address: &str,
    token: &str,
    transport: &dyn EnrollTransport,
    requested: Option<&str>,
    interactive: bool,
) -> Result<GatewayChoice, EnrollError> {
    let current = data.config(|_| None)?.connector.gateway;
    let before = current.clone();
    let choice = if let Some(value) = requested {
        if value == "automatic" {
            GatewayChoice::default()
        } else {
            from_argument(
                value,
                &enrollment_options(data, address, token, transport, false).await?,
            )
            .map_err(EnrollError::Config)?
        }
    } else if interactive && !data.is_enrolled() {
        if let Some(terminal) = terminal_input() {
            let mut rows = enrollment_options(data, address, token, transport, true).await?;
            retain_current(&mut rows, &current);
            prompt(
                &rows,
                &current,
                &mut io::BufReader::new(terminal),
                &mut io::stderr().lock(),
            )
            .map_err(|error| EnrollError::Config(error.to_string()))?
        } else {
            current
        }
    } else {
        current
    };
    if choice != before {
        data.update_config(|config| config.connector.gateway = choice.clone())?;
    }
    Ok(choice)
}
