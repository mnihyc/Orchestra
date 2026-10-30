//! Reading pending runtime changes together, without planning jobs or requiring replacement authority.

use super::{Engine, EngineError, PendingUpdate, ToolBuild, UpdateWhat, differences};
use crate::hosting::HarnessRequest;
use orch_access_contract::roles::NodeKind;
use orch_db::queries::{
    environments::hosting as environments, hierarchy, jobs::changes, resources::hosting as hosts,
    settings,
};
use orch_db::{Conn, DbError};
use orch_settings::merge::{Link, OwnValues, Rows, Stack, Supply, resolve_key};
use orch_settings_contract::registry::{self, Scope, keys};
use orch_types::{Id, Json};
use serde::de::DeserializeOwned;
use serde_json::json;
use std::collections::{BTreeMap, BTreeSet, HashMap};

#[derive(sqlx::FromRow)]
struct Input {
    runtime: Id,
    target: Id,
    home: Id,
    owner: Id,
    own: Json,
    revision: i64,
    template: Option<Id>,
    provider_kind: Option<String>,
    status: String,
    removing: bool,
    changing: bool,
    harness_kind: Option<String>,
    agent: Option<Id>,
    workspace: String,
    facts: Json,
    writer: Option<Id>,
    applied_mounts: Option<Json>,
}

struct Values<'a> {
    rows: &'a Rows,
    chain: Vec<Link>,
    own: OwnValues,
    offers: Option<Json>,
}

impl Values<'_> {
    fn get<T: DeserializeOwned>(&self, key: &str) -> Result<Option<T>, EngineError> {
        let def = registry::get(key)
            .ok_or_else(|| EngineError::unreadable("a runtime setting is not registered"))?;
        let supply = (key == keys::EXEC_OFFERS)
            .then_some(self.offers.as_ref())
            .flatten()
            .map(Supply::Environment);
        let resolved = resolve_key(
            def,
            &Stack {
                rows: self.rows,
                chain: &self.chain,
                own: Some(&self.own),
            },
            supply,
        );
        resolved
            .value
            .map(serde_json::from_value)
            .transpose()
            .map_err(EngineError::from)
    }
}

#[derive(sqlx::FromRow)]
struct Profile {
    id: Id,
    kind: String,
    version: Option<String>,
    system: bool,
    environments: Vec<Id>,
    providers: Vec<Id>,
}

type ToolKey = (String, String, Vec<HarnessRequest>);

