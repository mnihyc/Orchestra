//! Original raw issuer and actual one-use host reservation through native redemption.
use crate::{
    EngineError,
    operation_scope::{
        continuing::{self, Error, Source},
        http_origin::HttpOrigin,
        sources::Facts,
        targets,
    },
};
use futures::future::BoxFuture;
use orch_access_contract::{
    key_scope::{KeyResolvedTarget, KeyTargetKind},
    permissions::{ObjectKind, ResourceKind},
};
use orch_api_contract::{
    host_enrollment_sources::{self, Source as Role},
    operation_sources::Declaration,
    resources::HostEnrollmentTokenCreate,
    routes::{Endpoint, Route},
};
use orch_authz::{
    Caller, Policy, Principal,
    ownership::{self, Making},
};
use orch_db::{
    Conn, DbError,
    queries::{hierarchy, resources},
};
use orch_types::{Clock, Id, Time};
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Destination {
    pub home: Option<Id>,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Token {
    pub hash: Vec<u8>,
    pub issuer: Id,
    pub home: Option<Id>,
    pub access: Option<String>,
    pub roles: Vec<String>,
    pub name_hint: Option<String>,
    pub expires: Time,
}
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Issuer {
    raw: HttpOrigin,
    token: Token,
}
impl std::fmt::Debug for Issuer {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Issuer([private])")
    }
}

