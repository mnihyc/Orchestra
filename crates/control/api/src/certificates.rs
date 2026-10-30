//! Enrollment, machine identity checks and renewal endpoints.
//!
//! A machine joins with a token that somebody made for it. The token's row
//! is locked while the request is judged, the request is judged before the
//! token is spent (a malformed request does not use it up), and the machine's
//! row, its host's row, the spent mark and the audit rows are written in the
//! one transaction that ends the request: a refusal leaves nothing behind.
//!
//! A certificate is renewed with a new key. The renewal is not held against
//! the machine's session: the new certificate is kept beside the old one
//! until a session shows it, and both are the machine's meanwhile, so that a
//! reply that never arrived does not lock a machine out.
//!
//! Machines reach these requests through a gateway, which has checked what it
//! can see (the certificate of the connection, or nothing) and says so in
//! headers that only a gateway's own certificate can send.

use crate::server::Peer;
use crate::state::App;
use axum::Router;
use axum::body::{Body, to_bytes};
use axum::extract::{Extension, State};
use axum::response::{IntoResponse, Response};
use axum::routing::post;
use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use chrono::{DateTime, Utc};
use http::{HeaderMap, HeaderValue, StatusCode, header};
use orch_access_contract::decisions::ObjectRef;
use orch_access_contract::permissions::ResourceKind;
use orch_authz::ownership::{self, NewAccess};
use orch_config::ServerConfig;
use orch_db::audit::{self, Actor, Entry, Origin};
use orch_db::queries::resources::{self, NewHost, NewMachine, NextCertificate, TokenRow};
use orch_db::{Db, DbError};
use orch_link_contract::enrollment::{
    Endpoint, EnrollRefusal, EnrollReply, EnrollReplySystem, EnrollRequest, EnrollmentToken,
    RenewReply, RenewRequest, Trust,
};
use orch_link_contract::{enrollment_choices::EnrollmentChoices, session::Role};
use orch_pki::PkiError;
use orch_pki::ca::{CertificateAuthority, Holder};
use orch_pki::certificates::{Certificate, CertificateRequest, Identity, ServiceName};
use orch_pki::enrollment::{
    EnrollError, TokenGrant, TokenLedger, TokenPurpose, enroll as pki_enroll, issue_token,
    token_digest,
};
use orch_pki::renewal::{
    MachineRecord, RenewalError, RenewalPolicy, RenewalProof, RenewalProofFault,
    renew as pki_renew, verify_proof,
};
use orch_pki::sealing::RecipientKey;
use orch_pki::verification::{OrchClientVerifier, crypto};
use orch_types::{Clock, Id, Time, new_id};
use rustls::pki_types::{CertificateDer, UnixTime};
use serde::Serialize;
use std::net::IpAddr;
use std::path::Path;
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

mod frontend;

/// The header in which a gateway gives the address a machine connected from.
pub const CLIENT_IP_HEADER: &str = "x-orch-client-ip";
/// The header in which a gateway gives the certificate a machine showed in
/// the connection it checked, as unpadded base64url of its DER.
pub const CLIENT_CERTIFICATE_HEADER: &str = "x-orch-client-certificate";
/// The header in which a gateway gives the host the machine connected to.
pub const FORWARDED_HOST_HEADER: &str = "x-orch-forwarded-host";
pub use orch_link_contract::enrollment::{
    RENEW_CERTIFICATE_HEADER, RENEW_SENT_AT_HEADER, RENEW_SIGNATURE_HEADER,
};

/// The most bytes of a request to enroll or renew: a certificate request is
/// a few hundred bytes, and nothing here is a place to put a file.
const MAX_REQUEST_BYTES: usize = 64 * 1024;
/// How long a token that the system registers for itself from a file is good
/// for. A day: the stack starts, the node's image may have to be pulled, and
/// a start that failed is tried again by hand; the token is used once.
pub const BOOTSTRAP_TOKEN_SECS: u64 = 24 * 3_600;
/// Seconds in a day, for the grace in which an expired certificate is still
/// heard.
const SECONDS_PER_DAY: u64 = 86_400;
/// Milliseconds in a second, for the times the certificate checks take.
const MILLIS_PER_SECOND: i64 = 1_000;
/// The longest name a machine has.
const MAX_NAME_CHARS: usize = 80;
/// What the token the system made for its own node is called in the audit
/// trail.
const BOOTSTRAP_HINT: &str = "bundled node";

/// What the certificates are made under.
#[derive(Debug, Clone)]
pub struct CertificateSettings {
    /// How long a certificate is good for, in days.
    pub leaf_validity_days: u64,
    /// How long after it ran out a machine's certificate is still renewed,
    /// in days.
    pub expired_renew_grace_days: u64,
    /// The names a service's certificate is good for.
    pub service_dns_names: Vec<String>,
    /// How long a token made by a person is good for.
    pub token_ttl: Duration,
    /// The addresses a machine is told it may hold its session with.
    pub endpoints: Vec<Endpoint>,
    /// What the system is called.
    pub system_name: String,
    /// The address people reach it at.
    pub public_url: String,
}

impl CertificateSettings {
    /// The settings a deployment's configuration gives.
    #[must_use]
    pub fn from_config(config: &ServerConfig) -> Self {
        Self {
            leaf_validity_days: config.pki.leaf_validity_days,
            expired_renew_grace_days: config.pki.expired_renew_grace_days,
            service_dns_names: config.pki.service_dns_names.clone(),
            token_ttl: Duration::from_secs(config.pki.enrollment_token_ttl_secs),
            endpoints: config
                .gateway
                .endpoints
                .iter()
                .map(|endpoint| Endpoint {
                    url: endpoint.url.clone(),
                    trust: if endpoint.trust == "public" {
                        Trust::Public
                    } else {
                        Trust::Pinned
                    },
                })
                .collect(),
            system_name: config.system.name.clone(),
            public_url: config.system.public_url.clone(),
        }
    }
}

