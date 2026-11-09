//! Providers on a key: the ones the caller owns, may use and manages, reading one without its key, making one with its endpoints and its key sealed, changing, switching off and deleting it, and writing the key and the secret headers again.
//!
//! A provider is what a thread's model calls go through. An API provider is one upstream that one key
//! opens; the key is sealed before it is stored and no request returns it, nor any secret header's value:
//! a provider says only that it has a key and which secret headers have a value. Who sees a provider,
//! who may use it, who manages it, and who may make one where, is the access engine's to say; the
//! policies that govern a provider's home are settings, resolved there.

use crate::middleware::authorization;

use crate::middleware::authorization::refusals;

use crate::middleware::requests::ApiError;

use crate::middleware::requests::ApiResult;

use crate::registry::Ctx;

use crate::registry::Registry;

use crate::routes::nodes::Placed;

use crate::routes::nodes::actor_name;

use crate::routes::nodes::invalid;

use crate::routes::nodes::placed;

use crate::routes::nodes::policy_at;

use crate::routes::nodes::view as node_view;

use crate::routes::resources::agents::machine_reports;

use crate::routes::resources::sealing::API_KEY;

use crate::routes::resources::sealing::SECRET_MAX_BYTES;

use crate::routes::resources::sealing::header_secret;

use crate::routes::resources::sealing::put_provider_secret;

use crate::routes::resources::upstreams;

use crate::routes::resources::upstreams::Checked;

use crate::routes::resources::view::Holding;

use crate::routes::resources::view::accesses;

use crate::routes::resources::view::addresses;

use crate::routes::resources::view::agent_refs;

use crate::routes::resources::view::agent_refs_with_addresses;

use crate::routes::resources::view::environment_refs;

use crate::routes::resources::view::machine_refs;

use crate::routes::resources::view::parse;

use crate::routes::resources::Home;

use crate::routes::resources::begin_mutation;

use crate::routes::resources::clean_name;

use crate::routes::resources::record;

use crate::routes::resources::require;

use crate::routes::resources::provider_login;

use crate::routes::resources::provider_runtime;

use orch_access_contract::audit::AuditAction;

use orch_access_contract::decisions::ObjectRef;

use orch_access_contract::permissions::Permission;

use orch_access_contract::permissions::ResourceKind;

use orch_api_contract::access::NewResourceAccess;

use orch_api_contract::binding::ArrangementType;

use orch_api_contract::common::ErrorCode;

use orch_api_contract::nodes::NodeKind;

use orch_api_contract::resources as api;

use orch_api_contract::resources::ArrangementConsent;

use orch_api_contract::resources::ArrangementConsentForm;

use orch_api_contract::resources::HarnessState;

use orch_api_contract::resources::ModelSource;

use orch_api_contract::resources::Provider;

use orch_api_contract::resources::ProviderFacts;

use orch_api_contract::resources::ProviderHeldBy;

use orch_api_contract::resources::ProviderListScope;

use orch_api_contract::resources::ProviderModel;

use orch_api_contract::resources::ProviderRef;

use orch_api_contract::resources::ProviderSubscription;

use orch_api_contract::resources::ProviderWriteReply;

use orch_api_contract::settings as settings_api;

use orch_authz::engine::Graph;

use orch_authz::ownership;

use orch_authz::ownership::Basis;

use orch_authz::ownership::Making;

use orch_authz::ownership::NewAccess;

use orch_authz::ownership::Why;

use orch_authz::Policy;

use orch_authz::Principal;

use orch_authz::resources;

use orch_db::queries::resources::MachineRow;

use orch_db::queries::resources::binding as binding_rows;

use orch_db::queries::resources::provider_runtime as runtime_rows;

use orch_db::queries::resources::routes as queries;

use orch_db::queries::resources::routes::ModelRow;

use orch_db::queries::resources::routes::NewProvider;

use orch_db::queries::resources::routes::ProviderChange;

use orch_db::queries::resources::routes::ProviderRow;

use orch_db::queries::resources::thread_engine as agent_rows;

use orch_db::queries::threads::binding as thread_rows;

use orch_db::Conn;

use orch_db::DbError;

use orch_driver_contract::runtime::EnvironmentDataSource;

use orch_engine::harnesses::Discovered;

use orch_engine::harnesses::Restarted;

use orch_engine::jobs::provider_provision;

use orch_engine::jobs::provider_provision::Mode;

use orch_engine::provider_setup::Refreshed;

use orch_engine::provider_setup::SIGNED_OUT;

use orch_settings::Place;

use orch_settings::get as setting;

use orch_settings_contract::registry::Scope;

use orch_settings_contract::registry::keys;

use orch_types::Id;

use orch_types::Json;

use orch_types::Time;

use serde_json::json;

use std::collections::BTreeMap;

use std::collections::BTreeSet;

use std::collections::HashMap;

pub use orch_api_base::routes::resources::providers::HomePolicies;

pub use orch_api_base::routes::resources::providers::Standing;

pub use orch_api_base::routes::resources::providers::PreparedProviderViews;

pub use orch_api_base::routes::resources::providers::PreparedProjection;

pub use orch_api_base::routes::resources::providers::readable_name_roles;

pub use orch_api_base::routes::resources::providers::PreparedListProjection;
/// The members a provider's runtime has values for: the keys of scope `provider`.
const RUNTIME_KEYS: &[&str] = &[
    "runtime.template_id",
    "runtime.host_id",
    "runtime.resources",
    "runtime.storage",
    "runtime.home",
    "runtime.needs",
    "runtime.after_up",
    "runtime.idle_stop_minutes",
    "runtime.update",
    "runtime.renew_days",
    "runtime.variables",
    "runtime.secret_names",
];

/// Choice observations use the same seven UTC days as the existing performance picker.
const CHOICE_PERFORMANCE_DAYS: i64 = 7;

/// The types of arrangement a provider's owner can allow, in the order the dialog shows them.
const ARRANGEMENTS: [ArrangementType; 4] = [
    ArrangementType::Separated,
    ArrangementType::KeySeparated,
    ArrangementType::CredentialShared,
    ArrangementType::HandedOver,
];

/// What a key starts with: the types in which it stays home.
const KEY_ARRANGEMENTS: [&str; 2] = ["separated", "key_separated"];

/// Binds this module's requests.
pub fn mount(registry: &mut Registry) {
    super::capability_backends::mount(registry);
    super::provider_creation::duplicate::mount(registry);
    registry
        .creation(
            api::ProviderCreate,
            super::provider_creation::provider_binding(),
        )
        .creation(
            api::MachineProviderCreate,
            super::provider_creation::machine_binding(),
        )
        .on_snapshot_read_with_operation(
            api::ProviderList,
            super::provider_list::prepare,
            super::provider_list::get,
        )
        .on_with_operation(
            api::ProviderCreate,
            super::provider_creation::create,
            create,
        )
        .on_snapshot_read_with_operation(api::ProviderGet, super::provider_projection::prepare, get)
        .on_with_operation(
            api::ProviderUpdate,
            super::provider_projection::prepare,
            update,
        )
        .on(api::ProviderDelete, delete)
        .on(api::ProviderDisable, disable)
        .on(api::ProviderEnable, enable)
        .on(api::ProviderKeySet, key_set)
        .on_with_operation(
            api::ProviderSecretSet,
            crate::key_operations::direct::declared,
            secret_set,
        )
        .on_with_operation(
            settings_api::ProviderSettingList,
            super::provider_aux_readers::settings,
            super::provider_aux_readers::settings_get,
        )
        .on_with_operation(
            api::ProviderThreadList,
            super::provider_aux_readers::threads,
            super::provider_aux_readers::threads_get,
        )
        .on_with_operation(
            api::MachineProviderCreate,
            super::provider_creation::machine,
            machine_provider_create,
        )
        .on(api::ProviderRefresh, provider_refresh)
        .on_snapshot_read_with_operation(
            api::ProviderPlanGet,
            crate::key_operations::direct::declared,
            super::provider_plan::get,
        )
        .on(api::ProviderFactsOverridesSet, facts_overrides_set)
        .on(api::ProviderHarnessRestart, harness_restart)
        .on(api::ProviderHarnessSessionList, harness_sessions);
}

// ---- how an arrangement is put to the owner ----

/// What an arrangement puts into an environment, in one sentence.
fn puts(kind: ArrangementType) -> &'static str {
    match kind {
        ArrangementType::KeySeparated => "A token that works only at Orch's forwarder.",
        ArrangementType::CredentialShared => "A copy of the credential.",
        ArrangementType::HandedOver => "The credential itself, to one environment at a time.",
        _ => "Nothing: the harness and its tools stay apart.",
    }
}

/// The word an arrangement type has in the database and on the wire.
fn arrangement_word(kind: ArrangementType) -> String {
    serde_json::to_value(kind)
        .ok()
        .and_then(|value| value.as_str().map(str::to_owned))
        .unwrap_or_default()
}

/// Whether a type lets the credential itself leave the provider.
fn lets_credential_leave(word: &str) -> bool {
    matches!(word, "credential_shared" | "handed_over")
}

/// The arrangement types of an API provider, each with whether its owner allows it. Of a key, only
/// the two in which it stays home are on offer; a copy of it is allowed when the owner says so.
fn consents(allowed: &[String], available: Option<&[ArrangementType]>) -> Vec<ArrangementConsent> {
    ARRANGEMENTS
        .iter()
        .map(|kind| {
            let word = arrangement_word(*kind);
            let leaves = lets_credential_leave(&word);
            ArrangementConsent {
                kind: *kind,
                allowed: allowed.contains(&word),
                available: available.is_none_or(|types| types.contains(kind)),
                puts: puts(*kind).to_owned(),
                form: leaves.then(|| ArrangementConsentForm {
                    name: API_KEY.to_owned(),
                    title: "API key".to_owned(),
                    lifetime_secs: None,
                }),
            }
        })
        .collect()
}

/// What may now leave the provider, in a sentence, when the types allowed include one that lets a
/// copy of the credential go.
fn leaving_warning(allowed: &[String]) -> Option<String> {
    allowed
        .iter()
        .any(|word| lets_credential_leave(word))
        .then(|| {
            "A copy of the key may now leave the provider for an environment where a thread runs on it: whoever can read that environment can read the key.".to_owned()
        })
}

/// The arrangement types a body names, as the words they have in the database: at least one, each
/// a type a provider can allow, none twice.
fn clean_arrangements(path: &str, given: &[ArrangementType]) -> ApiResult<Vec<String>> {
    let mut words: Vec<String> = Vec::new();
    for kind in given {
        if !ARRANGEMENTS.contains(kind) {
            return Err(invalid(path, "a provider cannot allow this type"));
        }
        let word = arrangement_word(*kind);
        if !words.contains(&word) {
            words.push(word);
        }
    }
    if words.is_empty() {
        return Err(invalid(path, "at least one type stays allowed"));
    }
    Ok(words)
}

/// Saved allowances can be removed after policy narrows; enabling one uses the keeper's
/// current physical policy. Defaults take the same intersection as a thread's binding.
async fn admit_arrangements(
    conn: &mut Conn,
    id: Id,
    words: &[String],
    previous: &[String],
    explicit: bool,
) -> ApiResult<Vec<String>> {
    let policy = orch_db::queries::environment_facts::credential_policy(conn, id).await?;
    let Some(available) = orch_engine::binding::arrangements_of_policy(policy.as_ref()) else {
        return Ok(words.to_vec());
    };
    let permits = |word: &String| {
        available
            .iter()
            .any(|kind| arrangement_word(*kind) == *word)
    };
    if explicit {
        if words
            .iter()
            .any(|word| !previous.contains(word) && !permits(word))
        {
            return Err(invalid(
                "arrangements",
                "the machine keeping this credential does not allow this way of running an agent",
            ));
        }
        return Ok(words.to_vec());
    }
    let admitted: Vec<String> = words.iter().filter(|word| permits(word)).cloned().collect();
    if admitted.is_empty() {
        return Err(invalid(
            "arrangements",
            "this machine allows none of the proposed ways; choose another machine or agent",
        ));
    }
    Ok(admitted)
}

async fn admit_new_provider_arrangements(
    conn: &mut Conn,
    id: Id,
    words: &[String],
    explicit: bool,
) -> ApiResult<Vec<String>> {
    let admitted = admit_arrangements(conn, id, words, &[], explicit).await?;
    if admitted.as_slice() != words {
        sqlx::query("UPDATE providers SET arrangements=$2 WHERE id=$1")
            .bind(id)
            .bind(&admitted)
            .execute(&mut *conn)
            .await
            .map_err(DbError::from)?;
    }
    Ok(admitted)
}

async fn check_runtime_commands_owner(
    conn: &mut Conn,
    principal: &Principal,
    id: Id,
    before: &Json,
    after: &Json,
) -> ApiResult<()> {
    if orch_settings::write::provider_commands_need_owner(before, after) {
        let standing = resources::effective(
            conn,
            principal,
            ObjectRef {
                kind: ResourceKind::Provider.into(),
                id,
            },
            Policy::default(),
        )
        .await
        .map_err(ApiError::internal)?;
        if !standing.owner {
            let field = if before.get(keys::RUNTIME_AFTER_UP) != after.get(keys::RUNTIME_AFTER_UP) {
                "runtime_config.runtime.after_up"
            } else {
                "runtime_config.runtime.template_id"
            };
            return Err(ApiError::new(ErrorCode::AuthzDenied,"only this provider's owners may choose its template or commands that run beside its credential")
                .with("required","owner").with("field",field));
        }
    }
    Ok(())
}

/// The values of a provider's runtime: the keys of scope `provider`, and nothing else.
fn clean_runtime_config(path: &str, given: &Json) -> ApiResult<Json> {
    let Some(members) = given.as_object() else {
        return Err(invalid(path, "the runtime's values are an object"));
    };
    if let Some(unknown) = members
        .keys()
        .find(|name| !RUNTIME_KEYS.contains(&name.as_str()))
    {
        return Err(invalid(
            &format!("{path}.{unknown}"),
            "this is no value of a provider's runtime",
        ));
    }
    for (key, value) in members {
        orch_settings_contract::registry::validate_at(key, Scope::Provider, value)
            .map_err(|error| invalid(&format!("{path}.{key}"), &error.to_string()))?;
        if key == keys::RUNTIME_HOME {
            let home: orch_driver_contract::runtime::EnvironmentDataSource =
                serde_json::from_value(value.clone())
                    .map_err(|error| invalid(&format!("{path}.{key}"), &error.to_string()))?;
            if home.directory().is_some_and(|path| {
                !std::path::Path::new(path).is_absolute()
                    || path.contains('\0')
                    || std::path::Path::new(path)
                        .components()
                        .any(|part| matches!(part, std::path::Component::ParentDir))
            }) {
                return Err(invalid(
                    &format!("{path}.{key}.path"),
                    "an existing harness home must be an absolute directory under environment.roots",
                ));
            }
        }
        if key == keys::RUNTIME_VARIABLES
            && value.as_object().is_some_and(|values| {
                values.keys().any(|name| {
                    name.is_empty() || name.contains(['=', '\0']) || name.starts_with("ORCH_")
                })
            })
        {
            return Err(invalid(
                &format!("{path}.{key}"),
                "variable names must be nonempty, contain no equals sign or NUL, and leave ORCH_ names to the connector",
            ));
        }
    }
    Ok(given.clone())
}

