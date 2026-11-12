//! Native retained artifact rows supply identity and current placement, never authority hints.
#![allow(clippy::unwrap_used, reason = "test assertions fail by panicking")]

use super::*;
use crate::engine::NodeFacts;
use orch_access_contract::key_scope::{KeyReach, KeyTarget};
use orch_access_contract::permissions::Placement;
use orch_access_contract::roles::NodeKind;
use serde_json::json;

fn graph(id: Id) -> Graph {
    Graph::of(vec![NodeFacts {
        id,
        kind: NodeKind::Org,
        parent: None,
        root: id,
        path: vec![id],
        name: "Kept native place".into(),
        first_owner: Id::from_u128(9),
        archived: false,
        deleted: true,
        placement: Placement::default(),
    }])
}

fn archive(node: Option<Id>) -> exports::ExportRow {
    exports::ExportRow {
        id: Id::from_u128(2),
        user_id: Some(Id::from_u128(9)),
        node_id: node,
        request: json!({"node_id":Id::from_u128(100),"path":[Id::from_u128(100)]}),
        job_id: None,
        state: "ready".into(),
        blob: None,
        bytes: Some(0),
        counts: json!({}),
        left_out: json!([]),
        created_at: chrono::DateTime::UNIX_EPOCH,
        ready_at: None,
        expires_at: None,
        downloaded_at: None,
    }
}

fn kept_import(node: Option<Id>) -> imports::ImportRow {
    imports::ImportRow {
        id: Id::from_u128(2),
        user_id: Some(Id::from_u128(9)),
        node_id: node,
        client_id: "native-import-fixture".into(),
        request: json!({"node_id":Id::from_u128(100),"path":[Id::from_u128(100)]}),
        job_id: None,
        undo_job_id: None,
        state: "done".into(),
        files: Vec::new(),
        source_blobs: Vec::new(),
        package: None,
        counts: None,
        actions: None,
        losses: json!([]),
        failure: None,
        created_at: chrono::DateTime::UNIX_EPOCH,
        finished_at: None,
        undone_at: None,
    }
}

fn import_line(node: Option<Id>) -> imports::ImportLineRow {
    imports::ImportLineRow {
        id: Id::from_u128(2),
        node_id: node,
        state: "done".into(),
        job_id: None,
        undo_job_id: None,
        first_file: Some("native-first-file.txt".into()),
        file_count: 1,
        actions: None,
        created_at: chrono::DateTime::UNIX_EPOCH,
        finished_at: None,
        undone_at: None,
    }
}

#[test]
fn retained_artifacts_use_their_real_kind_and_current_stored_place_not_request_ancestry() {
    let org = Id::from_u128(1);
    let graph = graph(org);
    let export = from_export_graph(&graph, &archive(Some(org))).unwrap();
    let import = from_import_graph(&graph, &kept_import(Some(org))).unwrap();
    let line = from_import_line_graph(&graph, &import_line(Some(org))).unwrap();
    assert_eq!(import, line);
    assert_eq!(export.object.id, import.object.id);
    assert_eq!(
        export.object.kind,
        KeyTargetKind::Identity(KeyIdentityKind::Export)
    );
    assert_eq!(
        import.object.kind,
        KeyTargetKind::Identity(KeyIdentityKind::Import)
    );
    for target in [&export, &import, &line] {
        assert_eq!(
            target.node_ancestors,
            vec![reference(KeyTargetKind::Object(ObjectKind::Org), org)]
        );
        assert!(target.document_ancestors.is_empty());
        assert!(target.credential_account.is_none());
        assert!(
            KeyTarget {
                kind: KeyTargetKind::Object(ObjectKind::Org),
                id: Some(org),
                reach: KeyReach::Subtree
            }
            .matches(target)
        );
        assert!(
            !KeyTarget {
                kind: KeyTargetKind::Identity(KeyIdentityKind::Account),
                id: Some(Id::from_u128(9)),
                reach: KeyReach::Subtree
            }
            .matches(target)
        );
    }
    assert!(
        !KeyTarget {
            kind: export.object.kind,
            id: export.object.id,
            reach: KeyReach::SelfOnly
        }
        .matches(&import)
    );
    assert!(from_export_graph(&Graph::default(), &archive(Some(org))).is_none());
    assert!(from_import_graph(&Graph::default(), &kept_import(Some(org))).is_none());
    assert!(from_import_line_graph(&Graph::default(), &import_line(Some(org))).is_none());
}

#[test]
fn purged_artifact_places_leave_exact_artifact_identity_without_invented_placement() {
    for target in [
        from_export_graph(&Graph::default(), &archive(None)).unwrap(),
        from_import_graph(&Graph::default(), &kept_import(None)).unwrap(),
        from_import_line_graph(&Graph::default(), &import_line(None)).unwrap(),
    ] {
        assert_eq!(target.object.id, Some(Id::from_u128(2)));
        assert!(target.node_ancestors.is_empty());
        assert!(target.document_ancestors.is_empty());
        assert!(target.credential_account.is_none());
        assert!(
            KeyTarget {
                kind: target.object.kind,
                id: target.object.id,
                reach: KeyReach::SelfOnly
            }
            .matches(&target)
        );
        assert!(
            !KeyTarget {
                kind: KeyTargetKind::Object(ObjectKind::Org),
                id: Some(Id::from_u128(100)),
                reach: KeyReach::Subtree
            }
            .matches(&target)
        );
    }
}