/// Why a request to enroll or renew was refused, as it is answered to the
/// machine.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LinkFault {
    /// The HTTP status.
    pub status: u16,
    /// The stable code.
    pub code: &'static str,
    /// One sentence for whoever reads the log of the machine.
    pub detail: String,
}

impl LinkFault {
    pub(crate) fn new(status: u16, code: &'static str, detail: impl Into<String>) -> Self {
        Self {
            status,
            code,
            detail: detail.into(),
        }
    }

    /// A request that is not valid.
    #[must_use]
    pub fn invalid(detail: impl Into<String>) -> Self {
        Self::new(400, "request.invalid", detail)
    }

    /// A request that the caller may not make.
    #[must_use]
    pub fn forbidden(detail: impl Into<String>) -> Self {
        Self::new(403, "authz.denied", detail)
    }

    /// A failure on the system's side.
    #[must_use]
    pub fn internal(cause: &dyn std::fmt::Display) -> Self {
        tracing::error!(%cause, "a request of the link failed");
        Self::new(
            500,
            "internal",
            "something went wrong; the log has the cause",
        )
    }
}

impl From<EnrollRefusal> for LinkFault {
    fn from(refusal: EnrollRefusal) -> Self {
        Self::new(refusal.status(), refusal.code(), refusal.to_string())
    }
}

impl From<DbError> for LinkFault {
    fn from(error: DbError) -> Self {
        Self::internal(&error)
    }
}

impl From<PkiError> for LinkFault {
    fn from(error: PkiError) -> Self {
        Self::internal(&error)
    }
}

impl IntoResponse for LinkFault {
    fn into_response(self) -> Response {
        #[derive(Serialize)]
        struct Body<'a> {
            #[serde(rename = "type")]
            kind: String,
            code: &'a str,
            status: u16,
            detail: &'a str,
        }
        let body = serde_json::to_vec(&Body {
            kind: format!("orch:error:{}", self.code),
            code: self.code,
            status: self.status,
            detail: &self.detail,
        })
        .unwrap_or_default();
        let mut response = (
            StatusCode::from_u16(self.status).unwrap_or(StatusCode::INTERNAL_SERVER_ERROR),
            body,
        )
            .into_response();
        response.headers_mut().insert(
            header::CONTENT_TYPE,
            HeaderValue::from_static("application/problem+json"),
        );
        response
    }
}

/// What a token is made for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TokenFor {
    /// A person's machine.
    Machine,
    /// The same existing machine, with its ownership and registrations retained.
    Again {
        /// The machine that must receive the replacement certificate.
        machine: Id,
    },
    /// A host's node; the host's home is an org at any level, or none for
    /// the system's.
    Host {
        /// The org that will be the host's home.
        home: Option<Id>,
    },
    /// The connector inside the unit of a runtime that a host's node made: the
    /// machine that enrolls with it is managed, belongs to that runtime's host
    /// and is the runtime's own.
    Runtime {
        /// The runtime.
        runtime: Id,
    },
}

/// A token somebody asks for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TokenRequest {
    /// Who asks; becomes the owner for a new enrollment.
    pub created_by: Id,
    /// What it is for.
    pub purpose: TokenFor,
    /// What the machine is to be called, if the maker said.
    pub name_hint: Option<String>,
}

/// A token as it was just made: shown once, kept as a hash.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NewToken {
    /// The token, `oenr_…`.
    pub token: String,
    /// When it stops being good.
    pub expires_at: Time,
    /// One-way ledger reference, read only under the issuer's authenticated session.
    pub receipt: String,
}

/// What came of registering the token of a file.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Adopted {
    /// A row was made for it.
    Registered,
    /// It had a row already; if unused, it is good for another day.
    AlreadyKnown,
    /// There is no administrator yet to make it in the name of; it is
    /// registered when there is one.
    WaitingForAdministrator,
}

/// A caller of a renewal, and what is known of it.
#[derive(Debug, Clone)]
pub enum RenewCaller {
    /// A machine whose connection showed this certificate to a gateway.
    Certificate(Certificate),
    /// A machine that came through an edge that saw no certificate, and
    /// proves it holds the key of the one it gives.
    Proof {
        /// The proof.
        proof: RenewalProof,
        /// The host the machine connected to.
        host: String,
    },
}

/// The system's certificate authority and what is done with it.
#[derive(Clone)]
pub struct CertificateService {
    inner: Arc<Inner>,
}

struct Inner {
    db: Db,
    clock: Arc<dyn Clock>,
    authority: Arc<CertificateAuthority>,
    settings: CertificateSettings,
}

impl std::fmt::Debug for CertificateService {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CertificateService").finish_non_exhaustive()
    }
}

/// A one-entry ledger made from the row of the token being redeemed. Spending
/// is a mark here; the row is changed when the whole request commits.
struct RowLedger {
    digest: String,
    expires_at: Time,
    used: bool,
    grant: TokenGrant,
    spent: Mutex<bool>,
}

impl TokenLedger for RowLedger {
    fn redeem(&self, digest: &str, now: Time) -> Result<TokenGrant, EnrollRefusal> {
        let mut spent = self.spent.lock().unwrap_or_else(PoisonError::into_inner);
        if digest != self.digest || self.used || *spent || now >= self.expires_at {
            return Err(EnrollRefusal::Gone);
        }
        *spent = true;
        Ok(self.grant)
    }
}

