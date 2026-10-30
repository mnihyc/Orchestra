//! Port audiences admitted by the environment's capability grants and current policy.
//!
//! Who may be let in to an environment's ports is a capability of the environment, one for each kind of opening:
//! `port_place` (the viewers of a named place), `port_host` (whoever reaches the host's address) and `port_link`
//! (whoever holds a link). Orch's gateway gives them, so a grant or its revocation changes no unit. They stand in
//! the same lists as every other capability (a template allows them and may give them from creation) and under the
//! same policy, `capability.policy` at the environment's home: `allow`, `ask` or `deny`, with an approver scope
//! (`environment`: the holders of Ports Manage on the environment).
//!
//! What follows from it: a person who holds Ports Manage opens any kind the policy does not deny; an agent opens a
//! kind without a card where the environment holds it (granted, or from its template) or the policy allows it, and
//! asks on a card otherwise; nobody opens a denied kind, and a kind turned to `deny` closes what is open of it.
//! Taking a grant back, or the policy going from `allow` to `ask`, stops later automatic openings and closes none
//! made before. Each automatic opening keeps, unchanged, which grant, template or policy admitted it and whose
//! decision that was ([`Gate::admission`]).

use crate::ports::{Error, holds};
use chrono::{DateTime, Utc};
use orch_access_contract::audit::AuditAction;
use orch_access_contract::decisions::ObjectRef;
use orch_access_contract::permissions::{ObjectKind, Permission};
use orch_api_contract::ports::PortOpeningKind;
use orch_authz::{Act, Policy, Principal, nodes, resources};
use orch_db::Conn;
use orch_db::audit::{self, Actor, Entry, Origin};
use orch_db::queries::environments::capabilities as rows;
use orch_settings::{Place, Snapshot, Wanted};
use orch_settings_contract::registry::{self, keys};
use orch_types::{Id, Json};
use serde_json::json;

/// The three kinds of opening, in the order the Ports view shows them.
pub const KINDS: [PortOpeningKind; 3] = [
    PortOpeningKind::Place,
    PortOpeningKind::Link,
    PortOpeningKind::Host,
];

/// The gateway capability that admits this audience; the driver never receives a patch for it.
#[must_use]
pub const fn key(kind: PortOpeningKind) -> &'static str {
    match kind {
        PortOpeningKind::Place => "port_place",
        PortOpeningKind::Host => "port_host",
        PortOpeningKind::Link => "port_link",
    }
}

/// The audience a capability key admits, for the three port keys.
#[must_use]
pub fn kind_of_key(key: &str) -> Option<PortOpeningKind> {
    KINDS.into_iter().find(|kind| self::key(*kind) == key)
}

/// The audience of a stored opening's kind word (`place`, `link` or `host`).
#[must_use]
pub fn kind_of(word: &str) -> PortOpeningKind {
    match word {
        "place" => PortOpeningKind::Place,
        "host" => PortOpeningKind::Host,
        _ => PortOpeningKind::Link,
    }
}

/// One audience's capability on one environment as it stands now: the same facts for a person's opening, an
/// agent's request and the Ports view.
#[derive(Debug, Clone)]
pub struct Gate {
    /// The driver's actual support, kept separate from this audience's permission.
    pub driver_support: orch_driver_contract::descriptor::OperationSupport,
    /// The audience.
    pub kind: PortOpeningKind,
    /// The capability key.
    pub key: String,
    /// What the capability is called.
    pub label: String,
    /// `low`, `medium`, `high` or `critical`.
    pub risk: String,
    /// Whether the environment's template and the entry its creator used on it allow the capability; a connected
    /// environment has no template, and its machine's own Ports ceiling governs it instead.
    pub offered: bool,
    /// Whether it was granted to this environment: taken back on the Ports or Capabilities panel.
    pub granted: bool,
    /// Whether the template gives it from creation.
    pub from_template: bool,
    /// `allow`, `ask` or `deny`: `capability.policy` at the environment's home, else the definition's default.
    pub decision: String,
    /// Whose approvers decide a grant: `environment`, `host`, `org` or `system`.
    pub approver_scope: String,
    /// The scope of the policy entry that decides it (`default` where no place set one).
    pub policy_scope: String,
    /// The place whose policy row decides it; none for the system and for the definition's own default.
    pub policy_node: Option<Id>,
    /// Who wrote that policy row last.
    pub policy_by: Option<Id>,
    /// Who gave the grant the last time it was given, where a person did.
    pub granted_by: Option<Id>,
    /// When it was given.
    pub granted_at: Option<DateTime<Utc>>,
}

