//! A driver's import proposes a validated recipe and explains what it read, without saving a template.

use super::{clean_driver, clean_spec, creation_authority, unit_word, version_key};
use crate::middleware::requests::{ApiError, ApiResult};
use crate::registry::Ctx;
use crate::routes::nodes::invalid;
use crate::routes::resources::begin_mutation;
use orch_api_contract::common::{ErrorCode, FieldFault, Problem};
use orch_api_contract::resources::{self as api, UnitKind};
use orch_authz::usable::{self, Policies, Usable};
use orch_db::queries::resources::template_imports;
use orch_driver_contract::imports::ImportDef;
use orch_link_contract::commands::{
    NodeTemplateImport, NodeTemplateImportResult, NoteLevel, RuntimeKind, SerialKey,
};
use orch_types::{FieldType, Hash, Json};

/// Proposes a recipe on an online host the caller may use in the requested home.
pub(super) async fn import(
    ctx: Ctx,
    (): (),
    body: api::TemplateImportBody,
) -> ApiResult<api::TemplateImportReply> {
    let driver = clean_driver("driver", &body.driver)?;
    let kind = body.kind.unwrap_or(UnitKind::Environment);
    let (ctx, mut tx, principal) = begin_mutation(&ctx).await?;
    creation_authority(&mut tx, &principal, body.home_id, &driver).await?;
    let mut candidates = template_imports::candidates(&mut tx).await?;
    candidates.sort_by_cached_key(|candidate| {
        std::cmp::Reverse(version_key(
            candidate
                .drivers
                .get(&driver)
                .and_then(|descriptor| descriptor.get("version"))
                .and_then(Json::as_str)
                .unwrap_or_default(),
        ))
    });
    let mut chosen = None;
    for candidate in candidates {
        let Some(descriptor) = candidate.drivers.get(&driver) else {
            continue;
        };
        if descriptor
            .get("spec_schemas")
            .and_then(|schemas| schemas.get(unit_word(kind)))
            .is_none()
        {
            continue;
        }
        let import = descriptor
            .get("imports")
            .and_then(Json::as_array)
            .into_iter()
            .flatten()
            .find(|import| {
                import.get("format").and_then(Json::as_str) == Some(body.format.as_str())
            });
        let Some(import) = import else {
            continue;
        };
        if !matches!(
            usable::usable(
                &mut tx,
                &principal,
                candidate.host,
                body.home_id,
                &Policies::default()
            )
            .await
            .map_err(ApiError::internal)?,
            Usable::Yes(_)
        ) {
            continue;
        }
        let import: ImportDef = serde_json::from_value(import.clone())
            .map_err(|_| invalid("format", "the driver's import declaration is not usable"))?;
        let schema = descriptor["spec_schemas"][unit_word(kind)].clone();
        chosen = Some((candidate.machine, import, schema));
        break;
    }
    let (machine, import, schema) = chosen.ok_or_else(|| {
        ApiError::new(
            ErrorCode::SourceOffline,
            "no usable online host offers this driver's import for this pool",
        )
    })?;
    let mut values = serde_json::Map::new();
    for (name, value) in &body.values {
        let field = import.fields.iter().find(|field| field.name == *name);
        let value = match field.map(|field| field.kind) {
            Some(FieldType::Boolean) => value
                .parse::<bool>()
                .map(Json::Bool)
                .map_err(|_| invalid(&format!("values.{name}"), "must be true or false"))?,
            Some(FieldType::Map | FieldType::SecretMap) => {
                serde_json::from_str(value).map_err(|_| {
                    invalid(&format!("values.{name}"), "must be names with text values")
                })?
            }
            _ => Json::String(value.clone()),
        };
        values.insert(name.clone(), value);
    }
    for field in &import.fields {
        if !values.contains_key(&field.name)
            && let Some(default) = &field.default
        {
            values.insert(field.name.clone(), default.clone());
        }
    }
    let values = Json::Object(values);
    import.check_values(&values).map_err(|faults| {
        ApiError::from(Problem::invalid(
            faults
                .into_iter()
                .map(|fault| FieldFault {
                    path: format!("values.{}", fault.path),
                    message: fault.message,
                })
                .collect(),
        ))
    })?;
    // Blob and context_blob are the source contract's archive references; names alone confer no read authority.
    for (name, value) in &body.values {
        if name != "blob" && !name.ends_with("_blob") {
            continue;
        }
        let hash: Hash = value.parse().map_err(|_| {
            invalid(
                &format!("values.{name}"),
                "an archive names a completed upload",
            )
        })?;
        if !orch_db::queries::uploads::owns(&mut tx, principal.user(), &hash).await?
            || !ctx
                .app
                .blobs
                .store()
                .exists(&hash)
                .await
                .map_err(|_| ApiError::internal("the uploaded archive could not be checked"))?
        {
            return Err(invalid(
                &format!("values.{name}"),
                "this caller has no completed upload of that archive",
            ));
        }
    }
    let proposal: NodeTemplateImportResult = ctx
        .app
        .link
        .dispatcher()
        .run(
            machine,
            &SerialKey::Machine,
            &NodeTemplateImport {
                driver,
                kind: match kind {
                    UnitKind::Environment => RuntimeKind::Environment,
                    UnitKind::Provider => RuntimeKind::Provider,
                },
                format: body.format,
                values,
            },
        )
        .await
        .map_err(|_| {
            ApiError::new(
                ErrorCode::SourceOffline,
                "the host could not propose this template",
            )
        })?;
    clean_spec("spec", &proposal.spec)?;
    super::super::validation::value("spec", &schema, &proposal.spec)?;
    Ok(api::TemplateImportReply {
        spec: proposal.spec,
        notes: proposal
            .notes
            .into_iter()
            .map(|note| api::TemplateImportNote {
                level: match note.level {
                    NoteLevel::Info => api::TemplateImportNoteLevel::Info,
                    NoteLevel::Warning => api::TemplateImportNoteLevel::Warning,
                    NoteLevel::Blocker => api::TemplateImportNoteLevel::Blocker,
                },
                text: note.text,
            })
            .collect(),
    })
}
