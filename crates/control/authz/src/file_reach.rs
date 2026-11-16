//! The path limits of current file entries, before native machine ceilings and door availability.

use crate::engine::{Frame, Site};
use crate::{Error, Policy, Principal, options, resources};
use orch_access_contract::decisions::{ObjectRef, Through};
use orch_access_contract::permissions::{FileReach, ObjectKind, Options, Permission};
use orch_db::Conn;
use orch_types::Id;

/// Read and mutation stay separate: a wider read entry never widens a write entry.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Held {
    pub view: Option<FileReach>,
    pub manage: Option<FileReach>,
    /// This same credential can exercise Terminal Manage, not only its underlying account.
    pub terminal: bool,
}

fn permission(
    frame: &Frame<'_>,
    site: &Site,
    permission: Permission,
) -> Result<Option<FileReach>, Error> {
    let ways = frame.ways(site, permission);
    if ways.is_empty() {
        return Ok(None);
    }
    if ways.iter().any(|way| {
        matches!(
            way,
            Through::Owner | Through::SourceOwner { .. } | Through::Admin | Through::SystemAdmin
        )
    }) {
        return Ok(Some(FileReach::Machine));
    }
    let mut reach = FileReach::Roots;
    for level in std::iter::once(permission).chain(permission.held_through()) {
        for named in frame.matching(site, level, None) {
            if let Options::Files(entry) = options::read(level, named.entry)? {
                reach = reach.max(entry.reach);
            }
        }
    }
    Ok(Some(reach))
}

/// Uses the same counted entries, expiry, locks and credential restriction as the ordinary decision.
pub fn in_frame(frame: &Frame<'_>, site: &Site) -> Result<Held, Error> {
    Ok(Held {
        view: permission(frame, site, Permission::FilesView)?,
        manage: permission(frame, site, Permission::FilesManage)?,
        terminal: !frame.ways(site, Permission::TerminalManage).is_empty(),
    })
}

/// Reads one environment's current path holdings; it grants neither an absent permission nor a shell.
pub async fn held(
    conn: &mut Conn,
    principal: &Principal,
    environment: Id,
    policy: Policy,
) -> Result<Held, Error> {
    let (graph, _, site) = resources::site(
        conn,
        ObjectRef {
            kind: ObjectKind::Environment,
            id: environment,
        },
    )
    .await?;
    let frame = Frame::load(
        conn,
        principal,
        policy,
        graph,
        std::slice::from_ref(&site),
        &[
            Permission::FilesView,
            Permission::FilesManage,
            Permission::TerminalManage,
        ],
    )
    .await?;
    in_frame(&frame, &site)
}

impl Held {
    /// Terminal changes path geometry only for acts this credential already holds.
    #[must_use]
    pub fn resolved(
        self,
        terminal_available: bool,
        machine_bound: FileReach,
    ) -> orch_access_contract::permissions::FileReachFacts {
        let terminal = self.terminal && terminal_available && machine_bound == FileReach::Machine;
        let widen = |reach: Option<FileReach>| {
            reach.map(|reach| if terminal { FileReach::Machine } else { reach }.min(machine_bound))
        };
        orch_access_contract::permissions::FileReachFacts {
            view: widen(self.view),
            manage: widen(self.manage),
            view_from_terminal: terminal && self.view == Some(FileReach::Roots),
            manage_from_terminal: terminal && self.manage == Some(FileReach::Roots),
            machine_bound,
        }
    }
}

/// One current machine ceiling, shared by file admission and unconfined human shells.
pub async fn machine_bound(conn: &mut Conn, environment: Id) -> Result<FileReach, Error> {
    Ok(orch_db::queries::environment_facts::of(conn, environment)
        .await?
        .and_then(|machine| machine.policy)
        .and_then(|policy| policy.get("environment.files_reach").cloned())
        .map(|value| {
            serde_json::from_value(value).map_err(|error| Error::Stored(error.to_string()))
        })
        .transpose()?
        .unwrap_or(FileReach::Roots))
}

/// A human shell has no native roots sandbox; its usable reach is always machine.
pub async fn terminal_available(
    conn: &mut Conn,
    environment: Id,
    bound: FileReach,
) -> Result<bool, Error> {
    if bound != FileReach::Machine {
        return Ok(false);
    }
    let Some(site) = orch_db::queries::terminals::site(conn, environment).await? else {
        return Ok(false);
    };
    if !site.terminal
        || site.status != "running"
        || site.machine_id.is_none()
        || site.machine_online != Some(true)
    {
        return Ok(false);
    }
    if orch_db::queries::environment_facts::refuses(conn, environment, "environment.terminal")
        .await?
    {
        return Ok(false);
    }
    Ok(orch_settings::get::<bool>(
        conn,
        orch_settings::Place::Environment(environment),
        "terminal.enabled",
    )
    .await
    .map_err(|error| Error::Stored(error.to_string()))?
    .unwrap_or(true))
}

/// Every human file act and its source line use the same current authority and machine bound.
pub async fn effective(
    conn: &mut Conn,
    principal: &Principal,
    environment: Id,
    policy: Policy,
) -> Result<orch_access_contract::permissions::FileReachFacts, Error> {
    let held = held(conn, principal, environment, policy).await?;
    let bound = machine_bound(conn, environment).await?;
    let terminal = held.terminal && terminal_available(conn, environment, bound).await?;
    Ok(held.resolved(terminal, bound))
}