impl Gate {
    /// Whether the environment holds the capability: granted, or from its template.
    #[must_use]
    pub const fn held(&self) -> bool {
        self.granted || self.from_template
    }

    /// Whether this kind may be opened at all: allowed by the template, and not denied by the policy.
    #[must_use]
    pub fn permits(&self) -> bool {
        self.offered && self.decision != "deny"
    }

    /// Whether an agent's request of this kind opens without a card: the environment holds it, or the policy
    /// allows it.
    #[must_use]
    pub fn automatic(&self) -> bool {
        self.permits() && (self.held() || self.decision == "allow")
    }

    /// Whether a card for this kind may offer "Grant to this environment": the capability can be granted, and the
    /// card's own deciders (Ports Manage) are its approvers.
    #[must_use]
    pub fn card_can_grant(&self) -> bool {
        self.permits() && !self.granted && self.approver_scope == "environment"
    }

    /// What an agent's opening admitted now without a card keeps of its admission, unchanged: the grant (and whose
    /// decision it was), else the template default, else the policy (where it is set and who set it). None where
    /// a request of this kind needs a person.
    #[must_use]
    pub fn admission(&self) -> Option<Json> {
        if !self.automatic() {
            return None;
        }
        Some(if self.granted {
            json!({ "capability": self.key, "source": "grant", "by_user_id": self.granted_by, "at": self.granted_at })
        } else if self.from_template {
            json!({ "capability": self.key, "source": "template" })
        } else {
            json!({ "capability": self.key, "source": "policy", "scope": self.policy_scope,
                    "node_id": self.policy_node, "by_user_id": self.policy_by })
        })
    }
}

fn listed(value: Option<&Json>, key: &str) -> bool {
    value
        .and_then(Json::as_array)
        .is_some_and(|values| values.iter().any(|value| value.as_str() == Some(key)))
}

/// What a gate needs of a capability's definition.
struct Definition {
    label: String,
    risk: String,
    default_decision: String,
    default_approver_scope: String,
}