/// The name of a value as the wire writes it.
fn wire<T: Serialize>(value: &T) -> String {
    match serde_json::to_value(value) {
        Ok(serde_json::Value::String(text)) => text,
        Ok(other) => other.to_string(),
        Err(_) => String::new(),
    }
}

fn raw_hash(digest_hex: &str) -> Vec<u8> {
    hex::decode(digest_hex).unwrap_or_default()
}

fn utc(time: Time) -> DateTime<Utc> {
    time.get()
}

impl CertificateService {
    /// The service for `authority`, keeping its rows in `db`.
    #[must_use]
    pub fn new(
        db: Db,
        clock: Arc<dyn Clock>,
        authority: Arc<CertificateAuthority>,
        settings: CertificateSettings,
    ) -> Self {
        Self {
            inner: Arc::new(Inner {
                db,
                clock,
                authority,
                settings,
            }),
        }
    }

    /// The authority.
    #[must_use]
    pub fn authority(&self) -> &CertificateAuthority {
        &self.inner.authority
    }

    /// The authority's certificate.
    #[must_use]
    pub fn authority_certificate(&self) -> &Certificate {
        self.inner.authority.certificate()
    }

    /// What the certificates are made under.
    #[must_use]
    pub fn settings(&self) -> &CertificateSettings {
        &self.inner.settings
    }

    /// Makes a token and stores its hash. The token is returned once; nothing
    /// that is stored lets it be read back.
    pub async fn issue_token(&self, request: &TokenRequest) -> Result<NewToken, LinkFault> {
        self.issue_token_choices(request, None, None).await
    }

    /// Issues a connector token bound to the roles and local handover its command names.
    pub async fn issue_machine_token(
        &self,
        request: &TokenRequest,
        choices: &EnrollmentChoices,
    ) -> Result<NewToken, LinkFault> {
        if !matches!(request.purpose, TokenFor::Machine) || choices.roles.contains(&Role::Node) {
            return Err(LinkFault::invalid(
                orch_link_contract::enrollment_choices::CONNECTOR_NODE_REFUSAL,
            ));
        }
        self.issue_token_choices(request, Some(choices), None).await
    }

    /// Issues a machine token at the home whose creation permission the route already checked.
    pub async fn issue_machine_token_at(
        &self,
        request: &TokenRequest,
        choices: Option<&EnrollmentChoices>,
        home: Id,
    ) -> Result<NewToken, LinkFault> {
        if !matches!(request.purpose, TokenFor::Machine)
            || choices.is_some_and(|choices| choices.roles.contains(&Role::Node))
        {
            return Err(LinkFault::invalid(
                "a machine token needs connector roles and its own home",
            ));
        }
        self.issue_token_choices(request, choices, Some(home)).await
    }

    async fn personal_machine_home(conn: &mut orch_db::Conn, owner: Id) -> Result<Id, LinkFault> {
        orch_db::queries::identity::user_by_id(conn, owner)
            .await?
            .and_then(|user| user.personal_org_id.or(user.service_org_id))
            .ok_or_else(|| LinkFault::invalid("a person's machine needs a home org"))
    }

    async fn issue_token_choices(
        &self,
        request: &TokenRequest,
        choices: Option<&EnrollmentChoices>,
        home: Option<Id>,
    ) -> Result<NewToken, LinkFault> {
        let mut issued = issue_token(
            &self.inner.authority,
            self.inner.settings.token_ttl,
            self.inner.clock.as_ref(),
        );
        if let Some(choices) = choices {
            issued.token = issued
                .token
                .with_choices(choices)
                .map_err(|_| LinkFault::invalid("the enrollment choices are not valid"))?;
            issued.digest = token_digest(issued.token.secret());
        }
        let (purpose, org_id) = match request.purpose {
            TokenFor::Machine => {
                let home = if let Some(home) = home {
                    home
                } else {
                    let mut conn = self.inner.db.acquire().await?;
                    Self::personal_machine_home(&mut conn, request.created_by).await?
                };
                (
                    if choices.is_some() {
                        "machine"
                    } else {
                        "connected"
                    },
                    Some(home),
                )
            }
            TokenFor::Again { machine } => {
                return reenrollment::token(self, request, machine, &issued).await;
            }
            TokenFor::Host { home } => ("host", home),
            TokenFor::Runtime { runtime } => {
                let mut conn = self.inner.db.acquire().await?;
                resources::hosting::insert_runtime_token(
                    &mut conn,
                    &raw_hash(&issued.digest),
                    request.created_by,
                    request.name_hint.as_deref(),
                    runtime,
                    utc(issued.expires_at),
                )
                .await?;
                return Ok(NewToken {
                    token: issued.token.to_string(),
                    expires_at: issued.expires_at,
                    receipt: issued.digest.clone(),
                });
            }
        };
        let row = TokenRow {
            token_hash: raw_hash(&issued.digest),
            created_by: request.created_by,
            purpose: purpose.to_owned(),
            org_id,
            name_hint: request.name_hint.clone(),
            expires_at: utc(issued.expires_at),
            used_at: None,
            machine_id: None,
            again_machine_id: None,
        };
        let mut conn = self.inner.db.acquire().await?;
        resources::insert_enrollment_token(&mut conn, &row).await?;
        if let Some(choices) = choices {
            resources::set_enrollment_roles(
                &mut conn,
                &row.token_hash,
                &choices.roles.iter().map(wire).collect::<Vec<_>>(),
            )
            .await?;
        }
        Ok(NewToken {
            token: issued.token.to_string(),
            expires_at: issued.expires_at,
            receipt: issued.digest.clone(),
        })
    }

