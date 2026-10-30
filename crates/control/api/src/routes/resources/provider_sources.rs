//! Source choices use the endpoint scheme policy, explicit key consent and only programs registered beside the credential.

use super::{Home, begin_mutation, providers, record, require, sealing};
use crate::middleware::requests::{ApiError, ApiResult};
use crate::registry::{Ctx, Registry};
use crate::routes::nodes::invalid;
use orch_access_contract::{
    audit::AuditAction,
    permissions::{Permission, ResourceKind},
};
use orch_api_contract::{common::ErrorCode, resources as api};
use orch_db::{
    Conn, DbError,
    queries::{provider_facts, resources::routes as rows, settings},
};
use orch_provider_contract::builtins::BuiltinSources;
use orch_provider_contract::choice::Keeper;
use orch_provider_contract::{ProviderSources, SourceBy, SourceChoice, SourceElement};
use orch_types::{Id, Json};
use serde_json::json;
use std::collections::HashMap;

/// Binds the source-selection action after its contract has declared the route and permission.
pub(crate) fn mount(registry: &mut Registry) {
    registry.on(api::ProviderSourcesSet, set);
}

async fn stale(ctx: &Ctx, conn: &mut Conn, principal: &orch_authz::Principal, id: Id) -> ApiError {
    let current = providers::show(ctx, conn, principal, id).await.ok();
    ApiError::new(
        ErrorCode::ConflictStaleRevision,
        "the provider was changed since it was read",
    )
    .with("current", current)
}

async fn set(ctx: Ctx, (): (), body: api::ProviderSourcesSetBody) -> ApiResult<api::Provider> {
    let id = ctx.id("id")?;
    let (ctx, mut tx, principal) = begin_mutation(&ctx).await?;
    require(
        &mut tx,
        &principal,
        ResourceKind::Provider,
        id,
        Permission::ProviderManage,
    )
    .await?;
    if !provider_facts::lock_provider(&mut tx, id).await? {
        return Err(ApiError::not_found());
    }
    let row = rows::providers(&mut tx, &[id])
        .await?
        .pop()
        .ok_or_else(ApiError::not_found)?;
    if row.revision != body.expected_revision {
        return Err(stale(&ctx, &mut tx, &principal, id).await);
    }
    let changed = write(
        &ctx,
        &mut tx,
        &row,
        [
            body.facts.as_ref(),
            body.models.as_ref(),
            body.presenter.as_ref(),
        ],
    )
    .await?;
    tx.commit().await.map_err(DbError::from)?;
    // No authority or provider row is held while the keeping machine describes and reads its sources.
    first_reads(&ctx, id, changed).await;
    let (ctx, mut tx, principal) = begin_mutation(&ctx).await?;
    require(
        &mut tx,
        &principal,
        ResourceKind::Provider,
        id,
        Permission::ProviderManage,
    )
    .await?;
    let shown = providers::show(&ctx, &mut tx, &principal, id).await?;
    tx.commit().await.map_err(DbError::from)?;
    Ok(shown)
}

pub(crate) async fn first_reads(ctx: &Ctx, id: Id, changed: [bool; 3]) {
    if changed[0] {
        let _ = ctx
            .app
            .engine
            .refresh_provider_for_retry(id)
            .await
            .map_err(|error| tracing::warn!(%error,%id,"the first source read did not finish"));
    }
    if changed[1] {
        let _ = ctx.app.engine.discover_provider_models(id).await.map_err(
            |error| tracing::warn!(%error,%id,"the first model source read did not finish"),
        );
    }
    if changed[2] {
        let _ =
            ctx.app.engine.ensure_provider_up(id).await.map_err(
                |error| tracing::warn!(%error,%id,"the presenter could not be applied yet"),
            );
    }
}

