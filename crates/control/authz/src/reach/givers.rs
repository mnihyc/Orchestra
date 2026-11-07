//! A request's prepared ordinary giving decisions, reused by every matching contribution.

use super::{AREAS, Agent};
use crate::engine::{Frame, Graph, PageRef, Site, Store};
use crate::giving::{Acting, Giving};
use crate::{Caller, Error, Policy, Principal};
use orch_access_contract::pages::PageKind;
use orch_access_contract::permissions::{ObjectKind, Permission};
use orch_db::Conn;
use orch_types::{Id, Time};
use std::collections::{BTreeSet, HashMap, HashSet};

#[derive(Debug, Default)]
pub(super) struct Givers {
    sites: HashMap<(Id, ObjectKind, Id), HashSet<Permission>>,
    chats: HashMap<(Id, Id), HashSet<Permission>>,
}

fn settings_error(error: orch_settings::Error) -> Error {
    match error {
        orch_settings::Error::Db(error) => Error::Db(error),
        other => Error::Stored(other.to_string()),
    }
}

impl Givers {
    #[allow(
        clippy::too_many_arguments,
        reason = "one prepared target and contribution batch"
    )]
    pub(super) fn load(
        conn: &mut Conn,
        agent: &Agent,
        graph: &Graph,
        store: &Store,
        pages: &[PageRef],
        sites: &[Site],
        people_chats: &[orch_db::queries::chats::ChatRow],
        now: Time,
    ) -> impl std::future::Future<Output = Result<Self, Error>> {
        // A projection's giver batch must not embed every ordinary account frame.
        Box::pin(Self::prepare(
            conn,
            agent,
            graph,
            store,
            pages,
            sites,
            people_chats,
            now,
        ))
    }

    #[allow(
        clippy::too_many_arguments,
        reason = "one prepared target and contribution batch"
    )]
    async fn prepare(
        conn: &mut Conn,
        agent: &Agent,
        graph: &Graph,
        store: &Store,
        pages: &[PageRef],
        sites: &[Site],
        people_chats: &[orch_db::queries::chats::ChatRow],
        now: Time,
    ) -> Result<Self, Error> {
        let givers: BTreeSet<_> = pages
            .iter()
            .flat_map(|page| store.rows_of(*page))
            .flat_map(|row| &row.entries)
            .filter(|entry| entry.counts(now) && entry.holder.is_agents_on(&agent.path))
            .filter_map(|entry| entry.created_by)
            .collect();
        if givers.is_empty() {
            return Ok(Self::default());
        }
        let givers: Vec<_> = givers.into_iter().collect();
        let active = orch_db::queries::access::active_givers(conn, &givers).await?;
        let mut nodes: BTreeSet<_> = sites
            .iter()
            .filter_map(|site| {
                site.path
                    .as_ref()
                    .and_then(|path| path.last().copied())
                    .or(site.beside)
            })
            .collect();
        // A vanished chat home supplies no contribution and must not poison other live sources.
        nodes.extend(
            people_chats
                .iter()
                .filter_map(|chat| graph.site_of(chat.org_id).map(|site| site.id)),
        );
        let nodes: Vec<_> = nodes.into_iter().collect();
        let holders =
            orch_settings::resolver::get_at_nodes::<bool>(conn, &nodes, "sharing.allow_holders")
                .await
                .map_err(settings_error)?;
        let external =
            orch_settings::resolver::get_at_nodes::<bool>(conn, &nodes, "sharing.allow_external")
                .await
                .map_err(settings_error)?;
        let permissions: Vec<_> = AREAS
            .iter()
            .chain(Permission::EXPORTS.iter())
            .copied()
            .collect();
        let mut frame_permissions = permissions.clone();
        frame_permissions.extend([
            Permission::AccessManage,
            Permission::AccessOwn,
            Permission::AccessExternal,
        ]);
        let chat_ids: Vec<_> = people_chats.iter().map(|chat| chat.id).collect();
        let mut prepared = Self::default();
        for giver in active {
            let principal =
                match Principal::load(conn, Caller::session(giver).with_admin_mode(false), now)
                    .await
                {
                    Ok(principal) => principal,
                    Err(Error::NoSuchUser(_)) => continue,
                    Err(error) => return Err(error),
                };
            let frame = Frame::load(
                conn,
                &principal,
                Policy::default(),
                graph.clone(),
                sites,
                &frame_permissions,
            )
            .await?;
            for site in sites {
                let node = site
                    .path
                    .as_ref()
                    .and_then(|path| path.last().copied())
                    .or(site.beside);
                let policy = Policy {
                    holders_share: node
                        .and_then(|node| holders.get(&node).copied().flatten())
                        .unwrap_or(true),
                    owners_name_outsiders: node
                        .and_then(|node| external.get(&node).copied().flatten())
                        .unwrap_or(true),
                };
                let giving = Giving::of(&frame, Some(site), PageKind::Live, Acting::Share)
                    .with_policy(policy);
                let can_name = site.root == Some(agent.root)
                    || crate::external::may_name_outside(&frame, site, policy);
                let held = permissions
                    .iter()
                    .copied()
                    .filter(|permission| {
                        can_name
                            && giving
                                .can_give(*permission, frame.graph().profile(site, *permission))
                    })
                    .collect();
                prepared.sites.insert((giver, site.kind, site.id), held);
            }
            if !chat_ids.is_empty() {
                let members =
                    orch_db::queries::search::participant_chats(conn, giver, &chat_ids).await?;
                for member in members {
                    let Some(root) = graph.site_of(member.org_id) else {
                        continue;
                    };
                    let Some(chat) = people_chats.iter().find(|chat| chat.id == member.id) else {
                        continue;
                    };
                    let can_name = agent.root == chat.org_id
                        || !frame.ways(&root, Permission::AccessExternal).is_empty()
                        || (chat.created_by == Some(giver)
                            && external
                                .get(&chat.org_id)
                                .copied()
                                .flatten()
                                .unwrap_or(true));
                    let held = [
                        Permission::ChatView,
                        Permission::ChatInteract,
                        Permission::ChatExport,
                    ]
                    .into_iter()
                    .filter(|permission| {
                        can_name
                            && crate::chats::ordinary_participant(
                                &principal,
                                &root,
                                &member,
                                *permission,
                            )
                    })
                    .collect();
                    prepared.chats.insert((giver, member.id), held);
                }
            }
        }
        Ok(prepared)
    }

    pub(super) fn at(&self, giver: Option<Id>, site: &Site, permission: Permission) -> bool {
        giver
            .and_then(|giver| self.sites.get(&(giver, site.kind, site.id)))
            .is_some_and(|held| held.contains(&permission))
    }

    pub(super) fn in_chat(&self, giver: Option<Id>, chat: Id, permission: Permission) -> bool {
        giver
            .and_then(|giver| self.chats.get(&(giver, chat)))
            .is_some_and(|held| held.contains(&permission))
    }
}
