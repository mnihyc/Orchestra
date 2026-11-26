//! One native metadata supplier for every resource kind; consumers supply actual request facts.
use super::*;
use orch_access_contract::{
    decisions::ObjectRef,
    permissions::{ObjectKind, Permission},
};
use orch_authz::{Principal, resources, usable};
use orch_db::{Conn, queries::pools as stored};
use serde::{Deserialize, Serialize};

/// A consumer's installed backend and native request conditions; no ranking or retry belongs here.
#[async_trait]
pub trait Requirements: Send {
    /// A fixed native producer may supply its already-admitted dependency source. The ordinary
    /// request path retains current Pool Use and never derives permission from membership.
    async fn pool_reason(
        &mut self,
        conn: &mut Conn,
        principal: &Principal,
        id: Id,
    ) -> Result<Option<String>, EngineError> {
        let policies = crate::access::resource_policies(conn, id).await?;
        let allowed = matches!(
            usable::usable(conn, principal, id, None, &policies)
                .await
                .map_err(unreadable)?,
            usable::Usable::Yes(_)
        );
        Ok((!allowed).then(|| "no_use".into()))
    }
    /// Original accepted input quantities/params supply prices; no probe or new execution occurs.
    async fn measurements(
        &mut self,
        conn: &mut Conn,
        principal: &Principal,
        leaf: &Leaf,
        _: &[Hop],
        _: bool,
    ) -> Result<Measurements, EngineError> {
        Measurements::native(conn, leaf, principal.now()).await
    }
    fn inspect_only(&self) -> bool {
        false
    }
    async fn model(
        &mut self,
        conn: &mut Conn,
        provider: Id,
        model: &str,
    ) -> Result<Leaf, EngineError>;
    async fn reason(
        &mut self,
        conn: &mut Conn,
        principal: &Principal,
        leaf: &Leaf,
        path: &[Hop],
    ) -> Result<Option<String>, EngineError>;
}
/// The native source retains its actual caller; constructing it starts no runtime or model.
pub struct Database<'a, R: Requirements> {
    pub(super) conn: &'a mut Conn,
    pub(super) principal: &'a Principal,
    pub(super) requirements: R,
    root: Option<Id>,
}
impl<'a, R: Requirements> Database<'a, R> {
    pub fn new(conn: &'a mut Conn, principal: &'a Principal, requirements: R) -> Self {
        Self {
            conn,
            principal,
            requirements,
            root: None,
        }
    }
}
fn unreadable(error: impl std::fmt::Display) -> EngineError {
    EngineError::unreadable(error.to_string())
}
fn encode<T: Serialize>(value: &T) -> Result<String, EngineError> {
    serde_json::to_string(value).map_err(EngineError::from)
}
fn cursor<T: for<'de> Deserialize<'de>>(value: Option<&str>) -> Result<Option<T>, EngineError> {
    value
        .map(serde_json::from_str)
        .transpose()
        .map_err(EngineError::from)
}
fn window<T>(
    mut items: Vec<T>,
    batch: usize,
    next: impl FnOnce(&T) -> Result<String, EngineError>,
) -> Result<Page<T>, EngineError> {
    let more = items.len() > batch;
    items.truncate(batch);
    let after = if more {
        items.last().map(next).transpose()?
    } else {
        None
    };
    Ok(Page { items, next: after })
}
fn limit(batch: usize) -> Result<i64, EngineError> {
    batch
        .checked_add(1)
        .and_then(|value| i64::try_from(value).ok())
        .ok_or_else(|| EngineError::unreadable("the pool page exceeds its integer representation"))
}