/// Stores choices and write-only values in the caller's creation or management transaction.
/// Omitted elements stay unchanged; a different address/program never inherits another source's secrets.
pub(crate) async fn write(
    ctx: &Ctx,
    conn: &mut Conn,
    row: &rows::ProviderRow,
    given: [Option<&api::SourceChoiceSet>; 3],
) -> ApiResult<[bool; 3]> {
    let mut choices: ProviderSources =
        serde_json::from_value(row.sources.clone()).map_err(ApiError::internal)?;
    let programs = program_names(conn, row).await?;
    let http_allowed = providers::home_policies(conn, row.home_id)
        .await?
        .http_allowed;
    let names = rows::provider_secret_names(conn, &[row.id]).await?;
    let mut changed = [false; 3];
    let mut facts_changed = false;
    for (index, element) in SourceElement::ALL.into_iter().enumerate() {
        let Some(given) = given[index] else {
            continue;
        };
        let mut value = serde_json::to_value(given).map_err(ApiError::internal)?;
        if let Some(fields) = value.as_object_mut() {
            fields.remove("secrets");
            fields.remove("authorization");
        }
        let mut choice: SourceChoice =
            serde_json::from_value(value.clone()).map_err(ApiError::internal)?;
        if orch_link_contract::commands::has_references(&json!(choice.config)) {
            return Err(invalid(
                &format!("{element}.config"),
                "source configuration contains values, never link credential references",
            ));
        }
        let checked = check_source(
            element.as_str(),
            &value,
            row.kind == "api",
            http_allowed,
            &programs,
        )?;
        check_builtin(
            element,
            &choice,
            keeper(row) == Keeper::SignIn,
            &ctx.app.builtin_sources,
        )?;
        let old = choices.get(element);
        let same = old.is_some_and(|old| {
            old.by == choice.by
                && old.name == choice.name
                && match (&old.address, &choice.address) {
                    (Some(old), Some(new)) => {
                        old.url == new.url && old.auth == new.auth && old.header == new.header
                    }
                    (None, None) => true,
                    _ => false,
                }
        });
        let mut descriptor = if same {
            old.and_then(|old| old.descriptor.clone())
        } else {
            None
        };
        if choice.by == SourceBy::Program
            && let (Some(machine), Some(name)) = (row.key_machine_id, choice.name.as_deref())
            && let Some(reported) = program_descriptor(conn, machine, name).await?
        {
            descriptor = Some(reported);
        }
        let prefix = format!("source:{}:", element.as_str());
        let mut kept: std::collections::BTreeSet<String> = if same {
            names
                .iter()
                .filter(|(_, name)| name.starts_with(&prefix))
                .map(|(_, name)| name.clone())
                .collect()
        } else {
            Default::default()
        };
        for (field, value) in &given.secrets {
            if field.is_empty() || field.chars().any(char::is_control) {
                return Err(invalid(
                    &format!("{element}.secrets"),
                    "a secret field needs its own name without control characters",
                ));
            }
            let name = element.field_secret(field);
            if let Some(value) = value {
                check_secret(&format!("{element}.secrets.{field}"), value)?;
                sealing::put_provider_secret(conn, row.id, &name, value, ctx.user()?.id(), ctx.now)
                    .await?;
                kept.insert(name);
            } else {
                kept.remove(&name);
            }
        }
        let authorization = element.authorization_secret();
        if let Some(value) = &given.authorization {
            if let Some(value) = value {
                check_secret(&format!("{element}.authorization"), value)?;
                if !choice.address.as_ref().is_some_and(|address| {
                    address.auth != orch_provider_contract::address::AddressAuth::None
                }) {
                    return Err(invalid(
                        &format!("{element}.authorization"),
                        "only an authorized address takes this value",
                    ));
                }
                sealing::put_provider_secret(
                    conn,
                    row.id,
                    &authorization,
                    value,
                    ctx.user()?.id(),
                    ctx.now,
                )
                .await?;
                kept.insert(authorization.clone());
            } else {
                kept.remove(&authorization);
            }
        }
        if !choice.address.as_ref().is_some_and(|address| {
            address.auth != orch_provider_contract::address::AddressAuth::None
        }) {
            kept.remove(&authorization);
        }
        if matches!(
            choice.by,
            SourceBy::Default | SourceBy::None | SourceBy::Builtin
        ) && !given.secrets.is_empty()
        {
            return Err(invalid(
                &format!("{element}.secrets"),
                "this built-in source asks for no secret fields",
            ));
        }
        for (_, name) in names.iter().filter(|(_, name)| name.starts_with(&prefix)) {
            if !kept.contains(name) {
                settings::delete_secret(conn, sealing::PROVIDER, row.id, name).await?;
            }
        }
        choice.secrets = kept
            .iter()
            .filter_map(|name| {
                name.strip_prefix(&format!("{prefix}field:"))
                    .map(str::to_owned)
            })
            .collect();
        if let Some(descriptor) = descriptor.as_ref() {
            for field in &descriptor.fields {
                if field.kind.is_secret() && choice.config.contains_key(&field.name) {
                    return Err(invalid(
                        &format!("{element}.config.{}", field.name),
                        "write this field as a secret",
                    ));
                }
            }
            for field in &choice.secrets {
                if !descriptor
                    .secret_fields()
                    .any(|declared| declared.name == *field)
                {
                    return Err(invalid(
                        &format!("{element}.secrets.{field}"),
                        "the source does not declare this secret field",
                    ));
                }
            }
            let fields: Vec<_> = descriptor
                .fields
                .iter()
                .filter(|field| !field.kind.is_secret())
                .cloned()
                .collect();
            orch_types::fields::validate(&fields, &json!(choice.config)).map_err(|errors| {
                invalid(
                    &format!("{element}.config"),
                    &errors
                        .iter()
                        .map(|error| error.to_string())
                        .collect::<Vec<_>>()
                        .join("; "),
                )
            })?;
            for field in descriptor
                .secret_fields()
                .filter(|field| field.required && field.default.is_none())
            {
                if !choice.secrets.contains(&field.name) {
                    return Err(invalid(
                        &format!("{element}.secrets.{}", field.name),
                        "is required",
                    ));
                }
            }
            choice.descriptor = Some(descriptor.clone());
            choice.described_at = Some(orch_types::Time::new(ctx.now));
        }
        if element == SourceElement::Facts {
            let previous = old.cloned().unwrap_or_default();
            facts_changed = previous.by != choice.by
                || previous.name != choice.name
                || previous.address != choice.address
                || previous.config != choice.config
                || previous.secrets != choice.secrets
                || given.secrets.values().any(Option::is_some)
                || given.authorization.is_some();
        }
        let by = choice.by;
        let name = choice.name.clone();
        *choices.get_mut(element) = Some(choice);
        changed[index] = true;
        record(ctx,conn,orch_db::audit::action!(AuditAction::ProviderSourcesChanged),row.id,Home{node:Some(row.home_id),org:Some(row.org_id)},
            &format!("A provider manager changed where {} comes from",element.as_str()),
            json!({"element":element,"by":by,"name":name,"host":checked.host,"unencrypted":checked.unencrypted,"key_sent":checked.key_sent})).await?;
    }
    if changed.iter().any(|changed| *changed) {
        let choices = serde_json::to_value(choices).map_err(ApiError::internal)?;
        if !provider_facts::set_sources(
            conn,
            row.id,
            row.revision,
            &choices,
            facts_changed || changed[1],
            ctx.now,
        )
        .await?
        {
            return Err(ApiError::new(
                ErrorCode::ConflictStaleRevision,
                "the provider was changed since it was read",
            ));
        }
    }
    Ok(changed)
}

