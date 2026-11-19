//! One visible holder search: people, custom groups, roles, members by domain and the work in a place.

use crate::middleware::authorization::refusals;
use crate::middleware::requests::{ApiError, ApiResult};
use crate::registry::Ctx;
use orch_access_contract::permissions::{NodePermission, PermissionName};
use orch_api_contract::access::{
    Holder, HolderCandidate, HolderInput, HolderSearchQuery, HolderSearchReply, Role,
};
use orch_api_contract::directory::Group;
use orch_api_contract::nodes::{NodeKind, NodeRef};
use orch_authz::{Act, Policy};
use orch_db::queries::{access, identity};
use orch_types::Id;
use std::collections::{HashMap, HashSet};

pub(crate) mod scoped;

/// The same native locator grammar feeds candidate discovery and presentation.
struct Words {
    text: String,
    members: bool,
    every: bool,
}

impl Words {
    fn new(query: &HolderSearchQuery) -> Self {
        let text = query.q.trim().to_lowercase();
        let (members, every, text) = if let Some(rest) = text
            .strip_prefix("members of ")
            .or_else(|| text.strip_prefix("成员 "))
        {
            (true, false, rest.to_owned())
        } else if let Some(rest) = ["every ", "everyone ", "all ", "所有人 ", "全部 "]
            .iter()
            .find_map(|prefix| text.strip_prefix(prefix))
        {
            (false, true, rest.to_owned())
        } else {
            (false, false, text)
        };
        Self {
            text,
            members,
            every,
        }
    }

    fn parts(&self) -> (&str, &str) {
        self.text
            .split_once(' ')
            .map_or((self.text.as_str(), ""), |(path, tail)| (path, tail.trim()))
    }
}

/// A suffix need not start at the root. A partial last short name completes to a visible place.
fn address_matches(address: &str, wanted: &str) -> bool {
    address == wanted
        || address.ends_with(&format!(".{wanted}"))
        || address.split('.').enumerate().any(|(offset, _)| {
            address
                .split('.')
                .skip(offset)
                .collect::<Vec<_>>()
                .join(".")
                .starts_with(wanted)
        })
}

fn role_word(word: &str) -> Option<Role> {
    match word {
        "viewer" | "viewers" | "查看者" | "浏览者" => Some(Role::Viewer),
        "interactor" | "interactors" | "参与者" => Some(Role::Interactor),
        "developer" | "developers" | "开发者" => Some(Role::Developer),
        "admin" | "admins" | "administrator" | "管理员" => Some(Role::Admin),
        _ => None,
    }
}

fn role_locator(role: Role) -> &'static str {
    match role {
        Role::Viewer => "viewer",
        Role::Interactor => "interactor",
        Role::Developer => "developer",
        Role::Admin => "admin",
    }
}

fn role_completes(role: Role, word: &str) -> bool {
    let words: &[&str] = match role {
        Role::Viewer => &["viewer", "viewers", "查看者", "浏览者"],
        Role::Interactor => &["interactor", "interactors", "参与者"],
        Role::Developer => &["developer", "developers", "开发者"],
        Role::Admin => &["admin", "admins", "administrator", "管理员"],
    };
    words.iter().any(|known| known.starts_with(word))
}

fn human_place(
    place: &access::HolderPlaceRow,
    by_id: &HashMap<Id, &access::HolderPlaceRow>,
    visible: &HashSet<Id>,
) -> String {
    place
        .path
        .iter()
        .map(|id| {
            by_id
                .get(id)
                .filter(|_| visible.contains(id))
                .map_or("a private place", |row| row.leaf_name.as_str())
        })
        .collect::<Vec<_>>()
        .join(" > ")
}

fn safe_address(place: &access::HolderPlaceRow, visible: &HashSet<Id>) -> String {
    let skipped = place
        .path
        .iter()
        .rposition(|id| !visible.contains(id))
        .map_or(0, |offset| offset + 1);
    place
        .address
        .split('.')
        .skip(skipped)
        .collect::<Vec<_>>()
        .join(".")
}

