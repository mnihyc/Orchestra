//! Session, lease and terminal ticket issuance.
//!
//! Who may open a tunnel to whom is decided here and nowhere else. A ticket
//! names the one who opens it, the machine it ends at and the service there,
//! and is signed with a key that is made when the system starts; its public
//! half is given to every gateway over the backplane, which checks tickets
//! without asking anybody. A ticket opens channels until it expires; a channel
//! that is open outlives it.
//!
//! The command a harness's machine is given carries its tickets as requests,
//! not as tickets (`{"$ticket": {"kind": "llm", …}}`), and each time that
//! command is put on the wire the request is replaced by a ticket signed
//! then. What the request may say beside its kind is read here:
//!
//! | Member | Meaning |
//! |--------|---------|
//! | `service` | The service at the destination, as a ticket names it (`exec/codex`, `llm/<lease>`, `mcp/<server>`). Required. |
//! | `dst` | The machine the tunnel ends at. Required. |
//! | `src` | Who opens it: a machine, or `api`. The machine the command goes to when absent. |
//! | `thread_id` | The thread it serves. |
//! | `tool_set` | The tool set an executor lists on every channel of the ticket. |
//! | `environment_id` | The environment whose executor or tool server the ticket reaches. |
//! | `environment_generation` | The generation of that environment. |
//! | `ttl_secs` | How long channels may be opened with it. [`DEFAULT_TICKET_SECS`] when absent. |

use orch_link_contract::commands::{Object, TicketWanted};
use orch_link_contract::tunnels::{Service, TicketSource, TunnelKind};
use orch_pki::PkiError;
use orch_pki::tickets::{TicketRequest, TicketSigner, TicketVerifier};
use orch_types::{Clock, Id, Json, Time};
use std::sync::Arc;
use std::time::Duration;

/// How long channels may be opened with a ticket whose request does not say.
/// Twelve hours: a turn can run for hours, the system signs a new ticket
/// with every command that needs one, and a ticket only opens channels, so
/// one that is open does not end with it.
pub const DEFAULT_TICKET_SECS: u64 = 12 * 3_600;
/// How long the ticket of a terminal can be used to open its tunnel. A
/// browser attaches to a shell at once, and attaches anew when it must: a
/// minute is a few round trips of a slow line, and no longer than a stolen
/// ticket would be worth.
pub const TERMINAL_TICKET_SECS: u64 = 60;

/// Why a ticket was not made.
#[derive(Debug, thiserror::Error)]
pub enum TicketFault {
    /// The request lacks a member or has one that is not what it should be.
    #[error("the ticket request {0}")]
    Request(String),
    /// The signature could not be made.
    #[error("{0}")]
    Signing(#[from] PkiError),
}

/// The system's way to sign tickets.
pub struct Tickets {
    signer: TicketSigner,
    clock: Arc<dyn Clock>,
}

impl std::fmt::Debug for Tickets {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Tickets").finish_non_exhaustive()
    }
}

impl Tickets {
    /// Opens one independently authenticated display transport for one viewer.
    pub fn display(&self, machine: Id, display: Id) -> Result<String, TicketFault> {
        self.issue(&TicketRequest {
            src: TicketSource::Api,
            dst: machine,
            service: Service::Rfb {
                display_id: display,
            },
            thread_id: None,
            tool_set: None,
            expires_at: self.expires_in(TERMINAL_TICKET_SECS),
        })
    }
    /// The API opens one unretained stream for all viewers of a browser.
    pub fn browser(&self, machine: Id, browser: Id) -> Result<String, TicketFault> {
        self.issue(&TicketRequest {
            src: TicketSource::Api,
            dst: machine,
            service: Service::Browser {
                browser_id: browser,
            },
            thread_id: None,
            tool_set: None,
            expires_at: self.expires_in(TERMINAL_TICKET_SECS),
        })
    }