async fn check_runtime_home(conn: &mut Conn, provider: Id, config: &Json) -> ApiResult<()> {
    let Some(value) = config.get(keys::RUNTIME_HOME) else {
        return Ok(());
    };
    let home: orch_driver_contract::runtime::EnvironmentDataSource =
        serde_json::from_value(value.clone()).map_err(ApiError::internal)?;
    let Some(path) = home.directory() else {
        return Ok(());
    };
    let current = orch_db::queries::environment_facts::of_provider(conn, provider).await?;
    let facts = if current.is_some() {
        current
    } else if let Some(host) = config
        .get(keys::RUNTIME_HOST_ID)
        .and_then(Json::as_str)
        .and_then(|id| id.parse::<Id>().ok())
    {
        let host = orch_db::queries::resources::hosting::host(conn, host)
            .await?
            .ok_or_else(|| invalid("runtime_config.runtime.home", "the selected host is gone"))?;
        let template_id: Option<Id> = orch_settings::get(
            conn,
            orch_settings::Place::Provider(provider),
            keys::RUNTIME_TEMPLATE_ID,
        )
        .await
        .map_err(ApiError::internal)?;
        let template = match template_id {
            Some(id) => orch_db::queries::resources::hosting::template(conn, id).await?,
            None => None,
        };
        orch_db::queries::environment_facts::machine(
            conn,
            host.machine_id,
            template.as_ref().map(|template| template.driver.as_str()),
        )
        .await?
    } else {
        None
    };
    let Some(facts) = facts else {
        return Err(invalid(
            "runtime_config.runtime.home",
            "choose the runtime machine before selecting its existing harness home",
        ));
    };
    let offered = facts
        .descriptor
        .as_ref()
        .and_then(|descriptor| descriptor.pointer("/data_storage/existing_directory"))
        .and_then(Json::as_bool)
        == Some(true);
    let allowed = facts
        .policy
        .as_ref()
        .and_then(|policy| policy.get("environment.roots"))
        .and_then(Json::as_array)
        .is_some_and(|roots| {
            roots
                .iter()
                .filter_map(Json::as_str)
                .any(|root| std::path::Path::new(path).starts_with(root))
        });
    if !offered || !allowed {
        return Err(invalid(
            "runtime_config.runtime.home.path",
            "the runtime machine must offer existing directories and allow this home under environment.roots; it checks links and write access itself before starting",
        ));
    }
    Ok(())
}

pub(crate) async fn home_policies(conn: &mut Conn, home: Id) -> ApiResult<HomePolicies> {
    let place = Place::Node(home);
    let made: Option<Vec<String>> = setting(conn, place, keys::RESOURCES_CREATED_BY_MEMBERS)
        .await
        .map_err(ApiError::internal)?;
    let http: Option<bool> = setting(conn, place, keys::PROVIDERS_ALLOW_HTTP_UPSTREAMS)
        .await
        .map_err(ApiError::internal)?;
    let harnesses: Option<Vec<String>> =
        setting(conn, place, keys::PROVIDERS_SUBSCRIPTION_HARNESSES)
            .await
            .map_err(ApiError::internal)?;
    let sharing: Option<Vec<String>> = setting(conn, place, keys::PROVIDERS_SUBSCRIPTION_SHARING)
        .await
        .map_err(ApiError::internal)?;
    Ok(HomePolicies {
        members_make: made
            .unwrap_or_default()
            .iter()
            .filter_map(|kind| kind.parse().ok())
            .collect(),
        http_allowed: http.unwrap_or(false),
        subscription_harnesses: harnesses,
        subscription_sharing: sharing,
    })
}
orch_api_base::__api_bind_0f1e4ccdb2!();

/// Why a person may not make a resource where they asked to, as the answer a client gets.
pub async fn refused_making(conn: &mut Conn, home: Id, why: Why) -> ApiError {
    match why {
        Why::KeptByAdmins(refusal) => match refusals::denied(conn, *refusal).await {
            Ok(error) | Err(error) => error,
        },
        other => {
            let sentence = match other {
                Why::HomeIsNoOrg => "a resource is at home in an org",
                Why::NotAMember => "only a member of the org makes what is theirs there",
                Why::OutsideYourDomain { .. } => {
                    "a member makes a resource in their domain or inside it"
                }
                Why::SystemAdminsOnly => "only a system admin makes a resource of the system",
                _ => "a resource is not made this way",
            };
            ApiError::new(ErrorCode::AuthzDenied, sentence)
                .with("required", "settings.manage")
                .with("object", json!({ "kind": "org", "id": home }))
                .with("requestable", false)
                .with("override", false)
                .with("missing", Vec::<Json>::new())
        }
    }
}

/// The facts a provider keeps, with what the built-in readers told in their own members (a plan's credits, a
/// key's usage and its account's credits) turned into `balances` and `spend`, as every reader's are; members the
/// facts of a source do not have (the models found) are kept as they are.
fn normalized(facts: &Json) -> Json {
    let Ok(mut typed) =
        serde_json::from_value::<orch_agent_contract::facts::ProviderFacts>(facts.clone())
    else {
        return facts.clone();
    };
    orch_agent_contract::facts::normalize(&mut typed);
    let Ok(mut shown) = serde_json::to_value(&typed) else {
        return facts.clone();
    };
    if let (Some(shown), Some(kept)) = (shown.as_object_mut(), facts.as_object()) {
        for (name, value) in kept {
            shown.entry(name.clone()).or_insert_with(|| value.clone());
        }
    }
    shown
}

/// The ProviderFacts a provider shows: what its harness or endpoint reported, with the members
/// written in by hand taking the place of the same members; and the names of those members.
pub(super) fn facts_shown(
    facts: &Json,
    overrides: &Json,
) -> ApiResult<(ProviderFacts, Vec<String>)> {
    let mut shown = normalized(facts).as_object().cloned().unwrap_or_default();
    // Kept overrides from older readers remain internal inputs, and keep their precedence by item.
    let mut overrides = overrides.as_object().cloned().unwrap_or_default();
    let old = normalized(&Json::Object(overrides.clone()));
    let old_balances = [
        (
            "plan_credits",
            orch_provider_contract::members::PLAN_BALANCE_ID,
        ),
        (
            "key_credits",
            orch_provider_contract::members::ACCOUNT_BALANCE_ID,
        ),
    ];
    if !overrides.contains_key("balances")
        && old_balances
            .iter()
            .any(|(name, _)| overrides.contains_key(*name))
    {
        let mut balances = shown
            .get("balances")
            .and_then(Json::as_array)
            .cloned()
            .unwrap_or_default();
        balances.retain(|balance| {
            !old_balances.iter().any(|(name, id)| {
                overrides.contains_key(*name)
                    && balance.get("id").and_then(Json::as_str) == Some(*id)
            })
        });
        balances.extend(
            old.get("balances")
                .and_then(Json::as_array)
                .into_iter()
                .flatten()
                .cloned(),
        );
        overrides.insert("balances".into(), Json::Array(balances));
    }
    if !overrides.contains_key("spend") && overrides.contains_key("key_usage") {
        let mut spend = shown
            .get("spend")
            .and_then(Json::as_array)
            .cloned()
            .unwrap_or_default();
        spend.retain(|row| {
            !row.get("id")
                .and_then(Json::as_str)
                .is_some_and(|id| orch_provider_contract::members::KEY_SPEND_IDS.contains(&id))
        });
        spend.extend(
            old.get("spend")
                .and_then(Json::as_array)
                .into_iter()
                .flatten()
                .cloned(),
        );
        overrides.insert("spend".into(), Json::Array(spend));
    }
    for legacy in [
        "plan_credits",
        "key_credits",
        "key_usage",
        "key_usage_supported",
    ] {
        overrides.remove(legacy);
    }
    let mut overridden: BTreeSet<String> = BTreeSet::new();
    {
        let set = &overrides;
        for (name, value) in set {
            if matches!(
                name.as_str(),
                "credential_verified"
                    | "available_resets"
                    | "ordinary_usage_allowed"
                    | "models"
                    | "field_times"
            ) {
                continue;
            }
            if name == "credential" {
                let mut credential = shown
                    .get(name)
                    .and_then(Json::as_object)
                    .cloned()
                    .unwrap_or_default();
                for field in ["method", "yields"] {
                    if let Some(value) = value.get(field) {
                        credential.insert(field.to_owned(), value.clone());
                    }
                }
                if !credential.is_empty() {
                    shown.insert(name.clone(), Json::Object(credential));
                }
            } else {
                shown.insert(name.clone(), value.clone());
            }
            overridden.insert(name.clone());
        }
    }
    shown.entry("windows").or_insert_with(|| json!([]));
    for name in ["plan", "limit_reached", "identity", "credential"] {
        shown.entry(name).or_insert(Json::Null);
    }
    let parsed = serde_json::from_value(Json::Object(shown)).map_err(|error| {
        ApiError::internal(format!("the facts of a provider do not read: {error}"))
    })?;
    Ok((parsed, overridden.into_iter().collect()))
}

/// Whether a provider has what it needs to be asked for a model: a key that is held somewhere, or
/// no need of one.
fn has_credential(row: &ProviderRow, key_set: bool, keyless: bool) -> bool {
    // A subscription has the sign-in its harness holds, unless the machine looked and found none.
    if row.kind == "subscription" && row.status_detail.as_deref() == Some(SIGNED_OUT) {
        return false;
    }
    key_set || row.key_machine_id.is_some() || keyless
}

/// Where a provider stands once what it has is known: ready with a credential and a model that
/// is enabled, otherwise it has to be set up. A provider that is switched off, exhausted, in
/// error or out of reach stays as it is: its machine or its owner says when that ends.
fn derived_status(current: &str, credential: bool, enabled_models: usize) -> Option<&'static str> {
    match current {
        "ready" | "needs_setup" => Some(if credential && enabled_models > 0 {
            "ready"
        } else {
            "needs_setup"
        }),
        _ => None,
    }
}

/// Brings the status of an API provider in step with what it has: a key (or a machine that keeps
/// one, or an upstream that takes none) and an enabled model.
pub async fn refresh_standing(ctx: &Ctx, conn: &mut Conn, id: Id) -> ApiResult<()> {
    let Some(row) = queries::providers(conn, &[id]).await?.pop() else {
        return Ok(());
    };
    let key_set = queries::provider_secret_names(conn, &[id])
        .await?
        .iter()
        .any(|(_, name)| name == API_KEY);
    let keyless = upstreams::stored(&row.upstreams)?
        .iter()
        .all(|each| each.options.get("auth").and_then(Json::as_str) == Some("none"));
    let enabled = queries::models_of(conn, &[id])
        .await?
        .iter()
        .filter(|model| model.enabled)
        .count();
    if let Some(next) = derived_status(&row.status, has_credential(&row, key_set, keyless), enabled)
        && next != row.status
    {
        queries::set_provider_standing(conn, id, next, None, ctx.now).await?;
    }
    Ok(())
}

/// Prepare the ordinary provider/ACL/cap source set once on the actual read snapshot.
/// Public identities and counts remain intrinsic; full Groups and protected names are admitted later.
pub(super) async fn prepare_projection(
    ctx: &Ctx,
    conn: &mut Conn,
    principal: &Principal,
    id: Id,
) -> ApiResult<PreparedProjection> {
    if ctx.route.name == "ProviderGet" {
        return prepare_readable_projection(ctx, conn, principal, id).await;
    }
    let (row, standing) = read_provider(conn, principal, id).await?;
    require(
        conn,
        principal,
        ResourceKind::Provider,
        id,
        Permission::ProviderManage,
    )
    .await?;
    prepare_view_of_row(ctx, conn, principal, row, standing).await
}

/// The existing ProviderGet metadata gate and source preparation. The typed read producer
/// separately binds its operation's protected names before projecting this prepared value.
pub(crate) async fn prepare_readable_projection(
    ctx: &Ctx,
    conn: &mut Conn,
    principal: &Principal,
    id: Id,
) -> ApiResult<PreparedProjection> {
    let (row, standing) = provider_page_for(conn, principal, id).await?;
    prepare_view_of_row(ctx, conn, principal, row, standing).await
}
orch_api_base::__api_bind_1b8aab182e!();

async fn prepare_view_of_row(
    ctx: &Ctx,
    conn: &mut Conn,
    principal: &Principal,
    row: ProviderRow,
    standing: Standing,
) -> ApiResult<PreparedProjection> {
    let id = row.id;
    let projection = prepare_provider_views(
        ctx,
        conn,
        principal,
        std::slice::from_ref(&row),
        &HashMap::from([(id, standing.clone())]),
    )
    .await?;
    Ok(PreparedProjection {
        row,
        standing,
        projection,
    })
}

/// Prepare the same limited view after a handover's accepted ordinary authority was
/// rechecked. A holding-environment manager need not also manage or view the provider.
pub(super) async fn prepare_handover_projection(
    ctx: &Ctx,
    conn: &mut Conn,
    principal: &Principal,
    id: Id,
) -> ApiResult<PreparedProjection> {
    let (row, standing) = read_provider(conn, principal, id).await?;
    let projection = prepare_provider_views(
        ctx,
        conn,
        principal,
        std::slice::from_ref(&row),
        &HashMap::from([(id, standing.clone())]),
    )
    .await?;
    Ok(PreparedProjection {
        row,
        standing,
        projection,
    })
}

async fn prepare_provider_views(
    ctx: &Ctx,
    conn: &mut Conn,
    principal: &Principal,
    rows: &[ProviderRow],
    standing: &HashMap<Id, Standing>,
) -> ApiResult<PreparedProviderViews> {
    let holdings: Vec<_> = rows
        .iter()
        .map(|row| {
            let standing = standing
                .get(&row.id)
                .ok_or_else(|| ApiError::internal("the provider standing is missing"))?;
            Ok(Holding {
                id: row.id,
                admin_place: standing.admin_place,
                admin_only: &standing.admin_only,
                admin_access: standing.admin_access,
                participating: standing.participating,
                owner: standing.owner,
                administers: standing.administers,
                held: &standing.held,
            })
        })
        .collect::<ApiResult<_>>()?;
    let access = super::view::prepare_accesses(conn, &holdings).await?;
    let visible = rows
        .iter()
        .filter(|row| standing.get(&row.id).is_some_and(Standing::views))
        .map(|row| row.id)
        .collect::<Vec<_>>();
    let mut read_ctx = ctx.clone();
    read_ctx.now = principal.now().get();
    let caps =
        crate::routes::usage::caps::prepare_provider_choices(&read_ctx, conn, principal, &visible)
            .await?;
    let mut sources = access.names.clone();
    sources.users.extend(caps.names.users.iter().copied());
    sources.groups.extend(caps.names.groups.iter().copied());
    sources.nodes.extend(caps.names.nodes.iter().copied());
    sources
        .containers
        .extend(caps.names.containers.iter().copied());
    sources.nodes.extend(rows.iter().map(|row| row.home_id));
    Ok(PreparedProviderViews {
        access,
        caps,
        sources,
        bounds: Default::default(),
    })
}

pub(crate) async fn prepare_list_projection(
    ctx: &Ctx,
    conn: &mut Conn,
    principal: &Principal,
    seen: &[resources::Seen],
) -> ApiResult<PreparedListProjection> {
    let ids: Vec<_> = seen.iter().map(|each| each.resource.id).collect();
    let rows = queries::providers(conn, &ids).await?;
    let standing = seen
        .iter()
        .map(|each| {
            (
                each.resource.id,
                Standing {
                    admin_access: each.admin_access,
                    admin_place: each.admin_place,
                    participating: each.participating,
                    admin_only: each.admin_only.clone(),
                    owner: each.owner,
                    administers: each.administers,
                    held: each.held.clone(),
                },
            )
        })
        .collect();
    let projection = prepare_provider_views(ctx, conn, principal, &rows, &standing).await?;
    Ok(PreparedListProjection {
        rows,
        standing,
        projection,
    })
}
orch_api_base::__api_bind_23f24917ee!();

