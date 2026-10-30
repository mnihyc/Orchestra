//! Eligible decision makers, and the nearest of them a request is sent to: whoever manages the thread for a harness approval, whoever may change the page for each part of an access request, and a capability's host, org or system scope.
//!
//! A harness's authority is inside the boundary. Every other kind is governed by its own
//! approvers; a thread manager, reviewer, rule or approval mode cannot stand in for that scope.

/// Which authority decides a kind of request. No unknown kind falls back to the thread.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Authority {
    /// The harness's thread manager (or its own reviewer, for an automatic harness decision).
    HarnessThread,
    /// The host, org or system scope the capability's policy names.
    CapabilityScope,
    /// Ports Manage on the environment whose boundary would open.
    PortEnvironment,
    /// Access Manage on the requested thing, with the ordinary giving check when approved.
    AccessObject,
    /// The current permission for answering a proposed plan.
    PlanThread,
    /// Driving the thread and using its current model on one's own access.
    ModelUseThread,
}

/// The exhaustive authority boundary, shared by approval readers and decisions.
#[must_use]
pub fn authority(kind: &str) -> Option<Authority> {
    match kind {
        "harness" => Some(Authority::HarnessThread),
        "capability" => Some(Authority::CapabilityScope),
        "port" => Some(Authority::PortEnvironment),
        "access" => Some(Authority::AccessObject),
        "plan" => Some(Authority::PlanThread),
        "input" => Some(Authority::ModelUseThread),
        _ => None,
    }
}

/// Current holders and administrators who can decide under the selected permission.
pub async fn permission_deciders(
    conn: &mut orch_db::Conn,
    object: orch_access_contract::decisions::ObjectRef,
    permission: orch_access_contract::permissions::Permission,
    at: orch_types::Time,
) -> Result<Vec<orch_types::Id>, crate::Error> {
    let (graph, site) = crate::things::site(conn, crate::things::thing_of(object)).await?;
    let admins = crate::holders::notification_admins(conn, at).await?;
    let seeds: Vec<_> = admins
        .iter()
        .copied()
        .chain(site.first_owner)
        .chain(graph.nodes().map(|node| node.first_owner))
        .collect();
    let mut principal = None;
    for seed in seeds {
        if let Ok(active) = crate::Principal::load(conn, crate::Caller::session(seed), at).await {
            principal = Some(active);
            break;
        }
    }
    if principal.is_none() {
        // A deciding holder can outlive every owner/admin; the read frame needs an active identity, not their privileges.
        let person = orch_db::queries::identity::first_active_human(conn).await?;
        if let Some(person) = person {
            principal = crate::Principal::load(conn, crate::Caller::session(person), at)
                .await
                .ok();
        }
    }
    let Some(principal) = principal else {
        return Ok(Vec::new());
    };
    let mut asked = vec![permission];
    asked.extend(permission.held_through());
    let frame = crate::engine::Frame::load(
        conn,
        &principal,
        crate::Policy::default(),
        graph.clone(),
        std::slice::from_ref(&site),
        &asked,
    )
    .await?;
    let mut people =
        crate::holders::who_holds_for_notifications(conn, &frame, &site, permission).await?;
    let candidates = crate::missing::givers(
        conn,
        &frame,
        &site,
        crate::missing::Who {
            owners: false,
            named_in: None,
            admins: true,
        },
    )
    .await?;
    for candidate in candidates {
        let Ok(principal) = crate::Principal::load(
            conn,
            crate::Caller::session(candidate.user).with_admin_mode(true),
            at,
        )
        .await
        else {
            continue;
        };
        let candidate_frame = crate::engine::Frame::load(
            conn,
            &principal,
            crate::Policy::default(),
            graph.clone(),
            std::slice::from_ref(&site),
            &asked,
        )
        .await?;
        if !candidate_frame.ways(&site, permission).is_empty() {
            people.push(candidate.user);
        }
    }
    people.sort_unstable();
    people.dedup();
    Ok(people)
}

/// Selects only directly named people in the nearest eligible Owners row, then eligible org admins.
pub async fn owners_to_tell(
    conn: &mut orch_db::Conn,
    object: orch_access_contract::decisions::ObjectRef,
    eligible: &[orch_types::Id],
    at: orch_types::Time,
) -> Result<Vec<orch_types::Id>, crate::Error> {
    owners_starting_at(conn, object, None, eligible, at).await
}