fn check_secret(path: &str, value: &str) -> ApiResult<()> {
    if value.is_empty() || value.len() > sealing::SECRET_MAX_BYTES {
        Err(invalid(
            path,
            "a secret is text, not empty and not too long",
        ))
    } else {
        Ok(())
    }
}

/// Creation uses the same sealed writer, after the new provider has its ordinary ownership.
pub(crate) async fn write_new(
    ctx: &Ctx,
    conn: &mut Conn,
    id: Id,
    given: Option<&api::NewProviderSources>,
) -> ApiResult<()> {
    if let Some(given) = given {
        let row = rows::providers(conn, &[id])
            .await?
            .pop()
            .ok_or_else(ApiError::not_found)?;
        write(
            ctx,
            conn,
            &row,
            [
                given.facts.as_ref(),
                given.models.as_ref(),
                given.presenter.as_ref(),
            ],
        )
        .await?;
    }
    Ok(())
}

fn check_builtin(
    element: SourceElement,
    choice: &SourceChoice,
    sign_in: bool,
    table: &BuiltinSources,
) -> ApiResult<()> {
    use orch_provider_contract::choice::builtin;
    if choice.by != SourceBy::Builtin {
        return Ok(());
    }
    let allowed = match choice.name.as_deref() {
        Some(builtin::ADAPTER) => {
            sign_in && matches!(element, SourceElement::Facts | SourceElement::Models)
        }
        Some(builtin::ENDPOINTS) => element == SourceElement::Models,
        // A vendor's reader is one the program registered, and reads a key's facts.
        Some(name) if table.get(name).is_some() => !sign_in && element == SourceElement::Facts,
        _ => false,
    };
    if allowed {
        Ok(())
    } else {
        Err(invalid(
            element.as_str(),
            "this built-in source does not supply this element for this credential",
        ))
    }
}

