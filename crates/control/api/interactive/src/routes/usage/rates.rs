//! What each model usable at a node would be counted at now and from where that rate comes, the shipped catalogue of estimates as it is loaded, and its replacement by a system admin from a file, from the address an admin set, or from the release.
//!
//! A rate is looked for in the order the ledger counts a row at: the provider's
//! own entry for the model, the rate card of the place, the shipped catalogue
//! scaled to the place's unit. The table says which of the three a model's rate
//! is, which pattern of the card or the catalogue matched it, and how many
//! tokens of it were used in the last thirty days without one. The catalogue is
//! checked whole before it replaces the one in force, by the very function the
//! command line of the host calls, and the act is audited.

use super::csv::json_response;

use super::names::Names;

use super::names::rate_of;

use crate::middleware::authorization;

use crate::middleware::authorization::refusals;

use crate::middleware::requests::ApiError;

use crate::middleware::requests::ApiResult;

use crate::registry::Ctx;

use crate::sign_in::origin;

use axum::body::Body;

use axum::response::Response;

use chrono::Duration;

use chrono::NaiveDate;

use http::StatusCode;

use orch_access_contract::permissions::Permission;

use orch_agent_contract::discovery::ModelPattern;

use orch_agent_contract::parameters::ModelParameters;

use orch_agent_contract::parameters::Rate;

use orch_api_contract::common::ErrorCode;

use orch_api_contract::usage::RateCatalogue;

use orch_api_contract::usage::RateFamily;

use orch_api_contract::usage::RateSource;

use orch_api_contract::usage::RateTable;

use orch_api_contract::usage::RateTableCatalogue;

use orch_api_contract::usage::RateTableModel;

use orch_api_contract::usage::USAGE_CATALOGUE_MAX_BYTES;

use orch_api_contract::usage::UsageCatalogueGetQuery;

use orch_api_contract::usage::UsageRateTableGetQuery;

use orch_authz::Act;

use orch_authz::Policy;

use orch_db::Conn;

use orch_db::audit::Actor;

use orch_db::seeds;

use orch_db::seeds::SeedError;

use orch_db::seeds::Taken;

use orch_db::usage::ledger;

use orch_db::usage::ledger::CatalogueRow;

use orch_db::usage::rollup;

use orch_db::usage::rollup::Filter;

use orch_db::usage::rollup::Place as RollupPlace;

use orch_engine::rates::RateBook;

use orch_settings::Place;

use orch_settings_contract::registry::keys;

use orch_types::Id;

use orch_types::Time;

use orch_worker_core::catalogue as fetching;

use orch_worker_core::catalogue::CatalogueError;

use std::collections::BTreeMap;

use std::collections::HashMap;
/// How many days back the tokens used without a rate are counted for the table of rates.
const UNRATED_LOOK_BACK_DAYS: i64 = 30;

/// Where a release keeps the catalogue it came with, beside its programs and its `migrations/`.
const SHIPPED_FILE: &str = "share/model-rates.json";

/// Where a checkout of the repository has it.
const CHECKOUT_FILE: &str = "deploy/model-rates.json";

fn word<T: serde::Serialize>(value: &T) -> Option<RateSource> {
    serde_json::to_value(value)
        .ok()
        .and_then(|value| serde_json::from_value(value).ok())
}

/// The pattern of `candidates` that names `model` with the most characters that are not `*`.
fn matched<'a>(patterns: impl IntoIterator<Item = &'a str>, model: &str) -> Option<String> {
    ModelPattern::best(
        patterns.into_iter().map(|pattern| (pattern, pattern)),
        model,
    )
    .map(str::to_owned)
}

/// Says what each model usable at a node is counted at.
pub async fn table(ctx: Ctx, query: UsageRateTableGetQuery, (): ()) -> ApiResult<RateTable> {
    crate::key_operations::native::selected_read::<Table>(ctx, query, ()).await
}

