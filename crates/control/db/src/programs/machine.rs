//! Current machine catalogue operands without unrelated or unbounded machine facts.

use crate::{Conn, DbError};
use orch_types::{Id, Json};

/// The exact registry and enforcing local policy fields needed by the existing native formatter.
pub async fn registry_facts(conn: &mut Conn, machine: Id) -> Result<Option<Json>, DbError> {
    Ok(sqlx::query_scalar(
        "SELECT jsonb_build_object('policy',facts->'policy','programs',facts->'programs')
        FROM machines WHERE id=$1 AND deleted_at IS NULL",
    )
    .bind(machine)
    .fetch_optional(conn)
    .await?)
}

/// Real plugin records in the same order their machine reported; malformed declarations stay
/// with the existing native parser, and built-in/first-valid duplicate preference is unchanged.
pub async fn executor_plugins(conn: &mut Conn, machine: Id) -> Result<Vec<Json>, DbError> {
    Ok(sqlx::query_scalar(
        "SELECT payload FROM machine_executor_reports WHERE machine_id=$1 ORDER BY ordinal",
    )
    .bind(machine)
    .fetch_all(conn)
    .await?)
}

/// Only stored liveness, independently of ordinary visibility or credential admission.
pub async fn exists(conn: &mut Conn, machine: Id) -> Result<bool, DbError> {
    Ok(sqlx::query_scalar(
        "SELECT EXISTS(SELECT 1 FROM machines WHERE id=$1 AND deleted_at IS NULL)",
    )
    .bind(machine)
    .fetch_one(conn)
    .await?)
}