fn mirror<T: serde::Serialize, U: serde::de::DeserializeOwned>(value: &T) -> ApiResult<U> {
    serde_json::from_value(serde_json::to_value(value).map_err(ApiError::internal)?)
        .map_err(ApiError::internal)
}

async fn program_descriptor(
    conn: &mut Conn,
    machine: Id,
    name: &str,
) -> ApiResult<Option<orch_provider_contract::SourceDescriptor>> {
    let reported:Option<Json>=sqlx::query_scalar("SELECT plugin->'descriptor' FROM machines m CROSS JOIN LATERAL jsonb_array_elements(CASE WHEN jsonb_typeof(m.facts->'plugins')='array' THEN m.facts->'plugins' ELSE '[]'::jsonb END) plugin WHERE m.id=$1 AND m.deleted_at IS NULL AND plugin->>'kind'='provider' AND plugin->>'name'=$2 LIMIT 1")
        .bind(machine).bind(name).fetch_optional(conn).await.map_err(DbError::from)?.flatten();
    Ok(reported
        .and_then(|value| {
            serde_json::from_value::<orch_provider_contract::SourceDescriptor>(value).ok()
        })
        .filter(|descriptor| descriptor.check().is_ok()))
}

/// What keeps a provider's credential, by its kind, which says only that: the harness of a subscription's
/// sign-in, or a key, unless none of its endpoints takes one (the rule `refresh_standing` and the keeping
/// machine go by too). An API provider is never read as a sign-in, whatever profile its threads run.
fn keeper(row: &rows::ProviderRow) -> Keeper {
    if row.kind == "subscription" {
        return Keeper::SignIn;
    }
    let keyless = row
        .upstreams
        .as_array()
        .into_iter()
        .flatten()
        .all(|endpoint| endpoint.pointer("/options/auth").and_then(Json::as_str) == Some("none"));
    if keyless { Keeper::NoKey } else { Keeper::Key }
}

/// The default source of `element`, before the first read, by the one rule the keeping machine reads it by.
fn default_name(
    row: &rows::ProviderRow,
    element: SourceElement,
    table: &BuiltinSources,
) -> Option<&'static str> {
    let endpoints: Vec<url::Url> = row
        .upstreams
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|endpoint| {
            endpoint
                .get("base_url")
                .and_then(Json::as_str)
                .and_then(|base| url::Url::parse(base).ok())
        })
        .collect();
    orch_provider_contract::choice::default_source(element, keeper(row), &endpoints, table)
}

fn builtin_descriptor(
    name: &str,
    adapter: Option<&orch_agent_contract::descriptor::AgentDescriptor>,
    table: &BuiltinSources,
) -> Option<orch_provider_contract::SourceDescriptor> {
    use orch_provider_contract::choice::builtin;
    use orch_provider_contract::{
        SourceDescriptor,
        source::{SourceDescriptorFacts, SourceDescriptorModels},
    };
    match name {
        builtin::ENDPOINTS => {
            let mut descriptor = SourceDescriptor::new(
                name,
                "Endpoints",
                "Lists the models offered by the provider's endpoints.",
            );
            descriptor.models = Some(SourceDescriptorModels {});
            Some(descriptor)
        }
        builtin::ADAPTER => {
            let title = adapter
                .and_then(|adapter| adapter.native_provider.as_ref())
                .map_or("Harness adapter", |native| native.title.as_str());
            let mut descriptor = SourceDescriptor::new(
                name,
                title,
                "Reads the details declared by the harness that keeps the sign-in.",
            );
            descriptor.facts = Some(SourceDescriptorFacts::default());
            if let Some(adapter) = adapter {
                descriptor.facts = Some(SourceDescriptorFacts {
                    reports: orch_provider_contract::FactMember::of_kinds(
                        &adapter.capabilities.provider_facts,
                    ),
                    spends_resets: adapter.capabilities.provider_facts.contains(
                        &orch_agent_contract::descriptor::ProviderFactKind::AvailableResets,
                    ),
                    plans: adapter
                        .native_provider
                        .as_ref()
                        .map(|native| native.plans.clone())
                        .unwrap_or_default(),
                    ..SourceDescriptorFacts::default()
                });
                if adapter.capabilities.model_discovery {
                    descriptor.models = Some(SourceDescriptorModels {});
                }
            }
            Some(descriptor)
        }
        name => table.get(name).map(|entry| (entry.descriptor)()),
    }
}

