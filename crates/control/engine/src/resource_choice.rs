//! One paged resource-choice walk for every family and consumer.
//!
//! Native suppliers retain the original source, current authority and actual capability/placement
//! facts. This walk expands and explains intent; its result is never a reusable execution grant.
//! Recovery decides whether another attempt may run, and the native writer checks authority again.

use crate::EngineError;
use async_trait::async_trait;
use orch_provider_contract::pools::{Definition, Kind, Leaf, MemberRule, Target};
use orch_types::Id;
use serde::{Deserialize, Serialize};
use std::collections::VecDeque;
#[path = "resource_choice/provider.rs"]
mod provider;
pub use provider::ProviderSelection;

/// The effective registered defaults of the pool being visited.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Defaults {
    pub max_depth: u64,
    pub batch_size: usize,
    pub priority: u64,
    pub weight: String,
}

/// Current native pool metadata and ordinary Use; names are projected separately for each reader.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Pool {
    pub id: Id,
    pub revision: i64,
    pub definition: Definition,
    pub defaults: Defaults,
    pub reason: Option<String>,
}

/// A written rule's native stable pagination position.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Member {
    pub id: Id,
    pub position: i64,
    pub rule: MemberRule,
    pub enabled: bool,
    pub priority: Option<u64>,
    pub weight: Option<String>,
}

/// A factual supplier window. Its continuation continues rather than imposing a total cap.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Page<T> {
    pub items: Vec<T>,
    pub next: Option<String>,
}

/// The exact nested choice provenance needed by the native final fence and explanation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Hop {
    pub pool_id: Id,
    pub revision: i64,
    pub member_id: Id,
    pub priority: u64,
    pub weight: String,
}

/// Events retain nested policy boundaries instead of flattening their probabilities.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum Event {
    /// The policy of this node starts; previewing it advances no durable rotation.
    Enter { pool: Box<Pool>, path: Vec<Hop> },
    /// An exact leaf and current factual refusal, or no refusal when eligible.
    Candidate {
        leaf: Leaf,
        path: Vec<Hop>,
        reason: Option<String>,
    },
    /// A written rule or inner node cannot contribute; outward names remain separately protected.
    Skipped { path: Vec<Hop>, reason: String },
    /// All pages of this node have been considered.
    Leave { pool_id: Id, path: Vec<Hop> },
}

/// Native fact adapters have no ranking, retry, billing or execution loop of their own.
#[async_trait]
pub trait Source: Send {
    /// Stable key immediately after one written row, in the same supplier's cursor vocabulary.
    fn member_cursor(&self, member: &Member) -> Result<String, EngineError> {
        serde_json::to_string(&(member.position, member.id)).map_err(EngineError::from)
    }
    /// Stable live expansion key; an exact single rule is complete after its one leaf.
    fn leaf_cursor(&self, rule: &MemberRule, leaf: &Leaf) -> Result<Option<String>, EngineError> {
        if matches!(
            rule,
            MemberRule::Single { .. } | MemberRule::TemplateOnHost { .. }
        ) {
            return Ok(None);
        }
        let value = match leaf {
            Leaf::Model { backend } => {
                let offering = backend.offering.as_ref().ok_or_else(|| {
                    EngineError::unreadable("the expanded model has no offering identity")
                })?;
                serde_json::to_string(&(offering.provider, &offering.offering))?
            }
            Leaf::Provider { id } | Leaf::Environment { id } | Leaf::Host { id } => {
                serde_json::to_string(id)?
            }
            Leaf::EnvironmentTemplate { .. } => return Ok(None),
        };
        Ok(Some(value))
    }
    /// Inspecting saved structure does not enable it or acquire a choice.
    fn inspect_only(&self) -> bool {
        false
    }
    async fn pool(&mut self, id: Id) -> Result<Option<Pool>, EngineError>;
    async fn members(
        &mut self,
        pool: Id,
        after: Option<&str>,
        batch: usize,
    ) -> Result<Page<Member>, EngineError>;
    async fn expand(
        &mut self,
        pool: Id,
        rule: &MemberRule,
        after: Option<&str>,
        batch: usize,
    ) -> Result<Page<Leaf>, EngineError>;
    /// Indexed live membership decides the first authored occurrence without a growing in-memory leaf set.
    async fn first_occurrence(
        &mut self,
        pool: Id,
        member: Id,
        leaf: &Leaf,
    ) -> Result<bool, EngineError>;
    /// Current original-source, complete grant/lending path, placement and capability facts.
    async fn reason(&mut self, leaf: &Leaf, path: &[Hop]) -> Result<Option<String>, EngineError>;
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct Expansion {
    resume: Option<Option<String>>,
    member: Member,
    cursor: Option<String>,
    next: Option<String>,
    started: bool,
    leaves: VecDeque<Leaf>,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
struct Frame {
    resume: Option<String>,
    pool: Pool,
    path: Vec<Hop>,
    cursor: Option<String>,
    next: Option<String>,
    started: bool,
    members: VecDeque<Member>,
    expanding: Option<Expansion>,
}

/// A bounded native continuation. A transport must seal it for its original reader and target.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Checkpoint {
    expected: Kind,
    stack: Vec<Frame>,
    pending: VecDeque<Event>,
}

/// An iterative walk retains one supplier page per nesting level and no workspace contents.
pub struct Resolver<S: Source> {
    source: S,
    expected: Kind,
    stack: Vec<Frame>,
    pending: VecDeque<Event>,
}

impl<S: Source> Resolver<S> {
    /// Read-only preparation. No attempt, runtime start or paid trial is reserved here.
    pub async fn new(mut source: S, target: &Target, expected: Kind) -> Result<Self, EngineError> {
        target.validate(expected).map_err(EngineError::unreadable)?;
        let mut pending = VecDeque::new();
        let stack = Vec::new();
        if let Target::Single { resource } = target {
            let reason = source.reason(resource, &[]).await?;
            pending.push_back(Event::Candidate {
                leaf: resource.clone(),
                path: vec![],
                reason,
            });
        }
        let mut resolver = Self {
            source,
            expected,
            stack,
            pending,
        };
        if let Target::Pool { id, .. } = target {
            resolver.enter(*id, vec![]).await?;
        }
        Ok(resolver)
    }