async fn table_for(
    ctx: &Ctx,
    conn: &mut Conn,
    principal: &orch_authz::Principal,
    value: &PreparedTable,
) -> ApiResult<RateTable> {
    let query = &value.query;
    if query.system == Some(true) {
        if query.node_id.is_some() {
            return Err(ApiError::invalid("system", "choose a node or the system"));
        }
        ctx.ordinary_system_admin_with(ctx.user()?, principal.caller())?;
        // The system form edits its own patterns, not the contents of any private provider.
        let on = orch_settings::get(conn, Place::System, keys::USAGE_CATALOGUE)
            .await
            .map_err(ApiError::internal)?
            .unwrap_or(true);
        let scale = orch_settings::get(conn, Place::System, keys::USAGE_CATALOGUE_SCALE)
            .await
            .map_err(ApiError::internal)?
            .unwrap_or_else(|| "1".to_owned());
        let catalogue = ledger::catalogue_in_force(conn).await?;
        return Ok(RateTable {
            models: Vec::new(),
            catalogue: RateTableCatalogue {
                on,
                scale,
                as_of: catalogue
                    .map(|row| row.as_of.to_string())
                    .unwrap_or_default(),
            },
        });
    }
    let node = query
        .node_id
        .ok_or_else(|| ApiError::invalid("node_id", "choose a node or system=true"))?;
    authorization::require(
        conn,
        principal,
        node,
        authorization::exact(Permission::UsageView),
        Act::Use,
        Policy::default(),
    )
    .await?;
    let place = rollup::place_of(conn, node)
        .await?
        .ok_or_else(ApiError::not_found)?;
    let usable = value.usable.clone();
    let book = RateBook::load(conn, node)
        .await
        .map_err(ApiError::internal)?;
    let card: BTreeMap<String, Rate> =
        orch_settings::get(conn, Place::Node(node), keys::USAGE_RATES)
            .await
            .map_err(ApiError::internal)?
            .unwrap_or_default();
    let in_force = ledger::catalogue_in_force(conn).await?;
    let families: Vec<String> = in_force
        .as_ref()
        .map(|row| families_of(&row.doc))
        .unwrap_or_default()
        .into_iter()
        .map(|family| family.pattern)
        .collect();
    let on = orch_settings::get::<bool>(conn, Place::Node(node), keys::USAGE_CATALOGUE)
        .await
        .map_err(ApiError::internal)?
        .unwrap_or(true);
    let scale = orch_settings::get::<String>(conn, Place::Node(node), keys::USAGE_CATALOGUE_SCALE)
        .await
        .map_err(ApiError::internal)?
        .unwrap_or_else(|| "1".to_owned());
    let since = ctx.now.date_naive() - Duration::days(UNRATED_LOOK_BACK_DAYS);
    let unrated: HashMap<(Id, String), i64> = rollup::unrated_by_model(
        conn,
        &Filter {
            node: Some(RollupPlace { id: node, ..place }),
            ..Filter::default()
        },
        since,
    )
    .await?
    .into_iter()
    .map(|(provider, model, tokens)| ((provider, model), tokens))
    .collect();
    let mut provider_ids: Vec<Id> = usable.iter().map(|each| each.provider_id).collect();
    provider_ids.sort_unstable();
    provider_ids.dedup();
    let names = Names::load(conn, principal, &[], &provider_ids, &[]).await?;

    let mut models = Vec::new();
    for each in usable {
        let Some(provider) = names.provider(each.provider_id) else {
            continue;
        };
        let own = serde_json::from_value::<ModelParameters>(each.parameters.clone())
            .ok()
            .and_then(|parameters| parameters.rate);
        let found = book
            .find(conn, own.as_ref(), &each.model_name)
            .await
            .map_err(ApiError::internal)?;
        let (rate, source, pattern) = match found {
            None => (None, None, None),
            Some(found) => {
                let pattern = match found.source {
                    orch_engine::rates::Source::Provider | orch_engine::rates::Source::Reported => {
                        None
                    }
                    orch_engine::rates::Source::RateCard => {
                        matched(card.keys().map(String::as_str), &each.model_name)
                    }
                    orch_engine::rates::Source::Catalogue => {
                        matched(families.iter().map(String::as_str), &each.model_name)
                    }
                };
                (
                    rate_of(&serde_json::to_value(&found.rate).map_err(ApiError::internal)?),
                    word(&found.source.word()),
                    pattern,
                )
            }
        };
        let tokens = unrated
            .get(&(each.provider_id, each.model_name.clone()))
            .copied()
            .unwrap_or(0);
        models.push(RateTableModel {
            provider,
            display_name: if each.display_name.is_empty() {
                each.model_name.clone()
            } else {
                each.display_name
            },
            model_name: each.model_name,
            rate,
            source,
            pattern,
            unrated_tokens: tokens,
        });
    }
    Ok(RateTable {
        models,
        catalogue: RateTableCatalogue {
            on,
            scale,
            as_of: in_force
                .map(|row| row.as_of.to_string())
                .unwrap_or_default(),
        },
    })
}

