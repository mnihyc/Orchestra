//! Capture the current offering through the common capability projection at session preparation.
use crate::capability_routes::{self, AccountProjection, ThreadProjection};
use crate::sessions::StartSnapshot;
use crate::{Engine, EngineError};
use orch_agent_contract::session::NativeModelContext;
use orch_db::Conn;
use orch_db::queries::resources::ProviderModelRow;
use orch_provider_contract::capabilities::{Endpoint, Provider};
use orch_settings::{Place, Snapshot, Wanted};
use orch_settings_contract::registry;
use orch_types::{Id, Json};
use serde_json::json;
use std::collections::BTreeMap;

/// Read declared modes, exact backend choices and recovery rules through the inherited supplier.
async fn settings(
    conn: &mut Conn,
    thread: Id,
    sources: &orch_provider_contract::builtins::BuiltinSources,
) -> Result<BTreeMap<String, Json>, EngineError> {
    let defs = capability_routes::setting_definitions(sources);
    let snapshot = Snapshot::load(conn, Place::Node(thread), Wanted::Keys(&defs)).await?;
    let mut values = BTreeMap::new();
    for def in defs {
        let resolved = snapshot.resolve(def);
        if def.key == registry::keys::CALLS_RULES {
            values.insert(
                "calls.rules.source_scope".into(),
                serde_json::to_value(resolved.source.scope)?,
            );
        }
        if let Some(value) = resolved.value {
            values.insert(def.key.into(), value);
        }
    }
    Ok(values)
}

impl Engine {
    async fn native_backend_principals(
        &self,
        conn: &mut Conn,
        snapshot: &StartSnapshot,
    ) -> Result<Vec<orch_authz::Principal>, EngineError> {
        let mut unit = crate::unit::Unit::new(conn, self.inner.clock.now());
        let input = &snapshot.input;
        let mut callers = Vec::new();
        if input.author_id.is_none() || input.submission_authority.is_none() {
            return Ok(Vec::new());
        }
        for (person, raw) in [
            (input.author_id, input.submission_authority.as_ref()),
            (input.run_as, input.run_as_authority.as_ref()),
        ] {
            let Some(person) = person else { continue };
            let Some(authority) = crate::inputs::saved_authority(raw)? else {
                return Ok(Vec::new());
            };
            let Some(current) = crate::inputs::input_preparation_callers(
                &mut unit,
                &snapshot.thread,
                input,
                person,
                &authority,
                self.inner.clock.as_ref(),
            )
            .await?
            else {
                return Ok(Vec::new());
            };
            callers.extend(current);
        }
        let mut principals = Vec::new();
        for caller in callers {
            principals.push(
                orch_authz::Principal::load(&mut *unit.conn, caller, self.inner.clock.time())
                    .await?,
            );
        }
        Ok(principals)
    }

    /// Metadata only: `allowed` is supplied by the ordinary current input/job Use check.
    /// Endpoint names and capability selection are evaluated solely by the shared resolver.
    pub(crate) async fn capture_native_model(
        &self,
        conn: &mut Conn,
        snapshot: &StartSnapshot,
        allowed: bool,
    ) -> Result<Option<Box<NativeModelContext>>, EngineError> {
        let Some(model) = &snapshot.model else {
            return Ok(None);
        };
        let parameters = snapshot.model_parameters()?;
        let models = [ProviderModelRow {
            model_name: model.model_name.clone(),
            display_name: model.display_name.clone(),
            parameters: serde_json::to_value(&parameters)?,
            option_layers: model.option_layers.clone(),
            groups: model.groups.clone(),
            enabled: model.enabled,
        }];
        // Only endpoint identity, auth presence and the three eligibility facts are read.
        // Secret values, secret headers, runtime configuration and account identity never enter metadata.
        let upstreams =
            orch_db::queries::resources::provider_runtime::upstreams_of(conn, snapshot.provider.id)
                .await?;
        let has_key: bool = sqlx::query_scalar(
            "SELECT EXISTS (SELECT 1 FROM secrets WHERE owner_kind='provider' AND owner_id=$1 AND name='api_key')",
        ).bind(snapshot.provider.id).fetch_one(&mut *conn).await.map_err(orch_db::DbError::from)?;
        let mut account = Provider::default();
        for row in upstreams.as_array().into_iter().flatten() {
            let (Some(protocol), Some(address)) = (
                row.get("protocol").and_then(Json::as_str),
                row.get("base_url").and_then(Json::as_str),
            ) else {
                continue;
            };
            if snapshot.arrangement.protocol.as_deref() != Some(protocol) {
                continue;
            }
            account
                .endpoints
                .entry(protocol.into())
                .or_insert_with(|| Endpoint {
                    interface: protocol.into(),
                    address: address.into(),
                    credential_role: (row.pointer("/options/auth").and_then(Json::as_str)
                        != Some("none"))
                    .then(|| "model".into()),
                    ..Endpoint::default()
                });
        }
        let access = serde_json::to_value(snapshot.path)?;
        let arrangement = serde_json::to_value(snapshot.arrangement.kind)?;
        let sources = self
            .inner
            .capability_services
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()
            .unwrap_or_default();
        let mut values = Box::pin(settings(conn, snapshot.thread.node_id, &sources)).await?;
        // The session and its first turn both use the exact choices already frozen on that turn.
        values.insert("agent.params".into(), snapshot.adapter_params()?);
        let mut projection = capability_routes::project_with_account(
            ThreadProjection {
                descriptor: &snapshot.descriptor,
                access: access.as_str().unwrap_or_default(),
                arrangement: arrangement.as_str(),
                provider_id: snapshot.provider.id,
                provider: None,
                models: &models,
                current: Some(&snapshot.turn.model),
                allowed,
                settings: values.clone(),
            },
            &sources,
            Some(AccountProjection {
                account,
                credential_roles: if has_key {
                    vec!["model".into()]
                } else {
                    Vec::new()
                },
                facts: json!({
                    "plan": snapshot.provider.facts.get("plan"),
                    "credential": snapshot.provider.facts.get("credential"),
                    "credential_verified": snapshot.provider.facts.get("credential_verified"),
                }),
            }),
        )?;
        let has_backend = sources
            .capabilities()
            .declarations()
            .capabilities
            .values()
            .filter_map(|capability| capability.backend_setting.as_deref())
            .any(|key| {
                capability_routes::setting(&values, key).is_some_and(|value| !value.is_null())
            });
        let principals = if allowed && has_backend {
            Box::pin(self.native_backend_principals(conn, snapshot)).await?
        } else {
            Vec::new()
        };
        let attribution = if has_backend {
            orch_db::usage::ledger::attribution_of_turn(conn, snapshot.turn.id).await?
        } else {
            None
        };
        self.join_capability_backends(
            conn,
            &mut projection,
            &values,
            capability_routes::BackendRead {
                place: snapshot.thread.node_id,
                principals: if attribution.is_some() {
                    &principals
                } else {
                    &[]
                },
                person: attribution.and_then(|attribution| attribution.user_id),
                harness_kind: Some(&snapshot.descriptor.kind),
                provider: None,
            },
        )
        .await?;
        NativeModelContext::captured(
            snapshot.turn.model.clone(),
            parameters,
            &projection.resolve(),
        )
        .map(|context| Some(Box::new(context)))
        .map_err(EngineError::unreadable)
    }
}