/// Resolves one audience's capability on an environment: its allowlist and grant, and the ordinary capability
/// policy at the environment's home. Before the definitions are seeded the shipped one stands in; a capability
/// defined nowhere admits nothing.
pub async fn gate(conn: &mut Conn, environment: Id, kind: PortOpeningKind) -> Result<Gate, Error> {
    let key = key(kind);
    let facts = rows::environment_facts(conn, environment)
        .await?
        .filter(|facts| facts.deleted_at.is_none())
        .ok_or(Error::NotFound)?;
    let definition = match rows::definition(conn, key).await? {
        Some(row) => Some(Definition {
            label: row.label,
            risk: row.risk,
            default_decision: row.default_decision,
            default_approver_scope: row.default_approver_scope,
        }),
        None => orch_db::seeds::shipped_capability(key).map(|shipped| Definition {
            label: shipped.label.to_owned(),
            risk: shipped.risk.to_owned(),
            default_decision: shipped.decision.to_owned(),
            default_approver_scope: shipped.approver_scope.to_owned(),
        }),
    };
    let Some(definition) = definition else {
        return Ok(Gate {
            driver_support: orch_driver_contract::descriptor::OperationSupport::available(
                false,
                "this capability is not defined",
            ),
            kind,
            key: key.to_owned(),
            label: key.to_owned(),
            risk: "high".to_owned(),
            offered: false,
            granted: false,
            from_template: false,
            decision: "deny".to_owned(),
            approver_scope: "system".to_owned(),
            policy_scope: "default".to_owned(),
            policy_node: None,
            policy_by: None,
            granted_by: None,
            granted_at: None,
        });
    };
    let spec = facts.template_spec.as_ref();
    let machine = orch_db::queries::environment_facts::of(conn, environment).await?;
    let driver_support = if kind == PortOpeningKind::Host {
        crate::ports::host_listener_support_from(
            machine
                .as_ref()
                .and_then(|machine| machine.descriptor.as_ref()),
        )
    } else {
        orch_driver_contract::descriptor::OperationSupport::available(true, "")
    };
    let policy = machine
        .and_then(|m| m.policy)
        .and_then(|p| serde_json::from_value::<orch_link_contract::policy::LocalPolicy>(p).ok());
    let allowed = driver_support.supported
        && policy.is_some_and(|policy| policy.permits_port(Some(kind)))
        && spec
            .and_then(|spec| spec.get("allowed_capabilities"))
            .is_none_or(|allowed| listed(Some(allowed), key));
    let entry = facts
        .entry_options
        .as_ref()
        .and_then(|options| options.get("capabilities"));
    let offered = allowed && entry.is_none_or(|entry| listed(Some(entry), key));
    let granted = facts
        .granted_capabilities
        .iter()
        .any(|granted| granted == key);
    let from_template = listed(spec.and_then(|spec| spec.get("base_capabilities")), key);
    let def = registry::get(keys::CAPABILITY_POLICY).ok_or_else(|| {
        Error::Invalid("capability.policy is absent from the catalogue".to_owned())
    })?;
    let snapshot =
        Snapshot::load(conn, Place::Node(facts.home_node_id), Wanted::Keys(&[def])).await?;
    let resolved = snapshot.resolve(def);
    let configured = resolved
        .value
        .as_ref()
        .and_then(|table| table.get(key))
        .cloned();
    let word = |member: &str, fallback: &str| {
        configured
            .as_ref()
            .and_then(|entry| entry.get(member))
            .and_then(Json::as_str)
            .unwrap_or(fallback)
            .to_owned()
    };
    let decision = word("decision", &definition.default_decision);
    let approver_scope = word("approver_scope", &definition.default_approver_scope);
    // The nearest place whose row names this key decides it; values from places further up only fill in.
    let source = configured.as_ref().and_then(|_| {
        resolved
            .parts
            .iter()
            .rev()
            .find(|part| part.value.get(key).is_some())
            .map(|part| part.source.clone())
    });
    let (policy_scope, policy_node, policy_by) = match source {
        Some(source) if source.scope.lock().is_some() => {
            let row =
                orch_db::queries::settings::row(conn, source.node, keys::CAPABILITY_POLICY).await?;
            (
                source.scope.as_str().to_owned(),
                source.node,
                row.and_then(|row| row.updated_by),
            )
        }
        Some(source) => (source.scope.as_str().to_owned(), source.node, None),
        None => ("default".to_owned(), None, None),
    };
    let (granted_by, granted_at) = if granted {
        match orch_db::queries::ports::granted(conn, environment, key).await? {
            Some((by, at)) => (by, Some(at)),
            None => (None, None),
        }
    } else {
        (None, None)
    };
    Ok(Gate {
        driver_support,
        kind,
        key: key.to_owned(),
        label: definition.label,
        risk: definition.risk,
        offered,
        granted,
        from_template,
        decision,
        approver_scope,
        policy_scope,
        policy_node,
        policy_by,
        granted_by,
        granted_at,
    })
}