#[cfg(test)]
mod tests {
    #![allow(
        clippy::unwrap_used,
        reason = "failed native projection controls panic"
    )]
    use super::*;
    use crate::queries::resources::{binding, testing};
    use serde_json::json;

    async fn counts(conn: &mut Conn, kind: &str) -> (i64, i64, i64) {
        sqlx::query_as(
            "SELECT profile_count,machine_count,build_count FROM agent_program_kinds WHERE kind=$1",
        )
        .bind(kind)
        .fetch_one(conn)
        .await
        .unwrap()
    }

    #[tokio::test]
    async fn reported_installations_and_executor_order_follow_facts_liveness_and_restore_without_rewriting_facts()
     {
        let test = orch_testkit::database::TestDb::fresh().await;
        let mut conn = test.pool.acquire().await.unwrap();
        let machine = testing::insert_machine(&mut conn, "projected machine", "connected")
            .await
            .unwrap();
        let facts = json!({"adapters":[{"kind":"projection-test","builtin":false},{"kind":""},{"kind":42},{"kind":null}],
            "harnesses":[{"kind":"projection-test","version":"1.0.0"},{"kind":"projection-test","version":"1.0.0"},{"kind":"projection-test","version":"2.0.0"}],
            "plugins":[{"kind":"other","executor":{"kind":"ignored"}},
                {"kind":"executor","executor":{"kind":"malformed"},"state":2},
                {"kind":"executor","executor":{"kind":"external","title":"External","protocol":"mcp","sandbox_modes":["open"],"tool_sets":[]},"state":"running"}]});
        sqlx::query("UPDATE machines SET facts=$2 WHERE id=$1")
            .bind(machine)
            .bind(&facts)
            .execute(&mut *conn)
            .await
            .unwrap();
        assert_eq!(
            binding::machine_harness_versions(&mut conn).await.unwrap(),
            vec![
                ("projection-test".into(), "1.0.0".into()),
                ("projection-test".into(), "2.0.0".into())
            ]
        );
        assert_eq!(counts(&mut conn, "projection-test").await.1, 1);
        assert_eq!(
            binding::machine_adapter_kinds(&mut conn).await.unwrap(),
            vec!["".to_owned(), "42".to_owned(), "projection-test".to_owned()],
            "the narrow projection preserves the native kind extraction even for malformed declarations"
        );
        let plugins = executor_plugins(&mut conn, machine).await.unwrap();
        assert_eq!(plugins.len(), 2);
        assert_eq!(plugins[0]["executor"]["kind"], "malformed");
        assert_eq!(plugins[1]["state"], "running");
        let stamps: Vec<String> = sqlx::query_scalar(
            "SELECT xmin::text FROM machine_executor_reports WHERE machine_id=$1 ORDER BY ordinal",
        )
        .bind(machine)
        .fetch_all(&mut *conn)
        .await
        .unwrap();
        let mut changed = facts.clone();
        changed["unrelated"] = json!("large unrelated metadata".repeat(4096));
        sqlx::query("UPDATE machines SET facts=$2 WHERE id=$1")
            .bind(machine)
            .bind(&changed)
            .execute(&mut *conn)
            .await
            .unwrap();
        assert_eq!(sqlx::query_scalar::<_,String>("SELECT xmin::text FROM machine_executor_reports WHERE machine_id=$1 ORDER BY ordinal")
            .bind(machine).fetch_all(&mut *conn).await.unwrap(),stamps,"unchanged executor data does not churn on unrelated reports");
        assert_eq!(counts(&mut conn, "projection-test").await.1, 1);
        sqlx::query("UPDATE machines SET status='revoked' WHERE id=$1")
            .bind(machine)
            .execute(&mut *conn)
            .await
            .unwrap();
        assert!(
            binding::machine_harness_versions(&mut conn)
                .await
                .unwrap()
                .is_empty()
        );
        assert_eq!(counts(&mut conn, "projection-test").await.1, 0);
        assert_eq!(
            executor_plugins(&mut conn, machine).await.unwrap(),
            plugins,
            "revoked live machine metadata keeps its native declared executors"
        );
        sqlx::query("UPDATE machines SET deleted_at=now() WHERE id=$1")
            .bind(machine)
            .execute(&mut *conn)
            .await
            .unwrap();
        assert!(
            executor_plugins(&mut conn, machine)
                .await
                .unwrap()
                .is_empty()
        );
        sqlx::query("UPDATE machines SET deleted_at=NULL,status='active' WHERE id=$1")
            .bind(machine)
            .execute(&mut *conn)
            .await
            .unwrap();
        assert_eq!(
            binding::machine_harness_versions(&mut conn)
                .await
                .unwrap()
                .len(),
            2
        );
        assert_eq!(counts(&mut conn, "projection-test").await.1, 1);
        assert_eq!(executor_plugins(&mut conn, machine).await.unwrap(), plugins);
        let actual: Json = sqlx::query_scalar("SELECT facts FROM machines WHERE id=$1")
            .bind(machine)
            .fetch_one(&mut *conn)
            .await
            .unwrap();
        assert_eq!(
            actual, changed,
            "projection never rewrites the machine's authoritative facts"
        );
        sqlx::query("DELETE FROM machines WHERE id=$1")
            .bind(machine)
            .execute(&mut *conn)
            .await
            .unwrap();
        assert!(
            executor_plugins(&mut conn, machine)
                .await
                .unwrap()
                .is_empty()
        );
        assert_eq!(counts(&mut conn, "projection-test").await.1, 0);
    }

    #[tokio::test]
    async fn program_totals_track_profile_and_build_membership_changes_and_feed_the_existing_catalogue()
     {
        let test = orch_testkit::database::TestDb::fresh().await;
        let mut conn = test.pool.acquire().await.unwrap();
        let agent = orch_types::new_id();
        sqlx::query("INSERT INTO agents(id,name,harness_kind) VALUES($1,'Projected profile','counter-first')")
            .bind(agent).execute(&mut *conn).await.unwrap();
        assert_eq!(counts(&mut conn, "counter-first").await, (1, 0, 0));
        sqlx::query("UPDATE agents SET harness_kind='counter-second' WHERE id=$1")
            .bind(agent)
            .execute(&mut *conn)
            .await
            .unwrap();
        assert_eq!(counts(&mut conn, "counter-first").await, (0, 0, 0));
        assert_eq!(counts(&mut conn, "counter-second").await, (1, 0, 0));
        sqlx::query("UPDATE agents SET deleted_at=now() WHERE id=$1")
            .bind(agent)
            .execute(&mut *conn)
            .await
            .unwrap();
        assert_eq!(counts(&mut conn, "counter-second").await, (0, 0, 0));
        sqlx::query("UPDATE agents SET deleted_at=NULL WHERE id=$1")
            .bind(agent)
            .execute(&mut *conn)
            .await
            .unwrap();
        sqlx::query("INSERT INTO agent_program_builds(setting_key,kind,version,os,arch,priority,version_sort,payload,updated_at)
            VALUES('counter-build','counter-second','1.0.0','linux','x86_64',2,'1.0.0','{}',now())")
            .execute(&mut *conn).await.unwrap();
        assert_eq!(counts(&mut conn, "counter-second").await, (1, 0, 1));
        let cutoff = crate::programs::admin::snapshot(&mut conn).await.unwrap();
        let rows = crate::programs::admin::page(
            &mut conn,
            &json!([]),
            "",
            "",
            cutoff,
            10,
            Some("counter-second"),
        )
        .await
        .unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].profiles, 1);
        assert_eq!(rows[0].machines, 0);
        sqlx::query("UPDATE agent_program_builds SET kind='counter-first' WHERE setting_key='counter-build'").execute(&mut *conn).await.unwrap();
        assert_eq!(counts(&mut conn, "counter-second").await, (1, 0, 0));
        assert_eq!(counts(&mut conn, "counter-first").await, (0, 0, 1));
        sqlx::query("DELETE FROM agent_program_builds WHERE setting_key='counter-build'")
            .execute(&mut *conn)
            .await
            .unwrap();
        sqlx::query("DELETE FROM agents WHERE id=$1")
            .bind(agent)
            .execute(&mut *conn)
            .await
            .unwrap();
        assert_eq!(counts(&mut conn, "counter-first").await, (0, 0, 0));
        assert_eq!(counts(&mut conn, "counter-second").await, (0, 0, 0));
        assert!(
            crate::programs::admin::page(
                &mut conn,
                &json!([]),
                "",
                "",
                cutoff,
                10,
                Some("counter-second")
            )
            .await
            .unwrap()
            .is_empty()
        );
    }
}