    /// Stores a host token on the transaction that checked its creator's authority.
    pub(crate) async fn issue_host_token(
        &self,
        conn: &mut orch_db::Conn,
        request: &TokenRequest,
        access: Option<orch_api_contract::access::NewResourceAccess>,
    ) -> Result<NewToken, LinkFault> {
        self.issue_host_token_choices(conn, request, access, None)
            .await
    }

    /// Keeps a host's ordinary access choice and its explicit local policy handover on one token.
    pub(crate) async fn issue_host_token_choices(
        &self,
        conn: &mut orch_db::Conn,
        request: &TokenRequest,
        access: Option<orch_api_contract::access::NewResourceAccess>,
        choices: Option<&EnrollmentChoices>,
    ) -> Result<NewToken, LinkFault> {
        let TokenFor::Host { home } = request.purpose else {
            return Err(LinkFault::invalid("a host token needs a host purpose"));
        };
        let mut issued = issue_token(
            &self.inner.authority,
            self.inner.settings.token_ttl,
            self.inner.clock.as_ref(),
        );
        if let Some(choices) = choices {
            if choices.roles != [Role::Node] {
                return Err(LinkFault::invalid("the host token belongs to orch-node"));
            }
            issued.token = issued
                .token
                .with_choices(choices)
                .map_err(|_| LinkFault::invalid("the node enrollment choices are not valid"))?;
            issued.digest = token_digest(issued.token.secret());
        }
        if !resources::insert_enrollment_token(
            conn,
            &TokenRow {
                token_hash: raw_hash(&issued.digest),
                created_by: request.created_by,
                purpose: "host".to_owned(),
                org_id: home,
                name_hint: request.name_hint.clone(),
                expires_at: utc(issued.expires_at),
                used_at: None,
                machine_id: None,
                again_machine_id: None,
            },
        )
        .await?
        {
            return Err(LinkFault::internal(
                &"could not reserve an enrollment token",
            ));
        }
        resources::set_enrollment_roles(conn, &raw_hash(&issued.digest), &["node".into()]).await?;
        let choice = access.map(|access| match access {
            orch_api_contract::access::NewResourceAccess::Defaults => "defaults",
            orch_api_contract::access::NewResourceAccess::OnlyMe => "only_me",
            orch_api_contract::access::NewResourceAccess::Home => "home",
        });
        if !resources::set_host_enrollment_access(conn, &raw_hash(&issued.digest), choice).await? {
            return Err(LinkFault::internal(
                &"could not bind the host's enrollment access",
            ));
        }
        Ok(NewToken {
            token: issued.token.to_string(),
            expires_at: issued.expires_at,
            receipt: issued.digest.clone(),
        })
    }

    /// Registers the token that `compose-secrets` wrote for the bundled
    /// node, in the name of the first system administrator.
    pub async fn adopt_token_file(&self, path: &Path) -> Result<Adopted, LinkFault> {
        let text = std::fs::read_to_string(path).map_err(|error| {
            LinkFault::internal(&format!(
                "the token file {} cannot be read: {error}",
                path.display()
            ))
        })?;
        let token: EnrollmentToken = text
            .trim()
            .parse()
            .map_err(|_| LinkFault::invalid("the token file does not hold a token"))?;
        if !token.pins(&self.inner.authority.pin()) {
            return Err(LinkFault::invalid(
                "the token file was made under another authority",
            ));
        }
        let hash = raw_hash(&token_digest(token.secret()));
        let expires_at = utc(self.inner.clock.time())
            + chrono::Duration::seconds(i64::try_from(BOOTSTRAP_TOKEN_SECS).unwrap_or(i64::MAX));
        let mut conn = self.inner.db.acquire().await?;
        if resources::extend_unused_token(&mut conn, &hash, expires_at).await? {
            return Ok(Adopted::AlreadyKnown);
        }
        let Some(created_by) = resources::first_system_admin(&mut conn).await? else {
            return Ok(Adopted::WaitingForAdministrator);
        };
        let row = TokenRow {
            token_hash: hash,
            created_by,
            purpose: "host".to_owned(),
            org_id: None,
            name_hint: Some(BOOTSTRAP_HINT.to_owned()),
            expires_at,
            used_at: None,
            machine_id: None,
            again_machine_id: None,
        };
        let stored = resources::insert_enrollment_token(&mut conn, &row).await?;
        Ok(if stored {
            Adopted::Registered
        } else {
            Adopted::AlreadyKnown
        })
    }

