//! The skills a thread's agent may use now: those its places switch on and its starter's own skills where the place allows them, each at the version it is given and checked, at the moment it is asked, against the use of whoever named it.
//!
//! `agents.skills` is resolved down the thread's places, member by member, a lower place changing or
//! switching off what one above named, and a lock above keeping the rows below out. Each skill a row
//! names is given only while whoever last wrote that row may use it for this thread (`skill.use` on
//! its page: as an owner, by an entry that names them, or by one that names the thread or a place it
//! lies in), so that a person who loses the use, or a skill whose page stops naming the place, stops
//! reaching the thread at its next session without anybody rewriting the setting. Where
//! `agents.personal_skills` allows them, the skills of the thread's starter's own org follow, each at
//! what `agents.skills` resolves to there (its latest version unless the person pinned or switched
//! one off), checked against the starter's own use. Two skills of one name never reach one agent: the
//! one a place gave, the nearest place first, is kept.

use std::cmp::Reverse;
use std::collections::{BTreeMap, HashMap, HashSet};

use orch_api_contract::skills::{
    SkillArchive, SkillFile, SkillSkip, SkippedSkill, ThreadSkill, ThreadSkillFrom, ThreadSkills,
};
use orch_db::Conn;
use orch_db::queries::resources::skills::{self as rows, SkillRow, VersionRow};
use orch_settings::{Place, Snapshot, Wanted};
use orch_settings_contract::registry::{self, keys};
use orch_types::{Id, Json, Time};

use crate::EngineError;
use orch_authz::usable::{self, Policies, Usable};
use orch_authz::{Caller, Principal};

/// What a place gives its threads of one skill.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Choice {
    /// Nothing.
    Off,
    /// Its latest version.
    Latest,
    /// That version.
    Pinned(Id),
}

impl Choice {
    fn read(value: &Json) -> Option<Self> {
        match value.as_str()? {
            "off" => Some(Self::Off),
            "latest" => Some(Self::Latest),
            other => other.parse().ok().map(Self::Pinned),
        }
    }
}

/// One skill named for the thread, before it is checked.
#[derive(Debug, Clone)]
struct Named {
    skill: Id,
    choice: Choice,
    /// Whose use of it is asked.
    writer: Id,
    from: ThreadSkillFrom,
    /// Whether it is one of the starter's own skills, which come after every place's.
    personal: bool,
    /// How deep the place that named it lies: the nearer the thread, the deeper.
    depth: usize,
}

/// The members of `agents.skills` resolved at `place`, each with the place whose row named it last
/// and that row's writer; a row nobody wrote is answered for by its place's first owner.
async fn named_at(
    conn: &mut Conn,
    place: Id,
) -> Result<BTreeMap<Id, (Choice, Id, Id, usize)>, EngineError> {
    let def = registry::get(keys::AGENTS_SKILLS)
        .ok_or_else(|| EngineError::unreadable("the skills setting is not registered"))?;
    let snapshot = Snapshot::load(conn, Place::Node(place), Wanted::Keys(&[def])).await?;
    let resolved = snapshot.resolve(def);
    let chain: Vec<Id> = snapshot.chain().iter().map(|link| link.id).collect();
    let writers: HashMap<Id, Option<Id>> = rows::setting_writers(conn, &chain)
        .await?
        .into_iter()
        .collect();
    let owners: HashMap<Id, Id> = orch_db::queries::hierarchy::with_ancestors(conn, &[place])
        .await?
        .into_iter()
        .map(|node| (node.id, node.owner_id))
        .collect();
    let mut named = BTreeMap::new();
    for part in &resolved.parts {
        let Some(node) = part.source.node else {
            continue;
        };
        let depth = chain.iter().position(|id| *id == node).unwrap_or_default();
        let Some(writer) = writers
            .get(&node)
            .copied()
            .flatten()
            .or_else(|| owners.get(&node).copied())
        else {
            continue;
        };
        for (skill, value) in part.value.as_object().into_iter().flatten() {
            if let (Ok(skill), Some(choice)) = (skill.parse::<Id>(), Choice::read(value)) {
                named.insert(skill, (choice, node, writer, depth));
            }
        }
    }
    Ok(named)
}

/// Whether `agents.personal_skills` lets the own skills of a thread's starter reach it.
async fn personal_allowed(conn: &mut Conn, thread: Id) -> Result<bool, EngineError> {
    let def = registry::get(keys::AGENTS_PERSONAL_SKILLS)
        .ok_or_else(|| EngineError::unreadable("the personal skills setting is not registered"))?;
    let snapshot = Snapshot::load(conn, Place::Node(thread), Wanted::Keys(&[def])).await?;
    Ok(snapshot
        .resolve(def)
        .value
        .or_else(|| def.default_value())
        .and_then(|value| value.as_bool())
        .unwrap_or(true))
}