pub(crate) async fn show_list_prepared(
    ctx: &Ctx,
    conn: &mut Conn,
    principal: &Principal,
    prepared: &PreparedListProjection,
) -> ApiResult<Vec<Provider>> {
    let mut read_ctx = ctx.clone();
    read_ctx.now = principal.now().get();
    provider_views_with(
        &read_ctx,
        conn,
        principal,
        prepared.rows.clone(),
        &prepared.standing,
        Some(&prepared.projection),
    )
    .await
}
orch_api_base::__api_bind_9be6fd1d28!();

pub(crate) async fn show_prepared(
    ctx: &Ctx,
    conn: &mut Conn,
    principal: &Principal,
    prepared: PreparedProjection,
) -> ApiResult<Provider> {
    let mut read_ctx = ctx.clone();
    read_ctx.now = principal.now().get();
    show_read_with(
        &read_ctx,
        conn,
        principal,
        prepared.row.clone(),
        prepared.standing.clone(),
        Some(&prepared.projection),
    )
    .await
}
orch_api_base::__api_bind_4494adecd2!();

/// Providers as the API shows them, for any number of them in the same few statements. What the
/// caller does not manage is left out of what is shown: the addresses of the endpoints, the
/// machines, the options, the runtime and the processes of a provider belong to its managers.
async fn provider_views(
    ctx: &Ctx,
    conn: &mut Conn,
    principal: &Principal,
    rows: Vec<ProviderRow>,
    standing: &HashMap<Id, Standing>,
) -> ApiResult<Vec<Provider>> {
    provider_views_with(ctx, conn, principal, rows, standing, None).await
}

async fn provider_views_with(
    ctx: &Ctx,
    conn: &mut Conn,
    principal: &Principal,
    rows: Vec<ProviderRow>,
    standing: &HashMap<Id, Standing>,
    prepared: Option<&PreparedProviderViews>,
) -> ApiResult<Vec<Provider>> {
    let ids: Vec<Id> = rows.iter().map(|row| row.id).collect();
    let credential_policies: HashMap<Id, Json> =
        orch_db::queries::environment_facts::credential_policies(conn, &ids)
            .await?
            .into_iter()
            .collect();
    let clocks: HashMap<Id, _> = orch_db::queries::provider_facts::observation_times(conn, &ids)
        .await?
        .into_iter()
        .map(|(id, status, requested)| (id, (status, requested)))
        .collect();
    let agent_ids: Vec<Id> = rows.iter().filter_map(|row| row.agent_id).collect();
    let agent_references = if agent_ids.is_empty() {
        Vec::new()
    } else {
        queries::agents_among(conn, &agent_ids, &[]).await?
    };
    let managed: Vec<&ProviderRow> = rows
        .iter()
        .filter(|row| standing.get(&row.id).is_some_and(Standing::manages))
        .collect();
    let live = if managed.is_empty() {
        Vec::new()
    } else {
        let managed_ids: Vec<Id> = managed.iter().map(|row| row.id).collect();
        runtime_rows::live_runtimes(conn, &managed_ids).await?
    };
    let mut address_ids = ids.clone();
    address_ids.extend(agent_references.iter().map(|row| row.id));
    address_ids.extend(live.iter().map(|runtime| runtime.host_id));
    address_ids.sort_unstable();
    address_ids.dedup();
    let addresses = addresses(conn, &address_ids).await?;
    let people: Vec<Id> = rows.iter().map(|row| row.owner_id).collect();
    let people = refusals::user_refs(conn, &people).await?;
    let homes: Vec<Id> = rows.iter().map(|row| row.home_id).collect();
    let homes = match prepared {
        Some(prepared) => prepared.bounds.refs(conn, principal, &homes).await?,
        None => node_view::refs(conn, principal, &homes, Policy::default()).await?,
    };
    let agents = agent_refs_with_addresses(&ctx.app, conn, agent_references, &addresses).await?;
    let machine_ids: Vec<Id> = rows
        .iter()
        .flat_map(|row| [row.machine_id, row.key_machine_id])
        .flatten()
        .collect();
    let machines = machine_refs(&ctx.app, conn, &machine_ids).await?;
    let holders: Vec<Id> = rows
        .iter()
        .filter_map(|row| row.held_by_environment_id)
        .collect();
    let holders = environment_refs(conn, &holders, &[], principal.now()).await?;
    let secret_names = queries::provider_secret_names(conn, &ids).await?;
    let models = queries::models_of(conn, &ids).await?;
    let running: HashMap<Id, i64> = queries::running_on(conn, &ids).await?.into_iter().collect();
    let owners: HashMap<Id, i64> = queries::owner_counts(conn, &ids, ctx.now)
        .await?
        .into_iter()
        .collect();
    let processes = queries::harness_processes(conn, &ids).await?;
    let mut last_resets =
        crate::routes::resources::provider_resets::last_resets(conn, &ids, ctx.now).await?;
    let process_agents: Vec<Id> = processes.iter().map(|process| process.agent_id).collect();
    let process_agents = agent_refs(&ctx.app, conn, &process_agents, &[]).await?;
    let process_machines: Vec<Id> = processes.iter().map(|process| process.machine_id).collect();
    let process_machines = machine_refs(&ctx.app, conn, &process_machines).await?;
    let holdings: Vec<Holding<'_>> = rows
        .iter()
        .filter_map(|row| {
            standing.get(&row.id).map(|each| Holding {
                admin_only: &each.admin_only,
                admin_access: each.admin_access,
                admin_place: each.admin_place,
                participating: each.participating,
                id: row.id,
                owner: each.owner,
                administers: each.administers,
                held: &each.held,
            })
        })
        .collect();
    let access = match prepared {
        Some(prepared) => {
            super::view::accesses_prepared(
                conn,
                principal,
                &holdings,
                &prepared.access,
                Some(&prepared.bounds),
            )
            .await?
        }
        None => accesses(conn, principal, &holdings).await?,
    };
    let mut runtimes =
        provider_runtime::runtime_views(ctx, conn, &managed, &live, &addresses).await?;
    let visible_ids: Vec<Id> = rows
        .iter()
        .filter(|row| standing.get(&row.id).is_some_and(Standing::views))
        .map(|row| row.id)
        .collect();
    let mut group_homes: Vec<Id> = rows
        .iter()
        .filter(|row| standing.get(&row.id).is_some_and(Standing::views))
        .map(|row| row.home_id)
        .collect();
    group_homes.sort_unstable();
    group_homes.dedup();
    let group_limits: HashMap<Id, Option<i64>> =
        orch_settings::resolver::get_at_nodes(conn, &group_homes, keys::PROVIDERS_MODEL_GROUPS_MAX)
            .await
            .map_err(ApiError::internal)?;
    let runtime_facts: BTreeMap<_, _> =
        orch_db::queries::environment_facts::providers(conn, &visible_ids)
            .await?
            .into_iter()
            .map(|row| (row.provider_id, row.machine))
            .collect();
    let source_views = super::provider_sources::SourceViews::read(conn, &rows).await?;
    let subscription_homes: Vec<Id> = rows
        .iter()
        .filter(|row| row.kind == "subscription")
        .map(|row| row.home_id)
        .collect();
    let accepted_harnesses: HashMap<Id, Option<Vec<String>>> =
        orch_settings::resolver::get_at_nodes(
            conn,
            &subscription_homes,
            keys::PROVIDERS_SUBSCRIPTION_HARNESSES,
        )
        .await
        .map_err(ApiError::internal)?;
    let shared_subscriptions: HashMap<Id, Option<Vec<String>>> =
        orch_settings::resolver::get_at_nodes(
            conn,
            &subscription_homes,
            keys::PROVIDERS_SUBSCRIPTION_SHARING,
        )
        .await
        .map_err(ApiError::internal)?;
    let setting_places: Vec<orch_settings::ProviderAt<'_>> = rows
        .iter()
        .map(|row| orch_settings::ProviderAt {
            id: row.id,
            home_id: row.home_id,
            runtime_config: &row.runtime_config,
            revision: row.revision,
        })
        .collect();
    let chosen_homes: HashMap<Id, Option<EnvironmentDataSource>> =
        orch_settings::get_at_providers(conn, &setting_places, keys::RUNTIME_HOME)
            .await
            .map_err(ApiError::internal)?;
    let profile_homes = source_views.profile_homes()?;
    let sign_in_machines: Vec<Id> = rows
        .iter()
        .filter(|row| row.kind == "subscription")
        .filter_map(|row| row.key_machine_id.or(row.machine_id))
        .collect();
    let machine_facts: Vec<(Id, Option<Id>, Json)> = if sign_in_machines.is_empty() {
        Vec::new()
    } else {
        sqlx::query_as("SELECT id,runtime_id,facts FROM machines WHERE id=ANY($1)")
            .bind(sign_in_machines)
            .fetch_all(&mut *conn)
            .await
            .map_err(DbError::from)?
    };
    let machine_facts: HashMap<Id, (Option<Id>, Json)> = machine_facts
        .into_iter()
        .map(|(id, runtime, facts)| (id, (runtime, facts)))
        .collect();
    let mut choice_caps = match prepared {
        Some(prepared) => {
            crate::routes::usage::caps::project_provider_choices(
                ctx,
                conn,
                principal,
                prepared.caps.clone(),
                Some(&prepared.bounds),
            )
            .await?
        }
        None => {
            crate::routes::usage::caps::for_provider_choices(ctx, conn, principal, &visible_ids)
                .await?
        }
    };
    let mut comparisons: HashMap<Id, Vec<orch_api_contract::usage::UsageComparison>> =
        HashMap::new();
    if !visible_ids.is_empty() {
        let to = ctx.now.date_naive();
        let from = to - chrono::Duration::days(CHOICE_PERFORMANCE_DAYS - 1);
        for (provider, comparison) in
            orch_db::usage::rollup::provider_comparisons(conn, from, to, &visible_ids).await?
        {
            comparisons.entry(provider).or_default().push(comparison);
        }
    }
    let mut views = Vec::with_capacity(rows.len());
    for row in rows {
        let clock = clocks
            .get(&row.id)
            .ok_or_else(|| ApiError::internal("the provider's observation clocks were not read"))?;
        let can_view = standing.get(&row.id).is_some_and(Standing::views);
        let manages = standing.get(&row.id).is_some_and(Standing::manages);
        let choice_usage = if can_view {
            Some(api::ProviderChoiceUsage {
                caps: choice_caps.remove(&row.id).unwrap_or_default(),
                performance: comparisons.remove(&row.id).unwrap_or_default(),
                observed_at: Time::new(ctx.now),
            })
        } else {
            None
        };
        let (facts, facts_overridden) = if can_view {
            facts_shown(&row.facts, &row.facts_overrides)?
        } else {
            (
                ProviderFacts {
                    plan: None,
                    windows: Vec::new(),
                    limit_reached: None,
                    identity: None,
                    credential: None,
                    available_resets: None,
                    extra_usage: None,
                    balances: Vec::new(),
                    spend: Vec::new(),
                    ordinary_usage_allowed: None,
                    credential_verified: None,
                    models: None,
                    field_times: std::collections::BTreeMap::new(),
                },
                Vec::new(),
            )
        };
        let own_secrets: Vec<&String> = secret_names
            .iter()
            .filter(|(owner, _)| *owner == row.id)
            .map(|(_, name)| name)
            .collect();
        let subscription = if row.kind == "subscription" {
            let kind = row.harness_kind.clone().unwrap_or_default();
            Some(ProviderSubscription {
                accepted: accepted_harnesses
                    .get(&row.home_id)
                    .and_then(Option::as_ref)
                    .is_none_or(|accepted| accepted.contains(&kind)),
                shareable: shared_subscriptions
                    .get(&row.home_id)
                    .and_then(Option::as_ref)
                    .is_none_or(|shared| shared.contains(&kind)),
            })
        } else {
            None
        };
        let (sources, facts_state) = super::provider_sources::view(
            &row,
            &facts,
            &facts_overridden,
            can_view,
            manages,
            &ctx.app.builtin_sources,
            &source_views,
        )?;
        let profile_home = row
            .agent_id
            .and_then(|agent| profile_homes.get(&agent).cloned().flatten());
        let provider_home = chosen_homes
            .get(&row.id)
            .cloned()
            .flatten()
            .or_else(|| {
                (row.key_machine_id.is_some() && row.key_machine_id != row.machine_id)
                    .then_some(EnvironmentDataSource::MadeByOrch)
            })
            .or_else(|| profile_home.clone());
        let sign_in = if can_view && row.kind == "subscription" {
            let credential_home =
                if row.key_machine_id.is_none() || row.key_machine_id == row.machine_id {
                    provider_home.clone()
                } else {
                    profile_home
                };
            let home_owned = match credential_home {
                Some(EnvironmentDataSource::MadeByOrch) => true,
                Some(EnvironmentDataSource::ExistingDirectory { .. }) => false,
                None => row.key_machine_id.or(row.machine_id).is_none_or(|machine| {
                    machine_facts.get(&machine).is_some_and(|(runtime, facts)| {
                        orch_settings::machine_home_owned(
                            *runtime,
                            facts,
                            row.harness_kind.as_deref().unwrap_or(""),
                        )
                    })
                }),
            };
            let policy_allowed = credential_policies
                .get(&row.id)
                .and_then(|policy| policy.get("provider.signin"))
                .and_then(Json::as_bool);
            Some(orch_api_contract::resources::ProviderSignInSupport {
                home_owned,
                policy_allowed,
                allowed: home_owned || policy_allowed == Some(true),
            })
        } else {
            None
        };
        let harness_home = if manages {
            Some(provider_home.unwrap_or(EnvironmentDataSource::MadeByOrch))
        } else {
            None
        };
        views.push(Provider {
            service_catalogue: if can_view {
                Some(super::provider_services::views(
                    &ctx.app.builtin_sources,
                    &row.upstreams,
                    &serde_json::from_value(row.service_offerings.clone())
                        .map_err(ApiError::internal)?,
                )?)
            } else {
                None
            },
            service_offerings: if can_view {
                serde_json::from_value(row.service_offerings.clone()).map_err(ApiError::internal)?
            } else {
                BTreeMap::new()
            },
            model_group_options: if can_view {
                serde_json::from_value(row.model_group_options.clone())
                    .map_err(ApiError::internal)?
            } else {
                BTreeMap::new()
            },
            model_groups_max: if can_view {
                Some(
                    group_limits
                        .get(&row.home_id)
                        .copied()
                        .flatten()
                        .unwrap_or(orch_api_contract::resources::MODEL_GROUPS_DEFAULT),
                )
            } else {
                None
            },
            provenance: orch_api_contract::resource_groups::ResourceProvenance {
                // The generic provider page keeps the existing manager-only machine disclosure.
                // Binding choices separately judge their protected origin/arrangements.
                origin_machine: row
                    .key_machine_id
                    .or(row.machine_id)
                    .filter(|_| manages)
                    .and_then(|id| machines.get(&id).cloned()),
                availability: Vec::new(),
            },
            runtime_unavailable: row.runtime_unavailable.clone(),
            credential_keeper: row
                .key_machine_id
                .filter(|_| manages)
                .and_then(|machine| machines.get(&machine).cloned()),
            runtime_support: runtime_facts.get(&row.id).map(|facts| {
                let descriptor = facts.descriptor.as_ref().and_then(|raw| {
                    serde_json::from_value::<orch_driver_contract::descriptor::DriverDescriptor>(
                        raw.clone(),
                    )
                    .ok()
                });
                orch_api_contract::resources::ProviderRuntimeSupport {
                    resource_limits: descriptor
                        .as_ref()
                        .map(|descriptor| descriptor.resource_limits.clone()),
                    data_storage: descriptor
                        .as_ref()
                        .map(|descriptor| descriptor.data_storage.clone()),
                    machine_id: facts.machine_id,
                    driver: descriptor
                        .as_ref()
                        .filter(|descriptor| {
                            descriptor.lifecycle
                                == orch_driver_contract::descriptor::Lifecycle::Unit
                        })
                        .map(|descriptor| descriptor.name.clone()),
                    home: harness_home,
                    lifecycle: descriptor.as_ref().map(|descriptor| descriptor.lifecycle),
                    commands_after_start: descriptor
                        .map(|descriptor| descriptor.commands_after_start),
                    after_up: facts
                        .policy
                        .as_ref()
                        .and_then(|policy| policy.get("environment.after_up"))
                        .and_then(Json::as_bool),
                    sign_in,
                }
            }),
            runtime_changes: None,
            choice_usage,
            plan_forecasts: None,
            base: ProviderRef {
                address: addresses.get(&row.id).cloned().unwrap_or_default(),
                id: row.id,
                name: row.name.clone(),
                kind: parse(&row.kind)?,
                harness: row
                    .harness_kind
                    .as_deref()
                    .map(|kind| crate::programs::shipped_name(kind, None)),
                harness_kind: row.harness_kind.clone(),
                placement: parse(&row.placement)?,
                status: parse(&row.status)?,
            },
            owner: people
                .get(&row.owner_id)
                .cloned()
                .ok_or_else(|| ApiError::internal("the owner of a provider is not there"))?,
            owners: owners.get(&row.id).copied().unwrap_or(1),
            org_id: row.org_id,
            home: homes
                .get(&row.home_id)
                .cloned()
                .ok_or_else(|| ApiError::internal("the home of a provider is not there"))?,
            agent: row.agent_id.and_then(|agent| {
                agents
                    .iter()
                    .find(|(_, each)| each.id == agent)
                    .map(|(_, each)| each.clone())
            }),
            arrangements: consents(
                &row.arrangements,
                orch_engine::binding::arrangements_of_policy(credential_policies.get(&row.id))
                    .as_deref(),
            ),
            held_by: row.held_by_environment_id.and_then(|holder| {
                let environment = holders
                    .iter()
                    .find(|(_, each)| each.id == holder)
                    .map(|(_, each)| each.clone())?;
                Some(ProviderHeldBy {
                    environment,
                    since: Time::new(row.held_since?),
                })
            }),
            runtime_config: if manages {
                row.runtime_config.clone()
            } else {
                json!({})
            },
            upstreams: if manages {
                upstreams::stored(&row.upstreams)?
            } else {
                Vec::new()
            },
            secrets_set: if manages {
                own_secrets
                    .iter()
                    .filter_map(|name| {
                        name.strip_prefix(crate::routes::resources::sealing::HEADER_PREFIX)
                            .or_else(|| {
                                (name.starts_with("runtime:") || name.starts_with("source:"))
                                    .then_some(name.as_str())
                            })
                    })
                    .map(str::to_owned)
                    .collect()
            } else {
                Vec::new()
            },
            config: if manages {
                row.config.clone()
            } else {
                json!({})
            },
            subscription,
            protocols: row.protocols.clone(),
            key_set: own_secrets.iter().any(|name| name.as_str() == API_KEY),
            key_machine: row
                .key_machine_id
                .filter(|_| {
                    manages
                        && row.kind == "api"
                        && !own_secrets.iter().any(|name| name.as_str() == API_KEY)
                        && !upstreams::stored(&row.upstreams).is_ok_and(|endpoints| {
                            endpoints.iter().all(|endpoint| {
                                endpoint.options.get("auth").and_then(Json::as_str) == Some("none")
                            })
                        })
                })
                .and_then(|machine| machines.get(&machine).cloned()),
            machine: row
                .machine_id
                .filter(|_| manages)
                .and_then(|machine| machines.get(&machine).cloned()),
            status_detail: row.status_detail.clone().filter(|_| can_view),
            status_at: Time::new(clock.0),
            facts_state,
            sources,
            facts,
            facts_overridden,
            facts_at: row.facts_at.filter(|_| can_view).map(Time::new),
            facts_requested_at: clock.1.filter(|_| can_view).map(Time::new),
            last_reset: last_resets.remove(&row.id).filter(|_| can_view),
            models: models
                .iter()
                .filter(|model| model.provider_id == row.id)
                .map(model_view)
                .collect::<ApiResult<_>>()?,
            running_threads: running.get(&row.id).copied().unwrap_or(0),
            runtime: runtimes.remove(&row.id),
            harnesses: if manages {
                processes
                    .iter()
                    .filter(|process| process.provider_id == row.id)
                    .filter_map(|process| {
                        Some(HarnessState {
                            agent: process_agents
                                .iter()
                                .find(|(_, each)| each.id == process.agent_id)?
                                .1
                                .clone(),
                            machine: process_machines.get(&process.machine_id)?.clone(),
                            status: parse(&process.status).ok()?,
                            status_detail: process.status_detail.clone(),
                            harness_version: process.harness_version.clone(),
                            sessions_open: i64::from(process.sessions_open),
                            last_active_at: process.last_active_at.map(Time::new),
                            last_error: process.last_error_at.map(|at| api::HarnessProgramError {
                                at: Time::new(at),
                                detail: process.last_error_detail.clone(),
                            }),
                        })
                    })
                    .collect()
            } else {
                Vec::new()
            },
            revision: row.revision,
            created_at: Time::new(row.created_at),
            updated_at: Time::new(row.updated_at),
            access: access.get(&row.id).cloned().ok_or_else(|| {
                ApiError::internal("what the caller holds on a provider was not read")
            })?,
        });
    }
    Ok(views)
}