/// The families of a catalogue document; one that does not read is left out.
fn families_of(doc: &orch_types::Json) -> Vec<RateFamily> {
    doc.get("families")
        .and_then(orch_types::Json::as_array)
        .into_iter()
        .flatten()
        .filter_map(|family| serde_json::from_value(family.clone()).ok())
        .collect()
}

/// Where the catalogue the running release came with is looked for.
fn shipped_places(config: &orch_config::ServerConfig) -> Vec<std::path::PathBuf> {
    let mut places = Vec::new();
    if let Some(release) = std::path::Path::new(&config.database.migrations_dir).parent() {
        places.push(release.join(SHIPPED_FILE));
    }
    if let Some(bin) = std::env::current_exe()
        .ok()
        .and_then(|program| program.parent().map(std::path::Path::to_owned))
    {
        places.push(bin.join("..").join(SHIPPED_FILE));
        places.push(bin.join("..").join("..").join(CHECKOUT_FILE));
    }
    places.push(std::path::PathBuf::from(CHECKOUT_FILE));
    places
}

/// The text of the catalogue the running release came with, where the release has one.
fn shipped_text(config: &orch_config::ServerConfig) -> Option<String> {
    shipped_places(config)
        .into_iter()
        .find_map(|place| std::fs::read_to_string(place).ok())
}

/// The date of the figures of the catalogue the release came with, as far as it can be told.
fn shipped_as_of(config: &orch_config::ServerConfig, fallback: NaiveDate) -> String {
    shipped_text(config)
        .and_then(|text| serde_json::from_str::<orch_types::Json>(&text).ok())
        .and_then(|doc| {
            doc.get("as_of")
                .and_then(orch_types::Json::as_str)
                .map(str::to_owned)
        })
        .unwrap_or_else(|| fallback.to_string())
}

/// The catalogue in force as the contract has it, or only the family that matches `q`.
async fn shown(
    ctx: &Ctx,
    conn: &mut Conn,
    row: &CatalogueRow,
    q: Option<&str>,
) -> ApiResult<RateCatalogue> {
    let mut families = families_of(&row.doc);
    if let Some(model) = q {
        let best = matched(families.iter().map(|family| family.pattern.as_str()), model);
        families.retain(|family| Some(&family.pattern) == best.as_ref());
    }
    let who = match row.loaded_by {
        Some(user) => refusals::user_refs(conn, &[user]).await?.remove(&user),
        None => None,
    };
    Ok(RateCatalogue {
        format: row
            .doc
            .get("format")
            .and_then(orch_types::Json::as_u64)
            .and_then(|format| u32::try_from(format).ok())
            .unwrap_or(1),
        as_of: row.as_of.to_string(),
        unit: row
            .doc
            .get("unit")
            .and_then(orch_types::Json::as_str)
            .unwrap_or_default()
            .to_owned(),
        origin: serde_json::from_value(serde_json::Value::String(row.origin.clone()))
            .map_err(ApiError::internal)?,
        source: row.source.clone(),
        loaded_at: Time::new(row.loaded_at),
        loaded_by: who,
        shipped_as_of: shipped_as_of(&ctx.app.config, row.as_of),
        families,
    })
}

/// Reads the estimates as loaded, or the family that matches `q`; none is a successful empty read.
pub async fn catalogue(
    ctx: Ctx,
    query: UsageCatalogueGetQuery,
    (): (),
) -> ApiResult<Option<RateCatalogue>> {
    let _ = ctx.user()?;
    let mut conn = ctx.app.db.acquire().await?;
    let Some(row) = ledger::catalogue_in_force(&mut conn).await? else {
        return Ok(None);
    };
    shown(&ctx, &mut conn, &row, query.q.as_deref())
        .await
        .map(Some)
}

