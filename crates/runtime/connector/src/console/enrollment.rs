//! Read enrollment data pasted into Start, without executing a shell or displaying its token.

/// A pasted installer command is a few hundred bytes; sixteen KiB also bounds a mistaken clipboard dump.
pub const MAX_TEXT_BYTES: usize = 16 * 1024;

/// Enrollment choices extracted as data from an address, an address/token pair or a copied command.
#[derive(Clone, PartialEq, Eq)]
pub struct Parsed {
    /// System address.
    pub address: String,
    /// Enrollment proof; absent when the next prompt must ask for it.
    pub token: Option<String>,
    /// Optional machine name from the copied command.
    pub name: Option<String>,
    /// Explicit installer choice to hand all policy keys over.
    pub delegate_policy: bool,
    /// Roles and sections explicitly accepted in the copied command.
    pub roles: Option<String>,
    /// Role-section handover from --delegate/-Delegate.
    pub delegate: Option<String>,
    /// Local dedicated-machine consent.
    pub dedicated: bool,
    /// A command for the separate node program cannot enroll this connector console.
    pub node: bool,
    /// Explicit installer choice to leave its per-user service uninstalled.
    pub no_service: bool,
    /// A copied command's named directory, checked against this console's directory before enrollment.
    pub data_dir: Option<String>,
}

impl std::fmt::Debug for Parsed {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("Parsed")
            .field("address", &self.address)
            .field("token", &self.token.as_ref().map(|_| "<redacted>"))
            .field("name", &self.name)
            .field("delegate_policy", &self.delegate_policy)
            .field("no_service", &self.no_service)
            .field("data_dir", &self.data_dir)
            .finish()
    }
}

/// A safe explanation of invalid clipboard data; it never repeats a pasted value.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum Error {
    /// Empty or unexpectedly large clipboard data.
    #[error("Paste the system address or its enrollment command (at most sixteen KiB).")]
    Size,
    /// A quoted argument was not complete.
    #[error(
        "The pasted command has an unfinished quote or escape; copy the complete command again."
    )]
    Quote,
    /// A required value was missing or repeated.
    #[error("The pasted command must have one system address and at most one enrollment token.")]
    Arguments,
    /// Only the normal verified enrollment entrance may be used.
    #[error("The system address must use https:// or wss:// and contain no username or password.")]
    Address,
}

fn words(text: &str) -> Result<Vec<String>, Error> {
    let powershell = text.trim_start().starts_with('&') || text.contains("scriptblock");
    let mut input = text.chars().peekable();
    let (mut result, mut word, mut quote, mut present) = (Vec::new(), String::new(), None, false);
    while let Some(character) = input.next() {
        match (quote, character) {
            (Some('\''), '\'') if powershell && input.peek() == Some(&'\'') => {
                input.next();
                word.push('\'');
            }
            (Some(delimiter), character) if delimiter == character => quote = None,
            (None, '\'' | '"') => {
                quote = Some(character);
                present = true;
            }
            (None | Some('"'), '\\') if !powershell => {
                word.push(input.next().ok_or(Error::Quote)?);
                present = true;
            }
            (None, character) if character.is_whitespace() => {
                if present {
                    result.push(std::mem::take(&mut word));
                    present = false;
                }
            }
            _ => {
                word.push(character);
                present = true;
            }
        }
    }
    if quote.is_some() {
        return Err(Error::Quote);
    }
    if present {
        result.push(word);
    }
    Ok(result)
}

fn address(text: &str) -> Result<(), Error> {
    let url = url::Url::parse(text).map_err(|_| Error::Address)?;
    if !matches!(url.scheme(), "https" | "wss")
        || url.host_str().is_none()
        || !url.username().is_empty()
        || url.password().is_some()
        || url.query().is_some()
        || url.fragment().is_some()
    {
        return Err(Error::Address);
    }
    Ok(())
}

