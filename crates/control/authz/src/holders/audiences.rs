//! Requested holder audiences: reverse candidates confirmed by the current ordinary frame.

use crate::engine::Frame;
use crate::{Caller, Error, Principal};
use orch_access_contract::pages::Holder;
use orch_access_contract::roles::Role;
use orch_db::Conn;
use orch_db::queries::access;
use orch_types::{Id, Time};

/// Groups containing only user/domain/nested-group edges need no permission-page proof. The
/// existing inverse query already decides these exactly. Follow only requested group edges;
/// any role member (including a Viewer source) keeps its whole ancestor chain on the live path.
pub(super) async fn plain_groups(
    conn: &mut Conn,
    requested: impl IntoIterator<Item = Id>,
) -> Result<std::collections::HashSet<Id>, Error> {
    use std::collections::{HashMap, HashSet};
    let mut seen = HashSet::new();
    let mut frontier: Vec<_> = requested
        .into_iter()
        .filter(|id| seen.insert(*id))
        .collect();
    let mut children: HashMap<Id, Vec<Id>> = HashMap::new();
    let mut dependent = HashSet::new();
    while !frontier.is_empty() {
        let mut next = Vec::new();
        for (group, members) in access::group_holders_many(conn, &frontier, None).await? {
            for member in members {
                if member.member_node_id.is_some() {
                    dependent.insert(group);
                }
                if let Some(child) = member.member_group_id {
                    children.entry(group).or_default().push(child);
                    if seen.insert(child) {
                        next.push(child);
                    }
                }
            }
        }
        frontier = next;
    }
    loop {
        let newly: Vec<_> = children
            .iter()
            .filter(|(group, children)| {
                !dependent.contains(*group)
                    && children.iter().any(|child| dependent.contains(child))
            })
            .map(|(group, _)| *group)
            .collect();
        if newly.is_empty() {
            break;
        }
        dependent.extend(newly);
    }
    seen.retain(|group| !dependent.contains(group));
    Ok(seen)
}

/// Final current membership for requested holders. The inverse SQL finds candidates; only the
/// same scoped source closure used for permission matching can confirm viewers/groups.
/// Filtering precedes any caller's count or pagination. Implicit role chips retain native Admin
/// identity, including nonviewer-role group branches; an explicit viewers audience never
/// acquires that identity as ordinary View.
pub async fn audience_people(
    conn: &mut Conn,
    sources: &[access::HolderPeopleQuery],
    now: Time,
) -> Result<Vec<(i64, Id)>, Error> {
    let mut holders = Vec::with_capacity(sources.len());
    for source in sources {
        holders.push(match (source.kind, source.id) {
            ("group", Some(id)) => Some(Holder::Group { id }),
            ("visible", Some(id)) => Some(Holder::Role {
                role: Role::Viewer,
                at: Some(id),
            }),
            ("role", Some(id)) => Some(Holder::Role {
                role: source.role.parse()?,
                at: Some(id),
            }),
            ("user", Some(id)) => Some(Holder::User { id }),
            ("members", Some(id)) => Some(Holder::Members { id }),
            // An unlocated system-default role chip keeps its existing non-thread domain.
            ("role" | "visible", None) => None,
            _ => return Err(Error::Stored("unknown holder audience".into())),
        });
    }
    let plain = plain_groups(
        conn,
        holders.iter().filter_map(|holder| match holder {
            Some(Holder::Group { id }) => Some(*id),
            _ => None,
        }),
    )
    .await?;
    let plain_roles = super::plain_roles::sources(conn, sources, now).await?;
    let candidates = access::holder_people_many(conn, sources, now.get()).await?;
    let mut by_person: std::collections::BTreeMap<Id, Vec<i64>> = std::collections::BTreeMap::new();
    let mut found = Vec::new();
    for (index, user) in candidates {
        let Some(slot) = usize::try_from(index - 1)
            .ok()
            .filter(|slot| *slot < sources.len())
        else {
            continue;
        };
        if plain_roles.contains(&slot)
            || matches!(holders[slot], Some(Holder::Group { id }) if plain.contains(&id))
        {
            found.push((index, user));
        } else if matches!(
            holders[slot],
            Some(Holder::Group { .. } | Holder::Role { .. })
        ) {
            by_person.entry(user).or_default().push(index);
        } else {
            // Direct users/domains and unlocated system-role chips have no dynamic viewer
            // dependency to re-evaluate; their existing candidate query is already exact.
            found.push((index, user));
        }
    }
    for (user, indices) in by_person {
        let requested: Vec<_> = indices
            .iter()
            .filter_map(|index| {
                usize::try_from(*index - 1)
                    .ok()
                    .and_then(|slot| holders.get(slot))
                    .copied()
                    .flatten()
            })
            .collect();
        let principal =
            Principal::load(conn, Caller::session(user).with_admin_mode(true), now).await?;
        let frame = Frame::for_holders(conn, &principal, &requested).await?;
        for index in indices {
            let Some(slot) = usize::try_from(index - 1)
                .ok()
                .filter(|slot| *slot < sources.len())
            else {
                continue;
            };
            let admitted = match holders[slot] {
                None => true,
                Some(Holder::Role {
                    role,
                    at: Some(place),
                }) if sources[slot].kind == "role" => {
                    frame.graph().site_of(place).is_some_and(|site| {
                        !principal.suspension().applies(&site.orgs)
                            // Only actual role chips retain the system's Admin identity;
                            // it is never a seed for the ordinary group/viewer closure.
                            && (principal.flag
                                || frame
                                    .role_at(place)
                                    .is_some_and(|held| held.role.includes(role)))
                    })
                }
                Some(holder) => frame.holder_matches(&holder, None),
            };
            if admitted {
                found.push((index, user));
            }
        }
    }
    found.sort_unstable();
    found.dedup();
    Ok(found)
}