/// Source declarations needed by a page of providers, read in two statements for any page size.
pub(crate) struct SourceViews {
    adapters: HashMap<Id, orch_agent_contract::descriptor::AgentDescriptor>,
    programs: HashMap<(Id, String), orch_provider_contract::SourceDescriptor>,
}

impl SourceViews {
    pub(crate) async fn read(conn: &mut Conn, providers: &[rows::ProviderRow]) -> ApiResult<Self> {
        let agents = rows::agents(
            conn,
            &providers
                .iter()
                .filter_map(|row| row.agent_id)
                .collect::<Vec<_>>(),
        )
        .await?;
        let adapters = agents
            .into_iter()
            .filter_map(|row| {
                let mut value = row.descriptor;
                if let (Some(fields), Some(overrides)) =
                    (value.as_object_mut(), row.descriptor_overrides.as_object())
                {
                    fields.extend(overrides.clone());
                }
                serde_json::from_value::<orch_agent_contract::descriptor::AgentDescriptor>(value)
                    .ok()
                    .map(|descriptor| (row.id, descriptor))
            })
            .collect();
        let machine_ids: Vec<Id> = providers
            .iter()
            .filter_map(|row| row.key_machine_id)
            .collect();
        let found: Vec<(Id, String, Option<Json>)> = sqlx::query_as(
            "SELECT m.id, plugin->>'name', plugin->'descriptor'
               FROM machines m
               CROSS JOIN LATERAL jsonb_array_elements(
                 CASE WHEN jsonb_typeof(m.facts->'plugins')='array' THEN m.facts->'plugins'
                      ELSE '[]'::jsonb END) plugin
              WHERE m.id=ANY($1) AND m.deleted_at IS NULL
                AND plugin->>'kind'='provider' AND plugin->>'name' IS NOT NULL",
        )
        .bind(machine_ids)
        .fetch_all(&mut *conn)
        .await
        .map_err(DbError::from)?;
        let programs = found
            .into_iter()
            .filter_map(|(machine, name, value)| {
                value
                    .and_then(|value| {
                        serde_json::from_value::<orch_provider_contract::SourceDescriptor>(value)
                            .ok()
                    })
                    .filter(|descriptor| descriptor.check().is_ok())
                    .map(|descriptor| ((machine, name), descriptor))
            })
            .collect();
        Ok(Self { adapters, programs })
    }
}