impl Engine {
    /// Pending changes for exact units, including kept units, with shared rows read in batches.
    /// Catalogue resolution and mount author checks are reused for each distinct platform and
    /// work context. Reading never takes a lifecycle hold or sends a runtime command.
    pub async fn pending_runtime_updates(
        &self,
        conn: &mut Conn,
        ids: &[Id],
    ) -> Result<HashMap<Id, Vec<PendingUpdate>>, EngineError> {
        let mut result: HashMap<_, _> = ids.iter().map(|id| (*id, Vec::new())).collect();
        if ids.is_empty() {
            return Ok(result);
        }
        let Some(services) = self.host_services() else {
            return Ok(result);
        };
        let runtimes = environments::runtimes_by(conn, ids).await?;
        let inputs: Vec<Input> = sqlx::query_as("SELECT r.id AS runtime,COALESCE(e.id,p.id) AS target,
            COALESCE(e.home_node_id,p.home_id) AS home,COALESCE(e.owner_id,p.owner_id) AS owner,
            COALESCE(e.config,p.runtime_config) AS own,COALESCE(e.revision,p.revision) AS revision,
            e.template_id AS template,p.kind AS provider_kind,p.harness_kind,p.agent_id AS agent,
            COALESCE(e.status,p.status) AS status,e.remove_at IS NOT NULL AS removing,
            EXISTS(SELECT 1 FROM jobs j WHERE j.state IN ('queued','running') AND (
                (j.environment_id=e.id AND j.kind IN ('environment_provision','environment_recreate','environment_move','environment_remove'))
                OR (j.provider_id=p.id AND j.kind IN ('provider_provision','provider_recreate')))) AS changing,
            COALESCE(e.workspace_root,'') AS workspace,COALESCE(e.facts,'{}'::jsonb) AS facts,
            (SELECT a.actor_id FROM audit_log a WHERE a.object_kind='environment' AND a.object_id=e.id
                AND a.actor_kind='user' AND a.action='environment.updated' AND a.details->'config'='true'::jsonb
                AND a.actor_id IS NOT NULL ORDER BY a.at DESC,a.id DESC LIMIT 1) AS writer,
            (SELECT c.body->'mounts' FROM commands c WHERE c.kind='node.mounts.apply' AND c.body->>'runtime_id'=r.id::text
                AND c.state='succeeded' AND c.result->>'applied'='true' ORDER BY c.created_at DESC,c.id DESC LIMIT 1) AS applied_mounts
            FROM runtimes r LEFT JOIN environments e ON e.id=r.environment_id AND e.deleted_at IS NULL
            LEFT JOIN providers p ON p.id=r.provider_id AND p.deleted_at IS NULL
            WHERE r.id=ANY($1) AND (e.id IS NOT NULL OR p.id IS NOT NULL)")
            .bind(ids).fetch_all(&mut *conn).await.map_err(DbError::from)?;
        let homes: Vec<_> = inputs.iter().map(|input| input.home).collect();
        let nodes = hierarchy::with_ancestors(conn, &homes).await?;
        let node_ids: Vec<_> = nodes.iter().map(|node| node.id).collect();
        let rows = Rows::new(settings::rows_on_chain(conn, &node_ids, None).await?);
        let hosts = hosts::hosts(
            conn,
            Some(&runtimes.iter().map(|r| r.host_id).collect::<Vec<_>>()),
        )
        .await?;
        let definitions: HashMap<_, _> =
            changes::definitions(conn, ids).await?.into_iter().collect();
        let held: HashMap<Id, Vec<ToolBuild>> = changes::tools_among(conn, ids)
            .await?
            .into_iter()
            .map(|(id, tools)| {
                (
                    id,
                    tools
                        .and_then(|tools| serde_json::from_value(tools).ok())
                        .unwrap_or_default(),
                )
            })
            .collect();
        let mut values = HashMap::new();
        let mut template_ids = BTreeSet::new();
        let mut default_agents = Vec::new();
        let mut desired = HashMap::new();
        // EXEC_OFFERS is supplied by the environment's assigned template, as in the ordinary resolver.
        let assigned: Vec<hosts::TemplateState> = sqlx::query_as(
            "SELECT id,owner_id,org_id,home_id,name,kind,driver,spec,enabled,revision,deleted_at
            FROM env_templates WHERE id=ANY($1)",
        )
        .bind(inputs.iter().filter_map(|i| i.template).collect::<Vec<_>>())
        .fetch_all(&mut *conn)
        .await
        .map_err(DbError::from)?;
        for input in &inputs {
            let Some(home) = nodes
                .iter()
                .find(|node| node.id == input.home && node.deleted_at.is_none())
            else {
                continue;
            };
            let chain: Result<Vec<_>, EngineError> = home
                .path
                .iter()
                .map(|id| {
                    let node = nodes
                        .iter()
                        .find(|node| node.id == *id && node.deleted_at.is_none())
                        .ok_or_else(|| {
                            EngineError::unreadable("a runtime's home is unavailable")
                        })?;
                    let kind: NodeKind = node.kind.parse().map_err(|_| {
                        EngineError::unreadable("a runtime's home has an unknown kind")
                    })?;
                    Ok(Link {
                        id: *id,
                        scope: Scope::from(kind),
                    })
                })
                .collect();
            let Ok(chain) = chain else {
                continue;
            };
            let environment = input.provider_kind.is_none();
            let current = runtimes
                .iter()
                .find(|r| r.id == input.runtime)
                .ok_or_else(|| {
                    EngineError::unreadable("a runtime disappeared during its observation")
                })?;
            let template = input
                .own
                .get(if environment {
                    keys::ENV_TEMPLATE_ID
                } else {
                    keys::RUNTIME_TEMPLATE_ID
                })
                .and_then(Json::as_str)
                .and_then(|id| id.parse::<Id>().ok())
                .or(input.template)
                .unwrap_or(current.template_id);
            desired.insert(input.runtime, template);
            template_ids.insert(template);
            let settings = Values {
                rows: &rows,
                chain,
                own: OwnValues {
                    scope: if environment {
                        Scope::Environment
                    } else {
                        Scope::Provider
                    },
                    values: input.own.clone(),
                    revision: input.revision,
                },
                offers: assigned
                    .iter()
                    .find(|t| Some(t.id) == input.template)
                    .and_then(|t| t.spec.pointer("/exec/offers"))
                    .cloned(),
            };
            if environment && let Some(agent) = settings.get::<Id>(keys::AGENT_DEFAULT_AGENT_ID)? {
                default_agents.push(agent);
            }
            values.insert(input.runtime, settings);
        }
        let templates: Vec<hosts::TemplateState> = sqlx::query_as(
            "SELECT id,owner_id,org_id,home_id,name,kind,driver,spec,enabled,revision,deleted_at
            FROM env_templates WHERE id=ANY($1)",
        )
        .bind(template_ids.into_iter().collect::<Vec<_>>())
        .fetch_all(&mut *conn)
        .await
        .map_err(DbError::from)?;
        let environment_ids: Vec<_> = inputs
            .iter()
            .filter(|i| i.provider_kind.is_none())
            .map(|i| i.target)
            .collect();
        let provider_ids: Vec<_> = inputs
            .iter()
            .filter(|i| i.provider_kind.is_some())
            .map(|i| i.target)
            .collect();
        let agent_ids: Vec<_> = default_agents
            .into_iter()
            .chain(inputs.iter().filter_map(|i| i.agent))
            .collect();
        let has_api = inputs
            .iter()
            .any(|i| i.provider_kind.as_deref() == Some("api"));
        let profiles: Vec<Profile> = sqlx::query_as("SELECT a.id,a.harness_kind AS kind,a.profile->>'harness_version' AS version,
            a.home_id IS NULL AS system,ARRAY(SELECT DISTINCT t.environment_id FROM threads t JOIN nodes n ON n.id=t.node_id
                WHERE t.agent_id=a.id AND t.environment_id=ANY($1) AND n.deleted_at IS NULL) AS environments,
            ARRAY(SELECT DISTINCT t.provider_id FROM threads t JOIN nodes n ON n.id=t.node_id
                WHERE t.agent_id=a.id AND t.provider_id=ANY($4) AND n.deleted_at IS NULL) AS providers
            FROM agents a WHERE a.deleted_at IS NULL AND NOT a.disabled AND (a.id=ANY($2) OR ($3 AND a.home_id IS NULL)
                OR EXISTS(SELECT 1 FROM threads t JOIN nodes n ON n.id=t.node_id
                    WHERE t.agent_id=a.id AND (t.environment_id=ANY($1) OR t.provider_id=ANY($4)) AND n.deleted_at IS NULL))")
            .bind(&environment_ids).bind(agent_ids).bind(has_api).bind(&provider_ids).fetch_all(&mut *conn).await.map_err(DbError::from)?;
        let system_kinds = if has_api {
            orch_db::queries::resources::provider_runtime::system_harness_kinds(conn).await?
        } else {
            Vec::new()
        };
        let secret_ids: HashMap<(Id, String), Id> = settings::secret_names_of(
            conn,
            &provider_ids
                .iter()
                .map(|id| ("provider", *id))
                .collect::<Vec<_>>(),
        )
        .await?
        .into_iter()
        .map(|secret| ((secret.owner_id, secret.name), secret.id))
        .collect();
        let mut tools_cache: BTreeMap<ToolKey, Result<Vec<ToolBuild>, String>> = BTreeMap::new();
        let mut mounts_cache: HashMap<String, (Id, Json)> = HashMap::new();
        for input in &inputs {
            let Some(settings) = values.get(&input.runtime) else {
                continue;
            };
            let Some(old) = runtimes.iter().find(|runtime| runtime.id == input.runtime) else {
                continue;
            };
            // An unfinished unit has no installed definition to compare. An active replacement
            // or accepted removal already owns the lifecycle, so it is not another pending update.
            if old.state == "creating"
                || input.removing
                || input.changing
                || input.provider_kind.is_none()
                    && matches!(
                        input.status.as_str(),
                        "provisioning" | "recreating" | "moving"
                    )
            {
                continue;
            }
            let Some(template) = templates.iter().find(|template| {
                Some(&template.id) == desired.get(&old.id)
                    && template.enabled
                    && template.deleted_at.is_none()
            }) else {
                continue;
            };
            let Some(host) = hosts.iter().find(|host| host.id == old.host_id) else {
                continue;
            };
            let environment = input.provider_kind.is_none();
            let mut definition = template.spec.clone();
            if !environment {
                let mut env = definition
                    .get("env")
                    .and_then(Json::as_object)
                    .cloned()
                    .unwrap_or_default();
                if let Some(vars) = settings
                    .get::<Json>(keys::RUNTIME_VARIABLES)?
                    .and_then(|v| v.as_object().cloned())
                {
                    if vars.keys().any(|name| {
                        name.is_empty() || name.contains(['=', '\0']) || name.starts_with("ORCH_")
                    }) {
                        continue;
                    }
                    env.extend(vars);
                }
                let names = settings
                    .get::<Vec<String>>(keys::RUNTIME_SECRET_NAMES)?
                    .unwrap_or_default();
                let mut missing = false;
                for name in names {
                    match secret_ids.get(&(input.target, format!("runtime:{name}"))) {
                        Some(id) if !name.starts_with("ORCH_") => {
                            env.insert(name, json!({"$secret":id}));
                        }
                        _ => {
                            missing = true;
                            break;
                        }
                    }
                }
                if missing {
                    continue;
                }
                definition["env"] = Json::Object(env);
            }
            let resource_key = if environment {
                keys::ENV_RESOURCES
            } else {
                keys::RUNTIME_RESOURCES
            };
            let resources =
                crate::places::limits_of(&definition, settings.get::<Json>(resource_key)?.as_ref());
            let wanted = if environment {
                crate::jobs::provision::harnesses_wanted(
                    &definition,
                    &settings
                        .get::<Vec<String>>(keys::EXEC_OFFERS)?
                        .unwrap_or_default(),
                )
            } else {
                let mut wanted: BTreeSet<_> = input.harness_kind.iter().cloned().collect();
                if input.provider_kind.as_deref() == Some("api") {
                    wanted.extend(system_kinds.iter().cloned());
                }
                wanted.into_iter().collect()
            };
            let default_agent = if environment {
                settings.get::<Id>(keys::AGENT_DEFAULT_AGENT_ID)?
            } else {
                None
            };
            let mut requested = BTreeSet::new();
            for kind in wanted {
                let actual: Vec<_> = profiles
                    .iter()
                    .filter(|profile| {
                        profile.kind == kind
                            && (if environment {
                                Some(profile.id) == default_agent
                                    || profile.environments.contains(&input.target)
                            } else {
                                Some(profile.id) == input.agent
                                    || profile.providers.contains(&input.target)
                                    || (input.provider_kind.as_deref() == Some("api")
                                        && profile.system)
                            })
                    })
                    .collect();
                if actual.is_empty() {
                    requested.insert(HarnessRequest {
                        kind,
                        version: None,
                        newest: None,
                    });
                } else {
                    for profile in actual {
                        requested.insert(HarnessRequest {
                            kind: kind.clone(),
                            version: profile.version.clone(),
                            newest: None,
                        });
                    }
                }
            }
            let requested = crate::hosting::with_place_versions(
                requested,
                settings
                    .get::<Json>(keys::AGENTS_HARNESS_VERSIONS)?
                    .as_ref(),
            );
            let key = (
                host.platform_os.clone(),
                host.platform_arch.clone(),
                requested.into_iter().collect::<Vec<_>>(),
            );
            if !tools_cache.contains_key(&key) {
                // A read never resolves/downloads/hashes release payloads. Until the catalogue
                // has been observed by lifecycle work, retain the recorded tool facts; absence
                // of a new observation is not evidence that a tool was removed or changed.
                let tools = services
                    .observed_tools(&key.0, &key.1, &key.2)
                    .unwrap_or_else(|| held.get(&old.id).cloned().unwrap_or_default());
                tools_cache.insert(key.clone(), Ok(tools));
            }
            let Some(Ok(tools)) = tools_cache.get(&key) else {
                continue;
            };
            let recorded = definitions.get(&old.id).and_then(Option::as_ref);
            let mut updates = differences(
                old,
                template,
                &definition,
                &resources,
                recorded,
                held.get(&old.id).map_or(&[], Vec::as_slice),
                tools,
            );
            if !environment {
                if settings
                    .get::<Id>(keys::RUNTIME_HOST_ID)?
                    .is_some_and(|wanted| wanted != old.host_id)
                {
                    updates.push(PendingUpdate {
                        what: UpdateWhat::Settings,
                        name: "Host".to_owned(),
                        from: Some(host.name.clone()),
                        to: Some("the selected host".to_owned()),
                    });
                }
            } else {
                let storage = settings.get::<Json>(keys::ENV_STORAGE)?;
                if storage
                    .as_ref()
                    .and_then(Json::as_object)
                    .is_some_and(|storage| {
                        storage.iter().any(|(role, wanted)| {
                            wanted.as_str().is_some_and(|name| {
                                old.volumes
                                    .get(role)
                                    .and_then(|v| v.get("storage"))
                                    .and_then(Json::as_str)
                                    != Some(name)
                            })
                        })
                    })
                {
                    updates.push(PendingUpdate {
                        what: UpdateWhat::Settings,
                        name: "Storage".to_owned(),
                        from: Some(old.volumes.to_string()),
                        to: storage.as_ref().map(Json::to_string),
                    });
                }
                let selected = settings.get::<Json>(keys::ENV_MOUNTS)?;
                let mounts_key = json!([
                    input.home,
                    input.own.get(keys::ENV_MOUNTS),
                    selected,
                    input.writer.unwrap_or(input.owner),
                    host.machine_id,
                    old.driver,
                    input.workspace,
                    input.facts.get("home"),
                    storage
                ])
                .to_string();
                if !mounts_cache.contains_key(&mounts_key) {
                    let mounts =
                        if selected
                            .as_ref()
                            .and_then(Json::as_object)
                            .is_none_or(|mounts| {
                                mounts.values().all(|value| value == &Json::Bool(false))
                            })
                        {
                            json!([])
                        } else {
                            json!(
                                crate::mounts::resolve_for(
                                    conn,
                                    &crate::mounts::Target {
                                        environment: input.target,
                                        home: input.home,
                                        own: &input.own,
                                        owner: input.owner,
                                        machine: Some(host.machine_id),
                                        driver: Some(&old.driver),
                                        workspace: &input.workspace,
                                        home_path: input.facts.get("home").and_then(Json::as_str),
                                        storage: storage
                                            .as_ref()
                                            .and_then(|s| s.get("workspace"))
                                            .and_then(Json::as_str)
                                            .map(str::to_owned),
                                        planned: &[],
                                        detached: &[],
                                    },
                                    self.inner.clock.now()
                                )
                                .await?
                                .into_iter()
                                .filter_map(|mount| mount.spec)
                                .collect::<Vec<_>>()
                            )
                        };
                    mounts_cache.insert(mounts_key.clone(), (input.target, mounts));
                }
                let (source, found) = &mounts_cache[&mounts_key];
                let mut mounts = found.clone();
                if *source != input.target
                    && let Some(mounts) = mounts.as_array_mut()
                {
                    for mount in mounts {
                        if let Some(name) = mount.pointer_mut("/layer/name")
                            && let Some(suffix) = name
                                .as_str()
                                .and_then(|name| name.strip_prefix(&format!("orch-over-{source}-")))
                                .map(str::to_owned)
                        {
                            *name = json!(format!("orch-over-{}-{suffix}", input.target));
                        }
                    }
                }
                if input
                    .applied_mounts
                    .as_ref()
                    .or_else(|| recorded.and_then(|r| r.get("mounts")))
                    .is_some_and(|held| held != &mounts)
                {
                    updates.push(PendingUpdate {
                        what: UpdateWhat::Settings,
                        name: "Mount attachments".to_owned(),
                        from: input.facts.get("mounts").map(Json::to_string),
                        to: Some(mounts.to_string()),
                    });
                }
            }
            result.insert(old.id, updates);
        }
        Ok(result)
    }
}