/// Searching changes nothing. Only the result chosen by the screen supplies a holder to a writer.
pub(crate) async fn search(
    ctx: Ctx,
    query: HolderSearchQuery,
    (): (),
) -> ApiResult<HolderSearchReply> {
    crate::key_operations::native::selected_read::<scoped::Holders>(ctx, query, ()).await
}

async fn select(
    ctx: &Ctx,
    conn: &mut orch_db::Conn,
    caller: &orch_authz::Caller,
    query: &HolderSearchQuery,
) -> ApiResult<(
    scoped::Selected,
    Vec<crate::key_operations::native::Facts<orch_api_contract::access_sources::Source>>,
)> {
    let words = Words::new(query);
    let all = access::holder_places(conn).await?;
    let mut frame = scoped::Frame::load(ctx, conn, caller, query, &words, &all).await?;
    let principal = frame.principal(ctx, conn, caller).await?;
    let ids: Vec<Id> = all.iter().map(|place| place.id).collect();
    let visible: HashSet<Id> = orch_authz::nodes::holding(
        conn,
        &principal,
        &ids,
        PermissionName::OfNode(NodePermission::View),
        Act::Use,
        Policy::default(),
    )
    .await
    .map_err(ApiError::internal)?
    .into_iter()
    .map(|place| place.id)
    .collect();
    let by_id: HashMap<Id, &access::HolderPlaceRow> =
        all.iter().map(|place| (place.id, place)).collect();
    // Members may know the names of org ancestors without their full View; keep that normal name cut.
    let known: HashSet<Id> =
        crate::routes::nodes::view::refs(conn, &principal, &ids, Policy::default())
            .await?
            .into_iter()
            .filter_map(|(id, node)| (!node.redacted).then_some(id))
            .filter(|id| frame.allows_name(*id))
            .collect();
    let mut start = query.place_id;
    if let Some(id) = start
        && !by_id.contains_key(&id)
    {
        start = orch_db::queries::hierarchy::with_ancestors(conn, &[id])
            .await?
            .iter()
            .filter(|node| node.kind != "thread")
            .max_by_key(|node| node.path.len())
            .map(|node| node.id);
    }
    let home = query
        .org_id
        .and_then(|id| by_id.get(&id).map(|place| place.root))
        .or_else(|| start.and_then(|id| by_id.get(&id).map(|place| place.root)));
    let (members, every, text) = (words.members, words.every, words.text.as_str());
    let (path, tail) = words.parts();
    let wanted_role = role_word(tail);
    let direct_role = role_word(path).filter(|_| tail.is_empty());
    let completing_role = tail.is_empty()
        && !members
        && !every
        && !path.is_empty()
        && [Role::Viewer, Role::Interactor, Role::Developer, Role::Admin]
            .into_iter()
            .any(|role| role_completes(role, path));
    let listed = query.q.ends_with(' ') || text.is_empty();
    let mut matched: Vec<_> = all
        .iter()
        .filter(|place| {
            visible.contains(&place.id)
                && if text.is_empty() && query.list_places == Some(true) {
                    home.is_none_or(|root| place.root == root)
                } else if text.is_empty() || direct_role.is_some() || completing_role {
                    Some(place.id) == start
                } else {
                    address_matches(&safe_address(place, &known), path)
                        || (tail.is_empty() && place.leaf_name.to_lowercase().contains(path))
                }
        })
        .collect();
    if !text.is_empty()
        && direct_role.is_none()
        && !completing_role
        && matched.iter().any(|place| {
            let address = safe_address(place, &known);
            address == path || address.ends_with(&format!(".{path}"))
        })
    {
        matched.retain(|place| {
            let address = safe_address(place, &known);
            address == path || address.ends_with(&format!(".{path}"))
        });
    }
    let group_rows = frame
        .groups
        .iter()
        .filter(|group| visible.contains(&group.org_id))
        .cloned()
        .collect();
    let groups = crate::routes::directory::groups_named(conn, &principal, group_rows).await?;
    let mut items = Vec::new();
    for place in &matched {
        let human = human_place(place, &by_id, &known);
        let address = safe_address(place, &known);
        let kind = refusals::node_kind(&place.kind)
            .ok_or_else(|| ApiError::internal("a holder place has no kind"))?;
        let node = NodeRef {
            thread_display: None,
            id: place.id,
            kind,
            name: Some(human.clone()),
            redacted: false,
        };
        let outside = home.is_some_and(|root| root != place.root);
        let mut add = |input, holder, locator| {
            if !frame.allows_node(place.id) {
                return;
            }
            frame.record_node(place, &known);
            items.push(HolderCandidate {
                input,
                holder,
                locator,
                place: human.clone(),
                outside,
            })
        };
        if query.list_places == Some(true) {
            add(
                HolderInput::Role {
                    role: Role::Viewer,
                    at: Some(place.id),
                },
                Holder::Role {
                    role: Role::Viewer,
                    at: Some(node),
                    place: Some(human.clone()),
                    count: 0,
                },
                address,
            );
            continue;
        }
        if members {
            if kind == NodeKind::Org {
                add(
                    HolderInput::Members { id: place.id },
                    Holder::Members { node: node.clone() },
                    format!("members of {address}"),
                );
            }
            continue;
        }
        {
            let roles: Vec<Role> = if every {
                vec![Role::Viewer]
            } else if listed {
                vec![Role::Viewer, Role::Interactor, Role::Developer, Role::Admin]
            } else if let Some(role) = wanted_role.or(direct_role) {
                vec![role]
            } else if completing_role {
                [Role::Viewer, Role::Interactor, Role::Developer, Role::Admin]
                    .into_iter()
                    .filter(|role| role_completes(*role, path))
                    .collect()
            } else if tail.is_empty() {
                vec![Role::Viewer]
            } else {
                [Role::Viewer, Role::Interactor, Role::Developer, Role::Admin]
                    .into_iter()
                    .filter(|role| role_completes(*role, tail))
                    .collect()
            };
            for role in roles {
                add(
                    HolderInput::Role {
                        role,
                        at: Some(place.id),
                    },
                    Holder::Role {
                        role,
                        at: Some(node.clone()),
                        place: Some(human.clone()),
                        count: 0,
                    },
                    format!("{address} {}", role_locator(role)),
                );
            }
            if listed && kind == NodeKind::Org {
                add(
                    HolderInput::Members { id: place.id },
                    Holder::Members { node: node.clone() },
                    format!("members of {address}"),
                );
            }
            if listed && query.include_work == Some(true) {
                add(
                    HolderInput::Node { id: place.id },
                    Holder::Node { node: node.clone() },
                    format!("work in {address}"),
                );
            }
        }
        if !every && (listed || !tail.is_empty()) {
            let exact_group = !tail.is_empty()
                && groups
                    .iter()
                    .any(|group| group.org_id == place.org && group.name.to_lowercase() == tail);
            for group in groups.iter().filter(|group| {
                group.org_id == place.org
                    && (tail.is_empty()
                        || if exact_group {
                            group.name.to_lowercase() == tail
                        } else {
                            group.name.to_lowercase().starts_with(tail)
                        })
            }) {
                frame.record_group(group.id, group.org_id, &place.path, &by_id, &known);
                let group_place = by_id
                    .get(&group.org_id)
                    .map_or(human.clone(), |org| human_place(org, &by_id, &known));
                items.push(HolderCandidate {
                    input: HolderInput::Group { id: group.id },
                    holder: Holder::Group {
                        group: Group {
                            id: group.id,
                            org_id: group.org_id,
                            place: Some(group_place.clone()),
                            name: group.name.clone(),
                            description: group.description.clone(),
                            member_count: group.member_count,
                        },
                    },
                    locator: format!("{address} {}", group.name),
                    place: group_place,
                    outside,
                });
            }
        }
    }
    // A bare name also searches custom groups across all visible places.
    if query.list_places != Some(true)
        && !members
        && !every
        && tail.is_empty()
        && !path.contains('.')
        && !path.starts_with('@')
        && !path.is_empty()
        && direct_role.is_none()
    {
        for group in groups
            .iter()
            .filter(|group| group.name.to_lowercase().starts_with(path))
        {
            let Some(place) = by_id.get(&group.org_id) else {
                continue;
            };
            let human = human_place(place, &by_id, &known);
            frame.record_group(group.id, group.org_id, &place.path, &by_id, &known);
            items.push(HolderCandidate {
                input: HolderInput::Group { id: group.id },
                holder: Holder::Group {
                    group: Group {
                        id: group.id,
                        org_id: group.org_id,
                        place: Some(human.clone()),
                        name: group.name.clone(),
                        description: group.description.clone(),
                        member_count: group.member_count,
                    },
                },
                locator: format!("{} {}", safe_address(place, &known), group.name),
                place: human,
                outside: home.is_some_and(|root| root != place.root),
            });
        }
    }
    if query.list_places != Some(true)
        && !members
        && !every
        && tail.is_empty()
        && !path.is_empty()
        && direct_role.is_none()
    {
        let is_mail = path.contains('@') && !path.starts_with('@');
        let is_address = path.contains('.') && !is_mail;
        let people: Vec<_> = frame.people.iter().map(|row| row.user.clone()).collect();
        let user_ids: Vec<Id> = people.iter().map(|person| person.id).collect();
        let refs = refusals::user_refs(conn, &user_ids).await?;
        let addresses = access::holder_user_addresses(conn, &user_ids).await?;
        let local: HashSet<Id> = match home {
            Some(root) => access::local_holders_among(conn, root, &user_ids)
                .await?
                .into_iter()
                .collect(),
            None => user_ids.iter().copied().collect(),
        };
        for person in people {
            let mut person_addresses: Vec<_> = addresses
                .iter()
                .filter(|(user, _, domain, _)| *user == person.id && visible.contains(domain))
                .map(|(_, root, domain, address)| {
                    let safe = by_id.get(domain).map_or_else(
                        || format!("@{}", person.username),
                        |place| {
                            let prefix = safe_address(place, &known);
                            if prefix.is_empty() {
                                format!("@{}", person.username)
                            } else {
                                format!("{prefix}.{}", person.username)
                            }
                        },
                    );
                    (
                        *root,
                        *domain,
                        if safe.is_empty() {
                            address.clone()
                        } else {
                            safe
                        },
                    )
                })
                .collect();
            person_addresses
                .sort_by_key(|(root, _, address)| (Some(*root) != home, address.clone()));
            if is_address {
                person_addresses.retain(|(_, _, address)| {
                    address == path || address.ends_with(&format!(".{path}"))
                });
            }
            if is_address && person_addresses.is_empty() {
                continue;
            }
            let Some(user) = refs.get(&person.id) else {
                continue;
            };
            frame.record_account(
                person.id,
                person_addresses.first().map(|(_, domain, _)| *domain),
                &by_id,
                &known,
            );
            let address = person_addresses.first().map_or_else(
                || format!("@{}", person.username),
                |(_, _, address)| address.clone(),
            );
            let mut user = user.clone();
            user.address = Some(address.clone());
            items.push(HolderCandidate {
                input: HolderInput::User { id: person.id },
                holder: Holder::User { user },
                locator: address.clone(),
                place: address,
                outside: !local.contains(&person.id),
            });
        }
    }
    let mut seen = HashSet::new();
    items.retain(|item| seen.insert(serde_json::to_string(&item.input).unwrap_or_default()));
    items.sort_by(|one, other| {
        one.outside
            .cmp(&other.outside)
            .then_with(|| one.place.cmp(&other.place))
            .then_with(|| one.locator.cmp(&other.locator))
    });
    let (witness, facts) = frame.finish(ctx, caller)?;
    Ok((
        scoped::Selected {
            reply: HolderSearchReply { items },
            witness,
        },
        facts,
    ))
}
