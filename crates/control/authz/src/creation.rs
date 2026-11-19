//! Request-local source bounds through the same creation and page writers as the real request.

use crate::engine::{Frame, Graph, Site};
use crate::giving::{Acting, Giving, Judgement};
use crate::pages::{self, ChangeError, Target};
use crate::{Error, Policy, Principal, ownership};
use orch_access_contract::pages::{PageChange, PageKind, StoredKind};
use orch_access_contract::permissions::{NodePermission, Permission as P, Placement, Profile};
use orch_access_contract::roles::{NodeKind, PlaceRef};
use orch_db::Conn;
use orch_db::queries::{access, hierarchy, threads};
use orch_types::{Id, new_id};

fn settings_error(error: orch_settings::Error) -> Error {
    match error {
        orch_settings::Error::Db(error) => Error::Db(error),
        other => Error::Stored(other.to_string()),
    }
}

fn can_share(giving: &Giving<'_, '_>, rows: &[(P, Profile)]) -> bool {
    match giving.judge(rows) {
        Judgement::Allowed { rows } => {
            !rows.is_empty()
                && rows
                    .iter()
                    .all(|(permission, profile)| giving.can_give(*permission, *profile))
        }
        _ => false,
    }
}

fn source_permissions(frame: &Frame<'_>, site: &Site, policy: Policy, inside: bool) -> Vec<P> {
    let Some(view) = site
        .kind
        .node()
        .and_then(|kind| NodePermission::View.on(kind))
    else {
        return Vec::new();
    };
    if frame.ways(site, view).is_empty() {
        return Vec::new();
    }
    let live = Giving::of(frame, Some(site), PageKind::Live, Acting::Share).with_policy(policy);
    let defaults =
        Giving::of(frame, Some(site), PageKind::Defaults, Acting::Share).with_policy(policy);
    let mut allowed = Vec::new();
    let groups: &[&[P]] = if inside {
        &[
            &[P::ThreadView, P::ThreadInteract],
            &[P::ChatView, P::ChatInteract],
            &[P::DocView, P::DocEdit, P::DocManage],
        ]
    } else {
        &[
            &[P::ChatView, P::ChatInteract],
            &[P::DocView, P::DocEdit, P::DocManage],
        ]
    };
    for group in groups {
        for end in 1..=group.len() {
            let chosen = &group[..end];
            let own: Vec<_> = chosen
                .iter()
                .filter(|permission| {
                    **permission != P::ThreadView && **permission != P::ThreadInteract
                })
                .map(|permission| (*permission, Profile::Any))
                .collect();
            let inherited: Vec<_> = chosen
                .iter()
                .flat_map(|permission| {
                    if permission.profiles() {
                        vec![
                            (*permission, Profile::Managed),
                            (*permission, Profile::Connected),
                        ]
                    } else {
                        vec![(*permission, Profile::Any)]
                    }
                })
                .collect();
            if (!own.is_empty() && !can_share(&live, &own))
                || (inside && !can_share(&defaults, &inherited))
            {
                break;
            }
            allowed.push(group[end - 1]);
        }
    }
    allowed
}

/// Source permissions for This thread, This topic and This project, in that order.
/// One trial runs the normal creation/page producers and is always rolled back. It performs
/// no input, agent start or file work, and never commits a preview object or permission entry.
/// Admission remains with the calling route, so fork/adopt previews gain no parent door.
pub async fn agent_permissions(
    conn: &mut Conn,
    principal: &Principal,
    parent: Id,
    topic_change: Option<&PageChange>,
    thread_change: Option<&PageChange>,
    placement: Placement,
    environment: Option<Id>,
) -> Result<[Vec<P>; 3], ChangeError> {
    projected_permissions(
        conn,
        principal,
        parent,
        PagePlan {
            topic_change,
            thread_change,
            copy_from: None,
            existing_topic: false,
        },
        placement,
        environment,
    )
    .await
}

/// Source bounds of a fork after the same birth page change and copied-page overlay
/// as its normal job. The existing fork producer supplies the admitted source/topic;
/// this adds no parent binding-options permission check.
pub async fn fork_agent_permissions(
    conn: &mut Conn,
    principal: &Principal,
    topic: Id,
    thread_change: Option<&PageChange>,
    copy_from: Option<Id>,
    placement: Placement,
    environment: Option<Id>,
) -> Result<[Vec<P>; 3], ChangeError> {
    projected_permissions(
        conn,
        principal,
        topic,
        PagePlan {
            topic_change: None,
            thread_change,
            copy_from,
            existing_topic: true,
        },
        placement,
        environment,
    )
    .await
}

struct PagePlan<'a> {
    topic_change: Option<&'a PageChange>,
    thread_change: Option<&'a PageChange>,
    copy_from: Option<Id>,
    existing_topic: bool,
}