    async fn enter(&mut self, id: Id, path: Vec<Hop>) -> Result<(), EngineError> {
        let reason = if self.stack.iter().any(|frame| frame.pool.id == id) {
            Some("pool_cycle")
        } else if self.stack.iter().enumerate().any(|(index, frame)| {
            !self.source.inspect_only()
                && (self.stack.len() - index) as u64 >= frame.pool.defaults.max_depth
        }) {
            Some("pool_depth")
        } else {
            None
        };
        if let Some(reason) = reason {
            self.pending.push_back(Event::Skipped {
                path,
                reason: reason.into(),
            });
            return Ok(());
        }
        let Some(pool) = self.source.pool(id).await? else {
            self.pending.push_back(Event::Skipped {
                path,
                reason: "pool_unavailable".into(),
            });
            return Ok(());
        };
        let refusal = if pool.definition.kind != self.expected {
            Some("pool_kind".into())
        } else if pool.defaults.max_depth == 0 && !self.source.inspect_only() {
            Some("pool_depth".into())
        } else if !pool.definition.enabled && !self.source.inspect_only() {
            Some("disabled".into())
        } else if pool.definition.policy.is_none() && !self.source.inspect_only() {
            Some("pool_policy_required".into())
        } else {
            pool.reason.clone()
        };
        if let Some(reason) = refusal {
            self.pending.push_back(Event::Skipped { path, reason });
            return Ok(());
        }
        if pool.defaults.batch_size == 0 {
            return Err(EngineError::unreadable(
                "the registered pool read batch must be positive",
            ));
        }
        self.pending.push_back(Event::Enter {
            pool: Box::new(pool.clone()),
            path: path.clone(),
        });
        self.stack.push(Frame {
            resume: None,
            pool,
            path,
            cursor: None,
            next: None,
            started: false,
            members: VecDeque::new(),
            expanding: None,
        });
        Ok(())
    }