    /// Signs only the guarded server's short-lived port admission and actual ingress facts.
    pub fn port_use(
        &self,
        id: Id,
        machine: Id,
        port: orch_link_contract::ports::PortTicketClaims,
        expires: Time,
    ) -> Result<String, TicketFault> {
        Ok(self
            .signer
            .sign(&orch_link_contract::tunnels::TicketClaims {
                jti: id,
                kind: TunnelKind::Port,
                src: TicketSource::Api,
                dst: machine,
                service: Service::Port { use_id: id }.to_string(),
                thread_id: None,
                provider_id: None,
                tool_set: None,
                tool_environment_id: None,
                tool_environment_generation: None,
                port: Some(port),
                exp: expires.unix_millis().div_euclid(1_000),
            })?)
    }
    /// Tickets signed with `signer`, expiring by `clock`.
    #[must_use]
    pub fn new(signer: TicketSigner, clock: Arc<dyn Clock>) -> Self {
        Self { signer, clock }
    }

    /// Signs the ticket a host's node (`node`) opens one transport's channels of a host opening with,
    /// toward the environment on `machine`, until `expires`: the opening's own facts, its allocation, and
    /// the host as the place its callers come in at.
    pub fn port_publish(
        &self,
        node: Id,
        machine: Id,
        port: orch_link_contract::ports::PortTicketClaims,
        expires: Time,
    ) -> Result<String, TicketFault> {
        let jti = orch_types::new_id();
        Ok(self
            .signer
            .sign(&orch_link_contract::tunnels::TicketClaims {
                jti,
                kind: TunnelKind::Port,
                src: TicketSource::Machine(node),
                dst: machine,
                service: Service::Port { use_id: jti }.to_string(),
                thread_id: None,
                provider_id: None,
                tool_set: None,
                tool_environment_id: None,
                tool_environment_generation: None,
                port: Some(port),
                exp: expires.unix_millis().div_euclid(1_000),
            })?)
    }

    /// A new signing key, as the system makes one each time it starts.
    #[must_use]
    pub fn generate(clock: Arc<dyn Clock>) -> Self {
        Self::new(TicketSigner::generate(), clock)
    }

    /// The public half, as the gateways are given it.
    #[must_use]
    pub fn public_key(&self) -> String {
        self.signer.public_key_text()
    }

    /// The public half, to check tickets with.
    #[must_use]
    pub fn verifier(&self) -> TicketVerifier {
        self.signer.verifier()
    }

    fn expires_in(&self, seconds: u64) -> Time {
        Time::new(
            self.clock.now()
                + chrono::Duration::from_std(Duration::from_secs(seconds)).unwrap_or_default(),
        )
    }

    /// A ticket for `request`.
    pub fn issue(&self, request: &TicketRequest) -> Result<String, TicketFault> {
        Ok(self.signer.issue(request)?)
    }

    /// The ticket with which the system opens the tunnel of a terminal to
    /// `machine`: the source is the system itself, because the other end of
    /// the tunnel is a person's browser.
    pub fn terminal(&self, machine: Id, service: Service) -> Result<String, TicketFault> {
        if service.kind() != TunnelKind::Pty {
            return Err(TicketFault::Request(
                "is for a terminal and names another kind of service".to_owned(),
            ));
        }
        self.issue(&TicketRequest {
            src: TicketSource::Api,
            dst: machine,
            service,
            thread_id: None,
            tool_set: None,
            expires_at: self.expires_in(TERMINAL_TICKET_SECS),
        })
    }

