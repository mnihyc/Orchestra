//! Provider credential imports from standard input through the ordinary authenticated sign-in request.

use super::{api, console, host};
use clap::{Args, Subcommand};
use orch_types::{Id, Json};
use serde_json::json;
use std::io::{Read, Write};
use std::path::PathBuf;
use std::process::ExitCode;

/// A provider command.
#[derive(Debug, Args)]
pub struct ProviderArgs {
    /// What to do with the provider.
    #[command(subcommand)]
    command: ProviderCommand,
}

#[derive(Debug, Subcommand)]
enum ProviderCommand {
    /// Import a credential; its secret value comes only from standard input.
    Login {
        /// The provider's identifier or canonical address.
        provider: String,
        /// The import method's id, such as access_token, auth_file or setup_token.
        #[arg(long)]
        method: String,
        /// Read the import method's values as one JSON object from standard input.
        #[arg(long, required = true)]
        values_stdin: bool,
        /// A PEM authority to trust beside the machine's public authorities.
        #[arg(long)]
        cacert: Option<PathBuf>,
    },
}

/// A small credential file holds several tokens; the API's own field metadata imposes any narrower bound.
const INPUT_MAX_BYTES: u64 = 64 * 1024;

// Only named field faults are useful to an unattended import. Never echo credential values,
// even if an API or adapter accidentally includes one in its explanation.
fn refusal(
    status: u16,
    answer: &Json,
    values: &std::collections::BTreeMap<String, String>,
) -> String {
    let mut message = format!("the provider sign-in was refused (HTTP {status})");
    if let Some(errors) = answer.get("errors").and_then(Json::as_array) {
        for error in errors {
            if let (Some(path), Some(detail)) = (
                error.get("path").and_then(Json::as_str),
                error.get("message").and_then(Json::as_str),
            ) {
                message.push_str(&format!("\n{path}: {detail}"));
            }
        }
    }
    let mut secrets: Vec<&str> = values
        .values()
        .map(String::as_str)
        .filter(|value| !value.is_empty())
        .collect();
    secrets.sort_by_key(|value| std::cmp::Reverse(value.len()));
    for secret in secrets {
        message = message.replace(secret, "[redacted]");
    }
    message
}

async fn resolve_provider(selector: &str, cacert: Option<&std::path::Path>) -> Result<Id, String> {
    let id = if let Ok(id) = selector.parse::<Id>() {
        id
    } else {
        let (status, list) =
            api::json_request("GET", "/providers?scope=managed", None, cacert).await?;
        if !(200..300).contains(&status) {
            return Err("the provider's canonical address could not be read".to_owned());
        }
        let rows = list
            .as_array()
            .or_else(|| list.get("items").and_then(Json::as_array))
            .ok_or_else(|| "the API did not return its providers".to_owned())?;
        let matches: Vec<Id> = rows
            .iter()
            .filter_map(|row| {
                let row = row.get("item").unwrap_or(row);
                (row.get("address").and_then(Json::as_str) == Some(selector))
                    .then(|| row.get("id").and_then(Json::as_str)?.parse().ok())
                    .flatten()
            })
            .collect();
        if matches.len() != 1 {
            return Err(
                "the canonical provider address is missing or ambiguous; use its id".to_owned(),
            );
        }
        matches[0]
    };
    let (status, provider) =
        api::json_request("GET", &format!("/providers/{id}"), None, cacert).await?;
    if !(200..300).contains(&status) {
        return Err("the provider could not be read with this API token".to_owned());
    }
    if provider.get("placement").and_then(Json::as_str) != Some("managed") {
        return Err("Sign in on that machine with its harness".to_owned());
    }
    Ok(id)
}

/// Imports a provider credential with ORCH_URL and ORCH_TOKEN and prints only the sign-in's standing.
#[must_use]
pub fn run(args: &ProviderArgs) -> ExitCode {
    let ProviderCommand::Login {
        provider,
        method,
        values_stdin: _,
        cacert,
    } = &args.command;
    let mut secret = String::new();
    if let Err(error) = std::io::stdin()
        .lock()
        .take(INPUT_MAX_BYTES + 1)
        .read_to_string(&mut secret)
    {
        return console::refuse(format!(
            "cannot read the credential from standard input: {error}"
        ));
    }
    if u64::try_from(secret.len()).unwrap_or(u64::MAX) > INPUT_MAX_BYTES || secret.trim().is_empty()
    {
        return console::refuse("the credential must be nonempty and at most 64 KiB");
    }
    let values: std::collections::BTreeMap<String, String> = match serde_json::from_str(&secret) {
        Ok(values) => values,
        Err(_) => {
            return console::refuse(
                "standard input must be one JSON object of the import method's string values",
            );
        }
    };
    if values.is_empty() {
        return console::refuse("give the import method's credential values on standard input");
    }
    let resolved = host::block_on(resolve_provider(provider, cacert.as_deref()));
    let id = match resolved {
        Ok(Ok(id)) => id,
        Ok(Err(error)) => return console::refuse(error),
        Err(error) => return console::refuse(error),
    };
    let path = format!("/providers/{id}/login");
    let body = json!({"method":method,"values":&values});
    let answer = host::block_on(api::json_request(
        "POST",
        &path,
        Some(body),
        cacert.as_deref(),
    ));
    let (status, value) = match answer {
        Ok(Ok(answer)) => answer,
        Ok(Err(error)) => return console::refuse(error),
        Err(error) => return console::refuse(error),
    };
    if !(200..300).contains(&status) {
        return console::refuse(refusal(status, &value, &values));
    }
    let Some(state) = value.get("state").and_then(|state| state.as_str()) else {
        return console::refuse("the API did not say where the sign-in stands");
    };
    // The adapter's free-form diagnostics are not printed by an unattended credential import.
    if !matches!(
        state,
        "idle" | "starting" | "awaiting_user" | "completed" | "failed" | "expired"
    ) {
        return console::refuse("the API returned an unknown sign-in state");
    }
    if let Err(error) = writeln!(std::io::stdout(), "{state}") {
        return console::refuse(error);
    }
    if matches!(state, "failed" | "expired") {
        ExitCode::FAILURE
    } else {
        ExitCode::SUCCESS
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_import_refusal_names_its_faults_and_hint_without_printing_any_given_value() {
        let values =
            std::collections::BTreeMap::from([("token".into(), "private-credential".into())]);
        let text = refusal(
            400,
            &json!({"detail":"private-credential", "errors":[
                {"path":"method","message":"use an import method for a script"},
                {"path":"values.token","message":"private-credential is not valid"}
            ]}),
            &values,
        );
        assert!(text.contains("method: use an import method for a script"));
        assert!(text.contains("values.token:"));
        assert!(!text.contains("private-credential"));
    }
}
