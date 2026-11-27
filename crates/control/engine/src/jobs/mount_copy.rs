//! One reviewed location copy uses the shared copy, stop, undo and publication machinery.
use super::runner::{Done, Failure, Job, Phase, Phased, Stop, Undone};
use crate::{
    Engine, EngineError,
    mount_copies::{
        self, MountDecisions,
        location::{Review, operation::Confirm},
    },
    operation_scope::{continuing, http_origin::HttpOrigin},
};
use async_trait::async_trait;
use orch_db::{Conn, DbError, queries::jobs};
use orch_types::{Clock, Time, new_id};
use serde::{Deserialize, Serialize};
use serde_json::json;

pub mod command;
pub const KIND: &str = "mount_copy";
pub const PHASES: &[Phase] = &[
    Phase {
        name: "copy",
        weight: 1,
    },
    Phase {
        name: "commit",
        weight: 0,
    },
];
const MINIMUM: &str = "mount_copy_origin";

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Recipe {
    pub review: Review,
    pub decisions: MountDecisions,
    pub origin: HttpOrigin,
}
impl Recipe {
    pub fn valid(&self) -> bool {
        let [copy] = self.decisions.copies.as_slice() else {
            return false;
        };
        self.origin.origin().operation() == "MountCopyCreate"
            && self.decisions.detached.is_empty()
            && copy.mount_id == self.review.source().mount
            && copy.source_machine == self.review.source().machine
            && copy.target_machine == self.review.target_machine()
            && copy.source_driver == self.review.source_driver()
            && copy.target_driver == self.review.target_driver()
            && copy.from.kind == self.review.source().kind
            && copy.from.source == self.review.source().source
            && copy.from.storage == self.review.source().storage
    }
    fn matches(&self, row: &jobs::JobRow) -> Result<bool, EngineError> {
        Ok(self.valid()
            && row.kind == KIND
            && row.created_by == Some(self.origin.origin().user())
            && row.plan["copy"] == serde_json::to_value(self)?
            && row.plan["mount_decisions"] == serde_json::to_value(&self.decisions)?
            && row.plan["mount_id"] == json!(self.review.source().mount)
            && row.plan["target_machine_id"] == json!(self.review.target_machine()))
    }
}
fn refused() -> EngineError {
    EngineError::BeforeEffect(Box::new(continuing::Error::Refused))
}
fn mapped(error: continuing::Error) -> EngineError {
    match error {
        continuing::Error::Native(error) => error,
        error => EngineError::BeforeEffect(Box::new(error)),
    }
}

pub(crate) struct Checkpoint {
    pub parts: continuing::Parts<Confirm>,
    pub until: Option<Time>,
}
pub(crate) async fn frame(
    conn: &mut Conn,
    clock: &dyn Clock,
    row: &jobs::JobRow,
    recipe: &Recipe,
) -> Result<Checkpoint, EngineError> {
    if !recipe.matches(row)?
        || !row.is_open()
        || row.cancel_requested()
        || super::runner::abort_reason(row)?.is_some()
    {
        return Err(refused());
    }
    let minimum: HttpOrigin =
        serde_json::from_value(row.plan.get(MINIMUM).cloned().ok_or_else(refused)?)?;
    let origin = recipe.origin.intersect(&minimum).map_err(mapped)?;
    let accepted = origin
        .prepare::<Confirm>(conn, clock, recipe.review.clone())
        .await
        .map_err(mapped)?;
    let next = accepted.http_origin().cloned().ok_or_else(refused)?;
    jobs::merge_plan(conn, row.id, &json!({MINIMUM:next}), clock.now()).await?;
    Ok(Checkpoint {
        parts: accepted.into_parts(),
        until: next.until(),
    })
}

impl Engine {
    /// The API supplies its fully admitted Copy operation and immutable physical review.
    pub async fn queue_mount_copy(
        &self,
        conn: &mut Conn,
        recipe: &Recipe,
    ) -> Result<jobs::JobRow, EngineError> {
        if !recipe.valid() {
            return Err(refused());
        }
        Ok(jobs::insert(conn,&jobs::NewJob {
            id:new_id(),kind:KIND,node_id:None,environment_id:None,provider_id:None,
            plan:&json!({"mount_id":recipe.review.source().mount,"target_machine_id":recipe.review.target_machine(),
                "copy":recipe,"mount_decisions":recipe.decisions,MINIMUM:recipe.origin}),
            progress:&serde_json::to_value(super::runner::initial_progress(PHASES))?,
            created_by:Some(recipe.origin.origin().user()),
        },self.inner.clock.now()).await?)
    }
}

pub(crate) struct Copying {
    recipe: Recipe,
}
impl Copying {
    pub(crate) fn of(row: &jobs::JobRow) -> Result<Self, EngineError> {
        let recipe: Recipe = serde_json::from_value(row.plan["copy"].clone())?;
        if !recipe.matches(row)? {
            return Err(refused());
        }
        Ok(Self { recipe })
    }
}
#[async_trait]
impl Phased for Copying {
    async fn run(&self, job: &Job, phase: &str) -> Result<Done, Stop> {
        let row = job.row().await?;
        if row.plan["copy_published"] == true {
            return Ok(Done::Finished);
        }
        let mut tx = job.engine().db().begin().await?;
        let checkpoint = frame(
            &mut tx,
            job.engine().inner.clock.as_ref(),
            &row,
            &self.recipe,
        )
        .await?;
        let parts = checkpoint.parts;
        let owned = job.lock_owned(&mut tx).await?.ok_or(Stop::Lost)?;
        if owned.cancel_requested() {
            return Err(Stop::Cancelled);
        }
        match phase {
            "copy" => {
                parts
                    .first_effect
                    .check(job.engine().inner.clock.as_ref())
                    .map_err(mapped)?;
                tx.commit().await.map_err(DbError::from)?;
                mount_copies::apply(job, self.recipe.review.target_machine()).await?;
            }
            "commit" => {
                parts
                    .first_effect
                    .check(job.engine().inner.clock.as_ref())
                    .map_err(mapped)?;
                if !jobs::pass_commit(&mut tx, job.id(), job.now()).await? {
                    return Err(Stop::Cancelled);
                }
                mount_copies::commit_locations(
                    &mut tx,
                    &self.recipe.decisions,
                    None,
                    job.id(),
                    self.recipe.origin.origin().user(),
                    job.now(),
                )
                .await?;
                jobs::merge_plan(
                    &mut tx,
                    job.id(),
                    &json!({"copy_published":true}),
                    job.now(),
                )
                .await?;
                if !parts
                    .principal
                    .caller()
                    .key_deadline_allows(job.engine().inner.clock.as_ref())
                    || checkpoint
                        .until
                        .is_some_and(|until| job.engine().inner.clock.time() >= until)
                {
                    return Err(refused().into());
                }
                tx.commit().await.map_err(DbError::from)?;
            }
            _ => return Err(Failure::new("invalid_plan", "The copy job has no such phase").into()),
        }
        Ok(Done::Finished)
    }
    async fn undo(&self, job: &Job, _: &Stop) -> Undone {
        let left = match mount_copies::undo(job).await {
            Ok(left) => left,
            Err(_) => {
                return Undone::Deferred(crate::mount_copies::CUSTODY_PENDING.into());
            }
        };
        if left.is_empty() {
            Undone::Everything
        } else {
            let _ = job.remember(&json!({"copy_left":left})).await;
            Undone::Partly(
                "Some owned data could not be restored or removed; its original copy job is kept"
                    .into(),
            )
        }
    }
    fn answers_are_final(&self) -> bool {
        true
    }
}
