//! Current policy metadata uses the same resolver as native skill selection, without opening skills.
use crate::middleware::requests::{ApiError, ApiResult};
use orch_api_contract::share::PersonalSkillsPolicy;
use orch_authz::Principal;
use orch_db::Conn;
use orch_settings::{Place, Snapshot, Wanted};
use orch_settings_contract::registry::{self, keys};
use orch_types::{Id, Json};

pub(super) async fn read(
    conn: &mut Conn,
    principal: &Principal,
    thread: Id,
) -> PersonalSkillsPolicy {
    // An optional policy read cannot take the existing knowledge inventory away.
    read_current(conn, principal, thread)
        .await
        .unwrap_or(PersonalSkillsPolicy::Unavailable)
}

async fn read_current(
    conn: &mut Conn,
    principal: &Principal,
    thread: Id,
) -> ApiResult<PersonalSkillsPolicy> {
    let Some(def) = registry::get(keys::AGENTS_PERSONAL_SKILLS) else {
        return Ok(PersonalSkillsPolicy::Unsupported);
    };
    let snapshot = Box::pin(Snapshot::load(
        conn,
        Place::Node(thread),
        Wanted::Keys(&[def]),
    ))
    .await
    .map_err(ApiError::internal)?;
    match snapshot
        .resolve(def)
        .value
        .or_else(|| def.default_value())
        .and_then(|value| value.as_bool())
    {
        Some(true) => return Ok(PersonalSkillsPolicy::Allowed),
        Some(false) => {}
        None => return Ok(PersonalSkillsPolicy::Unavailable),
    }
    let setting =
        crate::routes::settings::shown_read_only(conn, principal, snapshot.view(def)).await?;
    let mut sources = Vec::new();
    let mut private_source = false;
    for part in setting.parts {
        if part.ignored || part.value != Json::Bool(false) {
            continue;
        }
        let mut source = part.source;
        if source
            .node
            .as_ref()
            .is_some_and(|node| node.redacted || node.name.is_none())
        {
            if private_source {
                continue;
            }
            private_source = true;
            source.node = None;
        }
        if !sources.contains(&source) {
            sources.push(source);
        }
    }
    Ok(PersonalSkillsPolicy::Off {
        reason: "policy_off".into(),
        sources,
    })
}