/// The three audiences' capabilities on an environment, in [`KINDS`] order.
pub async fn gates(conn: &mut Conn, environment: Id) -> Result<Vec<Gate>, Error> {
    let mut gates = Vec::with_capacity(KINDS.len());
    for kind in KINDS {
        gates.push(gate(conn, environment, kind).await?);
    }
    Ok(gates)
}

/// Who decides a grant of a capability whose approver scope is `scope`, in words for a refusal.
#[must_use]
pub fn approvers_in_words(scope: &str) -> &'static str {
    match scope {
        "environment" => "whoever holds Ports Manage on this environment",
        "host" => "whoever manages the environment's host",
        "org" => "the admins of the environment's home org",
        _ => "the system's admins",
    }
}

/// Whether `principal` is among the approvers of a grant whose scope is `scope`: the holders of Ports Manage on
/// the environment (`environment`), of Host Manage on its host (`host`), the admins of its home org (`org`); the
/// system's admins decide every scope and are the whole of `system`.
pub async fn may_grant(
    conn: &mut Conn,
    principal: &Principal,
    environment: Id,
    scope: &str,
) -> Result<bool, Error> {
    if principal.is_system_admin() {
        return Ok(true);
    }
    Ok(match scope {
        "environment" => holds(conn, principal, environment, Permission::PortsManage).await?,
        "host" => match rows::host_of(conn, environment).await? {
            Some((host, _)) => {
                match resources::check(
                    conn,
                    principal,
                    ObjectRef {
                        kind: ObjectKind::Host,
                        id: host,
                    },
                    Permission::HostManage,
                    Policy::default(),
                )
                .await
                {
                    Ok(decision) => decision.is_allowed(),
                    Err(orch_authz::Error::Suspended { .. } | orch_authz::Error::NotFound(_)) => {
                        false
                    }
                    Err(error) => return Err(error.into()),
                }
            }
            None => false,
        },
        "org" => {
            let facts = rows::environment_facts(conn, environment)
                .await?
                .ok_or(Error::NotFound)?;
            let org = orch_db::queries::threads::tools::nearest_org(conn, facts.home_node_id)
                .await?
                .unwrap_or(facts.org_id);
            match nodes::check(
                conn,
                principal,
                org,
                Permission::AccessManage,
                Act::Use,
                Policy::default(),
            )
            .await
            {
                Ok(decision) => decision.is_allowed(),
                Err(orch_authz::Error::Suspended { .. } | orch_authz::Error::NotFound(_)) => false,
                Err(error) => return Err(error.into()),
            }
        }
        _ => false,
    })
}

/// Writes a grant or a revocation of a port capability into the trail, as a line of the environment's history.
async fn record(
    conn: &mut Conn,
    environment: Id,
    by: Id,
    action: AuditAction,
    details: Json,
    origin: &Origin,
    at: DateTime<Utc>,
) -> Result<(), Error> {
    let facts = rows::environment_facts(conn, environment)
        .await?
        .ok_or(Error::NotFound)?;
    let summary = if action == AuditAction::CapabilityGranted {
        "A capability was given to an environment."
    } else {
        "A capability was taken back from an environment."
    };
    let mut entry = Entry::new(
        Actor::User(by),
        action.as_str(),
        "environment",
        Some(environment),
        summary,
    );
    entry.history = action.is_history_line();
    entry.environment_id = Some(environment);
    entry.node_id = Some(facts.home_node_id);
    entry.org_id = Some(facts.org_id);
    entry.details = details;
    audit::append(conn, &entry, origin, at).await?;
    Ok(())
}