/// Complete current membership of the requested groups, with ordinary explicit Viewer sources
/// and the existing identities of nonviewer roles.
pub async fn group_people(
    conn: &mut Conn,
    groups: &[Id],
    now: Time,
) -> Result<Vec<(Id, Id)>, Error> {
    let sources: Vec<_> = groups
        .iter()
        .map(|id| access::HolderPeopleQuery {
            kind: "group",
            id: Some(*id),
            role: "",
        })
        .collect();
    Ok(audience_people(conn, &sources, now)
        .await?
        .into_iter()
        .filter_map(|(index, user)| {
            usize::try_from(index - 1)
                .ok()
                .and_then(|slot| groups.get(slot))
                .map(|group| (*group, user))
        })
        .collect())
}

/// Role chip counts after current source membership is resolved. Unlocated system-default
/// chips keep their existing non-thread meaning; explicit viewers use actual ordinary View.
pub async fn role_head_counts(
    conn: &mut Conn,
    chips: &[(String, Option<Id>, bool)],
    now: Time,
) -> Result<Vec<i64>, Error> {
    let sources = chips
        .iter()
        .map(|(role, place, explicit)| {
            let role: Role = role.parse()?;
            Ok(access::HolderPeopleQuery {
                kind: if role == Role::Viewer && *explicit {
                    "visible"
                } else {
                    "role"
                },
                id: *place,
                role: role.as_str(),
            })
        })
        .collect::<Result<Vec<_>, Error>>()?;
    let mut counts = vec![0; chips.len()];
    for (index, _) in audience_people(conn, &sources, now).await? {
        if let Some(count) = usize::try_from(index - 1)
            .ok()
            .and_then(|slot| counts.get_mut(slot))
        {
            *count += 1;
        }
    }
    Ok(counts)
}

/// People specifically named by active role assignments at these places. The original inverse
/// query preserves target-path checks; the subject of each role must also match currently.
pub async fn given_role_people(
    conn: &mut Conn,
    places: &[Id],
    roles: &[String],
    now: Time,
) -> Result<Vec<(Id, Id)>, Error> {
    let rows: Vec<_> = access::roles_in_places(conn, places, now.get())
        .await?
        .into_iter()
        .filter(|row| roles.contains(&row.role))
        .collect();
    let sources = rows
        .iter()
        .map(|row| {
            let role = row
                .subject_role
                .as_deref()
                .map(str::parse::<Role>)
                .transpose()?;
            let kind = match row.subject_kind.as_str() {
                "user" => "user",
                "group" => "group",
                "members" => "members",
                "role" if role == Some(Role::Viewer) => "visible",
                "role" => "role",
                _ => return Err(Error::Stored("unknown role subject".into())),
            };
            Ok(access::HolderPeopleQuery {
                kind,
                id: Some(row.subject_id),
                role: role.map_or("", |role| role.as_str()),
            })
        })
        .collect::<Result<Vec<_>, Error>>()?;
    let valid: std::collections::HashSet<_> = audience_people(conn, &sources, now)
        .await?
        .into_iter()
        .filter_map(|(index, user)| {
            usize::try_from(index - 1)
                .ok()
                .and_then(|slot| rows.get(slot))
                .map(|row| (row.node_id, user))
        })
        .collect();
    Ok(
        access::people_given_role(conn, places, roles, now.get(), i64::MAX)
            .await?
            .into_iter()
            .filter(|pair| valid.contains(pair))
            .collect(),
    )
}

/// Complete object/person audiences after current holder checks. Existing SQL supplies target
/// suspension/deletion candidates; ownership membership remains specific to each source.
pub async fn holder_audiences(
    conn: &mut Conn,
    sources: &[access::HolderAudienceSource],
    paths: &[access::HolderAudiencePath<'_>],
    now: Time,
) -> Result<Vec<(Id, Id)>, Error> {
    let requested: Vec<_> = sources.iter().map(|source| source.holder).collect();
    let people = audience_people(conn, &requested, now).await?;
    let roots: std::collections::BTreeSet<_> = sources
        .iter()
        .filter_map(|source| source.owner_root)
        .collect();
    let users: Vec<_> = people
        .iter()
        .map(|(_, user)| *user)
        .collect::<std::collections::BTreeSet<_>>()
        .into_iter()
        .collect();
    let mut members = std::collections::HashMap::new();
    for root in roots {
        members.insert(
            root,
            access::members_among(conn, root, &users)
                .await?
                .into_iter()
                .collect::<std::collections::HashSet<_>>(),
        );
    }
    let valid: std::collections::HashSet<_> = people
        .into_iter()
        .filter_map(|(index, user)| {
            let source = usize::try_from(index - 1)
                .ok()
                .and_then(|slot| sources.get(slot))?;
            source
                .owner_root
                .is_none_or(|root| {
                    members
                        .get(&root)
                        .is_some_and(|people| people.contains(&user))
                })
                .then_some((source.object, user))
        })
        .collect();
    Ok(access::holder_audiences(conn, sources, paths, now.get())
        .await?
        .into_iter()
        .filter(|pair| valid.contains(pair))
        .collect())
}