#[async_trait]
impl<R: Requirements> Source for Database<'_, R> {
    fn inspect_only(&self) -> bool {
        self.requirements.inspect_only()
    }
    async fn pool(&mut self, id: Id) -> Result<Option<Pool>, EngineError> {
        let Some(row) = stored::get(self.conn, id)
            .await?
            .filter(|row| row.deleted_at.is_none())
        else {
            return Ok(None);
        };
        let definition: Definition = serde_json::from_value(row.definition)?;
        let at = orch_settings::Place::Pool(id);
        async fn setting<T: serde::de::DeserializeOwned>(
            conn: &mut Conn,
            at: orch_settings::Place,
            key: &str,
        ) -> Result<T, EngineError> {
            orch_settings::get(conn, at, key)
                .await
                .map_err(unreadable)?
                .ok_or_else(|| {
                    EngineError::unreadable(format!(
                        "the registered pool setting {key} has no default"
                    ))
                })
        }
        let root = *self.root.get_or_insert(id);
        let reason = if id == root {
            let allowed = if self.requirements.inspect_only() {
                resources::check(
                    self.conn,
                    self.principal,
                    ObjectRef {
                        kind: ObjectKind::Pool,
                        id,
                    },
                    Permission::PoolView,
                    Default::default(),
                )
                .await
                .map_err(unreadable)?
                .is_allowed()
            } else {
                self.requirements
                    .pool_reason(self.conn, self.principal, id)
                    .await?
                    .is_none()
            };
            (!allowed).then(|| "no_use".into())
        } else {
            None
        };
        Ok(Some(Pool {
            id,
            revision: row.revision,
            definition,
            defaults: Defaults {
                max_depth: setting(self.conn, at, "pools.max_depth").await?,
                batch_size: setting(self.conn, at, "pools.read_batch_size").await?,
                priority: setting(self.conn, at, "pools.member_priority").await?,
                weight: setting(self.conn, at, "pools.member_weight").await?,
            },
            reason,
        }))
    }
    async fn members(
        &mut self,
        pool: Id,
        after: Option<&str>,
        batch: usize,
    ) -> Result<Page<Member>, EngineError> {
        let rows = stored::members(self.conn, pool, cursor(after)?, limit(batch)?).await?;
        let rows = rows
            .into_iter()
            .map(|row| {
                Ok(Member {
                    id: row.id,
                    position: row.position,
                    rule: serde_json::from_value(row.rule)?,
                    enabled: row.enabled,
                    priority: row
                        .priority
                        .map(u64::try_from)
                        .transpose()
                        .map_err(unreadable)?,
                    weight: row.weight,
                })
            })
            .collect::<Result<Vec<_>, EngineError>>()?;
        window(rows, batch, |member| encode(&(member.position, member.id)))
    }
    async fn expand(
        &mut self,
        pool: Id,
        rule: &MemberRule,
        after: Option<&str>,
        batch: usize,
    ) -> Result<Page<Leaf>, EngineError> {
        let Some(parent) = stored::get(self.conn, pool).await? else {
            return Ok(Page {
                items: vec![],
                next: None,
            });
        };
        if matches!(
            rule,
            MemberRule::ProviderModels { .. }
                | MemberRule::Matching {
                    kind: Kind::Model,
                    ..
                }
                | MemberRule::Grouping {
                    kind: Kind::Model,
                    ..
                }
        ) {
            let rows = stored::model_candidates(
                self.conn,
                parent.org_id,
                rule,
                cursor(after)?,
                limit(batch)?,
            )
            .await?;
            let page = window(rows, batch, |row| {
                encode(&(row.provider_id, row.model_name.clone()))
            })?;
            let mut items = Vec::with_capacity(page.items.len());
            for row in page.items {
                let leaf = self
                    .requirements
                    .model(self.conn, row.provider_id, &row.model_name)
                    .await?;
                if !matches!(&leaf,Leaf::Model{backend}if backend.offering.as_ref().is_some_and(|offering|offering.provider==row.provider_id&&offering.offering==row.model_name))
                {
                    return Err(EngineError::unreadable(
                        "the native model factory changed the expanded offering",
                    ));
                }
                items.push(leaf);
            }
            return Ok(Page {
                items,
                next: page.next,
            });
        }
        match rule {
            MemberRule::Single { resource } => Ok(Page {
                items: if after.is_none() {
                    vec![resource.clone()]
                } else {
                    vec![]
                },
                next: None,
            }),
            MemberRule::TemplateOnHost {
                template_id,
                host_id,
            } => Ok(Page {
                items: if after.is_none() {
                    vec![Leaf::EnvironmentTemplate {
                        template_id: *template_id,
                        host_id: *host_id,
                    }]
                } else {
                    vec![]
                },
                next: None,
            }),
            MemberRule::Pool { .. } => Err(EngineError::unreadable(
                "an inner pool is traversed through its own policy node",
            )),
            _ => {
                let ids = stored::resource_candidates(
                    self.conn,
                    parent.org_id,
                    rule,
                    cursor(after)?,
                    limit(batch)?,
                )
                .await?;
                let page = window(ids, batch, encode)?;
                let kind = match rule {
                    MemberRule::HostEnvironments { .. } => Kind::Environment,
                    MemberRule::Matching { kind, .. } | MemberRule::Grouping { kind, .. } => *kind,
                    _ => {
                        return Err(EngineError::unreadable(
                            "the native member rule has no family",
                        ));
                    }
                };
                Ok(Page {
                    items: page
                        .items
                        .into_iter()
                        .map(|id| match kind {
                            Kind::Provider => Ok(Leaf::Provider { id }),
                            Kind::Environment => Ok(Leaf::Environment { id }),
                            Kind::Host => Ok(Leaf::Host { id }),
                            Kind::Model => Err(EngineError::unreadable(
                                "model expansion needs its native backend factory",
                            )),
                        })
                        .collect::<Result<Vec<_>, _>>()?,
                    next: page.next,
                })
            }
        }
    }
    async fn first_occurrence(
        &mut self,
        pool: Id,
        member: Id,
        leaf: &Leaf,
    ) -> Result<bool, EngineError> {
        let Some(current) = stored::member(self.conn, pool, member).await? else {
            return Ok(false);
        };
        let before = (current.position, current.id);
        if stored::first_exact(self.conn, pool, leaf)
            .await?
            .is_some_and(|first| first < before)
        {
            return Ok(false);
        }
        let Some(parent) = stored::get(self.conn, pool).await? else {
            return Ok(false);
        };
        let batch: usize = orch_settings::get(
            self.conn,
            orch_settings::Place::Pool(pool),
            "pools.read_batch_size",
        )
        .await
        .map_err(unreadable)?
        .ok_or_else(|| EngineError::unreadable("the pool read batch has no registered default"))?;
        let mut after = None;
        loop {
            let rows = stored::dynamic_members(
                self.conn,
                pool,
                before,
                after,
                i64::try_from(batch).map_err(unreadable)?,
            )
            .await?;
            if rows.is_empty() {
                return Ok(true);
            }
            for row in &rows {
                let rule: MemberRule = serde_json::from_value(row.rule.clone())?;
                if stored::rule_contains(self.conn, parent.org_id, &rule, leaf).await? {
                    return Ok(false);
                }
            }
            after = rows.last().map(|row| (row.position, row.id));
        }
    }
    async fn reason(&mut self, leaf: &Leaf, path: &[Hop]) -> Result<Option<String>, EngineError> {
        let reason = self
            .requirements
            .reason(self.conn, self.principal, leaf, path)
            .await?;
        if reason.is_some() || self.inspect_only() || path.is_empty() {
            return Ok(reason);
        }
        super::health::reason(self.conn, leaf, path, self.principal.now().get()).await
    }
}