/// A model of a provider as the API shows it.
pub fn model_view(row: &ModelRow) -> ApiResult<ProviderModel> {
    let parameters: orch_provider_contract::models::ModelParameters =
        serde_json::from_value(row.parameters.clone()).map_err(|error| {
            ApiError::internal(format!("model parameters do not read: {error}"))
        })?;
    let effective = orch_provider_contract::models::composed_model_parameters(
        &row.parameters,
        &row.groups,
        &row.option_layers,
        None,
    )
    .map_err(ApiError::internal)?;
    Ok(ProviderModel {
        id: row.id,
        token_estimate: orch_engine::token_density::of(&row.model_name),
        model_name: row.model_name.clone(),
        display_name: row.display_name.clone(),
        option_constraints: orch_model_contract::parameters::constraint_catalogue(&effective),
        parameters,
        groups: row.groups.clone(),
        enabled: row.enabled,
        source: match row.source.as_str() {
            "suggested" => ModelSource::Suggested,
            _ => ModelSource::Manual,
        },
    })
}

/// Bounded inventory rows reuse provider rendering without reading any off-page provider.
pub(crate) async fn inventory(
    ctx: &Ctx,
    conn: &mut Conn,
    principal: &Principal,
    seen: &[resources::Seen],
) -> ApiResult<Vec<Provider>> {
    let ids: Vec<_> = seen.iter().map(|each| each.resource.id).collect();
    let rows = queries::providers(conn, &ids).await?;
    let standing = seen
        .iter()
        .map(|each| {
            (
                each.resource.id,
                Standing {
                    admin_access: each.admin_access,
                    admin_place: each.admin_place,
                    participating: each.participating,
                    admin_only: each.admin_only.clone(),
                    owner: each.owner,
                    administers: each.administers,
                    held: each.held.clone(),
                },
            )
        })
        .collect();
    provider_views(ctx, conn, principal, rows, &standing).await
}
orch_api_base::__api_bind_420c6a586c!();

/// The stored provider and its current holdings, read together for this transaction.
async fn read_provider(
    conn: &mut Conn,
    principal: &Principal,
    id: Id,
) -> ApiResult<(ProviderRow, Standing)> {
    let row = queries::providers(conn, &[id])
        .await?
        .pop()
        .ok_or_else(ApiError::not_found)?;
    let object = ObjectRef {
        kind: ResourceKind::Provider.into(),
        id,
    };
    let effective = resources::effective(conn, principal, object, Policy::default())
        .await
        .map_err(ApiError::internal)?;
    Ok((
        row,
        Standing {
            admin_access: effective.admin_access,
            admin_place: effective.admin_place,
            participating: effective.participating,
            admin_only: effective.admin_only,
            owner: effective.owner,
            administers: effective.administers,
            held: effective.held,
        },
    ))
}

/// One provider as the caller sees it. The caller holds something on it already, or this is not
/// asked.
pub async fn show(
    ctx: &Ctx,
    conn: &mut Conn,
    principal: &Principal,
    id: Id,
) -> ApiResult<Provider> {
    let (row, standing) = read_provider(conn, principal, id).await?;
    show_read(ctx, conn, principal, row, standing).await
}
orch_api_base::__api_bind_e9b9616032!();

async fn show_read(
    ctx: &Ctx,
    conn: &mut Conn,
    principal: &Principal,
    row: ProviderRow,
    standing: Standing,
) -> ApiResult<Provider> {
    show_read_with(ctx, conn, principal, row, standing, None).await
}

async fn show_read_with(
    ctx: &Ctx,
    conn: &mut Conn,
    principal: &Principal,
    row: ProviderRow,
    standing: Standing,
    prepared: Option<&PreparedProviderViews>,
) -> ApiResult<Provider> {
    let id = row.id;
    let manages = standing.manages();
    let standing = HashMap::from([(id, standing)]);
    let mut view = provider_views_with(ctx, conn, principal, vec![row], &standing, prepared)
        .await?
        .pop()
        .ok_or_else(|| ApiError::internal("a provider was read and is not shown"))?;
    if manages {
        view.runtime_changes = ctx
            .app
            .engine
            .runtime_changes_from(conn, orch_engine::updates::Target::Provider(id))
            .await
            .map_err(ApiError::internal)?;
    }
    super::provider_plan::add_forecasts(conn, &mut view, ctx.app.clock.now()).await?;
    Ok(view)
}

/// The row of a provider that the caller may know of, or a refusal that says what is missing.
/// `permission` is what the request needs on it.
pub async fn provider_for(
    conn: &mut Conn,
    principal: &Principal,
    id: Id,
    permission: Permission,
) -> ApiResult<ProviderRow> {
    let row = queries::providers(conn, &[id])
        .await?
        .pop()
        .ok_or_else(ApiError::not_found)?;
    require(conn, principal, ResourceKind::Provider, id, permission).await?;
    Ok(row)
}

async fn provider_page_for(
    conn: &mut Conn,
    principal: &Principal,
    id: Id,
) -> ApiResult<(ProviderRow, Standing)> {
    let (row, standing) = read_provider(conn, principal, id).await?;
    if !standing.held.iter().any(|held| {
        !matches!(
            held.permission,
            Permission::AccessManage | Permission::AccessExternal
        )
    }) {
        require(
            conn,
            principal,
            ResourceKind::Provider,
            id,
            Permission::ProviderView,
        )
        .await?;
    }
    Ok((row, standing))
}

fn home_of(row: &ProviderRow) -> Home {
    Home {
        node: Some(row.home_id),
        org: Some(row.org_id),
    }
}

// ---- listing ----

/// The provider list and the inventory use one ownership/use/manage policy, before either pages.
pub(crate) async fn in_scope(
    conn: &mut Conn,
    principal: &Principal,
    mut seen: Vec<resources::Seen>,
    scope: ProviderListScope,
) -> ApiResult<Vec<resources::Seen>> {
    seen.retain(|each| {
        let holds = |permission| each.held.iter().any(|held| held.permission == permission);
        match scope {
            ProviderListScope::All | ProviderListScope::Viewable => true,
            ProviderListScope::Mine => each.owner,
            ProviderListScope::Managed => each.owner || holds(Permission::ProviderManage),
            ProviderListScope::Usable => {
                each.owner || holds(Permission::ProviderManage) || holds(Permission::ProviderUse)
            }
        }
    });
    if scope != ProviderListScope::Usable {
        return Ok(seen);
    }
    let homes: Vec<Id> = seen
        .iter()
        .filter(|each| {
            each.resource.subscription.is_some()
                && each.resource.first_owner != Some(principal.user())
        })
        .filter_map(|each| each.resource.home)
        .collect();
    let policies: HashMap<Id, Option<Vec<String>>> =
        orch_settings::resolver::get_at_nodes(conn, &homes, keys::PROVIDERS_SUBSCRIPTION_SHARING)
            .await
            .map_err(ApiError::internal)?;
    let mut kept = Vec::new();
    for each in seen {
        let Some(kind) = each
            .resource
            .subscription
            .as_ref()
            .filter(|_| each.resource.first_owner != Some(principal.user()))
        else {
            kept.push(each);
            continue;
        };
        let shared = each
            .resource
            .home
            .and_then(|home| policies.get(&home).cloned().flatten());
        if shared.is_none_or(|shared| shared.contains(kind)) {
            kept.push(each);
        }
    }
    Ok(kept)
}

/// Reads provider metadata through its native holding gate; View controls private content.
pub(crate) async fn get(ctx: Ctx, (): (), (): ()) -> ApiResult<Provider> {
    crate::key_operations::snapshot_read::run(ctx, read_snapshot).await
}

async fn read_snapshot(ctx: Ctx) -> ApiResult<Provider> {
    let completed = crate::key_operations::native::read_once(ctx, |ctx, conn, principal| {
        Box::pin(async move {
            let prepared = ctx.operation_data::<PreparedProjection>()?;
            // Persisted facts only; Refresh is the separate explicit native observation.
            show_prepared(ctx, conn, principal, prepared.as_ref().clone()).await
        })
    })
    .await?;
    Ok(completed.value)
}

// ---- making one ----

/// Makes a provider, the caller's own, and the job that makes its runtime. An API provider has its key
/// and secret headers sealed; a subscription is signed in on the runtime, by a method given with it or
/// afterwards.
pub(crate) async fn create(
    ctx: Ctx,
    (): (),
    body: api::CreateProvider,
) -> ApiResult<api::ProviderCreateResponse> {
    let mut caller = crate::middleware::authorization::caller_of(ctx.user()?)?;
    create_inner(ctx, body, &mut caller)
        .await
        .map_err(|error| super::provider_creation::problem(&caller, error))
}