/// A provider view carries resolved source declarations and the contract's member states.
/// Configuration and names of set secrets are shown only to its managers.
pub(crate) fn view(
    row: &rows::ProviderRow,
    facts: &api::ProviderFacts,
    overridden: &[String],
    can_view: bool,
    manages: bool,
    table: &BuiltinSources,
    batch: &SourceViews,
) -> ApiResult<(api::ProviderSources, api::ProviderFactsState)> {
    if !can_view {
        return Ok((Default::default(), Default::default()));
    }
    let mut choices: ProviderSources =
        serde_json::from_value(row.sources.clone()).map_err(ApiError::internal)?;
    let adapter = row.agent_id.and_then(|id| batch.adapters.get(&id));
    for element in SourceElement::ALL {
        if element == SourceElement::Presenter && choices.get(element).is_none() {
            continue;
        }
        let choice = choices
            .get_mut(element)
            .get_or_insert_with(SourceChoice::default);
        let name = if choice.by == SourceBy::Default {
            default_name(row, element, table).map(str::to_owned)
        } else {
            choice.name.clone()
        };
        // A declaration a default learned from another source than the one the rule names is not its own.
        if choice.by == SourceBy::Default
            && choice
                .descriptor
                .as_ref()
                .is_some_and(|descriptor| Some(descriptor.name.as_str()) != name.as_deref())
        {
            choice.descriptor = None;
        }
        if choice.descriptor.is_none() {
            choice.descriptor = name
                .as_deref()
                .and_then(|name| builtin_descriptor(name, adapter, table));
        }
        if choice.by == SourceBy::Program
            && choice.descriptor.is_none()
            && let (Some(machine), Some(name)) = (row.key_machine_id, choice.name.as_deref())
        {
            choice.descriptor = batch.programs.get(&(machine, name.to_owned())).cloned();
        }
        if choice.by == SourceBy::Default {
            choice.name = name;
        }
    }
    let mut typed: orch_agent_contract::facts::ProviderFacts = mirror(facts)?;
    typed.key_usage_supported = row.facts.get("key_usage_supported").and_then(Json::as_bool);
    // An explicitly empty override is told too; this state calculation invents no vendor read time.
    for member in overridden {
        typed
            .field_times
            .entry(member.clone())
            .or_insert(orch_types::Time::new(row.updated_at));
    }
    let keeper: &[_] = if keeper(row) == Keeper::SignIn {
        &orch_provider_contract::members::SIGN_IN_KEEPER
    } else {
        &orch_provider_contract::members::KEY_KEEPER
    };
    let declared = orch_provider_contract::members::declared(
        choices
            .facts
            .as_ref()
            .and_then(|choice| choice.descriptor.as_ref()),
        keeper,
    );
    let mut states = serde_json::to_value(orch_provider_contract::members::facts_state(
        &typed, &declared,
    ))
    .map_err(ApiError::internal)?;
    let lists_models = choices
        .models
        .as_ref()
        .and_then(|choice| choice.descriptor.as_ref())
        .is_some_and(|descriptor| descriptor.lists_models());
    states["models"] = json!(orch_provider_contract::MemberState::of(
        facts
            .models
            .as_ref()
            .is_some_and(|models| !models.is_empty()),
        facts.models.is_some() || lists_models && facts.field_times.contains_key("models"),
        lists_models
    ));
    let project = |choice: &SourceChoice| -> ApiResult<api::SourceChoice> {
        Ok(api::SourceChoice {
            by: mirror(&choice.by)?,
            name: choice.name.clone(),
            address: choice.address.as_ref().map(mirror).transpose()?,
            config: if manages {
                choice.config.clone()
            } else {
                Default::default()
            },
            secrets: if manages {
                choice.secrets.clone()
            } else {
                Vec::new()
            },
            descriptor: choice.descriptor.as_ref().map(mirror).transpose()?,
            described_at: choice.described_at,
            last_error: choice.last_error.as_ref().map(mirror).transpose()?,
        })
    };
    let shown = api::ProviderSources {
        facts: project(
            choices
                .facts
                .as_ref()
                .ok_or_else(|| ApiError::internal("the facts source was not resolved"))?,
        )?,
        models: project(
            choices
                .models
                .as_ref()
                .ok_or_else(|| ApiError::internal("the models source was not resolved"))?,
        )?,
        presenter: choices.presenter.as_ref().map(project).transpose()?,
    };
    Ok((
        shown,
        serde_json::from_value(states).map_err(ApiError::internal)?,
    ))
}

/// Setup offers come only from built-ins (the harness's, the vendors' readers the program registered, the
/// endpoints') and the machine/template resources the caller may select.
pub(crate) async fn offers(
    conn: &mut Conn,
    machines: &[api::MachineRef],
    templates: &[rows::RuntimeTemplateRow],
    table: &BuiltinSources,
) -> ApiResult<Vec<api::ProviderSetupSource>> {
    use orch_provider_contract::choice::builtin;
    let mut offers = Vec::new();
    let vendors = table.iter().map(|entry| {
        (
            entry.name,
            entry
                .hosts
                .iter()
                .map(|host| (*host).to_owned())
                .collect::<Vec<String>>(),
        )
    });
    let builtins = std::iter::once((builtin::ADAPTER, Vec::new()))
        .chain(vendors)
        .chain(std::iter::once((builtin::ENDPOINTS, Vec::new())));
    for (name, hosts) in builtins {
        offers.push(api::ProviderSetupSource {
            by: api::SourceBy::Builtin,
            name: name.into(),
            descriptor: builtin_descriptor(name, None, table)
                .as_ref()
                .map(mirror)
                .transpose()?,
            default_for: hosts,
            machine: None,
            template: None,
            command: None,
        });
    }
    for machine in machines {
        let plugins: Option<Json> = sqlx::query_scalar(
            "SELECT facts->'plugins' FROM machines WHERE id=$1 AND deleted_at IS NULL",
        )
        .bind(machine.id)
        .fetch_optional(&mut *conn)
        .await
        .map_err(DbError::from)?
        .flatten();
        for plugin in plugins
            .as_ref()
            .and_then(Json::as_array)
            .into_iter()
            .flatten()
            .filter(|plugin| plugin.get("kind").and_then(Json::as_str) == Some("provider"))
        {
            let Some(name) = plugin.get("name").and_then(Json::as_str) else {
                continue;
            };
            let descriptor = plugin
                .get("descriptor")
                .filter(|value| !value.is_null())
                .and_then(|value| {
                    serde_json::from_value::<api::SourceDescriptor>(value.clone()).ok()
                });
            offers.push(api::ProviderSetupSource {
                by: api::SourceBy::Program,
                name: name.into(),
                descriptor,
                default_for: Vec::new(),
                machine: Some(machine.clone()),
                template: None,
                command: None,
            });
        }
    }
    for template in templates {
        let plugins:Option<Json>=sqlx::query_scalar("SELECT spec->'plugins' FROM env_templates WHERE id=$1 AND kind='provider' AND enabled AND deleted_at IS NULL").bind(template.id).fetch_optional(&mut *conn).await.map_err(DbError::from)?.flatten();
        for plugin in plugins
            .as_ref()
            .and_then(Json::as_array)
            .into_iter()
            .flatten()
            .filter(|plugin| plugin.get("kind").and_then(Json::as_str) == Some("provider"))
        {
            let Some(name) = plugin.get("name").and_then(Json::as_str) else {
                continue;
            };
            let command = plugin
                .get("command")
                .and_then(|value| serde_json::from_value::<Vec<String>>(value.clone()).ok());
            offers.push(api::ProviderSetupSource {
                by: api::SourceBy::Program,
                name: name.into(),
                descriptor: None,
                default_for: Vec::new(),
                machine: None,
                template: Some(orch_api_contract::environment::TemplateRef {
                    id: template.id,
                    name: template.name.clone(),
                }),
                command,
            });
        }
    }
    Ok(offers)
}