/// The skills the thread `thread`'s agent may use now, with those named but left out and why.
pub async fn for_thread(
    conn: &mut Conn,
    thread: Id,
    at: Time,
) -> Result<ThreadSkills, EngineError> {
    let mut named: Vec<Named> = named_at(conn, thread)
        .await?
        .into_iter()
        .map(|(skill, (choice, node, writer, depth))| Named {
            skill,
            choice,
            writer,
            from: ThreadSkillFrom::Place { node_id: node },
            personal: false,
            depth,
        })
        .collect();
    let by_place: HashSet<Id> = named.iter().map(|each| each.skill).collect();
    if personal_allowed(conn, thread).await?
        && let Some((starter, Some(own))) = rows::thread_starter(conn, thread).await?
    {
        let choices = named_at(conn, own).await?;
        for skill in rows::in_org(conn, own).await? {
            if by_place.contains(&skill.id) {
                continue;
            }
            let choice = choices
                .get(&skill.id)
                .map_or(Choice::Latest, |(choice, ..)| *choice);
            named.push(Named {
                skill: skill.id,
                choice,
                writer: starter,
                from: ThreadSkillFrom::Personal { user_id: starter },
                personal: true,
                depth: 0,
            });
        }
    }
    named.retain(|each| each.choice != Choice::Off);
    let ids: Vec<Id> = named.iter().map(|each| each.skill).collect();
    let skills: HashMap<Id, SkillRow> = rows::any_skills(conn, &ids)
        .await?
        .into_iter()
        .map(|row| (row.id, row))
        .collect();
    let wanted: Vec<Id> = named
        .iter()
        .filter_map(|each| match each.choice {
            Choice::Pinned(version) => Some(version),
            Choice::Latest => skills
                .get(&each.skill)
                .and_then(|row| row.latest_version_id),
            Choice::Off => None,
        })
        .collect();
    let versions: HashMap<Id, VersionRow> = rows::versions_by_id(conn, &wanted)
        .await?
        .into_iter()
        .map(|row| (row.id, row))
        .collect();
    // The nearest place first, then by identifier, so that a name kept is always the same one.
    named.sort_by_key(|each| (each.personal, Reverse(each.depth), each.skill));
    let mut given: Vec<ThreadSkill> = Vec::new();
    let mut skipped: Vec<SkippedSkill> = Vec::new();
    let mut names: HashSet<String> = HashSet::new();
    let mut usable: HashMap<(Id, Id), bool> = HashMap::new();
    for each in named {
        let skip = |reason: SkillSkip, name: Option<String>| SkippedSkill {
            skill_id: each.skill,
            name,
            reason,
            from: each.from.clone(),
        };
        let Some(skill) = skills.get(&each.skill) else {
            skipped.push(skip(SkillSkip::Missing, None));
            continue;
        };
        let name = Some(skill.name.clone());
        if skill.deleted_at.is_some() {
            skipped.push(skip(SkillSkip::Deleted, name));
            continue;
        }
        if !skill.enabled {
            skipped.push(skip(SkillSkip::Disabled, name));
            continue;
        }
        let version = match each.choice {
            Choice::Pinned(id) => match versions.get(&id).filter(|row| row.skill_id == skill.id) {
                Some(row) => row,
                None => {
                    skipped.push(skip(SkillSkip::VersionNotFound, name));
                    continue;
                }
            },
            _ => match skill.latest_version_id.and_then(|id| versions.get(&id)) {
                Some(row) => row,
                None => {
                    skipped.push(skip(SkillSkip::NoVersion, name));
                    continue;
                }
            },
        };
        let may = match usable.get(&(each.writer, skill.id)) {
            Some(may) => *may,
            None => {
                let may = if each.personal {
                    // The starter's own skill is asked of the starter, wherever they are a member.
                    own_use(conn, each.writer, skill.id, thread, at).await?
                } else {
                    crate::mcp::source_may_use(conn, each.writer, skill.id, thread, at).await?
                };
                usable.insert((each.writer, skill.id), may);
                may
            }
        };
        if !may {
            skipped.push(skip(SkillSkip::NotUsable, name));
            continue;
        }
        if !names.insert(version.name.clone()) {
            skipped.push(skip(SkillSkip::NameTaken, name));
            continue;
        }
        let files: Vec<SkillFile> = serde_json::from_value(version.files.clone())?;
        given.push(ThreadSkill {
            skill_id: skill.id,
            version_id: version.id,
            number: i64::from(version.number),
            name: version.name.clone(),
            description: version.description.clone(),
            archive: SkillArchive {
                blob: version.archive_hash()?,
                size: version.archive_size,
            },
            files,
            from: each.from.clone(),
        });
    }
    given.sort_by(|left, right| left.name.cmp(&right.name));
    Ok(ThreadSkills {
        skills: given,
        skipped,
    })
}

/// Whether `user` may use the skill `skill` for the thread `thread` themself.
async fn own_use(
    conn: &mut Conn,
    user: Id,
    skill: Id,
    thread: Id,
    at: Time,
) -> Result<bool, EngineError> {
    let principal = match Principal::load(conn, Caller::session(user), at).await {
        Ok(principal) => principal,
        Err(orch_authz::Error::NoSuchUser(_)) => return Ok(false),
        Err(error) => return Err(error.into()),
    };
    Ok(matches!(
        usable::usable(conn, &principal, skill, Some(thread), &Policies::default()).await?,
        Usable::Yes(_)
    ))
}

/// Records on the turn `turn` the versions of `skills` it was given when it started.
pub async fn record_turn(
    conn: &mut Conn,
    turn: Id,
    skills: &ThreadSkills,
) -> Result<bool, EngineError> {
    let versions: Vec<Id> = skills.skills.iter().map(|skill| skill.version_id).collect();
    Ok(rows::record_turn(conn, turn, &versions).await?)
}