async fn create_inner(
    ctx: Ctx,
    body: api::CreateProvider,
    error_caller: &mut orch_authz::Caller,
) -> ApiResult<api::ProviderCreateResponse> {
    if matches!(body, api::CreateProvider::Subscription { .. }) {
        return create_subscription(ctx, body, error_caller).await;
    }
    let (ctx, mut tx, principal, operands) = super::provider_creation::begin(&ctx).await?;
    // The guard may have intersected a newly changed credential mode. Subsequent
    // refusal content follows that exact current Caller, not the entry snapshot.
    *error_caller = principal.caller().clone();
    let api::CreateProvider::Api {
        runtime_machine_id,
        name,
        home_id,
        access,
        arrangements,
        runtime_config,
        upstreams: given,
        api_key,
        secrets,
        key_machine_id,
        sources,
    } = operands.request.clone()
    else {
        return Err(invalid("kind", "a provider is a subscription or an API"));
    };
    let name = clean_name("name", &name)?;
    let explicit_arrangements = arrangements.is_some();
    let arrangements = match arrangements {
        Some(given) => clean_arrangements("arrangements", &given)?,
        None => KEY_ARRANGEMENTS.map(str::to_owned).to_vec(),
    };
    let runtime_config = match runtime_config {
        Some(given) => clean_runtime_config("runtime_config", &given)?,
        None => json!({}),
    };
    let (place, kind) = placed(&mut tx, home_id).await?;
    if !matches!(kind, NodeKind::Org | NodeKind::Project) {
        return Err(invalid(
            "home_id",
            "a provider is at home in an org or project",
        ));
    }
    let policies = home_policies(&mut tx, home_id).await?;
    let checked = upstreams::check(&given, policies.http_allowed)?;
    let Checked {
        upstreams: endpoints,
        protocols,
        secret_headers,
        keyless: _,
        clear_text,
    } = checked;
    // The key: held by the server, kept on a machine of the caller's, or none.
    if let Some(key) = &api_key
        && (key.is_empty() || key.len() > SECRET_MAX_BYTES)
    {
        return Err(invalid(
            "api_key",
            "a key is text, not empty and not too long",
        ));
    }
    // Creation may leave the key for Sign-in and keys. The stored provider stays needs_setup;
    // an absent value is never treated as a credential or as a keyless endpoint declaration.
    let secrets = secrets.unwrap_or_default();
    for (secret, value) in &secrets {
        if !secret_headers.contains(secret) {
            return Err(invalid(
                &format!("secrets.{secret}"),
                "this is no secret header of an endpoint",
            ));
        }
        if value.is_empty() || value.len() > SECRET_MAX_BYTES {
            return Err(invalid(
                &format!("secrets.{secret}"),
                "a secret is text, not empty and not too long",
            ));
        }
    }
    if let Some(machine) = key_machine_id {
        check_key_machine(&mut tx, &principal, machine, &endpoints).await?;
    }
    if let Some(machine) = runtime_machine_id {
        check_runtime_machine(&mut tx, &principal, machine, "provider").await?;
    }
    if key_machine_id.is_none()
        && let Some(machine) = runtime_machine_id
    {
        check_key_machine(&mut tx, &principal, machine, &endpoints).await?;
    }
    // Whether the caller may make a provider here is the access engine's to say.
    let policy = policy_at(&mut tx, home_id).await?;
    let making = match ownership::may_make(
        &mut tx,
        &principal,
        ResourceKind::Provider,
        Some(home_id),
        &policies.members_make,
        policy,
    )
    .await
    {
        Ok(Making::Allowed(Basis::Member | Basis::Rules(_) | Basis::SystemAdmin)) => Ok(()),
        Ok(Making::Refused(why)) => Err(refused_making(&mut tx, home_id, why).await),
        Err(error) => Err(refusals::problem(&mut tx, error).await),
    };
    crate::key_operations::native::observed(&ctx, "provider.creation.make", making)?;
    let id = orch_types::new_id();
    let endpoints_json = serde_json::to_value(&endpoints).map_err(ApiError::internal)?;
    queries::insert_provider(
        &mut tx,
        &NewProvider {
            id,
            name: &name,
            owner_id: principal.user(),
            home_id,
            upstreams: &endpoints_json,
            protocols: &protocols,
            machine_id: runtime_machine_id,
            key_machine_id,
            arrangements: &arrangements,
            runtime_config: &runtime_config,
            status: "needs_setup",
        },
        ctx.now,
    )
    .await?;
    super::provider_creation::pin_home(&mut tx, id, &operands, principal.user(), principal.now())
        .await?;
    let arrangements =
        admit_new_provider_arrangements(&mut tx, id, &arrangements, explicit_arrangements).await?;
    ownership::start_resource(
        &mut tx,
        ObjectRef {
            kind: ResourceKind::Provider.into(),
            id,
        },
        principal.user(),
        match access {
            None | Some(NewResourceAccess::Defaults) => NewAccess::Defaults,
            Some(NewResourceAccess::OnlyMe) => NewAccess::OnlyMe,
            Some(NewResourceAccess::Home) => NewAccess::Home,
        },
        Time::new(ctx.now),
    )
    .await
    .map_err(ApiError::internal)?;
    let by = principal.user();
    check_runtime_commands_owner(&mut tx, &principal, id, &json!({}), &runtime_config).await?;
    check_runtime_home(&mut tx, id, &runtime_config).await?;
    if let Some(key) = &api_key {
        put_provider_secret(&mut tx, id, API_KEY, key, by, ctx.now).await?;
    }
    for (secret, value) in &secrets {
        put_provider_secret(&mut tx, id, &header_secret(secret), value, by, ctx.now).await?;
    }
    let who = actor_name(&ctx)?;
    record(
        &ctx,
        &mut tx,
        orch_db::audit::action!(AuditAction::ProviderCreated),
        id,
        Home {
            node: Some(home_id),
            org: Some(place.org),
        },
        &format!("{who} added the provider {name}"),
        json!({
            "provider": id,
            "name": name,
            "protocols": protocols,
            "key": if api_key.is_some() { "held" } else if key_machine_id.is_some() { "on a machine" } else { "none" },
            "secret_headers": secrets.keys().collect::<Vec<_>>(),
        }),
    )
    .await?;
    refresh_standing(&ctx, &mut tx, id).await?;
    super::provider_creation::write_sources(&ctx, &mut tx, id, &operands).await?;
    // Its runtime is made by a job of its own, whose bar its page shows.
    if runtime_machine_id.is_none() {
        let queued =
            provider_provision::queue(&mut tx, ctx.now, id, Mode::Make, Some(principal.user()))
                .await?;
        crate::key_operations::native::observed(
            &ctx,
            "provider.creation.origin",
            super::provider_creation::retain_make(&ctx, &mut tx, &principal, &operands, id, queued)
                .await,
        )?;
    }
    let mut receipt = super::provider_creation::receipt(&mut tx, id, None).await?;
    super::provider_creation::observed(&ctx, &mut tx, &receipt, &operands).await?;
    let mut warnings: Vec<String> = Vec::new();
    warnings.extend(leaving_warning(&arrangements));
    if clear_text {
        warnings.push("The key will travel in clear to an endpoint that is not https.".to_owned());
    }
    tx.commit().await.map_err(DbError::from)?;
    crate::key_operations::publication::receipt(&ctx, &receipt)?;
    ctx.app.engine.kick();
    let source_credentials = sources.as_ref().is_some_and(|sources| {
        [&sources.facts, &sources.models, &sources.presenter]
            .into_iter()
            .flatten()
            .any(|source| {
                source
                    .authorization
                    .as_ref()
                    .and_then(Option::as_ref)
                    .is_some_and(|value| !value.is_empty())
                    || source
                        .secrets
                        .values()
                        .flatten()
                        .any(|value| !value.is_empty())
            })
    });
    if api_key.is_some() || source_credentials {
        super::provider_sources::first_reads(&ctx, id, [true, true, false]).await;
    }
    let provider =
        super::provider_creation::read_after(&ctx, principal.caller(), &operands, id).await?;
    let warning = (!warnings.is_empty()).then(|| warnings.join(" "));
    receipt.warning = warning.clone();
    crate::key_operations::publication::receipt(&ctx, &receipt)?;
    Ok(match provider {
        Some(provider) if !ctx.key_receipt_required() => {
            api::ProviderCreateResponse::full(ProviderWriteReply { provider, warning })
        }
        _ => {
            receipt.warning = warning;
            api::ProviderCreateResponse::Receipt(receipt)
        }
    })
}

/// A key kept on a machine: it must be a machine of the caller's, which lists every endpoint in its
/// local policy.
async fn check_key_machine(
    conn: &mut Conn,
    principal: &Principal,
    machine: Id,
    endpoints: &[orch_api_contract::resources::Upstream],
) -> ApiResult<()> {
    let row = orch_db::queries::resources::machine(conn, machine)
        .await?
        .filter(|row| row.deleted_at.is_none() && row.is_active());
    let Some(row) = row else {
        return Err(invalid(
            "keeper_machine_id",
            "this credential keeper is unavailable",
        ));
    };
    super::machines::require_machine(conn, principal, machine, Permission::MachineUse).await?;
    if !plays(&row, "provider") {
        return Err(invalid(
            "keeper_machine_id",
            "this machine has not enabled the provider role",
        ));
    }
    crate::backplane::require_facts(&row.facts)?;
    let listed: Vec<String> = row
        .facts
        .get("policy")
        .and_then(|policy| policy.get("provider.upstreams"))
        .and_then(Json::as_array)
        .map(|urls| {
            urls.iter()
                .filter_map(|url| url.as_str().map(str::to_owned))
                .collect()
        })
        .unwrap_or_default();
    for (index, endpoint) in endpoints.iter().enumerate() {
        if !listed
            .iter()
            .any(|url| url == "*" || url == &endpoint.base_url)
        {
            return Err(invalid(
                &format!("upstreams[{index}].base_url"),
                "the machine's local policy does not let it forward model calls there",
            ));
        }
    }
    Ok(())
}

/// A runtime selects a declared directory service, under the same machine Use authority as any attachment.
async fn check_runtime_machine(
    conn: &mut Conn,
    principal: &Principal,
    id: Id,
    role: &str,
) -> ApiResult<MachineRow> {
    let row = orch_db::queries::resources::machine(conn, id)
        .await?
        .filter(|row| row.deleted_at.is_none() && row.is_active())
        .ok_or_else(ApiError::not_found)?;
    super::machines::require_machine(conn, principal, id, Permission::MachineUse).await?;
    crate::backplane::require_facts(&row.facts)?;
    if row
        .facts
        .get("provider_runtime_descriptor")
        .or_else(|| row.facts.get("environment_descriptor"))
        .and_then(|descriptor| descriptor.get("lifecycle"))
        .and_then(Json::as_str)
        != Some("attach")
    {
        return Err(invalid(
            "runtime_machine_id",
            "this machine has not declared a directory runtime service",
        ));
    }
    if !plays(&row, role) {
        return Err(invalid(
            "runtime_machine_id",
            &format!("this machine has not enabled its {role} role"),
        ));
    }
    Ok(row)
}

// ---- changing one ----

/// The answer to a change that was made against a revision that is out of date: what the
/// provider is now.
async fn stale(ctx: &Ctx, conn: &mut Conn, principal: &Principal, id: Id) -> ApiError {
    let current = show(ctx, conn, principal, id).await.ok();
    ApiError::new(
        ErrorCode::ConflictStaleRevision,
        "the provider was changed since it was read",
    )
    .with("current", current)
}

/// F02 update conflicts use that operation's current optional Read projection.
async fn update_stale(ctx: &Ctx, accepted: &orch_authz::Caller, id: Id) -> ApiError {
    let stale = || {
        ApiError::new(
            ErrorCode::ConflictStaleRevision,
            "the provider was changed since it was read",
        )
    };
    let mut reading = match ctx.app.db.begin_snapshot().await {
        Ok(reading) => reading,
        Err(_) => return stale(),
    };
    let projection = ctx.clone();
    let current = super::provider_projection::read(
        ctx,
        &mut reading,
        accepted,
        id,
        move |conn, reader, prepared| {
            Box::pin(async move {
                show_prepared(
                    &projection,
                    conn,
                    reader,
                    prepared.ok_or_else(|| {
                        ApiError::internal("the bounded provider projection is absent")
                    })?,
                )
                .await
            })
        },
    )
    .await
    .ok()
    .flatten();
    if reading.commit().await.is_err() {
        ctx.complete_disclosure(crate::key_operations::resolver::Disclosure::Denied);
        return stale();
    }
    match current {
        Some(current) => stale().with("current", current),
        None => stale(),
    }
}

/// Optional current metadata after the provider mutation has already committed.
/// Both real changes and accepted no-ops use the same truthful receipt fallback.
async fn update_reply(
    ctx: &Ctx,
    accepted: &orch_authz::Caller,
    id: Id,
    revision: i64,
    warning: Option<String>,
) -> ApiResult<api::ProviderUpdateResponse> {
    let projection = ctx.clone();
    let provider = crate::key_operations::completed_projection::run(ctx, async {
        let mut reading = ctx.app.db.begin_snapshot().await?;
        let provider = super::provider_projection::read(
            ctx,
            &mut reading,
            accepted,
            id,
            move |conn, reader, prepared| {
                Box::pin(async move {
                    show_prepared(
                        &projection,
                        conn,
                        reader,
                        prepared.ok_or_else(|| {
                            ApiError::internal("the bounded provider projection is absent")
                        })?,
                    )
                    .await
                })
            },
        )
        .await?;
        reading.commit().await.map_err(DbError::from)?;
        Ok(provider)
    })
    .await?;
    Ok(match provider {
        Some(provider) => {
            api::ProviderUpdateResponse::full(ProviderWriteReply { provider, warning })
        }
        None => api::ProviderUpdateResponse::Receipt(api::ProviderUpdateReceipt {
            projection: orch_api_contract::common::ReceiptProjection::Receipt,
            provider_id: id,
            revision,
        }),
    })
}

/// Changes a provider; a change its running runtime cannot take is stored for the next update.
/// The native schema check shared by updates and a copied model configuration.
pub(crate) fn checked_model_groups(
    groups: &BTreeMap<String, orch_provider_contract::models::ModelGroupOptions>,
) -> ApiResult<Json> {
    for (name, options) in groups {
        if !api::is_model_group_name(name) {
            return Err(invalid(
                "model_group_options",
                "each group needs its usual short name",
            ));
        }
        orch_provider_contract::models::ModelParameters {
            request_options: options.request_options.clone(),
            ..Default::default()
        }
        .check_option_schemas()
        .map_err(|reason| invalid("model_group_options", reason))?;
    }
    serde_json::to_value(groups).map_err(ApiError::internal)
}