    /// The ticket that a request in a stored command stands for, signed now.
    /// `machine` is the machine the command goes to, which opens the tunnel
    /// unless the request names another.
    pub fn for_request(&self, wanted: &TicketWanted, machine: Id) -> Result<String, TicketFault> {
        let more = &wanted.more;
        let text = |member: &str| -> Result<Option<&str>, TicketFault> {
            match more.get(member) {
                None | Some(Json::Null) => Ok(None),
                Some(Json::String(text)) => Ok(Some(text.as_str())),
                Some(_) => Err(TicketFault::Request(format!(
                    "has a `{member}` that is not text"
                ))),
            }
        };
        let id = |member: &str| -> Result<Option<Id>, TicketFault> {
            text(member)?
                .map(|text| {
                    text.parse::<Id>().map_err(|_| {
                        TicketFault::Request(format!("has a `{member}` that is not an identifier"))
                    })
                })
                .transpose()
        };
        let service: Service = text("service")?
            .ok_or_else(|| TicketFault::Request("lacks its `service`".to_owned()))?
            .parse()
            .map_err(|_| TicketFault::Request("names a service that does not exist".to_owned()))?;
        if service.kind() != wanted.kind {
            return Err(TicketFault::Request(
                "asks for one kind of tunnel and names the service of another".to_owned(),
            ));
        }
        let dst = id("dst")?.ok_or_else(|| TicketFault::Request("lacks its `dst`".to_owned()))?;
        let src = match text("src")? {
            None => TicketSource::Machine(machine),
            Some(source) => serde_json::from_value(Json::String(source.to_owned()))
                .map_err(|_| TicketFault::Request("has a `src` that names nobody".to_owned()))?,
        };
        let seconds = match more.get("ttl_secs") {
            None | Some(Json::Null) => DEFAULT_TICKET_SECS,
            Some(value) => value.as_u64().ok_or_else(|| {
                TicketFault::Request("has a `ttl_secs` that is not a number of seconds".to_owned())
            })?,
        };
        let provider = id("provider_id")?;
        let thread_id = id("thread_id")?;
        if provider.is_some() && wanted.kind != TunnelKind::Llm {
            return Err(TicketFault::Request(
                "names a provider for a tunnel that is not a model tunnel".to_owned(),
            ));
        }
        if provider.is_some() && thread_id.is_none() {
            return Err(TicketFault::Request(
                "names a provider without its thread".to_owned(),
            ));
        }
        let request = TicketRequest {
            src,
            dst,
            service,
            thread_id,
            tool_set: text("tool_set")?.map(str::to_owned),
            expires_at: self.expires_in(seconds),
        };
        if matches!(wanted.kind, TunnelKind::Exec | TunnelKind::Mcp) {
            let environment = id("environment_id")?
                .ok_or_else(|| TicketFault::Request("lacks its `environment_id`".to_owned()))?;
            let generation = more
                .get("environment_generation")
                .and_then(Json::as_i64)
                .filter(|value| *value >= 0)
                .ok_or_else(|| {
                    TicketFault::Request("lacks a nonnegative `environment_generation`".to_owned())
                })?;
            Ok(self
                .signer
                .issue_for_tool(&request, environment, generation)?)
        } else if let Some(provider) = provider {
            Ok(self.signer.issue_for_provider(&request, provider)?)
        } else {
            self.issue(&request)
        }
    }
}

/// A request for a ticket of `kind`, as a stored command holds it, for the
/// code that makes commands.
#[must_use]
pub fn request(
    kind: TunnelKind,
    service: &Service,
    dst: Id,
    thread_id: Option<Id>,
) -> TicketWanted {
    let mut more = Object::new();
    more.insert("service".to_owned(), Json::String(service.to_string()));
    more.insert("dst".to_owned(), Json::String(dst.to_string()));
    if let Some(thread) = thread_id {
        more.insert("thread_id".to_owned(), Json::String(thread.to_string()));
    }
    TicketWanted { kind, more }
}

#[cfg(test)]
mod tests {
    use super::*;
    use orch_link_contract::tunnels::OpenErrorCode;
    use orch_types::{ManualClock, new_id};

    fn tickets() -> (Tickets, ManualClock) {
        let clock = ManualClock::new(orch_testkit::clock::start());
        (Tickets::generate(Arc::new(clock.clone())), clock)
    }

