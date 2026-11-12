use super::*;
use crate::sources::bind_required;
use orch_access_contract::{
    key_scope::{KeyIdentityKind, KeyObjectRef, KeyResolvedTarget, KeyScope, KeyTargetKind},
    permissions::ObjectKind,
};
use orch_api_contract::{imports::NodeImportCreate, routes::Endpoint};
use orch_authz::{CapturedOperation, TokenGrant};
use orch_types::Id;

type TestResult = Result<(), Box<dyn std::error::Error + Send + Sync>>;
use orch_api_contract::import_creation_sources::{CANDIDATES, Source};
use orch_api_contract::upload_sources;
const ROWS: &[Declaration<Source>] = &CANDIDATES;
const ACCOUNT: KeyTargetKind = KeyTargetKind::Identity(KeyIdentityKind::Account);
const UPLOAD: KeyTargetKind = KeyTargetKind::Identity(KeyIdentityKind::Upload);
const ORG: KeyTargetKind = KeyTargetKind::Object(ObjectKind::Org);
fn target(kind: KeyTargetKind, id: u128) -> KeyResolvedTarget {
    KeyResolvedTarget {
        object: KeyObjectRef {
            kind,
            id: Some(Id::from_u128(id)),
        },
        node_ancestors: vec![],
        document_ancestors: vec![],
        credential_account: None,
    }
}
fn facts() -> Vec<Facts<Source>> {
    ROWS[0]
        .bindings
        .iter()
        .take(upload_sources::BLOB_GET.bindings.len())
        .map(|binding| {
            Facts::collection(
                binding.source,
                if binding.source == Source::Bytes(upload_sources::Source::ReadUploads) {
                    vec![(UPLOAD, vec![target(ACCOUNT, 1)])]
                } else {
                    Vec::new()
                },
            )
        })
        .collect()
}

fn key(
    variants: &[&str],
    manage: bool,
) -> Result<Caller, Box<dyn std::error::Error + Send + Sync>> {
    let allow = variants
        .iter()
        .map(|variant| serde_json::json!({ "operation": variant, "actions": ["read"] }))
        .collect::<Vec<_>>();
    let scope: KeyScope = serde_json::from_value(serde_json::json!({ "rules": [
        { "target": { "kind": "account", "id": Id::from_u128(1), "reach": "self" }, "resources": ["upload"], "allow": allow },
        { "target": { "kind": "org", "id": Id::from_u128(2), "reach": "self" }, "resources": ["org"], "allow": [
            { "operation": "NodeImportCreate.content", "actions": ["write"] },
            { "operation": "NodeImportCreate.restore_access", "actions": if manage { vec!["write", "manage"] } else { vec!["write"] } }
        ] }
    ] }))?;
    Ok(Caller::token(
        Id::from_u128(1),
        TokenGrant::from_stored_scope(&[], None, Some(scope), None)?,
    ))
}

#[test]
fn import_common_read_keeps_both_candidates_partial_and_never_borrows_preview() -> TestResult {
    let common = Common::bind(NodeImportCreate::ROUTE, ROWS, &facts())?;
    let both = key(&[ROWS[0].operation, ROWS[1].operation], true)?;
    let prepared = common.preparation_caller(&both)?;
    assert!(prepared.admitted_operation().is_none());
    assert!(prepared.admitted_required_roles().is_none());
    assert!(CapturedOperation::capture(&prepared).is_err());
    assert!(prepared.narrow_source_admission(&prepared).is_err());
    for row in ROWS {
        let single = key(&[row.operation], true)?;
        let read = common.preparation_caller(&single)?;
        assert!(read.admitted_operation().is_none());
        assert!(read.admitted_required_roles().is_none());
        assert!(CapturedOperation::capture(&read).is_err());
    }
    assert!(
        common
            .preparation_caller(&key(&["NodeImportPreview"], true)?)
            .is_err()
    );
    Ok(())
}