async fn update(
    ctx: Ctx,
    (): (),
    body: api::ProviderUpdateBody,
) -> ApiResult<api::ProviderUpdateResponse> {
    let id = ctx.id("id")?;
    let name = body
        .name
        .as_deref()
        .map(|name| clean_name("name", name))
        .transpose()?;
    let (ctx, mut tx, principal) = begin_mutation(&ctx).await?;
    let row = queries::providers(&mut tx, &[id])
        .await?
        .pop()
        .ok_or_else(ApiError::not_found)?;
    let policies = home_policies(&mut tx, row.home_id).await?;
    let checked = match &body.upstreams {
        Some(given) => {
            if row.kind != "api" {
                return Err(invalid("upstreams", "only an API provider has endpoints"));
            }
            Some(upstreams::check(given, policies.http_allowed)?)
        }
        None => None,
    };
    if let Some(config) = &body.config {
        if !config.is_object() {
            return Err(invalid("config", "the options are an object"));
        }
        if row.kind == "api"
            && config
                .as_object()
                .is_some_and(|options| !options.is_empty())
        {
            return Err(invalid(
                "config",
                "only a subscription has options of its adapter",
            ));
        }
        if row.kind == "subscription" {
            let descriptor = match row.agent_id {
                Some(agent) => agent_rows::agent(&mut tx, agent)
                    .await?
                    .filter(|agent| agent.deleted_at.is_none())
                    .map(|agent| orch_engine::harnesses::descriptor_of(&agent))
                    .transpose()
                    .map_err(ApiError::internal)?
                    .flatten(),
                None => None,
            };
            match descriptor.and_then(|descriptor| descriptor.native_provider) {
                Some(native) => {
                    super::validation::provider_options(&native.options_schema, config)?
                }
                None if config
                    .as_object()
                    .is_some_and(|options| !options.is_empty()) =>
                {
                    return Err(ApiError::new(
                        ErrorCode::SourceOffline,
                        "the subscription's option schema is not available",
                    ));
                }
                None => {}
            }
        }
    }
    let arrangements = body
        .arrangements
        .as_deref()
        .map(|given| clean_arrangements("arrangements", given))
        .transpose()?;
    if let Some(proposed) = &arrangements {
        let _ = admit_arrangements(&mut tx, id, proposed, &row.arrangements, true).await?;
    }
    let runtime_config = body
        .runtime_config
        .as_ref()
        .map(|given| clean_runtime_config("runtime_config", given))
        .transpose()?;
    if let Some(config) = &runtime_config {
        check_runtime_commands_owner(&mut tx, &principal, id, &row.runtime_config, config).await?;
        check_runtime_home(&mut tx, id, config).await?;
    }
    let endpoints_json = checked
        .as_ref()
        .map(|checked| serde_json::to_value(&checked.upstreams))
        .transpose()
        .map_err(ApiError::internal)?;
    let protocols = checked.as_ref().map(|checked| checked.protocols.clone());
    let group_options = body
        .model_group_options
        .as_ref()
        .map(checked_model_groups)
        .transpose()?;
    let service_offerings = body
        .service_offerings
        .as_ref()
        .map(|choices| {
            super::provider_services::checked(
                &ctx.app.builtin_sources,
                endpoints_json.as_ref().unwrap_or(&row.upstreams),
                choices,
                &row.service_offerings,
            )
        })
        .transpose()?;
    let change = ProviderChange {
        service_offerings: service_offerings.as_ref(),
        model_group_options: group_options.as_ref(),
        name: name.as_deref(),
        upstreams: endpoints_json.as_ref().zip(protocols.as_deref()),
        config: body.config.as_ref(),
        arrangements: arrangements.as_deref(),
        runtime_config: runtime_config.as_ref(),
        facts_overrides: None,
    };
    if change == ProviderChange::default() {
        if row.revision != body.expected_revision {
            drop(tx);
            return Err(update_stale(&ctx, principal.caller(), id).await);
        }
        tx.commit().await.map_err(DbError::from)?;
        return update_reply(&ctx, principal.caller(), id, row.revision, None).await;
    }
    // Authority and the revision are checked in the transaction of the change.
    if !queries::change_provider(&mut tx, id, body.expected_revision, &change, ctx.now).await? {
        drop(tx);
        return Err(update_stale(&ctx, principal.caller(), id).await);
    }
    if let Some(checked) = &checked {
        // A secret header that no endpoint lists any more has no use for its value.
        let mut keep: Vec<String> = vec![API_KEY.to_owned()];
        keep.extend(
            checked
                .secret_headers
                .iter()
                .map(|name| header_secret(name)),
        );
        keep.extend(
            orch_db::queries::settings::secret_names_of(&mut tx, &[("provider", id)])
                .await?
                .into_iter()
                .map(|secret| secret.name)
                .filter(|name| name.starts_with("runtime:")),
        );
        queries::delete_provider_secrets_except(&mut tx, id, &keep).await?;
    }
    let who = actor_name(&ctx)?;
    let describes = name.is_some()
        || checked.is_some()
        || body.config.is_some()
        || runtime_config.is_some()
        || body.model_group_options.is_some()
        || body.service_offerings.is_some();
    if describes {
        record(
            &ctx,
            &mut tx,
            orch_db::audit::action!(AuditAction::ProviderUpdated),
            id,
            home_of(&row),
            &format!("{who} changed the provider {}", row.name),
            json!({
                "provider": id,
                "name": name,
                "upstreams": checked.is_some(),
                "config": body.config.is_some(),
                "runtime_config": runtime_config.is_some(),
                "model_group_options": body.model_group_options.is_some(),
                "service_offerings": body.service_offerings.is_some(),
            }),
        )
        .await?;
    }
    if let Some(allowed) = &arrangements
        && *allowed != row.arrangements
    {
        record(
            &ctx,
            &mut tx,
            orch_db::audit::action!(AuditAction::ProviderArrangementsChanged),
            id,
            home_of(&row),
            &format!(
                "{who} changed the types the provider {} may be used in",
                row.name
            ),
            json!({ "provider": id, "before": row.arrangements, "after": allowed }),
        )
        .await?;
    }
    refresh_standing(&ctx, &mut tx, id).await?;
    let revision = queries::providers(&mut tx, &[id])
        .await?
        .pop()
        .ok_or_else(|| ApiError::internal("the changed provider disappeared"))?
        .revision;
    tx.commit().await.map_err(DbError::from)?;
    if runtime_config.is_some()
        && let Err(error) = ctx.app.engine.apply_provider_resources(id).await
    {
        tracing::warn!(%id,%error,"provider resource changes remain pending");
    }
    // A rename changes the provider's Orch address. Pending work resolves the policy again,
    // including inputs whose files are being prepared; accepted running turns keep their work.
    let pending = {
        let mut conn = ctx.app.db.acquire().await?;
        orch_db::queries::threads::pending_threads_of_provider(&mut conn, id).await?
    };
    for thread in pending {
        ctx.app.engine.wake(thread);
    }
    let mut warnings: Vec<String> = Vec::new();
    if let Some(allowed) = &arrangements {
        let added = allowed
            .iter()
            .any(|word| lets_credential_leave(word) && !row.arrangements.contains(word));
        if added {
            warnings.extend(leaving_warning(allowed));
        }
    }
    if checked.as_ref().is_some_and(|checked| checked.clear_text) {
        warnings.push("The key will travel in clear to an endpoint that is not https.".to_owned());
    }
    // Runtime application may have waited after commit. Only optional metadata
    // may narrow; a completed effect always retains its accepted revision.
    update_reply(
        &ctx,
        principal.caller(),
        id,
        revision,
        (!warnings.is_empty()).then(|| warnings.join(" ")),
    )
    .await
}

/// Deletes a provider while its threads keep their binding and history; its
/// key and secret headers are deleted.
async fn delete(ctx: Ctx, (): (), (): ()) -> ApiResult<()> {
    let id = ctx.id("id")?;
    let (ctx, mut tx, principal) = begin_mutation(&ctx).await?;
    let row = queries::providers(&mut tx, &[id])
        .await?
        .pop()
        .ok_or_else(ApiError::not_found)?;
    let (stop, finished) = orch_engine::providers::deleting::apply(
        &ctx.app.engine,
        &mut tx,
        &row,
        principal.user(),
        ctx.now,
    )
    .await
    .map_err(ApiError::internal)?;
    // The admitted stop retires its runtime after the turns stop; the reaper then erases
    // its state volume and the sign-in kept there without waiting on a missing machine.
    let who = actor_name(&ctx)?;
    record(
        &ctx,
        &mut tx,
        orch_db::audit::action!(AuditAction::ProviderDeleted),
        id,
        home_of(&row),
        &format!("{who} deleted the provider {}", row.name),
        json!({ "provider": id, "name": row.name }),
    )
    .await?;
    tx.commit().await.map_err(DbError::from)?;
    for thread in finished.wakes {
        ctx.app.engine.wake(thread);
    }
    ctx.app
        .engine
        .stop_resource_now(stop.id)
        .await
        .map_err(ApiError::internal)?;
    Ok(())
}

/// Switches a provider off.
async fn disable(
    ctx: Ctx,
    query: orch_api_contract::resource_facts::ResourcePauseQuery,
    (): (),
) -> ApiResult<Provider> {
    let note = super::impact::note(query.note.as_deref())?;
    switch(&ctx, true, note, false).await
}

/// Switches a provider on again: it stands where what it has puts it.
async fn enable(
    ctx: Ctx,
    query: orch_api_contract::resource_facts::ResourceResumeQuery,
    (): (),
) -> ApiResult<Provider> {
    switch(&ctx, false, None, query.run_waiting.unwrap_or(false)).await
}

async fn switch(
    ctx: &Ctx,
    off: bool,
    note: Option<&str>,
    run_waiting: bool,
) -> ApiResult<Provider> {
    let id = ctx.id("id")?;
    let (current, mut tx, principal) = begin_mutation(ctx).await?;
    let ctx = &current;
    let row = queries::providers(&mut tx, &[id])
        .await?
        .pop()
        .ok_or_else(ApiError::not_found)?;
    let was_off = row.status == "disabled";
    let mut stopping = None;
    let mut enable_target = None;
    let mut resumed = Vec::new();
    if off != was_off {
        let target = orch_api_contract::resource_facts::FactTarget {
            object: orch_access_contract::decisions::ObjectRef {
                kind: orch_access_contract::permissions::ObjectKind::Provider,
                id,
            },
            model_id: None,
        };
        if off {
            let (job, finished) = ctx
                .app
                .engine
                .stop_resource_in(
                    &mut tx,
                    orch_engine::resource_pause::ResourceStop {
                        target,
                        name: &row.name,
                        by: principal.user(),
                        note,
                        deleted: false,
                    },
                    ctx.now,
                )
                .await
                .map_err(ApiError::internal)?;
            stopping = Some((job.id, finished));
        } else {
            if orch_engine::resource_facts::activity::stopping(&mut tx, &target)
                .await
                .map_err(ApiError::internal)?
            {
                return Err(ApiError::new(
                    ErrorCode::PlanBlocked,
                    "Waiting for the accepted stop to finish",
                ));
            }
            enable_target = Some(target);
        }
        let (status, action, word) = if off {
            (
                "disabled",
                orch_db::audit::action!(AuditAction::ProviderDisabled),
                "switched off",
            )
        } else {
            (
                "needs_setup",
                orch_db::audit::action!(AuditAction::ProviderEnabled),
                "switched on",
            )
        };
        queries::set_provider_standing(&mut tx, id, status, if off { note } else { None }, ctx.now)
            .await?;
        if !off {
            refresh_standing(ctx, &mut tx, id).await?;
        }
        if let Some(target) = enable_target {
            let finished = ctx
                .app
                .engine
                .enable_resource_waiting_in(
                    &mut tx,
                    orch_engine::resource_pause::ResourceResume {
                        target: &target,
                        name: &row.name,
                        by: principal.user(),
                        run_waiting,
                    },
                    ctx.now,
                )
                .await
                .map_err(ApiError::internal)?;
            resumed.extend(finished.wakes);
        }
        let who = actor_name(ctx)?;
        record(
            ctx,
            &mut tx,
            action,
            id,
            home_of(&row),
            &format!("{who} {word} the provider {}", row.name),
            json!({ "provider": id }),
        )
        .await?;
    }
    let provider = show(ctx, &mut tx, &principal, id).await?;
    tx.commit().await.map_err(DbError::from)?;
    for thread in resumed {
        ctx.app.engine.wake(thread);
    }
    if let Some((job, finished)) = stopping {
        for thread in finished.wakes {
            ctx.app.engine.wake(thread);
        }
        ctx.app
            .engine
            .stop_resource_now(job)
            .await
            .map_err(ApiError::internal)?;
    }
    Ok(provider)
}

// ---- the key and the secret headers ----

/// Saves a key without changing its keeper, then returns the provider after its sources are asked.
async fn key_set(ctx: Ctx, (): (), body: api::ProviderKeySetBody) -> ApiResult<Provider> {
    let id = ctx.id("id")?;
    if body.api_key.is_empty() || body.api_key.len() > SECRET_MAX_BYTES {
        return Err(invalid(
            "api_key",
            "a key is text, not empty and not too long",
        ));
    }
    let (ctx, mut tx, _) = begin_mutation(&ctx).await?;
    let row = queries::providers(&mut tx, &[id])
        .await?
        .pop()
        .ok_or_else(ApiError::not_found)?;
    // Replacing the value does not move the credential keeper or the provider runtime.
    if row.kind != "api" {
        return Err(invalid("api_key", "only an API provider takes a key here"));
    }
    let by = ctx.user()?.id();
    let replaced = put_provider_secret(&mut tx, id, API_KEY, &body.api_key, by, ctx.now).await?;
    let who = actor_name(&ctx)?;
    record(
        &ctx,
        &mut tx,
        orch_db::audit::action!(AuditAction::ProviderKeyRotated),
        id,
        home_of(&row),
        &format!(
            "{who} {} the key of the provider {}",
            if replaced { "replaced" } else { "set" },
            row.name
        ),
        json!({ "provider": id, "replaced": replaced }),
    )
    .await?;
    refresh_standing(&ctx, &mut tx, id).await?;
    tx.commit().await.map_err(DbError::from)?;
    // The key is durably saved before its source is asked. A failed discovery must not turn
    // that successful write into a failed save; the source's own error remains in the view.
    super::provider_sources::first_reads(&ctx, id, [true, true, false]).await;
    let (ctx, mut tx, principal) = begin_mutation(&ctx).await?;
    require(
        &mut tx,
        &principal,
        ResourceKind::Provider,
        id,
        Permission::ProviderManage,
    )
    .await?;
    let shown = show(&ctx, &mut tx, &principal, id).await?;
    tx.commit().await.map_err(DbError::from)?;
    Ok(shown)
}

/// Writes the value of one secret header of a provider's endpoints, sealed. Like the key, it is
/// not returned.
async fn secret_set(ctx: Ctx, (): (), body: api::ProviderSecretSetBody) -> ApiResult<()> {
    let id = ctx.id("id")?;
    let name = ctx.param("name")?.to_owned();
    if body.value.is_empty() || body.value.len() > SECRET_MAX_BYTES {
        return Err(invalid(
            "value",
            "a secret is text, not empty and not too long",
        ));
    }
    let (ctx, mut tx, _) = begin_mutation(&ctx).await?;
    let row = queries::providers(&mut tx, &[id])
        .await?
        .pop()
        .ok_or_else(ApiError::not_found)?;
    let listed: BTreeSet<String> = upstreams::stored(&row.upstreams)?
        .iter()
        .flat_map(|endpoint| {
            endpoint
                .options
                .get("secret_headers")
                .and_then(Json::as_array)
                .into_iter()
                .flatten()
                .filter_map(|name| name.as_str().map(str::to_owned))
                .collect::<Vec<_>>()
        })
        .collect();
    let runtime_name = name.strip_prefix("runtime:");
    let runtime_names = runtime_secret_names(&mut tx, id).await?;
    let runtime_secret =
        runtime_name.is_some_and(|name| runtime_names.iter().any(|listed| listed == name));
    if !listed.contains(&name) && !runtime_secret {
        return Err(ApiError::not_found());
    }
    let by = ctx.user()?.id();
    let stored_name = if runtime_secret {
        name.clone()
    } else {
        header_secret(&name)
    };
    let replaced = put_provider_secret(&mut tx, id, &stored_name, &body.value, by, ctx.now).await?;
    let who = actor_name(&ctx)?;
    record(
        &ctx,
        &mut tx,
        AuditAction::ProviderUpdated,
        id,
        home_of(&row),
        &format!(
            "{who} {} the secret header {name} of the provider {}",
            if replaced { "replaced" } else { "set" },
            row.name
        ),
        json!({ "provider": id, "secret_header": name, "replaced": replaced }),
    )
    .await?;
    tx.commit().await.map_err(DbError::from)?;
    Ok(())
}

/// The same resolved native declaration governs writes and copies of runtime secret values.
pub(crate) async fn runtime_secret_names(conn: &mut Conn, provider: Id) -> ApiResult<Vec<String>> {
    setting::<Vec<String>>(conn, Place::Provider(provider), keys::RUNTIME_SECRET_NAMES)
        .await
        .map_err(ApiError::internal)
        .map(Option::unwrap_or_default)
}

// ---- its own settings, and the threads that are bound to it ----

// ---- a provider that is a person's machine ----

/// The org or project something is registered in, within the caller's domain.
/// `field` names the request member that supplied it.
pub(crate) async fn registration_home(
    conn: &mut Conn,
    principal: &Principal,
    home: Id,
    field: &str,
) -> ApiResult<Placed> {
    let (place, kind) = placed(conn, home).await?;
    if !matches!(kind, NodeKind::Org | NodeKind::Project) {
        return Err(invalid(field, "it is registered in an org or project"));
    }
    let graph = Graph::load(conn, &[home])
        .await
        .map_err(ApiError::internal)?;
    let in_domain = graph.get(home).is_some_and(|node| {
        principal
            .memberships()
            .domain_in(node.root)
            .is_some_and(|domain| node.path.contains(&domain))
    });
    if !in_domain {
        return Err(invalid(
            field,
            "it is registered in the caller's domain or an org inside it",
        ));
    }
    Ok(place)
}