    /// Enrolls a machine. `origin` says where the request came from, for the
    /// audit trail.
    pub async fn enroll(
        &self,
        request: &EnrollRequest,
        origin: &Origin,
    ) -> Result<EnrollReply, LinkFault> {
        let inner = &self.inner;
        let token: EnrollmentToken = request
            .token
            .parse()
            .map_err(|_| LinkFault::from(EnrollRefusal::Invalid))?;
        let hash = raw_hash(&token_digest(token.secret()));
        let mut tx = inner.db.begin().await?;
        let row = resources::lock_enrollment_token(&mut tx, &hash)
            .await?
            .ok_or_else(|| LinkFault::from(EnrollRefusal::Gone))?;
        if request.machine_id.is_some() && request.machine_id != row.again_machine_id {
            return Err(LinkFault::new(
                403,
                "enroll_again_required",
                "This token cannot replace the identity in this data directory. Use Enroll again on this machine's page.",
            ));
        }
        if (row.purpose == "host" && request.roles != [Role::Node])
            || (matches!(row.purpose.as_str(), "machine" | "connected")
                && request.roles.contains(&Role::Node))
        {
            return Err(LinkFault::invalid(
                orch_link_contract::enrollment_choices::CONNECTOR_NODE_REFUSAL,
            ));
        }
        if let Some(choices) = token
            .choices()
            .map_err(|_| LinkFault::from(EnrollRefusal::Invalid))?
        {
            let requested = EnrollmentChoices::new(
                request.roles.clone(),
                choices.delegate.clone(),
                choices.dedicated,
            )
            .map_err(|_| LinkFault::from(EnrollRefusal::Invalid))?;
            if !choices.matches(&requested) {
                return Err(LinkFault::invalid(
                    "the roles do not match this enrollment token",
                ));
            }
        }
        let ledger = RowLedger {
            digest: token_digest(token.secret()),
            expires_at: Time::new(row.expires_at),
            used: row.used_at.is_some(),
            grant: TokenGrant {
                purpose: if row.purpose == "host" {
                    TokenPurpose::Host
                } else {
                    TokenPurpose::Machine
                },
                created_by: row.created_by,
            },
            spent: Mutex::new(false),
        };
        let enrolled = pki_enroll(
            &inner.authority,
            &ledger,
            request,
            inner.settings.leaf_validity_days,
            inner.clock.as_ref(),
        )
        .map_err(|error| match error {
            EnrollError::Refused(refusal) => LinkFault::from(refusal),
            EnrollError::Internal(error) => LinkFault::from(error),
        })?;
        if let Some(machine) = row.again_machine_id {
            let reply =
                reenrollment::finish(self, &mut tx, &row, machine, enrolled, request, origin)
                    .await?;
            tx.commit().await.map_err(DbError::from)?;
            return Ok(reply);
        }
        let now = utc(inner.clock.time());
        let certificate = &enrolled.certificate;
        let name: String = request.name.trim().chars().take(MAX_NAME_CHARS).collect();
        let roles: Vec<String> = request.roles.iter().map(wire).collect();
        let host = row.purpose == "host";
        let runtime = if row.purpose == "runtime" {
            resources::hosting::runtime_of_token(&mut tx, &hash).await?
        } else {
            None
        };
        if row.purpose == "runtime" && runtime.is_none() {
            return Err(EnrollRefusal::Gone.into());
        }
        let managed = host || runtime.is_some();
        let serial = hex::encode(certificate.serial());
        let fingerprint = raw_hash(&certificate.fingerprint());
        let public_key = RecipientKey::from_certificate(certificate).to_text();
        resources::insert_machine(
            &mut tx,
            &NewMachine {
                id: enrolled.machine_id,
                name: &name,
                owner_id: runtime.is_none().then_some(row.created_by),
                placement: if managed { "managed" } else { "connected" },
                roles: &roles,
                platform_os: &wire(&request.platform.os),
                platform_arch: &wire(&request.platform.arch),
                connector_version: &request.connector_version,
                protocol_version: i32::try_from(enrolled.protocol_version).unwrap_or(1),
                cert_serial: &serial,
                cert_fingerprint: &fingerprint,
                cert_not_after: utc(certificate.not_after()),
                public_key: &public_key,
            },
            now,
        )
        .await?;
        if !host && runtime.is_none() {
            // Older unspent machine tokens carried no home. They still enroll privately at
            // their owner's personal org, rather than creating an owner with no root membership.
            let home = if let Some(home) = row.org_id {
                home
            } else {
                Self::personal_machine_home(&mut tx, row.created_by).await?
            };
            resources::routes::machine_home(&mut tx, enrolled.machine_id, Some(home)).await?;
            ownership::start_resource(
                &mut tx,
                ObjectRef {
                    kind: ResourceKind::Machine.into(),
                    id: enrolled.machine_id,
                },
                row.created_by,
                NewAccess::OnlyMe,
                Time::new(now),
            )
            .await
            .map_err(|error| LinkFault::internal(&error))?;
        }
        if !resources::spend_enrollment_token(&mut tx, &hash, enrolled.machine_id, now).await? {
            return Err(EnrollRefusal::Gone.into());
        }
        if let Some(runtime) = runtime
            && !resources::hosting::adopt_machine(&mut tx, enrolled.machine_id, runtime, now)
                .await?
        {
            // The unit it was made for is gone: the token opens nothing any more.
            return Err(EnrollRefusal::Gone.into());
        }
        let summary = format!("Machine {name} enrolled");
        let mut enrolled_entry = Entry::new(
            Actor::User(row.created_by),
            orch_db::audit::name!("machine.enrolled"),
            "machine",
            Some(enrolled.machine_id),
            &summary,
        );
        enrolled_entry.details = serde_json::json!({
            "placement": if managed { "managed" } else { "connected" },
            "roles": roles,
            "platform": { "os": wire(&request.platform.os), "arch": wire(&request.platform.arch) },
            "connector_version": request.connector_version,
        });
        audit::append(&mut tx, &enrolled_entry, origin, now).await?;
        if host {
            let access = match resources::host_enrollment_access(&mut tx, &hash)
                .await?
                .as_deref()
            {
                Some("only_me") => NewAccess::OnlyMe,
                Some("home") if row.org_id.is_some() => NewAccess::Home,
                None | Some("defaults") if row.org_id.is_none() => NewAccess::Everyone,
                None | Some("defaults") => NewAccess::Defaults,
                _ => {
                    return Err(LinkFault::invalid(
                        "the host token has an invalid access choice",
                    ));
                }
            };
            let host_id = new_id();
            resources::insert_host(
                &mut tx,
                &NewHost {
                    id: host_id,
                    machine_id: enrolled.machine_id,
                    owner_id: Some(row.created_by),
                    home_id: row.org_id,
                    name: &name,
                },
                now,
            )
            .await?;
            // Its page keeps the token maker as first owner and applies only the choice bound to that token.
            ownership::start_resource(
                &mut tx,
                ObjectRef {
                    kind: ResourceKind::Host.into(),
                    id: host_id,
                },
                row.created_by,
                access,
                Time::new(now),
            )
            .await
            .map_err(|error| LinkFault::internal(&error))?;
            let summary = format!("Host {name} enrolled");
            let mut created = Entry::new(
                Actor::User(row.created_by),
                orch_db::audit::name!("host.created"),
                "host",
                Some(host_id),
                &summary,
            );
            created.node_id = row.org_id;
            audit::append(&mut tx, &created, origin, now).await?;
        }
        if host || !managed {
            // Delivered only after this transaction commits. The receiving session reprojects
            // its resource permissions; no command, secret or canonical database row is published.
            orch_db::sequences::notify_user_event(
                &mut tx,
                &[row.created_by],
                &serde_json::json!({"what":"enrollment","receipt":hex::encode(&hash)}),
            )
            .await?;
        }
        let endpoints =
            endpoints_for_machine(&mut tx, enrolled.machine_id, &inner.settings.endpoints).await?;
        tx.commit().await.map_err(DbError::from)?;
        Ok(EnrollReply {
            machine_id: enrolled.machine_id,
            certificate_pem: certificate.pem(),
            ca_pem: inner.authority.certificate_pem(),
            endpoints,
            system: EnrollReplySystem {
                name: inner.settings.system_name.clone(),
                public_url: inner.settings.public_url.clone(),
            },
            protocol_version: enrolled.protocol_version,
        })
    }

