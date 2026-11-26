//! Private factual selection carried by native provider requests, without a reusable permission.
use super::{Hop, Leaf, Target};
use crate::EngineError;
use orch_access_contract::{permissions::UsagePurpose, pools::PathMember};
use orch_db::{Conn, DbError};
use orch_types::{Id, Json};

#[derive(Clone, PartialEq, Eq)]
pub struct ProviderSelection {
    choice: Option<Id>,
    leaf: Leaf,
    path: Vec<PathMember>,
    purpose: UsagePurpose,
    caller: orch_authz::Caller,
}
impl std::fmt::Debug for ProviderSelection {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("ProviderSelection([private])")
    }
}
impl ProviderSelection {
    pub fn selection(&self) -> orch_authz::pools::Selection<'_> {
        orch_authz::pools::Selection {
            leaf: &self.leaf,
            path: &self.path,
        }
    }
    pub fn purpose(&self) -> UsagePurpose {
        self.purpose
    }
    pub fn choice_id(&self) -> Option<Id> {
        self.choice
    }
    pub fn caller(&self) -> &orch_authz::Caller {
        &self.caller
    }
    pub fn matches_request(&self, provider: Id, model: Option<&str>) -> bool {
        match &self.leaf {
            Leaf::Provider { id } => *id == provider,
            Leaf::Model { backend } => backend.offering.as_ref().is_some_and(|offering| {
                offering.provider == provider && model == Some(offering.offering.as_str())
            }),
            _ => false,
        }
    }
    pub fn targets(&self, target: &Target) -> bool {
        matches!(target,Target::Pool {id,kind} if self.path.first().is_some_and(|hop|hop.pool_id==*id) && self.leaf.kind()==*kind)
    }
    pub(crate) fn prospective(
        leaf: &Leaf,
        path: &[Hop],
        purpose: UsagePurpose,
        caller: &orch_authz::Caller,
    ) -> Option<Self> {
        (!path.is_empty()).then(|| Self {
            choice: None,
            leaf: leaf.clone(),
            path: path
                .iter()
                .map(|hop| PathMember {
                    pool_id: hop.pool_id,
                    member_id: hop.member_id,
                    revision: hop.revision,
                })
                .collect(),
            purpose,
            caller: caller.clone(),
        })
    }
    /// The native producer supplies its own retained choice identity and actual request operands.
    /// Loading these facts is insufficient for use: its writer repeats all current pool/lender checks.
    pub async fn load(
        conn: &mut Conn,
        choice: Id,
        node: Id,
        provider: Id,
        model: Option<&str>,
        purpose: UsagePurpose,
        caller: &orch_authz::Caller,
    ) -> Result<Self, EngineError> {
        let unavailable =
            || EngineError::unreadable("the selected pool source is no longer available");
        let row: Option<(Json,Json,Json,Json)> = sqlx::query_as("SELECT target,leaf,path,source FROM pool_choices WHERE id=$1 AND state='selected' AND source->>'node'=$2")
            .bind(choice).bind(node.to_string()).fetch_optional(&mut *conn).await.map_err(DbError::from)?;
        let Some((target, leaf, path, source)) = row else {
            return Err(unavailable());
        };
        let target: Target = serde_json::from_value(target)?;
        let leaf: Leaf = serde_json::from_value(leaf)?;
        let path: Vec<Hop> = serde_json::from_value(path)?;
        let expected = if purpose == UsagePurpose::Turn {
            serde_json::json!("agent")
        } else {
            serde_json::to_value(purpose)?
        };
        if source.get("purpose") != Some(&expected)
            || !matches!(target, Target::Pool { .. })
            || !super::reserve::current_membership(conn, &target, &leaf, &path).await?
        {
            return Err(unavailable());
        }
        let matches = match &leaf {
            Leaf::Provider { id } => *id == provider,
            Leaf::Model { backend } => backend.offering.as_ref().is_some_and(|offering| {
                offering.provider == provider && model == Some(offering.offering.as_str())
            }),
            _ => false,
        };
        if !matches {
            return Err(unavailable());
        }
        let mut selected =
            Self::prospective(&leaf, &path, purpose, caller).ok_or_else(unavailable)?;
        selected.choice = Some(choice);
        Ok(selected)
    }
}