/// Program choices come from the keeping machine, plus the current selected managed template.
/// This reads declarations; no API request registers or starts a program.
pub(crate) async fn program_names(
    conn: &mut orch_db::Conn,
    provider: &orch_db::queries::resources::routes::ProviderRow,
) -> ApiResult<Vec<String>> {
    let machine = provider.key_machine_id;
    let mut names:Vec<String>=match machine {
        Some(machine)=>sqlx::query_scalar("SELECT plugin->>'name' FROM machines m CROSS JOIN LATERAL jsonb_array_elements(CASE WHEN jsonb_typeof(m.facts->'plugins')='array' THEN m.facts->'plugins' ELSE '[]'::jsonb END) plugin WHERE m.id=$1 AND m.deleted_at IS NULL AND plugin->>'kind'='provider' AND plugin->>'name' IS NOT NULL").bind(machine).fetch_all(&mut *conn).await.map_err(DbError::from)?,
        None=>Vec::new(),
    };
    if (provider.key_machine_id.is_none() || provider.key_machine_id == provider.machine_id)
        && orch_db::queries::environment_facts::of_provider(conn, provider.id)
            .await?
            .is_none_or(|machine| !machine.attaches())
    {
        let template = orch_settings::get::<orch_types::Id>(
            conn,
            orch_settings::Place::Provider(provider.id),
            orch_settings_contract::registry::keys::RUNTIME_TEMPLATE_ID,
        )
        .await
        .map_err(crate::middleware::requests::ApiError::internal)?;
        let template = match template {
            Some(template) => Some(template),
            None => orch_db::queries::resources::provider_runtime::live_runtime(conn, provider.id)
                .await?
                .map(|runtime| runtime.template_id),
        };
        if let Some(template) = template {
            let offered:Vec<String>=sqlx::query_scalar("SELECT plugin->>'name' FROM env_templates t CROSS JOIN LATERAL jsonb_array_elements(CASE WHEN jsonb_typeof(t.spec->'plugins')='array' THEN t.spec->'plugins' ELSE '[]'::jsonb END) plugin WHERE t.id=$1 AND t.kind='provider' AND t.deleted_at IS NULL AND plugin->>'kind'='provider' AND plugin->>'name' IS NOT NULL").bind(template).fetch_all(&mut *conn).await.map_err(DbError::from)?;
            names.extend(offered);
        }
    }
    names.sort();
    names.dedup();
    Ok(names)
}

/// Only safe audit facts about a validated selection; credentials and configuration values never enter it.
pub(crate) struct CheckedSource {
    pub unencrypted: bool,
    pub key_sent: bool,
    pub host: Option<String>,
}

