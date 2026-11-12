//! Retained artifact custody joins the existing Storage selection, name bounds and physical accounting.

use super::*;

use crate::key_operations::targets;

use orch_api_contract::storage::StorageArtifact;

use orch_api_contract::storage::StorageArtifactKind;

use orch_api_contract::storage::StoragePart;

use orch_authz::engine::Graph;
struct Candidate {
    record: KeptRecord,
    owner: Option<Id>,
    target: orch_access_contract::key_scope::KeyResolvedTarget,
    witness: Json,
}

fn parts(
    hashes: impl IntoIterator<Item = String>,
    blobs: &HashMap<String, i64>,
) -> Vec<StoragePart> {
    hashes
        .into_iter()
        .collect::<BTreeSet<_>>()
        .into_iter()
        .map(|hash| StoragePart {
            bytes: blobs.get(&hash).copied(),
            key: format!("blob:{hash}"),
        })
        .collect()
}

fn bytes(parts: &[StoragePart]) -> Option<i64> {
    parts.iter().try_fold(0_i64, |sum, part| {
        part.bytes
            .filter(|bytes| *bytes >= 0)
            .map(|bytes| sum.saturating_add(bytes))
    })
}

fn artifact(
    id: Id,
    kind: StorageArtifactKind,
    name: String,
    state: String,
    at: Time,
) -> KeptRecord {
    let (family, storage_kind) = match kind {
        StorageArtifactKind::Snapshot => ("snapshot", StorageKind::Snapshot),
        StorageArtifactKind::Import => ("import", StorageKind::Import),
        StorageArtifactKind::Export => ("export", StorageKind::Export),
    };
    let mut row = model::record(format!("{family}:{id}"), storage_kind, name, at);
    row.artifact = Some(StorageArtifact { id, kind, state });
    row.why = KeptWhy::Restore;
    row.availability = StorageAvailability::Ready;
    row
}