    #[test]
    fn a_host_publication_verifies_as_its_named_node_and_keeps_the_opening_separate_from_its_ticket()
     {
        use orch_api_contract::ports::{
            PortCaller, PortConnectionOutside, PortOpeningKind, PortTransport,
        };
        use orch_link_contract::ports::{HostPortTemplate, PortTicketClaims};
        let (tickets, clock) = tickets();
        let (node, machine, environment, opening, host) =
            (new_id(), new_id(), new_id(), new_id(), new_id());
        let signed = tickets
            .port_publish(
                node,
                machine,
                PortTicketClaims {
                    environment_id: environment,
                    generation: 1,
                    port: 3000,
                    transport: PortTransport::Tcp,
                    opening_id: Some(opening),
                    audience: Some(PortOpeningKind::Host),
                    authority_revision: 2,
                    allocation_revision: Some(1),
                    host_template: Some(HostPortTemplate {
                        host_id: host,
                        proxy_protocol: true,
                    }),
                    source: PortConnectionOutside {
                        address: "host.example".into(),
                        port: 41001,
                        caller: PortCaller::Address,
                    },
                },
                tickets.expires_in(DEFAULT_TICKET_SECS),
            )
            .unwrap();
        let claims = tickets
            .verifier()
            .verify(&signed, TicketSource::Machine(node), clock.time())
            .unwrap();
        assert_eq!(claims.dst, machine);
        assert_eq!(claims.port.as_ref().unwrap().opening_id, Some(opening));
        assert_eq!(
            claims.port.as_ref().unwrap().audience,
            Some(PortOpeningKind::Host)
        );
        assert_eq!(
            claims.service,
            Service::Port { use_id: claims.jti }.to_string()
        );
        assert_eq!(
            tickets
                .verifier()
                .verify(&signed, TicketSource::Machine(new_id()), clock.time()),
            Err(OpenErrorCode::BadTicket)
        );
    }

    #[test]
    fn a_request_in_a_stored_command_becomes_the_ticket_of_the_machine_it_goes_to() {
        let (tickets, clock) = tickets();
        let (agent, environment, lease) = (new_id(), new_id(), new_id());
        let wanted = request(
            TunnelKind::Llm,
            &Service::Llm { lease_id: lease },
            environment,
            None,
        );
        let ticket = tickets.for_request(&wanted, agent).unwrap();
        let claims = tickets
            .verifier()
            .verify(
                &ticket,
                TicketSource::Machine(agent),
                Time::new(clock.now()),
            )
            .unwrap();
        assert_eq!(claims.dst, environment);
        assert_eq!(claims.service, format!("llm/{lease}"));
        assert_eq!(
            claims.exp,
            Time::new(clock.now()).unix_millis() / 1_000
                + i64::try_from(DEFAULT_TICKET_SECS).unwrap()
        );
    }

    #[test]
    fn a_model_ticket_keeps_the_provider_and_thread_the_command_names() {
        let (tickets, clock) = tickets();
        let (agent, keeper, thread, provider, lease) =
            (new_id(), new_id(), new_id(), new_id(), new_id());
        let mut wanted = request(
            TunnelKind::Llm,
            &Service::Llm { lease_id: lease },
            keeper,
            Some(thread),
        );
        wanted
            .more
            .insert("provider_id".to_owned(), Json::String(provider.to_string()));
        let signed = tickets.for_request(&wanted, agent).unwrap();
        let claims = tickets
            .verifier()
            .verify(
                &signed,
                TicketSource::Machine(agent),
                Time::new(clock.now()),
            )
            .unwrap();
        assert_eq!(claims.provider_id, Some(provider));
        assert_eq!(claims.thread_id, Some(thread));
        assert_eq!(claims.service, format!("llm/{lease}"));
    }

    #[test]
    fn a_ticket_is_good_for_nobody_but_the_machine_it_was_signed_for() {
        let (tickets, clock) = tickets();
        let (agent, other, environment) = (new_id(), new_id(), new_id());
        let mut wanted = request(
            TunnelKind::Exec,
            &Service::Exec {
                executor: "codex".into(),
            },
            environment,
            None,
        );
        wanted
            .more
            .insert("environment_id".into(), Json::String(new_id().to_string()));
        wanted
            .more
            .insert("environment_generation".into(), Json::from(0));
        let ticket = tickets.for_request(&wanted, agent).unwrap();
        let verifier = tickets.verifier();
        assert_eq!(
            verifier.verify(
                &ticket,
                TicketSource::Machine(other),
                Time::new(clock.now())
            ),
            Err(OpenErrorCode::BadTicket)
        );
    }