pub async fn target(conn: &mut Conn, home: Option<Id>) -> Result<KeyResolvedTarget, EngineError> {
    match home {
        Some(id) => targets::node(conn, id)
            .await
            .map_err(EngineError::unreadable),
        None => Ok(targets::system()),
    }
}
async fn require(conn: &mut Conn, principal: &Principal, home: Option<Id>) -> Result<(), Error> {
    use orch_settings::{Place, get};
    use orch_settings_contract::registry::keys;
    let (members, policy) = if let Some(id) = home {
        let nodes = hierarchy::with_ancestors(conn, &[id]).await?;
        if !nodes
            .iter()
            .any(|node| node.id == id && node.kind == "org" && node.deleted_at.is_none())
        {
            return Err(Error::Refused);
        }
        let members = get::<Vec<String>>(conn, Place::Node(id), keys::RESOURCES_CREATED_BY_MEMBERS)
            .await
            .map_err(EngineError::unreadable)?
            .unwrap_or_default()
            .into_iter()
            .filter_map(|kind| kind.parse().ok())
            .collect::<Vec<_>>();
        let external = get::<bool>(conn, Place::Node(id), keys::SHARING_ALLOW_EXTERNAL)
            .await
            .map_err(EngineError::unreadable)?
            .unwrap_or(true);
        let holders = get::<bool>(conn, Place::Node(id), keys::SHARING_ALLOW_HOLDERS)
            .await
            .map_err(EngineError::unreadable)?
            .unwrap_or(true);
        (
            members,
            Policy {
                owners_name_outsiders: external,
                holders_share: holders,
            },
        )
    } else {
        (Vec::new(), Policy::default())
    };
    if !matches!(
        ownership::may_make(conn, principal, ResourceKind::Host, home, &members, policy).await?,
        Making::Allowed(_)
    ) {
        return Err(Error::Refused);
    }
    Ok(())
}
async fn lock_home(conn: &mut Conn, home: Option<Id>) -> Result<(), Error> {
    if let Some(id) = home {
        let nodes = hierarchy::with_ancestors(conn, &[id]).await?;
        let mut ids = nodes
            .iter()
            .find(|node| node.id == id)
            .map(|node| vec![id, node.org_id])
            .ok_or(Error::Refused)?;
        ids.sort_unstable();
        ids.dedup();
        let selected: Vec<Id> =
            sqlx::query_scalar("SELECT id FROM nodes WHERE id=ANY($1) ORDER BY id FOR SHARE")
                .bind(&ids)
                .fetch_all(conn)
                .await
                .map_err(DbError::from)?;
        if selected != ids {
            return Err(Error::Refused);
        }
    }
    Ok(())
}
pub struct Creation;
impl Source for Creation {
    type Role = Role;
    type Input = Destination;
    type Value = KeyResolvedTarget;
    const ROUTE: Route = HostEnrollmentTokenCreate::ROUTE;
    const DECLARATION: &'static Declaration<Role> = &host_enrollment_sources::CREATE;
    fn prepare<'a>(
        conn: &'a mut Conn,
        _caller: &'a Caller,
        input: &'a Destination,
        _now: Time,
    ) -> BoxFuture<'a, Result<(Self::Value, Vec<Facts<Role>>), Error>> {
        Box::pin(async move {
            let target = target(conn, input.home).await?;
            Ok((
                target.clone(),
                vec![Facts::one(
                    Role::Home,
                    KeyTargetKind::Object(ObjectKind::Host),
                    target,
                )],
            ))
        })
    }
    fn require<'a>(
        conn: &'a mut Conn,
        principal: &'a Principal,
        input: &'a Destination,
        _value: &'a Self::Value,
    ) -> BoxFuture<'a, Result<(), Error>> {
        Box::pin(require(conn, principal, input.home))
    }
    fn locks<'a>(
        conn: &'a mut Conn,
        _principal: &'a Principal,
        input: &'a Destination,
        _value: &'a Self::Value,
    ) -> BoxFuture<'a, Result<(), Error>> {
        Box::pin(lock_home(conn, input.home))
    }
    fn check_opened<'a>(
        _conn: &'a mut Conn,
        _principal: &'a Principal,
        _input: &'a Destination,
        old: &'a Self::Value,
        fresh: &'a Self::Value,
    ) -> BoxFuture<'a, Result<bool, Error>> {
        Box::pin(async move { Ok(old == fresh) })
    }
}
pub struct Redemption;
impl Source for Redemption {
    type Role = Role;
    type Input = Token;
    type Value = KeyResolvedTarget;
    const ROUTE: Route = Creation::ROUTE;
    const DECLARATION: &'static Declaration<Role> = Creation::DECLARATION;
    fn prepare<'a>(
        conn: &'a mut Conn,
        caller: &'a Caller,
        input: &'a Token,
        now: Time,
    ) -> BoxFuture<'a, Result<(Self::Value, Vec<Facts<Role>>), Error>> {
        Box::pin(async move {
            let row = resources::enrollment_token(conn, &input.hash)
                .await?
                .ok_or(Error::Refused)?;
            let roles: Vec<String> =
                sqlx::query_scalar("SELECT roles FROM enrollment_tokens WHERE token_hash=$1")
                    .bind(&input.hash)
                    .fetch_one(&mut *conn)
                    .await
                    .map_err(DbError::from)?;
            let access = resources::host_enrollment_access(conn, &input.hash).await?;
            if caller.user() != input.issuer
                || input.roles != ["node"]
                || row.purpose != "host"
                || row.used_at.is_some()
                || row.again_machine_id.is_some()
                || row.created_by != input.issuer
                || row.org_id != input.home
                || row.name_hint != input.name_hint
                || roles != input.roles
                || access != input.access
                || row.expires_at.timestamp_micros() != input.expires.get().timestamp_micros()
                || input.expires <= now
            {
                return Err(Error::Refused);
            }
            Creation::prepare(conn, caller, &Destination { home: input.home }, now).await
        })
    }
    fn require<'a>(
        conn: &'a mut Conn,
        principal: &'a Principal,
        input: &'a Token,
        _value: &'a Self::Value,
    ) -> BoxFuture<'a, Result<(), Error>> {
        Box::pin(require(conn, principal, input.home))
    }
    fn locks<'a>(
        conn: &'a mut Conn,
        _principal: &'a Principal,
        input: &'a Token,
        _value: &'a Self::Value,
    ) -> BoxFuture<'a, Result<(), Error>> {
        Box::pin(async move {
            lock_home(conn, input.home).await?;
            resources::lock_enrollment_token(conn, &input.hash)
                .await?
                .ok_or(Error::Refused)?;
            Ok(())
        })
    }
    fn check_opened<'a>(
        _conn: &'a mut Conn,
        _principal: &'a Principal,
        _input: &'a Token,
        old: &'a Self::Value,
        fresh: &'a Self::Value,
    ) -> BoxFuture<'a, Result<bool, Error>> {
        Box::pin(async move { Ok(old == fresh) })
    }
}
impl Issuer {
    pub fn capture(raw: HttpOrigin, token: Token, caller: &Caller) -> Result<Self, Error> {
        if raw.origin().operation() != Creation::ROUTE.name || raw.origin().user() != token.issuer {
            return Err(Error::Refused);
        }
        let raw = raw.with_ceiling(caller, Some(token.expires))?;
        Ok(Self { raw, token })
    }
    pub fn token(&self) -> &Token {
        &self.token
    }
    pub async fn current(
        &self,
        conn: &mut Conn,
        clock: &dyn Clock,
    ) -> Result<crate::operation_scope::http_origin::Current, Error> {
        self.raw.current(conn, clock).await
    }
    pub async fn prepare(
        &self,
        conn: &mut Conn,
        clock: &dyn Clock,
    ) -> Result<continuing::Frame<Redemption>, Error> {
        if self.raw.origin().operation() != Creation::ROUTE.name
            || self.raw.origin().user() != self.token.issuer
        {
            return Err(Error::Refused);
        }
        self.raw
            .prepare::<Redemption>(conn, clock, self.token.clone())
            .await
    }
}