/// Selection checks independent of storage/sealing; the route supplies the current keeping machine/template offers.
pub(crate) fn check_source(
    element: &str,
    choice: &Json,
    key_credential: bool,
    http_allowed: bool,
    programs: &[String],
) -> ApiResult<CheckedSource> {
    let typed: orch_provider_contract::SourceChoice = serde_json::from_value(choice.clone())
        .map_err(|_| {
            invalid(
                element,
                "the source choice does not have the declared shape",
            )
        })?;
    typed
        .check()
        .map_err(|error| invalid(element, &error.to_string()))?;
    if element == "presenter"
        && (!matches!(
            typed.by,
            orch_provider_contract::SourceBy::Default
                | orch_provider_contract::SourceBy::None
                | orch_provider_contract::SourceBy::Program
        ) || (!key_credential
            && !matches!(
                typed.by,
                orch_provider_contract::SourceBy::Default | orch_provider_contract::SourceBy::None
            )))
    {
        return Err(invalid(
            element,
            "only a local program may present a provider's key; a sign-in is never presented by a source",
        ));
    }
    let mut checked = CheckedSource {
        unencrypted: false,
        key_sent: false,
        host: None,
    };
    let by = choice.get("by").and_then(Json::as_str).unwrap_or("default");
    match by {
        "default" | "none" | "builtin" => {}
        "program" => {
            let name = choice
                .get("name")
                .and_then(Json::as_str)
                .unwrap_or_default();
            if !programs.iter().any(|registered| registered == name) {
                return Err(invalid(
                    &format!("{element}.name"),
                    "the program is not registered on the machine that keeps this provider or declared by its selected runtime template",
                ));
            }
        }
        "address" => {
            let address = choice.get("address").ok_or_else(|| {
                invalid(
                    &format!("{element}.address"),
                    "an address source needs its address",
                )
            })?;
            let source_address = typed
                .address
                .as_ref()
                .ok_or_else(|| invalid(element, "an address source needs its address"))?;
            source_address
                .check(http_allowed)
                .map_err(|error| invalid(&format!("{element}.address"), &error.to_string()))?;
            checked.unencrypted = source_address.unencrypted();
            checked.host = source_address.host();
            checked.key_sent = address.get("send_credential").and_then(Json::as_bool) == Some(true);
            if checked.key_sent && !key_credential {
                return Err(invalid(
                    &format!("{element}.address.send_credential"),
                    "a sign-in is never given to an address; only a provider key can be explicitly sent",
                ));
            }
        }
        _ => {
            return Err(invalid(
                &format!("{element}.by"),
                "choose default, builtin, program, address or none",
            ));
        }
    }
    Ok(checked)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    #[test]
    fn only_a_registered_program_can_present_a_key_and_no_source_can_present_a_sign_in() {
        let program = json!({"by":"program","name":"exchange"});
        assert!(check_source("presenter", &program, true, false, &["exchange".into()]).is_ok());
        assert!(check_source("presenter", &program, false, false, &["exchange".into()]).is_err());
        assert!(check_source("presenter", &program, true, false, &[]).is_err());
        for choice in [
            json!({"by":"builtin","name":"openrouter"}),
            json!({"by":"address","address":{"url":"https://example.invalid/present"}}),
        ] {
            assert!(check_source("presenter", &choice, true, false, &[]).is_err());
        }
    }
    #[test]
    fn the_source_address_obeys_the_same_plain_http_policy_as_an_endpoint() {
        let choice = json!({"by":"address","address":{"url":"http://details.internal/report","send_credential":false}});
        assert!(check_source("facts", &choice, true, false, &[]).is_err());
        let allowed = check_source("facts", &choice, true, true, &[]).unwrap();
        assert!(allowed.unencrypted);
        assert_eq!(allowed.host.as_deref(), Some("details.internal"));
    }
    #[test]
    fn a_sign_in_cannot_be_opted_into_an_address_and_a_program_from_another_machine_is_refused() {
        let choice = json!({"by":"address","address":{"url":"https://details.example.invalid/report","send_credential":true}});
        assert!(check_source("facts", &choice, false, false, &[]).is_err());
        assert!(
            check_source("facts", &choice, true, false, &[])
                .unwrap()
                .key_sent
        );
        let program = json!({"by":"program","name":"account"});
        assert!(check_source("facts", &program, true, false, &["another".into()]).is_err());
        assert!(check_source("facts", &program, true, false, &["account".into()]).is_ok());
    }
}