async fn projected_permissions(
    conn: &mut Conn,
    principal: &Principal,
    parent: Id,
    page: PagePlan<'_>,
    placement: Placement,
    environment: Option<Id>,
) -> Result<[Vec<P>; 3], ChangeError> {
    let PagePlan {
        topic_change,
        thread_change,
        copy_from,
        existing_topic,
    } = page;
    let mut trial = access::begin_trial(conn).await?;
    let outcome = async {
        let graph = Graph::load(&mut trial, &[parent]).await?;
        let node = graph.get(parent).ok_or(Error::NotFound(parent))?;
        if existing_topic && node.kind != NodeKind::Topic {
            return Err(ChangeError::Invalid(vec![orch_types::fields::FieldError {
                path: "topic_id".into(),
                message: "fork and adoption previews use the selected existing topic".into(),
            }]));
        }
        if !matches!(node.kind, NodeKind::Topic | NodeKind::Project) {
            return Err(ChangeError::Invalid(vec![orch_types::fields::FieldError {
                path: "access".into(),
                message: "a creation preview starts in a topic or project".into(),
            }]));
        }
        let project = node
            .path
            .iter()
            .rev()
            .copied()
            .find(|id| {
                graph
                    .get(*id)
                    .is_some_and(|node| node.kind == NodeKind::Project)
            })
            .ok_or(Error::NotFound(parent))?;
        let topic = if node.kind == NodeKind::Project {
            let first = ownership::owner_of_new(&mut trial, principal.user(), Some(parent)).await?;
            let topic = new_id();
            hierarchy::create(
                &mut trial,
                &hierarchy::NewNode {
                    id: topic,
                    kind: "topic",
                    parent: Some(parent),
                    name: "New topic",
                    name_by_person: false,
                    description: "",
                    description_by_person: false,
                    owner: first,
                    org: None,
                    at: principal.now().get(),
                },
            )
            .await?;
            pages::start(
                &mut trial,
                StoredKind::Node,
                topic,
                principal.user(),
                principal.now(),
            )
            .await?;
            if let Some(change) = topic_change {
                // Exactly as normal topic creation: its live page and its defaults are separate acts.
                for target in [Target::Live(topic), Target::Defaults(PlaceRef::Node(topic))] {
                    pages::change(
                        &mut trial,
                        principal,
                        target,
                        change,
                        Policy::default(),
                        &Default::default(),
                    )
                    .await?;
                }
            }
            topic
        } else {
            if topic_change.is_some() {
                return Err(ChangeError::Invalid(vec![orch_types::fields::FieldError {
                    path: "topic_access".into(),
                    message: "an existing topic is not changed by a thread preview".into(),
                }]));
            }
            parent
        };
        // The normal first-thread producer reloads this same scoped Caller after the topic exists.
        let mut creator =
            Principal::load(&mut trial, principal.caller().clone(), principal.now()).await?;
        if let Some(hold) = principal.second_step_hold() {
            creator = creator.held_for_second_step(hold.clone());
        }
        let thread = new_id();
        if !threads::insert_thread(
            &mut trial,
            thread,
            topic,
            "New thread",
            creator.user(),
            creator.now().get(),
        )
        .await?
        {
            return Err(Error::NotFound(topic).into());
        }
        pages::start(
            &mut trial,
            StoredKind::Node,
            thread,
            creator.user(),
            creator.now(),
        )
        .await?;
        threads::make_node_chat(&mut trial, thread, creator.user(), creator.now().get()).await?;
        if let Some(environment) = environment {
            threads::set_environment(&mut trial, thread, environment, "", 0).await?;
        }
        if let Some(change) = thread_change {
            pages::change(
                &mut trial,
                &creator,
                Target::Live(thread),
                change,
                Policy::default(),
                &Default::default(),
            )
            .await?;
        }
        if let Some(source) = copy_from {
            // Production applies Birth.page first, then carries the selected source page
            // at fork commit. Preserve that order, including its own locks and entries.
            ownership::carry_fork_page(&mut trial, source, thread, creator.user(), creator.now())
                .await?;
        }
        let graph = Graph::load(&mut trial, &[thread, topic, project]).await?;
        let thread = graph.site_of(thread).ok_or(Error::NotFound(thread))?;
        let topic = graph.site_of(topic).ok_or(Error::NotFound(topic))?;
        let project = graph.site_of(project).ok_or(Error::NotFound(project))?;
        // A real chosen environment carries its actual hosting/section facts into the common frame.
        // The only unregistered promise this existing endpoint makes is a new managed template.
        if thread.placement != placement {
            return Err(Error::Stored(
                "the resolved environment placement differs from its preview".into(),
            )
            .into());
        }
        let sites = [thread.clone(), topic.clone(), project.clone()];
        let policies = [parent, project.id];
        let holders = orch_settings::resolver::get_at_nodes::<bool>(
            &mut trial,
            &policies,
            "sharing.allow_holders",
        )
        .await
        .map_err(settings_error)?;
        let external = orch_settings::resolver::get_at_nodes::<bool>(
            &mut trial,
            &policies,
            "sharing.allow_external",
        )
        .await
        .map_err(settings_error)?;
        let policy_at = |id| Policy {
            holders_share: holders.get(&id).copied().flatten().unwrap_or(true),
            owners_name_outsiders: external.get(&id).copied().flatten().unwrap_or(true),
        };
        let visible = if creator.caller().as_admin() {
            let frame = Frame::load_with_participation(
                &mut trial,
                &creator,
                Policy::default(),
                graph.clone(),
                &sites,
                P::ALL,
                &[],
            )
            .await?;
            !frame.ways(&thread, P::ThreadView).is_empty()
        } else {
            true
        };
        let ordinary = creator.ordinary(&mut trial).await?;
        let frame = Frame::load_with_participation(
            &mut trial,
            &ordinary,
            Policy::default(),
            graph,
            &sites,
            P::ALL,
            &[],
        )
        .await?;
        if !visible
            || (!creator.caller().as_admin() && frame.ways(&thread, P::ThreadView).is_empty())
            || thread.is_archived()
        {
            return Ok([Vec::new(), Vec::new(), Vec::new()]);
        }
        Ok([
            source_permissions(&frame, &thread, policy_at(parent), false),
            source_permissions(&frame, &topic, policy_at(parent), true),
            source_permissions(&frame, &project, policy_at(project.id), true),
        ])
    }
    .await;
    access::end_trial(trial).await?;
    outcome
}