#[test]
fn resolved_import_needs_its_own_full_roles_and_the_original_upload() -> TestResult {
    let common = Common::bind(NodeImportCreate::ROUTE, ROWS, &facts())?;
    let mut full = facts();
    full.push(Facts::collection(
        Source::Destinations,
        vec![(ORG, vec![target(ORG, 2)])],
    ));
    full.push(Facts::collection(
        Source::Delegation,
        vec![(ORG, vec![target(ORG, 2)])],
    ));
    let resolved = bind_required(NodeImportCreate::ROUTE, &ROWS[1], &full)?;
    assert!(common.covers(&resolved));
    assert!(
        resolved
            .admit(&key(&[ROWS[0].operation], true)?, false)
            .is_err()
    );
    assert!(
        resolved
            .admit(&key(&[ROWS[1].operation], false)?, false)
            .is_err()
    );
    let admitted = resolved.admit(&key(&[ROWS[1].operation], true)?, false)?;
    assert_eq!(
        CapturedOperation::capture(&admitted)?.operation(),
        ROWS[1].operation
    );
    full[4] = Facts::collection(
        Source::Bytes(upload_sources::Source::ReadUploads),
        vec![(UPLOAD, vec![target(ACCOUNT, 3)])],
    );
    assert!(!common.covers(&bind_required(NodeImportCreate::ROUTE, &ROWS[1], &full)?));
    Ok(())
}

#[test]
fn common_initial_refuses_missing_variants_writes_and_empty_sources() -> TestResult {
    assert!(Common::bind(NodeImportCreate::ROUTE, &ROWS[..1], &facts()).is_err());
    assert!(Common::bind(NodeImportCreate::ROUTE, &[ROWS[0], ROWS[0]], &facts()).is_err());
    assert!(Common::bind(NodeImportCreate::ROUTE, ROWS, &[]).is_err());
    let mut supplied = facts();
    supplied.push(Facts::collection(
        Source::Destinations,
        vec![(ORG, vec![target(ORG, 2)])],
    ));
    assert!(Common::bind(NodeImportCreate::ROUTE, ROWS, &supplied).is_err());
    assert!(
        Common::bind(
            NodeImportCreate::ROUTE,
            ROWS,
            &[Facts::collection(
                Source::Bytes(upload_sources::Source::ReadUploads),
                vec![]
            )]
        )
        .is_err()
    );
    Ok(())
}

#[test]
fn single_canonical_preparation_can_keep_known_setup_without_capturing_unresolved_roles()
-> TestResult {
    use orch_api_contract::{
        document_sources::{self, TextCreationSource as S},
        documents::NodeDocumentImport,
    };
    let rows = [document_sources::TEXT_IMPORT];
    let known = vec![Facts::one(
        S::Destination,
        KeyTargetKind::Object(ObjectKind::Document),
        target(ORG, 2),
    )];
    // This phase really needs destination Create, not an invented empty copied-label Read.
    assert!(Common::bind(NodeDocumentImport::ROUTE, &rows, &known).is_err());
    let preparation = Common::bind_preparation(NodeDocumentImport::ROUTE, &rows, &known)?;
    let actor = Caller::session(Id::from_u128(1));
    let caller = preparation.preparation_caller(&actor)?;
    assert!(caller.admitted_operation().is_none());
    assert!(caller.admitted_required_roles().is_none());
    assert!(CapturedOperation::capture(&caller).is_err());
    assert!(caller.narrow_source_admission(&caller).is_err());
    assert!(preparation.covers_common(&preparation));
    let different = Common::bind_preparation(
        NodeDocumentImport::ROUTE,
        &rows,
        &[Facts::one(
            S::Destination,
            KeyTargetKind::Object(ObjectKind::Document),
            target(ORG, 3),
        )],
    )?;
    assert!(!different.covers_common(&preparation));
    assert!(bind_required(NodeDocumentImport::ROUTE, &rows[0], &known).is_err());
    Ok(())
}