/// Whether a machine reported that it plays `role`.
fn plays(machine: &MachineRow, role: &str) -> bool {
    machine.roles.iter().any(|each| each == role)
}

/// Registers a provider whose runtime is a machine of the caller's. A subscription is the login its
/// harness already has there, which the machine reports (`provider.found`): registering it asks the
/// harness nothing. An API is reached with a key that is typed on the machine and never sent. Either is the caller's own, as the machine is, and is used on that
/// machine and nowhere else until its owner allows more.
async fn machine_provider_create(
    ctx: Ctx,
    (): (),
    _body: api::MachineProviderCreateBody,
) -> ApiResult<api::MachineProviderCreateResponse> {
    // Use the guard's normalized request; the writer refreshes it again on its transaction.
    let request = ctx
        .operation_data::<super::provider_creation::Operands>()?
        .request
        .clone();
    Ok(match create(ctx, (), request).await? {
        api::ProviderCreateResponse::Full(reply) => {
            api::MachineProviderCreateResponse::full(reply.provider)
        }
        api::ProviderCreateResponse::Receipt(receipt) => {
            api::MachineProviderCreateResponse::Receipt(receipt)
        }
    })
}

// ---- looking at it, writing what is known by hand, and its harness ----

/// What a failed look at a provider's machine is to the caller.
fn unreachable_provider(error: &Refreshed) -> Option<ApiError> {
    match error {
        Refreshed::Answered { .. } => None,
        Refreshed::Gone => Some(ApiError::not_found()),
        Refreshed::NoMachine => Some(ApiError::new(
            ErrorCode::ProviderUnavailable,
            "the provider has no machine to run on",
        )),
        Refreshed::Offline => Some(ApiError::new(
            ErrorCode::SourceOffline,
            "the machine that keeps the provider is offline",
        )),
        Refreshed::Failed(text) => Some(ApiError::new(
            ErrorCode::ProviderUnavailable,
            format!("the provider could not be looked at: {text}"),
        )),
    }
}

/// Has the machine that keeps a provider look at its credential and report: the state it is in, and
/// the plan and the limits its harness reports.
async fn provider_refresh(ctx: Ctx, (): (), body: api::ProviderRefreshBody) -> ApiResult<Provider> {
    let id = ctx.id("id")?;
    let (ctx, mut tx, principal) = begin_mutation(&ctx).await?;
    let row = provider_for(&mut tx, &principal, id, Permission::ProviderView).await?;
    if body.retry == Some(true) {
        require(
            &mut tx,
            &principal,
            ResourceKind::Provider,
            id,
            Permission::ProviderManage,
        )
        .await?;
    }
    let retry = body.retry == Some(true) && row.status == "error";
    let origin = crate::sign_in::origin(&ctx);
    let refreshed = if retry {
        ctx.app
            .engine
            .refresh_provider_for_retry_in(tx, &principal, &origin, id)
            .await
    } else {
        ctx.app
            .engine
            .refresh_provider_in(tx, &principal, &origin, id)
            .await
    }
    .map_err(ApiError::internal)?;
    if let Some(refusal) = unreachable_provider(&refreshed)
        && (!retry || !matches!(refreshed, Refreshed::Failed(_)))
    {
        return Err(refusal);
    }
    // Discovery is another independent source: its refusal must not erase freshly read plan or key facts.
    if let Err(error) = ctx.app.engine.discover_provider_models(id).await {
        tracing::debug!(%id,%error,"the provider's model list could not be refreshed");
    }
    let (ctx, mut tx, principal) = begin_mutation(&ctx).await?;
    provider_for(&mut tx, &principal, id, Permission::ProviderView).await?;
    let mut trial = None;
    if retry {
        require(
            &mut tx,
            &principal,
            ResourceKind::Provider,
            id,
            Permission::ProviderManage,
        )
        .await?;
        if !orch_db::queries::provider_facts::lock_provider(&mut tx, id).await? {
            return Err(ApiError::not_found());
        }
        if !ctx
            .app
            .engine
            .provider_has_credential_read(&mut tx, id)
            .await
            .map_err(ApiError::internal)?
        {
            trial = orch_db::queries::provider_facts::claim_trial(&mut tx, id, ctx.now).await?;
            if trial.is_none()
                && orch_db::queries::resources::thread_engine::provider(&mut tx, id)
                    .await?
                    .is_some_and(|current| current.status == "error")
                && !orch_db::queries::provider_facts::trial_pending(&mut tx, id).await?
            {
                return Err(ApiError::invalid(
                    "retry",
                    "No eligible waiting input is available for a credential trial.",
                )
                .with("current", show(&ctx, &mut tx, &principal, id).await?));
            }
            if let Some((input, thread)) = trial {
                record(
                    &ctx,
                    &mut tx,
                    AuditAction::ProviderUpdated,
                    id,
                    home_of(&row),
                    "A manager requested one credential trial",
                    json!({"credential_trial_input":input,"thread":thread}),
                )
                .await?;
            }
        }
    }
    refresh_standing(&ctx, &mut tx, id).await?;
    let shown = show(&ctx, &mut tx, &principal, id).await?;
    tx.commit().await.map_err(DbError::from)?;
    if let Some((_, thread)) = trial {
        ctx.app.engine.wake(thread);
    }
    Ok(shown)
}

/// Writes the facts of a provider by hand: the members given win over what its harness or endpoint
/// reports, and a member left out is as reported again. The members given replace those given before.
async fn facts_overrides_set(
    mut ctx: Ctx,
    (): (),
    body: api::ProviderFactsOverridesSetBody,
) -> ApiResult<Provider> {
    let id = ctx.id("id")?;
    let mut overrides = serde_json::to_value(&body).map_err(ApiError::internal)?;
    if let Some(members) = overrides.as_object_mut() {
        members.remove("expected_revision");
    }
    let (mut tx, guard) = crate::middleware::transaction_guard::begin(&ctx).await?;
    ctx.now = guard.now().get();
    let principal = guard.principal();
    require(
        &mut tx,
        principal,
        ResourceKind::Provider,
        id,
        Permission::ProviderManage,
    )
    .await?;
    let row = queries::providers(&mut tx, &[id])
        .await?
        .pop()
        .ok_or_else(ApiError::not_found)?;
    if row.revision != body.expected_revision {
        return Err(stale(&ctx, &mut tx, principal, id).await);
    }
    validate_fact_overrides(&body)?;
    // What is written in must still read as the facts of a provider.
    if let Err(error) = facts_shown(&row.facts, &overrides) {
        return Err(invalid("facts", &error.to_string()));
    }
    let change = ProviderChange {
        model_group_options: None,
        facts_overrides: Some(&overrides),
        ..ProviderChange::default()
    };
    if !queries::change_provider(&mut tx, id, body.expected_revision, &change, ctx.now).await? {
        return Err(stale(&ctx, &mut tx, principal, id).await);
    }
    let who = actor_name(&ctx)?;
    let members: Vec<&String> = overrides
        .as_object()
        .map(|each| each.keys().collect())
        .unwrap_or_default();
    record(
        &ctx,
        &mut tx,
        AuditAction::ProviderUpdated,
        id,
        home_of(&row),
        &format!("{who} wrote facts of the provider {} by hand", row.name),
        json!({ "provider": id, "facts_overrides": members }),
    )
    .await?;
    let shown = show(&ctx, &mut tx, principal, id).await?;
    tx.commit().await.map_err(DbError::from)?;
    Ok(shown)
}

/// Numeric facts have the same domains whether a runtime observes them or a manager enters them.
fn validate_fact_overrides(body: &api::ProviderFactsOverridesSetBody) -> ApiResult<()> {
    use orch_provider_contract::limits;
    for (name, len, maximum) in [
        (
            "windows",
            body.windows.as_ref().map_or(0, Vec::len),
            limits::WINDOWS_MAX,
        ),
        (
            "balances",
            body.balances.as_ref().map_or(0, Vec::len),
            limits::BALANCES_MAX,
        ),
        (
            "spend",
            body.spend.as_ref().map_or(0, Vec::len),
            limits::SPEND_MAX,
        ),
    ] {
        if len > maximum {
            return Err(invalid(name, &format!("has at most {maximum} items")));
        }
    }
    let amount = |path: &str, value: Option<&str>| -> ApiResult<()> {
        if value.is_some_and(|value| !limits::is_decimal(value)) {
            Err(invalid(
                path,
                &format!(
                    "is decimal text of at most {} characters",
                    limits::DECIMAL_MAX_CHARS
                ),
            ))
        } else {
            Ok(())
        }
    };
    for (index, balance) in body.balances.iter().flatten().enumerate() {
        for (name, value) in [
            ("remaining", balance.remaining.as_deref()),
            ("total", balance.total.as_deref()),
            ("used", balance.used.as_deref()),
        ] {
            amount(&format!("balances.{index}.{name}"), value)?;
        }
        for (part_index, part) in balance.parts.iter().flatten().enumerate() {
            amount(
                &format!("balances.{index}.parts.{part_index}.amount"),
                Some(&part.amount),
            )?;
        }
    }
    for (index, spend) in body.spend.iter().flatten().enumerate() {
        for (name, value) in [
            ("remaining", spend.remaining.as_deref()),
            ("limit", spend.limit.as_deref()),
            ("used", spend.used.as_deref()),
        ] {
            amount(&format!("spend.{index}.{name}"), value)?;
        }
        if spend.period_minutes.is_some_and(|minutes| minutes <= 0) {
            return Err(invalid(
                &format!("spend.{index}.period_minutes"),
                "is a positive number of minutes",
            ));
        }
    }
    // A warning is a position within a complete usage window, whose end is 100 percent.
    const FULL_WINDOW_PERCENT: f64 = 100.0;
    for (index, window) in body.windows.iter().flatten().enumerate() {
        if !window.used_percent.is_finite() || window.used_percent < 0.0 {
            return Err(invalid(
                &format!("windows.{index}.used_percent"),
                "is a finite, nonnegative percentage",
            ));
        }
        if window.duration_minutes.is_some_and(|minutes| minutes <= 0) {
            return Err(invalid(
                &format!("windows.{index}.duration_minutes"),
                "is a positive number of minutes",
            ));
        }
        if window.warning_percent.is_some_and(|percent| {
            !percent.is_finite() || !(0.0..=FULL_WINDOW_PERCENT).contains(&percent)
        }) {
            return Err(invalid(
                &format!("windows.{index}.warning_percent"),
                "is a percentage between zero and a complete window",
            ));
        }
    }
    Ok(())
}

/// The profile of a harness of this provider: one that serves it, or would.
async fn harness_of(ctx: &Ctx, conn: &mut Conn, provider: Id) -> ApiResult<Id> {
    let agent = ctx.id("agent_id")?;
    queries::providers(conn, &[provider])
        .await?
        .pop()
        .ok_or_else(ApiError::not_found)?;
    if !binding_rows::harness_is_known(conn, provider, agent).await? {
        return Err(ApiError::not_found());
    }
    Ok(agent)
}

/// Brings the harness process of a profile for a provider up anew, wherever it runs.
async fn harness_restart(ctx: Ctx, (): (), (): ()) -> ApiResult<Provider> {
    let id = ctx.id("id")?;
    let (ctx, mut tx, principal) = begin_mutation(&ctx).await?;
    let agent = harness_of(&ctx, &mut tx, id).await?;
    let restarted = ctx
        .app
        .engine
        .restart_harness_in(tx, &principal, &crate::sign_in::origin(&ctx), id, agent)
        .await
        .map_err(ApiError::internal)?;
    match restarted {
        Restarted::Done(_) => {}
        Restarted::Gone => return Err(ApiError::not_found()),
        Restarted::NoMachine => {
            return Err(ApiError::new(
                ErrorCode::ProviderUnavailable,
                "the provider has no machine to run on",
            ));
        }
        Restarted::Offline => {
            return Err(ApiError::new(
                ErrorCode::SourceOffline,
                "the machine the harness runs on is offline",
            ));
        }
        Restarted::Failed(text) => {
            return Err(ApiError::new(
                ErrorCode::ProviderUnavailable,
                format!("the harness could not be started again: {text}"),
            ));
        }
    }
    let mut conn = ctx.app.db.acquire().await?;
    let principal = authorization::principal(&ctx, &mut conn).await?;
    show(&ctx, &mut conn, &principal, id).await
}

/// The conversations a harness already holds, where its adapter can say; the thread that adopted
/// one is told with it.
async fn harness_sessions(ctx: Ctx, (): (), (): ()) -> ApiResult<Vec<api::NativeSession>> {
    let id = ctx.id("id")?;
    let agent = {
        let mut conn = ctx.app.db.acquire().await?;
        harness_of(&ctx, &mut conn, id).await?
    };
    let found = ctx
        .app
        .engine
        .discover_sessions(id, agent)
        .await
        .map_err(ApiError::internal)?;
    let listed = match found {
        Discovered::Sessions(listed) => listed,
        Discovered::Gone => return Err(ApiError::not_found()),
        Discovered::Unsupported => {
            return Err(ApiError::new(
                ErrorCode::CapabilityUnsupported,
                "this harness cannot list the conversations it holds",
            ));
        }
        Discovered::NoMachine => {
            return Err(ApiError::new(
                ErrorCode::HarnessSessionsNoRuntime,
                "this agent has no runtime for this provider yet; start a thread with this agent first",
            ));
        }
        Discovered::Offline => {
            return Err(ApiError::new(
                ErrorCode::SourceOffline,
                "the machine the harness runs on is offline",
            ));
        }
        Discovered::Failed(text) => {
            return Err(ApiError::new(
                ErrorCode::HarnessSessionsUnavailable,
                format!("the harness could not list its conversations: {text}"),
            ));
        }
    };
    let mut sessions: Vec<api::NativeSession> = listed
        .into_iter()
        .map(|each| {
            serde_json::from_value(each).map_err(|error| {
                ApiError::internal(format!("a conversation does not read: {error}"))
            })
        })
        .collect::<ApiResult<_>>()?;
    let references: Vec<String> = sessions
        .iter()
        .map(|each| each.native_session_ref.clone())
        .collect();
    let mut conn = ctx.app.db.acquire().await?;
    let adopted: HashMap<String, Id> = thread_rows::adopting_threads(&mut conn, &references)
        .await?
        .into_iter()
        .collect();
    for each in &mut sessions {
        each.adopted_thread_id = adopted.get(&each.native_session_ref).copied();
    }
    Ok(sessions)
}

// ---- a subscription, whose sign-in a managed runtime holds ----