/// Replaces the catalogue in force by a file, by one fetched from the configured address, or by the
/// shipped one, through the ordinary administrative operation gate.
pub async fn set(ctx: Ctx, (): (), body: Body) -> ApiResult<Response> {
    let mut conn = ctx.app.db.acquire().await?;
    crate::routes::admin::admin_of(&ctx)?;
    let limit = usize::try_from(USAGE_CATALOGUE_MAX_BYTES).unwrap_or(usize::MAX);
    let bytes = axum::body::to_bytes(body, limit).await.map_err(|_| {
        ApiError::new(ErrorCode::RequestTooLarge, "a catalogue is at most 2 MiB")
            .with("limit", USAGE_CATALOGUE_MAX_BYTES)
    })?;
    let text =
        String::from_utf8(bytes.to_vec()).map_err(|_| ApiError::invalid("", "is not text"))?;
    let document: orch_types::Json =
        serde_json::from_str(&text).map_err(|error| ApiError::invalid("", &error.to_string()))?;
    let flag = |name: &str| {
        document.as_object().is_some_and(|members| {
            members.len() == 1 && members.get(name) == Some(&orch_types::Json::Bool(true))
        })
    };
    // Fetching may take time; only the later guarded transaction puts it in force.
    let fetched = if flag("fetch") {
        let url = orch_settings::get::<String>(&mut conn, Place::System, keys::USAGE_CATALOGUE_URL)
            .await
            .map_err(ApiError::internal)?
            .filter(|url| !url.trim().is_empty())
            .ok_or_else(|| {
                ApiError::invalid(
                    "fetch",
                    "usage.catalogue_url is not set, so there is nowhere to fetch it from",
                )
            })?;
        let text = fetching::fetch_text(&url)
            .await
            .map_err(|error| match error {
                // The contract has no code for an address that does not answer; it is the admin's setting that is wrong.
                CatalogueError::Fetch(reason) => ApiError::invalid("fetch", &reason),
                CatalogueError::Refused(reason) => ApiError::invalid("", &reason),
                other => ApiError::internal(other),
            })?;
        Some((url, text))
    } else {
        None
    };
    let (mut tx, guard) = crate::routes::admin::action(&ctx).await?;
    let by = Actor::User(guard.user().id());
    let from = origin(&ctx);
    let now = guard.now().get();
    if let Some((url, text)) = &fetched {
        seeds::load_catalogue(&mut tx, text, Taken::Fetched { from: url }, by, &from, now)
            .await
            .map_err(refused)?;
    } else if flag("shipped") {
        let Some(shipped) = shipped_text(&ctx.app.config) else {
            return Err(ApiError::invalid(
                "shipped",
                "this release came with no catalogue file, as a development build does not",
            ));
        };
        seeds::put_back_shipped_catalogue(&mut tx, &shipped, by, &from, now)
            .await
            .map_err(refused)?;
    } else {
        seeds::load_catalogue(&mut tx, &text, Taken::Uploaded, by, &from, now)
            .await
            .map_err(refused)?;
    }
    tx.commit().await.map_err(orch_db::DbError::from)?;
    let row = ledger::catalogue_in_force(&mut conn)
        .await?
        .ok_or_else(ApiError::not_found)?;
    let answer = shown(&ctx, &mut conn, &row, None).await?;
    json_response(&answer, StatusCode::OK)
}

/// What the loader's refusal comes to as an answer: a catalogue that is wrong names its entry,
/// placed at the member that is wrong.
fn refused(error: SeedError) -> ApiError {
    match error {
        SeedError::Catalogue { path, reason } => ApiError::invalid(&path, &reason),
        other => ApiError::internal(other),
    }
}

/// Actual usable model rows, retained before rate and provider-reference hydration.
pub(crate) struct PreparedTable {
    query: UsageRateTableGetQuery,
    usable: Vec<rollup::UsableModel>,
}

pub(super) struct Table;