    /// Renews a machine's certificate over a new key.
    pub async fn renew_machine(
        &self,
        caller: &RenewCaller,
        request: &RenewRequest,
        origin: &Origin,
    ) -> Result<RenewReply, LinkFault> {
        let inner = &self.inner;
        let csr = CertificateRequest::from_pem(&request.csr_pem)
            .map_err(|_| LinkFault::invalid("the certificate request cannot be read"))?;
        let presented = match caller {
            RenewCaller::Certificate(certificate) => {
                self.check_chain(certificate)?;
                certificate.clone()
            }
            RenewCaller::Proof { proof, host } => {
                let named = Certificate::from_pem(&proof.certificate_pem)
                    .map_err(|_| LinkFault::invalid("the certificate cannot be read"))?;
                let machine = named
                    .machine_id()
                    .ok_or_else(|| LinkFault::forbidden("the certificate is no machine's"))?;
                verify_proof(
                    inner.authority.certificate(),
                    proof,
                    machine,
                    &request.csr_pem,
                    host,
                    inner.settings.expired_renew_grace_days,
                    inner.clock.as_ref(),
                )
                .map_err(|fault| match fault {
                    RenewalProofFault::Stale => {
                        LinkFault::forbidden("the proof is out of step with the system's clock")
                    }
                    RenewalProofFault::Proof(_) => LinkFault::forbidden("the proof does not hold"),
                })?
            }
        };
        let machine_id = presented
            .machine_id()
            .ok_or_else(|| LinkFault::forbidden("the certificate is no machine's"))?;
        let mut tx = inner.db.begin().await?;
        let row = resources::reenrollment::lock(&mut tx, machine_id).await?;
        let known = row.as_ref().filter(|row| row.is_active());
        if known.is_some_and(|row| {
            row.link()
                .next
                .is_some_and(|next| next.again && next.fingerprint != presented.fingerprint())
        }) {
            return Err(LinkFault::forbidden(
                "a replacement enrollment is waiting for its first session",
            ));
        }
        let record = known.map(|row| {
            let mut fingerprints = vec![hex::encode(&row.cert_fingerprint)];
            fingerprints.extend(row.link().next.map(|next| next.fingerprint));
            MachineRecord {
                active: true,
                fingerprints,
            }
        });
        let policy = RenewalPolicy {
            validity_days: inner.settings.leaf_validity_days,
            grace_days: inner.settings.expired_renew_grace_days,
            service_names: Vec::new(),
        };
        let renewed = pki_renew(
            &inner.authority,
            &presented,
            record.as_ref(),
            &csr,
            &policy,
            inner.clock.as_ref(),
        )
        .map_err(|error| match error {
            RenewalError::Refused(refusal) => LinkFault::forbidden(refusal.to_string()),
            RenewalError::Internal(error) => LinkFault::from(error),
        })?;
        let now = utc(inner.clock.time());
        // A machine that renews with the certificate a renewal gave it has
        // plainly received that one: it is its own now.
        if let Some(row) = known
            && row
                .link()
                .next
                .as_ref()
                .map(|next| next.fingerprint.as_str())
                == Some(presented.fingerprint().as_str())
        {
            resources::promote_next_certificate(&mut tx, machine_id, &presented.fingerprint(), now)
                .await?;
        }
        resources::set_next_certificate(
            &mut tx,
            machine_id,
            &NextCertificate {
                fingerprint: renewed.fingerprint(),
                public_key: RecipientKey::from_certificate(&renewed).to_text(),
                serial: hex::encode(renewed.serial()),
                not_after: utc(renewed.not_after()),
                again: false,
            },
            now,
        )
        .await?;
        let name = known.map_or("", |row| row.name.as_str());
        let summary = format!("Certificate of machine {name} renewed");
        let mut entry = Entry::new(
            Actor::System,
            orch_db::audit::name!("machine.renewed"),
            "machine",
            Some(machine_id),
            &summary,
        );
        entry.details = serde_json::json!({ "serial": hex::encode(renewed.serial()) });
        audit::append(&mut tx, &entry, origin, now).await?;
        tx.commit().await.map_err(DbError::from)?;
        Ok(RenewReply {
            certificate_pem: renewed.pem(),
            ca_pem: inner.authority.certificate_pem(),
        })
    }