/// Makes a subscription, the caller's own, and the job that makes the runtime its harness signs in on.
/// The profile needs `agent.use` and its adapter has to declare a sign-in of its own; the provider's
/// home may accept some harnesses' subscriptions and not others, and a subscription that stays with its
/// first owner is not shared with the home. A method given with the request is started as soon as the
/// provider exists, and waits for the runtime, with its secrets sealed, if that is not up.
async fn create_subscription(
    ctx: Ctx,
    _body: api::CreateProvider,
    error_caller: &mut orch_authz::Caller,
) -> ApiResult<api::ProviderCreateResponse> {
    let (ctx, mut tx, principal, operands) = super::provider_creation::begin(&ctx).await?;
    *error_caller = principal.caller().clone();
    let api::CreateProvider::Subscription {
        runtime_machine_id,
        key_machine_id,
        name,
        home_id,
        access,
        arrangements,
        runtime_config,
        agent_id,
        config,
        sign_in,
        sources: _,
    } = operands.request.clone()
    else {
        return Err(invalid("kind", "a provider is a subscription or an API"));
    };
    let name = clean_name("name", &name)?;
    let runtime_config = match runtime_config {
        Some(given) => clean_runtime_config("runtime_config", &given)?,
        None => json!({}),
    };
    let config = config.unwrap_or_else(|| json!({}));
    if !config.is_object() {
        return Err(invalid("config", "the options are an object"));
    }
    let (place, kind) = placed(&mut tx, home_id).await?;
    if !matches!(kind, NodeKind::Org | NodeKind::Project) {
        return Err(invalid(
            "home_id",
            "a provider is at home in an org or project",
        ));
    }
    let policies = home_policies(&mut tx, home_id).await?;
    // The profile whose harness holds the sign-in: one that is there, and that the caller may use.
    let profile = agent_rows::agent(&mut tx, agent_id)
        .await?
        .filter(|profile| profile.deleted_at.is_none())
        .ok_or_else(|| invalid("agent_id", "this is no profile"))?;
    require(
        &mut tx,
        &principal,
        ResourceKind::Agent,
        agent_id,
        Permission::AgentUse,
    )
    .await?;
    if profile.disabled {
        return Err(invalid("agent_id", "this profile is switched off"));
    }
    if profile.machine_id.is_some()
        && key_machine_id.is_some()
        && profile.machine_id != key_machine_id
    {
        return Err(invalid(
            "keeper_machine_id",
            "this native profile is registered on another machine",
        ));
    }
    // A registered native profile declares its own keeper; that fact never chooses its runtime.
    let key_machine_id = key_machine_id.or(profile.machine_id);
    if let Some(machine) = runtime_machine_id {
        let runtime = check_runtime_machine(&mut tx, &principal, machine, "agent").await?;
        if !machine_reports(&runtime.facts, &profile.harness_kind) {
            return Err(invalid(
                "runtime_machine_id",
                "this machine has no permitted installed program for this native harness",
            ));
        }
    }
    if let Some(machine) = key_machine_id {
        let keeper = orch_db::queries::resources::machine(&mut tx, machine)
            .await?
            .filter(|row| row.deleted_at.is_none() && row.is_active())
            .ok_or_else(ApiError::not_found)?;
        super::machines::require_machine(&mut tx, &principal, machine, Permission::MachineUse)
            .await?;
        crate::backplane::require_facts(&keeper.facts)?;
        if !plays(&keeper, "agent") || !machine_reports(&keeper.facts, &profile.harness_kind) {
            return Err(invalid(
                "keeper_machine_id",
                "this credential keeper has not declared the profile's native harness",
            ));
        }
    }
    let descriptor = orch_engine::harnesses::descriptor_of(&profile).map_err(ApiError::internal)?;
    // A harness that has not said what it is cannot be asked: what its adapter declares is known once
    // its runtime has run it, and what is given then is checked against it.
    let native = match &descriptor {
        Some(descriptor) => Some(descriptor.native_provider.clone().ok_or_else(|| {
            invalid(
                "agent_id",
                "the harness of this profile has no sign-in of its own",
            )
        })?),
        None => None,
    };
    if let Some(native) = &native {
        super::validation::provider_options(&native.options_schema, &config)?;
        let options = native
            .options_schema
            .get("properties")
            .and_then(Json::as_object);
        let closed = native
            .options_schema
            .get("additionalProperties")
            .and_then(Json::as_bool)
            == Some(false);
        if let (Some(options), true, Some(given)) = (options, closed, config.as_object())
            && let Some(unknown) = given.keys().find(|key| !options.contains_key(key.as_str()))
        {
            return Err(invalid(
                &format!("config.{unknown}"),
                "is no option that this provider's harness has",
            ));
        }
    } else if config
        .as_object()
        .is_some_and(|options| !options.is_empty())
    {
        return Err(ApiError::new(
            ErrorCode::SourceOffline,
            "the subscription's option schema is not available",
        ));
    }
    if policies
        .subscription_harnesses
        .as_ref()
        .is_some_and(|accepted| !accepted.contains(&profile.harness_kind))
    {
        return Err(ApiError::new(
            ErrorCode::ProviderSubscriptionNotAllowed,
            "subscriptions of this harness are not allowed in this org",
        )
        .with("harness_kind", profile.harness_kind.clone()));
    }
    if matches!(access, Some(NewResourceAccess::Home))
        && policies
            .subscription_sharing
            .as_ref()
            .is_some_and(|shared| !shared.contains(&profile.harness_kind))
    {
        return Err(ApiError::new(
            ErrorCode::ProviderSubscriptionNotShared,
            "a subscription of this harness stays with its owner in this org",
        )
        .with("harness_kind", profile.harness_kind.clone()));
    }
    let explicit_arrangements = arrangements.is_some();
    let arrangements = match arrangements {
        Some(given) => clean_arrangements("arrangements", &given)?,
        None => match &native {
            Some(native) if !native.arrangements.is_empty() => native
                .arrangements
                .iter()
                .filter_map(|kind| {
                    serde_json::to_value(kind)
                        .ok()
                        .and_then(|word| word.as_str().map(str::to_owned))
                })
                .collect(),
            _ => vec!["separated".to_owned()],
        },
    };
    // Whether the caller may make a provider here is the access engine's to say.
    let policy = policy_at(&mut tx, home_id).await?;
    match ownership::may_make(
        &mut tx,
        &principal,
        ResourceKind::Provider,
        Some(home_id),
        &policies.members_make,
        policy,
    )
    .await
    {
        Ok(Making::Allowed(Basis::Member | Basis::Rules(_) | Basis::SystemAdmin)) => {}
        Ok(Making::Refused(why)) => return Err(refused_making(&mut tx, home_id, why).await),
        Err(error) => return Err(refusals::problem(&mut tx, error).await),
    }
    // A method named with the request is checked now, where the harness has said what it declares.
    if let Some(request) = &sign_in {
        provider_login::check_request(
            native.as_ref().map(|native| &native.login_methods),
            request,
        )?;
    }
    let id = orch_types::new_id();
    runtime_rows::insert_subscription(
        &mut tx,
        &runtime_rows::NewSubscription {
            id,
            name: &name,
            owner_id: principal.user(),
            home_id,
            harness_kind: &profile.harness_kind,
            agent_id,
            config: &config,
            arrangements: &arrangements,
            runtime_config: &runtime_config,
        },
        ctx.now,
    )
    .await?;
    sqlx::query("UPDATE providers SET machine_id=$2,key_machine_id=$3 WHERE id=$1")
        .bind(id)
        .bind(runtime_machine_id)
        .bind(key_machine_id)
        .execute(&mut *tx)
        .await
        .map_err(DbError::from)?;
    super::provider_creation::pin_home(&mut tx, id, &operands, principal.user(), principal.now())
        .await?;
    let arrangements =
        admit_new_provider_arrangements(&mut tx, id, &arrangements, explicit_arrangements).await?;
    ownership::start_resource(
        &mut tx,
        ObjectRef {
            kind: ResourceKind::Provider.into(),
            id,
        },
        principal.user(),
        match access {
            None | Some(NewResourceAccess::Defaults) => NewAccess::Defaults,
            Some(NewResourceAccess::OnlyMe) => NewAccess::OnlyMe,
            Some(NewResourceAccess::Home) => NewAccess::Home,
        },
        Time::new(ctx.now),
    )
    .await
    .map_err(ApiError::internal)?;
    let who = actor_name(&ctx)?;
    check_runtime_commands_owner(&mut tx, &principal, id, &json!({}), &runtime_config).await?;
    check_runtime_home(&mut tx, id, &runtime_config).await?;
    record(
        &ctx,
        &mut tx,
        AuditAction::ProviderCreated,
        id,
        Home {
            node: Some(home_id),
            org: Some(place.org),
        },
        &format!(
            "{who} added the subscription {name} of {}",
            profile.harness_kind
        ),
        json!({
            "provider": id,
            "name": name,
            "harness_kind": profile.harness_kind,
            "agent": agent_id,
        }),
    )
    .await?;
    super::provider_creation::write_sources(&ctx, &mut tx, id, &operands).await?;
    // Its runtime is made by a job of its own, whose bar its page shows.
    if runtime_machine_id.is_none() {
        let queued =
            provider_provision::queue(&mut tx, ctx.now, id, Mode::Make, Some(principal.user()))
                .await?;
        super::provider_creation::retain_make(&ctx, &mut tx, &principal, &operands, id, queued)
            .await?;
    }
    // Runtime changes return the creation snapshot before the runner can complete or fail its job.
    let provider = super::provider_creation::read(&ctx, &mut tx, &principal, &operands, id).await?;
    let mut receipt = super::provider_creation::receipt(&mut tx, id, None).await?;
    super::provider_creation::observed(&ctx, &mut tx, &receipt, &operands).await?;
    tx.commit().await.map_err(DbError::from)?;
    crate::key_operations::publication::receipt(&ctx, &receipt)?;
    ctx.app.engine.kick();
    let mut warnings = leaving_warning(&arrangements)
        .into_iter()
        .collect::<Vec<_>>();
    if let Some(request) = &sign_in {
        // The provider is made either way: a sign-in that cannot be begun is told by the answer.
        if let Err(error) = provider_login::begin(&ctx, id, request).await {
            tracing::warn!(%id,%error,"the provider was created but its requested sign-in could not begin");
            warnings.push(
                "The provider was created. Open Sign-in and keys to start its sign-in.".to_owned(),
            );
        }
    }
    let warning = (!warnings.is_empty()).then(|| warnings.join(" "));
    receipt.warning = warning.clone();
    crate::key_operations::publication::receipt(&ctx, &receipt)?;
    Ok(match provider {
        Some(provider) if !ctx.key_receipt_required() => {
            api::ProviderCreateResponse::full(ProviderWriteReply { provider, warning })
        }
        _ => {
            receipt.warning = warning;
            api::ProviderCreateResponse::Receipt(receipt)
        }
    })
}

#[cfg(any(test, feature = "test-support"))]
pub mod tests {

    use super::*;
    #[cfg(any(test, feature = "test-support"))]
    fn a_provider_with_a_credential_and_a_model_is_ready_and_without_either_it_has_to_be_set_up() {
        assert_eq!(derived_status("needs_setup", true, 1), Some("ready"));
        assert_eq!(derived_status("ready", true, 0), Some("needs_setup"));
        assert_eq!(derived_status("ready", false, 3), Some("needs_setup"));
        // What a switch, a machine or a turn said is not undone by counting.
        for kept in ["disabled", "exhausted", "error", "offline"] {
            assert_eq!(derived_status(kept, true, 3), None, "{kept}");
        }
    }
    #[cfg(any(test, feature = "test-support"))]
    #[doc(hidden)]
    pub fn __api_run_a_provider_with_a_credential_and_a_model_is_ready_and_without_either_it_has_to_be_set_up()
     {
        self::a_provider_with_a_credential_and_a_model_is_ready_and_without_either_it_has_to_be_set_up()
    }

    #[cfg(any(test, feature = "test-support"))]
    fn a_copy_of_the_credential_leaving_is_said_and_staying_home_is_not() {
        assert!(leaving_warning(&["separated".into(), "key_separated".into()]).is_none());
        assert!(leaving_warning(&["separated".into(), "handed_over".into()]).is_some());
        let consents = consents(&["separated".to_owned()], None);
        assert_eq!(consents.len(), 4);
        assert!(consents[0].allowed && !consents[1].allowed);
        assert!(consents[2].form.is_some() && consents[0].form.is_none());
    }
    #[cfg(any(test, feature = "test-support"))]
    #[doc(hidden)]
    pub fn __api_run_a_copy_of_the_credential_leaving_is_said_and_staying_home_is_not() {
        self::a_copy_of_the_credential_leaving_is_said_and_staying_home_is_not()
    }

    #[cfg(any(test, feature = "test-support"))]
    fn a_provider_allows_at_least_one_type_and_each_type_once() {
        use ArrangementType::{KeySeparated, Separated};
        assert!(clean_arrangements("arrangements", &[]).is_err());
        assert_eq!(
            clean_arrangements("arrangements", &[Separated, Separated, KeySeparated]).unwrap(),
            ["separated", "key_separated"]
        );
    }
    #[cfg(any(test, feature = "test-support"))]
    #[doc(hidden)]
    pub fn __api_run_a_provider_allows_at_least_one_type_and_each_type_once() {
        self::a_provider_allows_at_least_one_type_and_each_type_once()
    }

    #[cfg(any(test, feature = "test-support"))]
    fn a_runtime_has_values_for_the_keys_of_its_own_scope_only() {
        assert!(
            clean_runtime_config("runtime_config", &json!({ "runtime.update": "manual" })).is_ok()
        );
        assert!(clean_runtime_config("runtime_config", &json!({ "env.mounts": [] })).is_err());
        assert!(clean_runtime_config("runtime_config", &json!([])).is_err());
    }
    #[cfg(any(test, feature = "test-support"))]
    #[doc(hidden)]
    pub fn __api_run_a_runtime_has_values_for_the_keys_of_its_own_scope_only() {
        self::a_runtime_has_values_for_the_keys_of_its_own_scope_only()
    }

    #[cfg(any(test, feature = "test-support"))]
    fn old_kept_credit_overrides_keep_their_precedence_as_balances_without_legacy_members_in_the_answer()
     {
        let known = json!({"plan_credits":{"balance":"10.5","unit":"credits","unlimited":false},"balances":[{"id":"account","unit":"USD","remaining":"3.00"}]});
        let (facts, overridden) = facts_shown(
            &known,
            &json!({"plan_credits":{"balance":"24.75","unit":"credits","unlimited":false}}),
        )
        .unwrap();
        assert_eq!(
            facts
                .balances
                .iter()
                .find(|balance| balance.id == "plan")
                .unwrap()
                .remaining
                .as_deref(),
            Some("24.75")
        );
        assert_eq!(
            facts
                .balances
                .iter()
                .find(|balance| balance.id == "account")
                .unwrap()
                .remaining
                .as_deref(),
            Some("3.00")
        );
        assert_eq!(overridden, ["balances"]);
        let json = serde_json::to_value(facts).unwrap();
        for legacy in [
            "plan_credits",
            "key_credits",
            "key_usage",
            "key_usage_supported",
        ] {
            assert!(json.get(legacy).is_none());
        }
    }
    #[cfg(any(test, feature = "test-support"))]
    #[doc(hidden)]
    pub fn __api_run_old_kept_credit_overrides_keep_their_precedence_as_balances_without_legacy_members_in_the_answer()
     {
        self::old_kept_credit_overrides_keep_their_precedence_as_balances_without_legacy_members_in_the_answer()
    }

    #[cfg(any(test, feature = "test-support"))]
    fn manual_balance_and_spend_amounts_keep_decimal_precision_and_refuse_exponent_or_non_decimal_text()
     {
        for amount in ["0.000000000000000000001", "-3.25"] {
            let body:api::ProviderFactsOverridesSetBody=serde_json::from_value(json!({"expected_revision":1,"balances":[{"id":"plan","unit":"credits","remaining":amount}],"spend":[{"id":"month","unit":"USD","period":"month","used":amount}]})).unwrap();
            assert!(validate_fact_overrides(&body).is_ok());
        }
        for amount in ["1e3", "NaN", "1."] {
            let body:api::ProviderFactsOverridesSetBody=serde_json::from_value(json!({"expected_revision":1,"balances":[{"id":"plan","unit":"credits","remaining":amount}]})).unwrap();
            assert!(validate_fact_overrides(&body).is_err());
        }
    }
    #[cfg(any(test, feature = "test-support"))]
    #[doc(hidden)]
    pub fn __api_run_manual_balance_and_spend_amounts_keep_decimal_precision_and_refuse_exponent_or_non_decimal_text()
     {
        self::manual_balance_and_spend_amounts_keep_decimal_precision_and_refuse_exponent_or_non_decimal_text()
    }
}