/// The same ownership walk for a membership-only object beside a permission-bearing place.
pub async fn owners_to_tell_beside(
    conn: &mut orch_db::Conn,
    own: orch_db::queries::access::PageKey,
    parent: orch_access_contract::decisions::ObjectRef,
    eligible: &[orch_types::Id],
    at: orch_types::Time,
) -> Result<Vec<orch_types::Id>, crate::Error> {
    owners_starting_at(conn, parent, Some(own), eligible, at).await
}

async fn owners_starting_at(
    conn: &mut orch_db::Conn,
    object: orch_access_contract::decisions::ObjectRef,
    first: Option<orch_db::queries::access::PageKey>,
    eligible: &[orch_types::Id],
    at: orch_types::Time,
) -> Result<Vec<orch_types::Id>, crate::Error> {
    use orch_access_contract::pages::StoredKind;
    use orch_access_contract::roles::PlaceRef;
    use orch_db::queries::access::{self, PageKey};
    let (graph, site) = crate::things::site(conn, crate::things::thing_of(object)).await?;
    let own = PageKey::new(StoredKind::from(site.kind).as_str(), site.id, "live");
    let mut pages: Vec<_> = first.into_iter().collect();
    if !pages.contains(&own) {
        pages.push(own);
    }
    for id in graph.document_ancestors(site.id) {
        pages.push(PageKey::new("document_container", id, "live"));
    }
    for place in &site.chain {
        if let PlaceRef::Node(id) = place {
            let page = PageKey::new("node", *id, "live");
            if !pages.contains(&page) {
                pages.push(page);
            }
        }
    }
    let entries = access::owner_entries(conn, &pages).await?;
    for page in pages {
        let mut people: Vec<_> = entries
            .iter()
            .filter(|entry| {
                entry.object_kind == page.object_kind
                    && entry.object_id == page.object_id
                    && entry.holder_kind == "user"
                    && entry.expires_at.is_none_or(|end| end > at.get())
            })
            .filter_map(|entry| entry.holder_id)
            .filter(|person| eligible.contains(person))
            .collect();
        people.sort_unstable();
        people.dedup();
        if !people.is_empty() {
            return Ok(people);
        }
    }
    // If no directly named owner can decide, only admins of the nearest org are disturbed.
    for org in &site.admin_places {
        if !graph
            .get(*org)
            .is_some_and(|node| node.kind == orch_access_contract::roles::NodeKind::Org)
        {
            continue;
        }
        let mut people = Vec::new();
        for user in eligible {
            let Ok(principal) = crate::Principal::load(
                conn,
                crate::Caller::session(*user).with_admin_mode(true),
                at,
            )
            .await
            else {
                continue;
            };
            let frame = crate::engine::Frame::load(
                conn,
                &principal,
                crate::Policy::default(),
                graph.clone(),
                std::slice::from_ref(&site),
                &[orch_access_contract::permissions::Permission::AccessManage],
            )
            .await?;
            if crate::admins::is_admin_at(&frame, *org) {
                people.push(*user);
            }
        }
        if !people.is_empty() {
            return Ok(people);
        }
    }
    // A wait still needs somebody when no eligible person occurs in that ownership/admin walk.
    let mut people = eligible.to_vec();
    people.sort_unstable();
    people.dedup();
    Ok(people)
}

/// Tells the eligible person who started the turn, otherwise the nearest eligible people in Owners rows.
pub async fn request_recipients(
    conn: &mut orch_db::Conn,
    object: orch_access_contract::decisions::ObjectRef,
    eligible: &[orch_types::Id],
    turn: Option<orch_types::Id>,
    at: orch_types::Time,
) -> Result<Vec<orch_types::Id>, crate::Error> {
    if let Some(turn) = turn
        && let Some(person) = orch_db::queries::approvals::turn_starter(conn, turn).await?
        && eligible.contains(&person)
    {
        return Ok(vec![person]);
    }
    owners_to_tell(conn, object, eligible, at).await
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn a_threads_authority_is_never_the_boundarys_authority() {
        assert_eq!(authority("harness"), Some(Authority::HarnessThread));
        for kind in ["capability", "port", "access"] {
            assert_ne!(authority(kind), Some(Authority::HarnessThread));
        }
        assert_eq!(authority("unknown"), None);
    }
}