pub(super) async fn prepare(
    conn: &mut Conn,
    at: Time,
    blobs: &HashMap<String, i64>,
    mut book: Option<&mut report_sources::Book>,
) -> ApiResult<Vec<KeptRecord>> {
    let exports = orch_db::queries::exports::kept(conn).await?;
    let imports = orch_db::queries::imports::kept(conn).await?;
    let snapshots = orch_db::queries::storage::environment_exports(conn).await?;
    let nodes = exports
        .iter()
        .filter_map(|row| row.node_id)
        .chain(imports.iter().filter_map(|row| row.row.node_id))
        .collect::<BTreeSet<_>>();
    let graph = Graph::load(conn, &nodes.into_iter().collect::<Vec<_>>())
        .await
        .map_err(ApiError::internal)?;
    let environments = snapshots
        .iter()
        .map(|row| row.environment_id)
        .collect::<BTreeSet<_>>();
    let mut resource_graph = Graph::default();
    let environments = orch_authz::resources::read_into(
        conn,
        &mut resource_graph,
        &environments.into_iter().collect::<Vec<_>>(),
    )
    .await
    .map_err(ApiError::internal)?;
    // Reuse the actual retained resource rows and their shared graph in one batch.
    let environment_targets: HashMap<_, _> = environments
        .iter()
        .filter(|row| row.kind == orch_access_contract::permissions::ResourceKind::Environment)
        .filter_map(|row| {
            orch_engine::operation_scope::targets::from_resource_identity(&resource_graph, row)
                .map(|target| (row.id, target))
        })
        .collect();
    let ids = snapshots.iter().map(|row| row.id).collect::<Vec<_>>();
    let references: Vec<(Id, String)> = sqlx::query_as(
        "SELECT snapshot_id,encode(blob,'hex') FROM snapshot_blob_refs WHERE snapshot_id=ANY($1)",
    )
    .bind(&ids)
    .fetch_all(&mut *conn)
    .await
    .map_err(DbError::from)?;
    let mut snapshot_parts = HashMap::<Id, Vec<String>>::new();
    for (snapshot, hash) in references {
        snapshot_parts.entry(snapshot).or_default().push(hash);
    }
    let mut candidates = Vec::new();
    for stored in exports {
        let target = targets::from_export_graph(&graph, &stored)?;
        let mut row = artifact(
            stored.id,
            StorageArtifactKind::Export,
            String::new(),
            stored.state.clone(),
            at,
        );
        row.physical_parts = parts(stored.blob.iter().cloned(), blobs);
        row.bytes = if row.physical_parts.is_empty() {
            stored.bytes
        } else {
            bytes(&row.physical_parts)
        };
        if let Some(blob) = stored.blob.as_ref() {
            row.accounting_key = format!("blob:{blob}");
        }
        row.since = Some(Time::new(stored.created_at));
        row.until = stored.expires_at.map(Time::new);
        row.can_clean = stored.state != "pending";
        row.blocker = (stored.state == "pending").then(|| "export_running".into());
        let witness = serde_json::json!({"id":stored.id,"owner":stored.user_id,"node":stored.node_id,"blob":stored.blob,"bytes":stored.bytes,"state":stored.state,"created":stored.created_at,"expires":stored.expires_at,"parts":&row.physical_parts});
        candidates.push(Candidate {
            record: row,
            owner: stored.user_id,
            target,
            witness,
        });
    }
    for stored in imports {
        let target = targets::from_import_line_graph(&graph, &stored.row)?;
        let mut row = artifact(
            stored.row.id,
            StorageArtifactKind::Import,
            stored.row.first_file.clone().unwrap_or_default(),
            stored.row.state.clone(),
            at,
        );
        row.physical_parts = parts(stored.source_blobs.iter().cloned(), blobs);
        row.bytes = bytes(&row.physical_parts);
        row.since = Some(Time::new(stored.row.created_at));
        if matches!(stored.row.state.as_str(), "pending" | "undoing") {
            row.why = KeptWhy::InUse;
        }
        let witness = serde_json::json!({"id":stored.row.id,"owner":stored.user_id,"node":stored.row.node_id,"sources":stored.source_blobs,"state":stored.row.state,"created":stored.row.created_at,"name":stored.row.first_file,"parts":&row.physical_parts});
        candidates.push(Candidate {
            record: row,
            owner: stored.user_id,
            target,
            witness,
        });
    }
    for stored in snapshots {
        let Some(target) = environment_targets.get(&stored.environment_id).cloned() else {
            return Err(ApiError::not_found());
        };
        let hashes = snapshot_parts.remove(&stored.id).unwrap_or_default();
        let manifest = stored.manifest_blob.as_ref().map(ToString::to_string);
        let missing_files =
            stored.total_bytes > 0 && !hashes.iter().any(|hash| Some(hash) != manifest.as_ref());
        let mut row = artifact(
            stored.id,
            StorageArtifactKind::Snapshot,
            stored.label.clone(),
            stored.state.clone(),
            at,
        );
        row.physical_parts = parts(
            hashes
                .into_iter()
                .chain(stored.manifest_blob.as_ref().map(|hash| hash.to_string())),
            blobs,
        );
        if missing_files {
            row.physical_parts.push(StoragePart {
                key: format!("snapshot:{}:unmeasured", stored.id),
                bytes: None,
            });
        }
        row.bytes = if row.physical_parts.is_empty() && stored.state == "running" {
            None
        } else {
            bytes(&row.physical_parts)
        };
        row.since = Some(Time::new(stored.created_at));
        row.until = stored.expires_at.map(Time::new);
        if stored.state == "running" {
            row.why = KeptWhy::InUse;
        }
        let witness = serde_json::json!({"id":stored.id,"owner":stored.created_by,"environment":stored.environment_id,"generation":stored.env_generation,"label":stored.label,"state":stored.state,"created":stored.created_at,"manifest":stored.manifest_blob,"parts":&row.physical_parts});
        candidates.push(Candidate {
            record: row,
            owner: stored.created_by,
            target,
            witness,
        });
    }
    let mut selected = Vec::new();
    let mut names = HashSet::new();
    for Candidate {
        record,
        owner,
        target,
        witness,
    } in candidates
    {
        if let Some(book) = book.as_deref_mut() {
            if !book.keep(record.id.clone(), target, witness)? {
                continue;
            }
            if let Some(owner) = owner
                && book.account_name(conn, owner).await?
            {
                names.insert(owner);
            }
        } else {
            names.extend(owner);
        }
        selected.push((record, owner));
    }
    let owners = refusals::user_refs(conn, &names.into_iter().collect::<Vec<_>>()).await?;
    Ok(selected
        .into_iter()
        .map(|(mut row, owner)| {
            row.owner = owner.and_then(|owner| owners.get(&owner).cloned());
            row
        })
        .collect())
}