    /// Yield factual choices and reasons in written order across every native continuation.
    pub async fn next(&mut self) -> Result<Option<Event>, EngineError> {
        loop {
            if let Some(event) = self.pending.pop_front() {
                return Ok(Some(event));
            }
            let Some(frame) = self.stack.last_mut() else {
                return Ok(None);
            };
            if let Some(expansion) = frame.expanding.as_mut() {
                if let Some(leaf) = expansion.leaves.pop_front() {
                    expansion.resume =
                        Some(self.source.leaf_cursor(&expansion.member.rule, &leaf)?);
                    let mut path = frame.path.clone();
                    path.push(Hop {
                        pool_id: frame.pool.id,
                        revision: frame.pool.revision,
                        member_id: expansion.member.id,
                        priority: expansion
                            .member
                            .priority
                            .unwrap_or(frame.pool.defaults.priority),
                        weight: expansion
                            .member
                            .weight
                            .clone()
                            .unwrap_or_else(|| frame.pool.defaults.weight.clone()),
                    });
                    let reason = if leaf.kind() != self.expected {
                        Some("member_kind".into())
                    } else if !self
                        .source
                        .first_occurrence(frame.pool.id, expansion.member.id, &leaf)
                        .await?
                    {
                        Some("duplicate_member".into())
                    } else {
                        self.source.reason(&leaf, &path).await?
                    };
                    return Ok(Some(Event::Candidate { leaf, path, reason }));
                }
                if expansion.started && expansion.next.is_none() {
                    frame.expanding = None;
                    continue;
                }
                expansion.cursor = expansion.next.take();
                let page = self
                    .source
                    .expand(
                        frame.pool.id,
                        &expansion.member.rule,
                        expansion.cursor.as_deref(),
                        frame.pool.defaults.batch_size,
                    )
                    .await?;
                if page.next.is_some() && page.next == expansion.cursor {
                    return Err(EngineError::unreadable(
                        "the native member continuation did not advance",
                    ));
                }
                expansion.started = true;
                expansion.next = page.next;
                expansion.leaves = page.items.into();
                continue;
            }
            if let Some(member) = frame.members.pop_front() {
                frame.resume = Some(self.source.member_cursor(&member)?);
                let mut path = frame.path.clone();
                path.push(Hop {
                    pool_id: frame.pool.id,
                    revision: frame.pool.revision,
                    member_id: member.id,
                    priority: member.priority.unwrap_or(frame.pool.defaults.priority),
                    weight: member
                        .weight
                        .clone()
                        .unwrap_or_else(|| frame.pool.defaults.weight.clone()),
                });
                if !member.enabled {
                    return Ok(Some(Event::Skipped {
                        path,
                        reason: "member_disabled".into(),
                    }));
                }
                member
                    .rule
                    .validate(self.expected)
                    .map_err(EngineError::unreadable)?;
                if let MemberRule::Pool { id, .. } = &member.rule {
                    self.enter(*id, path).await?;
                } else {
                    frame.expanding = Some(Expansion {
                        resume: None,
                        member,
                        cursor: None,
                        next: None,
                        started: false,
                        leaves: VecDeque::new(),
                    });
                }
                continue;
            }
            if frame.started && frame.next.is_none() {
                let finished = self.stack.pop().ok_or_else(|| {
                    EngineError::unreadable("the pool walk lost its current node")
                })?;
                return Ok(Some(Event::Leave {
                    pool_id: finished.pool.id,
                    path: finished.path,
                }));
            }
            frame.cursor = frame.next.take();
            let page = self
                .source
                .members(
                    frame.pool.id,
                    frame.cursor.as_deref(),
                    frame.pool.defaults.batch_size,
                )
                .await?;
            if page.next.is_some() && page.next == frame.cursor {
                return Err(EngineError::unreadable(
                    "the native pool continuation did not advance",
                ));
            }
            frame.started = true;
            frame.next = page.next;
            frame.members = page.items.into();
        }
    }

    /// Save only traversal positions and current node facts, without unread payloads.
    pub fn checkpoint(&self) -> Checkpoint {
        let mut stack = self.stack.clone();
        for frame in &mut stack {
            if frame.started {
                frame.next = frame.resume.clone().or_else(|| frame.cursor.clone());
                frame.cursor = None;
                frame.started = false;
            }
            frame.members.clear();
            if frame
                .expanding
                .as_ref()
                .is_some_and(|expansion| expansion.resume == Some(None))
            {
                frame.expanding = None;
            } else if let Some(expansion) = frame.expanding.as_mut() {
                expansion.next = expansion
                    .resume
                    .clone()
                    .flatten()
                    .or_else(|| expansion.cursor.clone());
                expansion.cursor = None;
                expansion.started = false;
                expansion.leaves.clear();
            }
        }
        Checkpoint {
            expected: self.expected,
            stack,
            pending: self.pending.clone(),
        }
    }
    /// Continuation is factual only. Every current node and default is checked before it resumes.
    pub async fn resume(mut source: S, saved: Checkpoint) -> Result<Self, EngineError> {
        for frame in &saved.stack {
            let fresh = source.pool(frame.pool.id).await?.ok_or_else(|| {
                EngineError::unreadable("the pool continuation's source is unavailable")
            })?;
            if fresh != frame.pool {
                return Err(EngineError::unreadable(
                    "the pool continuation changed; read from the beginning",
                ));
            }
        }
        Ok(Self {
            source,
            expected: saved.expected,
            stack: saved.stack,
            pending: saved.pending,
        })
    }
    pub(super) fn source_mut(&mut self) -> &mut S {
        &mut self.source
    }
    /// Return the original native supplier for its ordinary final admission or read projection.
    pub fn into_source(self) -> S {
        self.source
    }
}

/// Native database facts for the same shared traversal.
pub mod database;
/// Native host creation facts, using the same expansion, policy and journal as other targets.
pub mod host;

#[path = "resource_choice/decimal.rs"]
mod decimal;

#[path = "resource_choice/reserve.rs"]
mod reserve;
pub(crate) use reserve::current_membership;
pub use reserve::{Choice, ChoiceRequest};

#[path = "resource_choice/facts.rs"]
mod facts;
pub use facts::{Measurements, Quote, measured_environment_load};

#[path = "resource_choice/health.rs"]
pub mod health;

#[path = "resource_choice/retention.rs"]
mod retention;