    /// Renews a service's certificate, over a new key, for the service that
    /// presented the old one.
    pub fn renew_service(
        &self,
        presented: &Certificate,
        request: &RenewRequest,
    ) -> Result<RenewReply, LinkFault> {
        let inner = &self.inner;
        let csr = CertificateRequest::from_pem(&request.csr_pem)
            .map_err(|_| LinkFault::invalid("the certificate request cannot be read"))?;
        let policy = RenewalPolicy {
            validity_days: inner.settings.leaf_validity_days,
            grace_days: 0,
            service_names: inner.settings.service_dns_names.clone(),
        };
        let renewed = pki_renew(
            &inner.authority,
            presented,
            None,
            &csr,
            &policy,
            inner.clock.as_ref(),
        )
        .map_err(|error| match error {
            RenewalError::Refused(refusal) => LinkFault::forbidden(refusal.to_string()),
            RenewalError::Internal(error) => LinkFault::from(error),
        })?;
        Ok(RenewReply {
            certificate_pem: renewed.pem(),
            ca_pem: inner.authority.certificate_pem(),
        })
    }

    /// The current preference list delivered to every authenticated machine connection.
    #[must_use]
    pub fn advertised_endpoints(&self) -> Vec<Endpoint> {
        self.inner.settings.endpoints.clone()
    }

    /// Makes the certificate of a service, with a key made here: for a
    /// service that is its own authority's neighbour.
    pub fn issue_service(
        &self,
        service: ServiceName,
    ) -> Result<(orch_pki::certificates::LeafKey, Certificate), PkiError> {
        self.inner.authority.issue_with_new_key(
            &Holder::Service {
                service,
                names: self.inner.settings.service_dns_names.clone(),
            },
            self.inner.settings.leaf_validity_days,
            self.inner.clock.as_ref(),
        )
    }

    /// Keeps the web edge's certificate current under this system's existing authority.
    pub fn keep_frontend_certificate(&self, config: &ServerConfig) -> Result<(), PkiError> {
        frontend::keep(&self.inner.authority, config, self.inner.clock.as_ref())
    }

    /// Checks that a certificate chains to the authority and names somebody
    /// of the system, as the gateway that gave it says it did. An expired
    /// machine certificate passes for as long as renewal forgives it.
    fn check_chain(&self, certificate: &Certificate) -> Result<(), LinkFault> {
        let inner = &self.inner;
        let grace = Duration::from_secs(
            inner
                .settings
                .expired_renew_grace_days
                .saturating_mul(SECONDS_PER_DAY),
        );
        let verifier = OrchClientVerifier::new(inner.authority.certificate(), grace, &crypto())
            .map_err(LinkFault::from)?;
        let der: CertificateDer<'_> = certificate.to_rustls();
        let now = u64::try_from(inner.clock.time().unix_millis() / MILLIS_PER_SECOND).unwrap_or(0);
        verifier
            .check(
                &der,
                &[],
                UnixTime::since_unix_epoch(Duration::from_secs(now)),
            )
            .map_err(|_| LinkFault::forbidden("the certificate is not one of this system's"))
    }

    /// Revokes a machine: the row says so, and what was asked of it and not
    /// answered is cancelled. Returns whether the machine was active.
    pub async fn revoke(&self, machine: Id, by: Actor, origin: &Origin) -> Result<bool, DbError> {
        let inner = &self.inner;
        let mut tx = inner.db.begin().await?;
        let now = utc(inner.clock.time());
        if !Self::revoke_in(&mut tx, machine, by, origin, now).await? {
            return Ok(false);
        }
        tx.commit().await?;
        Ok(true)
    }

    /// Revokes a machine as [`Self::revoke`] does, as of `now`, in the caller's transaction: a request
    /// that asked its caller's authority again in that transaction commits the revocation with it.
    pub async fn revoke_in(
        conn: &mut orch_db::Conn,
        machine: Id,
        by: Actor,
        origin: &Origin,
        now: DateTime<Utc>,
    ) -> Result<bool, DbError> {
        if !resources::revoke_machine(conn, machine, now).await? {
            return Ok(false);
        }
        orch_db::queries::link::cancel_open_of(conn, machine, now).await?;
        let entry = Entry::new(
            by,
            orch_db::audit::name!("machine.revoked"),
            "machine",
            Some(machine),
            "Machine revoked",
        );
        audit::append(conn, &entry, origin, now).await?;
        Ok(true)
    }
}

// ---------------------------------------------------------------- the requests a gateway hands over

fn header_text<'a>(headers: &'a HeaderMap, name: &str) -> Option<&'a str> {
    headers.get(name).and_then(|value| value.to_str().ok())
}