/// Grants an environment one audience's capability, for a person among its approvers: the gateway gives it at
/// once and no unit changes. True when this call gave it; giving what is held already changes nothing. Refused
/// where the template does not allow it or the policy denies it.
pub async fn grant(
    conn: &mut Conn,
    principal: &Principal,
    environment: Id,
    kind: PortOpeningKind,
    origin: &Origin,
) -> Result<bool, Error> {
    let gate = gate(conn, environment, kind).await?;
    if !gate.offered {
        return Err(Error::NotAllowed(format!(
            "this environment's template does not allow \"{}\"",
            gate.label
        )));
    }
    if gate.decision == "deny" {
        return Err(Error::NotAllowed(format!(
            "\"{}\" is denied by capability.policy at this environment's place",
            gate.label
        )));
    }
    if !may_grant(conn, principal, environment, &gate.approver_scope).await? {
        return Err(Error::NotAllowed(format!(
            "\"{}\" is granted by {}",
            gate.label,
            approvers_in_words(&gate.approver_scope)
        )));
    }
    let now = principal.now().get();
    if !rows::grant(conn, environment, &gate.key, now).await? {
        return Ok(false);
    }
    record(
        conn,
        environment,
        principal.user(),
        AuditAction::CapabilityGranted,
        json!({ "capability": gate.key, "decided_by": principal.user() }),
        origin,
        now,
    )
    .await?;
    Ok(true)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn gate(decision: &str, offered: bool, granted: bool, from_template: bool) -> Gate {
        Gate {
            driver_support: orch_driver_contract::descriptor::OperationSupport::available(true, ""),
            kind: PortOpeningKind::Place,
            key: "port_place".to_owned(),
            label: "Ports for a place".to_owned(),
            risk: "medium".to_owned(),
            offered,
            granted,
            from_template,
            decision: decision.to_owned(),
            approver_scope: "environment".to_owned(),
            policy_scope: "org".to_owned(),
            policy_node: None,
            policy_by: None,
            granted_by: None,
            granted_at: None,
        }
    }

    #[test]
    fn an_agent_opens_without_a_card_only_what_is_held_or_allowed_and_never_what_is_denied() {
        assert!(gate("ask", true, true, false).automatic());
        assert!(gate("ask", true, false, true).automatic());
        assert!(gate("allow", true, false, false).automatic());
        assert!(
            !gate("ask", true, false, false).automatic(),
            "ask without a grant makes a card"
        );
        assert!(
            !gate("deny", true, true, false).automatic(),
            "a grant does not outlive a denying policy"
        );
        assert!(
            !gate("allow", false, false, false).automatic(),
            "a template that does not allow it admits nothing"
        );
        assert!(gate("ask", true, false, false).permits());
        assert!(!gate("deny", true, false, false).permits());
    }

    #[test]
    fn an_automatic_opening_names_the_grant_before_the_template_and_the_policy() {
        let by = orch_types::new_id();
        let mut granted = gate("allow", true, true, true);
        granted.granted_by = Some(by);
        let admission = granted.admission().unwrap();
        assert_eq!(admission["source"], "grant");
        assert_eq!(admission["by_user_id"], json!(by));
        assert_eq!(
            gate("allow", true, false, true).admission().unwrap()["source"],
            "template"
        );
        let policy = gate("allow", true, false, false).admission().unwrap();
        assert_eq!(policy["source"], "policy");
        assert_eq!(policy["scope"], "org");
        assert!(gate("ask", true, false, false).admission().is_none());
    }

    #[test]
    fn a_card_offers_the_grant_only_to_deciders_who_are_its_approvers() {
        assert!(gate("ask", true, false, false).card_can_grant());
        let mut org = gate("ask", true, false, false);
        org.approver_scope = "org".to_owned();
        assert!(!org.card_can_grant());
        assert!(!gate("ask", false, false, false).card_can_grant());
        assert!(
            !gate("ask", true, true, false).card_can_grant(),
            "a held grant is not given twice"
        );
    }

    #[test]
    fn each_port_key_names_one_audience() {
        for kind in KINDS {
            assert_eq!(kind_of_key(key(kind)), Some(kind));
        }
        assert_eq!(kind_of_key("kvm"), None);
        assert_eq!(kind_of("place"), PortOpeningKind::Place);
        assert_eq!(kind_of("host"), PortOpeningKind::Host);
        assert_eq!(kind_of("link"), PortOpeningKind::Link);
    }
}