impl crate::key_operations::native::Selected for Table {
    type Source = orch_api_contract::usage_sources::Source;
    type Query = UsageRateTableGetQuery;
    type Value = PreparedTable;
    type Reply = RateTable;
    const DECLARATION: &'static orch_api_contract::operation_sources::Declaration<Self::Source> =
        &orch_api_contract::usage_sources::RATES_NODE;
    fn declaration(
        _query: &Self::Query,
        value: &Self::Value,
    ) -> &'static orch_api_contract::operation_sources::Declaration<Self::Source> {
        if value.query.system == Some(true) {
            &orch_api_contract::usage_sources::RATES_SYSTEM
        } else {
            Self::DECLARATION
        }
    }
    fn select<'a>(
        ctx: &'a Ctx,
        conn: &'a mut Conn,
        caller: &'a orch_authz::Caller,
        query: &'a Self::Query,
    ) -> futures::future::BoxFuture<
        'a,
        ApiResult<(
            Self::Value,
            Vec<crate::key_operations::native::Facts<Self::Source>>,
        )>,
    > {
        Box::pin(async move {
            use crate::key_operations::{native, targets};
            use orch_api_contract::usage_sources::{self as sources, Source};
            if query.system == Some(true) {
                if query.node_id.is_some() {
                    return Err(ApiError::invalid("system", "choose a node or the system"));
                }
                let facts = vec![native::Facts::one(
                    Source::System,
                    orch_access_contract::key_scope::KeyTargetKind::Identity(
                        orch_access_contract::key_scope::KeyIdentityKind::System,
                    ),
                    targets::system(),
                )];
                let plan = native::bind(ctx, &sources::RATES_SYSTEM, &facts)?;
                let principal = native::preparation_principal(ctx, conn, caller, &plan).await?;
                ctx.ordinary_system_admin_with(ctx.user()?, principal.caller())?;
                return Ok((
                    PreparedTable {
                        query: query.clone(),
                        usable: Vec::new(),
                    },
                    facts,
                ));
            }
            let node = query
                .node_id
                .ok_or_else(|| ApiError::invalid("node_id", "choose a node or system=true"))?;
            let target = targets::node(conn, node).await?;
            let mut facts = vec![native::Facts::one(Source::Node, target.object.kind, target)];
            let plan = native::bind_known(ctx, &sources::RATES_NODE, &facts)?;
            let principal = native::preparation_principal(ctx, conn, caller, &plan).await?;
            authorization::require(
                conn,
                &principal,
                node,
                authorization::exact(Permission::UsageView),
                Act::Use,
                Policy::default(),
            )
            .await?;
            let place = rollup::place_of(conn, node)
                .await?
                .ok_or_else(ApiError::not_found)?;
            let root = orch_db::usage::caps::node_org(conn, node)
                .await?
                .ok_or_else(ApiError::not_found)?;
            let candidates = rollup::models_usable_at(conn, root, Some(place)).await?;
            let mut allowed = HashMap::new();
            let mut providers = Vec::new();
            for each in &candidates {
                if allowed.contains_key(&each.provider_id) {
                    continue;
                }
                let target = targets::resource_identity(
                    conn,
                    orch_access_contract::decisions::ObjectRef {
                        kind: orch_access_contract::permissions::ObjectKind::Provider,
                        id: each.provider_id,
                    },
                )
                .await?;
                let yes = !native::modern(caller)
                    || native::permits(
                        &plan,
                        &sources::RATES_NODE,
                        caller,
                        &Source::RateProviders,
                        target.object.kind,
                        &target,
                    )?;
                allowed.insert(each.provider_id, yes);
                if yes {
                    providers.push(target)
                }
            }
            let usable = candidates
                .into_iter()
                .filter(|each| allowed.get(&each.provider_id) == Some(&true))
                .collect();
            facts.push(super::native::collection(Source::RateProviders, providers));
            Ok((
                PreparedTable {
                    query: query.clone(),
                    usable,
                },
                facts,
            ))
        })
    }
    fn require<'a>(
        _ctx: &'a Ctx,
        _conn: &'a mut Conn,
        _principal: &'a orch_authz::Principal,
    ) -> futures::future::BoxFuture<'a, ApiResult<()>> {
        Box::pin(async { Ok(()) })
    }
    fn render<'a>(
        ctx: &'a Ctx,
        conn: &'a mut Conn,
        principal: &'a orch_authz::Principal,
        value: &'a Self::Value,
    ) -> futures::future::BoxFuture<'a, ApiResult<Self::Reply>> {
        Box::pin(table_for(ctx, conn, principal, value))
    }
}