fn origin_of(headers: &HeaderMap) -> Origin {
    Origin {
        as_admin: None,
        ip: header_text(headers, CLIENT_IP_HEADER).and_then(|text| text.parse::<IpAddr>().ok()),
        user_agent: header_text(headers, header::USER_AGENT.as_str()).map(str::to_owned),
        request_id: None,
    }
}

async fn read_json<T: serde::de::DeserializeOwned>(body: Body) -> Result<T, LinkFault> {
    let bytes = to_bytes(body, MAX_REQUEST_BYTES)
        .await
        .map_err(|_| LinkFault::invalid("the request is too large"))?;
    serde_json::from_slice(&bytes).map_err(|error| LinkFault::invalid(error.to_string()))
}

fn json_reply<T: Serialize>(status: StatusCode, reply: &T) -> Response {
    let body = serde_json::to_vec(reply).unwrap_or_default();
    let mut response = (status, body).into_response();
    response.headers_mut().insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static("application/json"),
    );
    response
}

async fn enroll_request(
    State(app): State<App>,
    Extension(peer): Extension<Peer>,
    headers: HeaderMap,
    body: Body,
) -> Result<Response, LinkFault> {
    peer.require(ServiceName::Gateway)
        .map_err(|_| LinkFault::forbidden("only a gateway hands over an enrollment"))?;
    let request: EnrollRequest = read_json(body).await?;
    let reply = app
        .link
        .certificates()
        .enroll(&request, &origin_of(&headers))
        .await?;
    Ok(json_reply(StatusCode::CREATED, &reply))
}

fn renew_caller(headers: &HeaderMap) -> Result<RenewCaller, LinkFault> {
    let decode = |name: &str| -> Result<Option<Vec<u8>>, LinkFault> {
        header_text(headers, name)
            .map(|text| {
                URL_SAFE_NO_PAD
                    .decode(text)
                    .map_err(|_| LinkFault::invalid(format!("the header {name} is not base64url")))
            })
            .transpose()
    };
    if let Some(der) = decode(CLIENT_CERTIFICATE_HEADER)? {
        let certificate = Certificate::from_der(&der)
            .map_err(|_| LinkFault::invalid("the certificate cannot be read"))?;
        return Ok(RenewCaller::Certificate(certificate));
    }
    let (Some(certificate), Some(signature), Some(sent_at), Some(host)) = (
        decode(RENEW_CERTIFICATE_HEADER)?,
        header_text(headers, RENEW_SIGNATURE_HEADER),
        header_text(headers, RENEW_SENT_AT_HEADER),
        header_text(headers, FORWARDED_HOST_HEADER),
    ) else {
        return Err(LinkFault::forbidden(
            "a renewal shows its certificate, or proves it holds the key of the one it gives",
        ));
    };
    Ok(RenewCaller::Proof {
        proof: RenewalProof {
            certificate_pem: String::from_utf8(certificate)
                .map_err(|_| LinkFault::invalid("the certificate is not text"))?,
            signature: signature.to_owned(),
            sent_at: sent_at
                .parse()
                .map_err(|_| LinkFault::invalid("the time of the proof cannot be read"))?,
        },
        host: host.to_owned(),
    })
}

async fn renew_request(
    State(app): State<App>,
    Extension(peer): Extension<Peer>,
    headers: HeaderMap,
    body: Body,
) -> Result<Response, LinkFault> {
    peer.require(ServiceName::Gateway)
        .map_err(|_| LinkFault::forbidden("only a gateway hands over a renewal"))?;
    let request: RenewRequest = read_json(body).await?;
    let caller = renew_caller(&headers)?;
    let reply = app
        .link
        .certificates()
        .renew_machine(&caller, &request, &origin_of(&headers))
        .await?;
    Ok(json_reply(StatusCode::OK, &reply))
}

async fn service_renew_request(
    State(app): State<App>,
    Extension(peer): Extension<Peer>,
    body: Body,
) -> Result<Response, LinkFault> {
    let presented = peer
        .certificate
        .as_ref()
        .filter(|certificate| {
            certificate
                .identities()
                .iter()
                .all(|identity| matches!(identity, Identity::Service(_)))
        })
        .ok_or_else(|| LinkFault::forbidden("only a service renews a service's certificate"))?;
    let request: RenewRequest = read_json(body).await?;
    let reply = app.link.certificates().renew_service(presented, &request)?;
    Ok(json_reply(StatusCode::OK, &reply))
}

/// The routes of the internal listener that enroll and renew certificates.
pub fn internal_routes() -> Router<App> {
    Router::new()
        .route("/internal/link/enroll", post(enroll_request))
        .route("/internal/link/renew", post(renew_request))
        .route("/internal/pki/renew", post(service_renew_request))
}

// Tokens and certificates for an existing machine enrolled again.
mod reenrollment;

/// A managed machine follows its node's advertised or locally selected addresses.
pub(crate) async fn endpoints_for_machine(
    conn: &mut orch_db::Conn,
    machine: Id,
    system: &[Endpoint],
) -> Result<Vec<Endpoint>, DbError> {
    use orch_db::queries::settings;
    let parent: Option<Id> = sqlx::query_scalar("SELECT h.machine_id FROM runtimes r JOIN hosts h ON h.id=r.host_id WHERE r.machine_id=$1 LIMIT 1")
        .bind(machine).fetch_optional(&mut *conn).await?;
    if let Some(parent) = parent
        && let Some(row) = settings::row(conn, None, &format!("machine_endpoints:{parent}")).await?
        && let Ok(endpoints) = serde_json::from_value(row.value)
    {
        return Ok(endpoints);
    }
    Ok(system.to_vec())
}