/// Extracts data from the POSIX or PowerShell command the Add machine dialog gives; never executes any part.
pub fn parse(text: &str) -> Result<Parsed, Error> {
    if text.trim().is_empty() || text.len() > MAX_TEXT_BYTES {
        return Err(Error::Size);
    }
    let words = words(text)?;
    let mut parsed = Parsed {
        address: String::new(),
        token: None,
        name: None,
        delegate_policy: false,
        roles: None,
        delegate: None,
        dedicated: false,
        node: words
            .iter()
            .any(|word| word == "orch-node" || word.contains("install-node.sh")),
        no_service: false,
        data_dir: None,
    };
    if words.len() <= 2
        && words
            .first()
            .is_some_and(|word| word.starts_with("https://") || word.starts_with("wss://"))
    {
        parsed.address.clone_from(&words[0]);
        parsed.token = words.get(1).cloned();
    } else {
        let mut index = 0;
        while index < words.len() {
            let (flag, inline) = words[index]
                .split_once('=')
                .map_or((words[index].as_str(), None), |(flag, value)| {
                    (flag, Some(value))
                });
            let flag = flag.to_ascii_lowercase();
            let slot = match flag.as_str() {
                "--url" | "-url" => Some(&mut parsed.address),
                "--token" | "-token" => Some(parsed.token.get_or_insert_with(String::new)),
                "--name" | "-name" => Some(parsed.name.get_or_insert_with(String::new)),
                "--roles" | "-roles" => Some(parsed.roles.get_or_insert_with(String::new)),
                "--delegate" | "-delegate" => Some(parsed.delegate.get_or_insert_with(String::new)),
                "--data-dir" | "-datadir" => Some(parsed.data_dir.get_or_insert_with(String::new)),
                "--delegate-policy" | "-delegatepolicy" => {
                    parsed.delegate_policy = true;
                    None
                }
                "--dedicated" | "-dedicated" => {
                    parsed.dedicated = true;
                    None
                }
                "--no-service" | "-noservice" => {
                    parsed.no_service = true;
                    None
                }
                _ => None,
            };
            if let Some(slot) = slot {
                if !slot.is_empty() {
                    return Err(Error::Arguments);
                }
                let value = match inline {
                    Some(value) => value.to_owned(),
                    None => {
                        index += 1;
                        words.get(index).cloned().ok_or(Error::Arguments)?
                    }
                };
                if value.is_empty() {
                    return Err(Error::Arguments);
                }
                *slot = value;
            }
            index += 1;
        }
    }
    address(&parsed.address)?;
    if parsed.delegate_policy && parsed.delegate.is_some() {
        return Err(Error::Arguments);
    }
    Ok(parsed)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_two_copied_installer_commands_and_a_plain_address_keep_the_same_private_data() {
        for text in [
            "curl -fsSL 'https://orch.test/api/v1/downloads/install.sh' | sh -s -- --url 'https://orch.test' --token 'oenr_private' --delegate-policy",
            "& ([scriptblock]::Create((iwr 'https://orch.test/api/v1/downloads/install.ps1' -UseBasicParsing))) -Url 'https://orch.test' -Token 'oenr_private' -DelegatePolicy",
        ] {
            let parsed = parse(text).unwrap();
            assert_eq!(parsed.address, "https://orch.test");
            assert_eq!(parsed.token.as_deref(), Some("oenr_private"));
            assert!(parsed.delegate_policy);
            assert!(!format!("{parsed:?}").contains("oenr_private"));
        }
        assert!(parse("https://orch.test").unwrap().token.is_none());
        assert_eq!(
            parse("https://orch.test oenr_private")
                .unwrap()
                .token
                .as_deref(),
            Some("oenr_private")
        );
    }

    #[test]
    fn a_role_enrollment_paste_preserves_its_exact_local_consent_without_executing_the_command() {
        for text in [
            "install.sh --url https://orch.test --token secret --roles environment,provider --delegate environment --dedicated",
            "install.ps1 -Url https://orch.test -Token secret -Roles environment,provider -Delegate environment -Dedicated",
        ] {
            let choices = parse(text).unwrap();
            assert_eq!(choices.roles.as_deref(), Some("environment,provider"));
            assert_eq!(choices.delegate.as_deref(), Some("environment"));
            assert!(choices.dedicated);
            assert!(!choices.node);
            assert!(!format!("{choices:?}").contains("secret"));
        }
        assert!(
            parse("install-node.sh --url wss://gw.test --token secret")
                .unwrap()
                .node
        );
    }

    #[test]
    fn a_paste_never_executes_commands_or_reveals_a_refused_token() {
        let parsed=parse("orch-connector enroll --url https://orch.test --token '$(do-not-run)' --name 'a shell; remains data' --no-service").unwrap();
        assert_eq!(parsed.token.as_deref(), Some("$(do-not-run)"));
        assert_eq!(parsed.name.as_deref(), Some("a shell; remains data"));
        assert!(parsed.no_service);
        assert!(
            parse("orch-connector enroll --url https://orch.test --token secret --token another")
                .is_err()
        );
        assert!(
            !parse("orch-connector enroll --url http://orch.test --token secret")
                .unwrap_err()
                .to_string()
                .contains("secret")
        );
    }
}