    #[test]
    fn a_tool_ticket_signs_one_environment_and_refuses_an_unscoped_request() {
        use orch_link_contract::tunnels::Ticket;

        let (tickets, clock) = tickets();
        let (agent, machine, environment, other) = (new_id(), new_id(), new_id(), new_id());
        for service in [
            Service::Exec {
                executor: "codex".into(),
            },
            Service::Mcp {
                server: "files".into(),
            },
        ] {
            let mut wanted = request(service.kind(), &service, machine, Some(new_id()));
            assert!(tickets.for_request(&wanted, agent).is_err());
            wanted.more.insert(
                "environment_id".into(),
                Json::String(environment.to_string()),
            );
            assert!(tickets.for_request(&wanted, agent).is_err());
            wanted
                .more
                .insert("environment_generation".into(), Json::from(3));
            let signed = tickets.for_request(&wanted, agent).unwrap();
            let verifier = tickets.verifier();
            let presenter = TicketSource::Machine(agent);
            let claims = verifier
                .verify(&signed, presenter, Time::new(clock.now()))
                .unwrap();
            assert_eq!(claims.tool_environment_id, Some(environment));
            assert_eq!(claims.tool_environment_generation, Some(3));
            let original = Ticket::parse(&signed).unwrap();
            let mut changed = claims;
            changed.tool_environment_id = Some(other);
            let altered = Ticket::assemble(
                &Ticket::signing_input(&changed).unwrap(),
                original.signature(),
            );
            assert_eq!(
                verifier.verify(&altered, presenter, Time::new(clock.now())),
                Err(OpenErrorCode::BadTicket)
            );
            changed.tool_environment_id = Some(environment);
            changed.tool_environment_generation = Some(4);
            let altered = Ticket::assemble(
                &Ticket::signing_input(&changed).unwrap(),
                original.signature(),
            );
            assert_eq!(
                verifier.verify(&altered, presenter, Time::new(clock.now())),
                Err(OpenErrorCode::BadTicket)
            );
        }
    }

    #[test]
    fn a_ticket_for_a_terminal_is_the_systems_own_and_short_lived() {
        let (tickets, clock) = tickets();
        let (machine, terminal) = (new_id(), new_id());
        let ticket = tickets
            .terminal(
                machine,
                Service::Pty {
                    terminal_id: terminal,
                },
            )
            .unwrap();
        let verifier = tickets.verifier();
        let now = Time::new(clock.now());
        let claims = verifier.verify(&ticket, TicketSource::Api, now).unwrap();
        assert_eq!((claims.dst, claims.kind), (machine, TunnelKind::Pty));
        clock.advance(chrono::Duration::seconds(
            i64::try_from(TERMINAL_TICKET_SECS).unwrap() + 1,
        ));
        assert_eq!(
            verifier.verify(&ticket, TicketSource::Api, Time::new(clock.now())),
            Err(OpenErrorCode::Expired)
        );
    }

    #[test]
    fn a_request_that_cannot_be_a_ticket_says_what_is_wrong_with_it() {
        let (tickets, _) = tickets();
        let machine = new_id();
        let mut wanted = request(
            TunnelKind::Mcp,
            &Service::Mcp {
                server: "tmux".into(),
            },
            new_id(),
            None,
        );
        wanted.more.remove("dst");
        assert!(
            tickets
                .for_request(&wanted, machine)
                .unwrap_err()
                .to_string()
                .contains("dst")
        );
        let wrong_kind = request(
            TunnelKind::Exec,
            &Service::Mcp {
                server: "tmux".into(),
            },
            new_id(),
            None,
        );
        assert!(tickets.for_request(&wrong_kind, machine).is_err());
        assert!(
            tickets
                .terminal(machine, Service::Llm { lease_id: new_id() })
                .is_err()
        );
    }
}
