//! The agent programs the system knows: those Orch ships, and those the connectors of its machines have reported.
//!
//! A connector whose adapter is not part of Orch sends the program's manifest in its facts. Those manifests join
//! the registry of what Orch ships, as long as each is consistent and takes no kind that is taken: the first
//! machine to report a kind speaks for it, and one that claims a kind of a shipped program is ignored.

use orch_agent_programs::{ProgramManifest, Programs};
use orch_types::Json;

use crate::{Conn, DbError};

/// Bounded program administration backed by indexed report and build projections.
pub mod admin;

/// The manifests that machines report in their facts, in the order of their machines. A member that is not a
/// manifest is skipped.
#[must_use]
pub fn reported_in(facts: &[Json]) -> Vec<ProgramManifest> {
    facts
        .iter()
        .flat_map(|machine| {
            machine
                .get("adapters")
                .and_then(Json::as_array)
                .into_iter()
                .flatten()
        })
        .filter(|adapter| adapter.get("builtin").and_then(Json::as_bool) != Some(true))
        .filter_map(|adapter| adapter.get("manifest"))
        .filter_map(|manifest| serde_json::from_value::<ProgramManifest>(manifest.clone()).ok())
        .collect()
}

/// `base` with the programs `facts` report added; a manifest that does not hold together, or that takes a kind
/// that is taken, is left out.
#[must_use]
pub fn with_reported(base: &Programs, facts: &[Json]) -> Programs {
    let mut programs = base.clone();
    for manifest in reported_in(facts) {
        if let Ok(more) = programs.with_program(manifest) {
            programs = more;
        }
    }
    programs
}

/// The manifests that the active machines report for programs that are not part of the build, one for each kind: the
/// first machine to report a kind speaks for it. Only that member of the facts is read, so what this reads does not
/// grow with the facts of the machines but with the programs there are.
///
/// # Errors
/// The database failed.
pub async fn reported_manifests(conn: &mut Conn) -> Result<Vec<Json>, DbError> {
    Ok(sqlx::query_scalar(
        "SELECT DISTINCT ON (kind) manifest FROM agent_program_reports
          WHERE NOT builtin AND jsonb_typeof(manifest)='object'
          ORDER BY kind,reported_at,machine_id",
    )
    .fetch_all(conn)
    .await?)
}

/// Every program the system knows now: what Orch ships, and what its machines report.
///
/// # Errors
/// The database failed.
pub async fn registry(conn: &mut Conn) -> Result<Programs, DbError> {
    let mut programs = Programs::builtin().clone();
    for manifest in reported_manifests(conn).await? {
        if let Ok(manifest) = serde_json::from_value::<ProgramManifest>(manifest)
            && let Ok(more) = programs.with_program(manifest)
        {
            programs = more;
        }
    }
    Ok(programs)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn reported(kind: &str, builtin: bool) -> Json {
        json!({ "adapters": [ {
            "kind": kind, "adapter_version": 1, "builtin": builtin,
            "manifest": { "manifest_version": 1, "kind": kind, "names": { "en": "Third" }, "protocol": "fake" }
        } ] })
    }

    #[test]
    fn a_program_a_machine_reports_joins_the_registry_and_cannot_take_a_shipped_kind() {
        let facts = [
            reported("third", false),
            reported("codex", false),
            reported("third", false),
        ];
        let programs = with_reported(Programs::builtin(), &facts);
        assert!(programs.program("third").is_some());
        assert_eq!(
            programs.program("codex"),
            Programs::builtin().program("codex"),
            "a report cannot replace what Orch ships"
        );
        assert_eq!(
            programs.kinds().count(),
            Programs::builtin().kinds().count() + 1
        );
    }

    #[tokio::test]
    async fn the_registry_of_the_system_reads_only_the_manifests_of_active_machines_one_for_each_kind()
     {
        use orch_testkit::database::TestDb;
        let test = TestDb::fresh().await;
        let mut conn = test.pool.acquire().await.unwrap();
        let first =
            crate::queries::resources::testing::insert_machine(&mut conn, "first", "connected")
                .await
                .unwrap();
        let second =
            crate::queries::resources::testing::insert_machine(&mut conn, "second", "connected")
                .await
                .unwrap();
        let removed =
            crate::queries::resources::testing::insert_machine(&mut conn, "removed", "connected")
                .await
                .unwrap();
        for (machine, name, kind, builtin) in [
            (first, "Third of the first", "third", false),
            (second, "Third of the second", "third", false),
            (removed, "Fourth of a removed one", "fourth", false),
            (second, "Shipped", "codex", true),
        ] {
            let mut facts: Json = sqlx::query_scalar("SELECT facts FROM machines WHERE id = $1")
                .bind(machine)
                .fetch_one(&mut *conn)
                .await
                .unwrap();
            if !facts.is_object() {
                facts = json!({});
            }
            let mut adapters = facts["adapters"].as_array().cloned().unwrap_or_default();
            adapters.push(json!({
                "kind": kind, "adapter_version": 1, "builtin": builtin,
                "manifest": { "manifest_version": 1, "kind": kind, "names": { "en": name }, "protocol": "fake" }
            }));
            facts["adapters"] = json!(adapters);
            sqlx::query("UPDATE machines SET facts = $2 WHERE id = $1")
                .bind(machine)
                .bind(facts)
                .execute(&mut *conn)
                .await
                .unwrap();
        }
        sqlx::query("UPDATE machines SET deleted_at = now() WHERE id = $1")
            .bind(removed)
            .execute(&mut *conn)
            .await
            .unwrap();
        let programs = registry(&mut conn).await.unwrap();
        assert_eq!(
            programs.program("third").map(|manifest| manifest.name()),
            Some("Third of the first"),
            "the first machine to report a kind speaks for it"
        );
        assert!(
            programs.program("fourth").is_none(),
            "a removed machine reports nothing"
        );
        assert_eq!(
            programs.program("codex"),
            Programs::builtin().program("codex"),
            "a report cannot replace what Orch ships"
        );
        assert_eq!(reported_manifests(&mut conn).await.unwrap().len(), 1);
    }

    #[tokio::test]
    async fn what_is_read_for_the_registry_and_for_the_carry_of_a_kind_does_not_grow_with_machines_or_profiles()
     {
        use orch_testkit::database::TestDb;
        const MANY: i64 = 20_000;
        let test = TestDb::fresh().await;
        let mut conn = test.pool.acquire().await.unwrap();
        // Twenty thousand machines, made by one statement, each reporting the adapters of the build and the same
        // program of its own, and as many profiles, each with a descriptor, of three kinds.
        let facts = json!({ "adapters": [
            { "kind": "codex", "adapter_version": 1, "builtin": true },
            { "kind": "claude-code", "adapter_version": 1, "builtin": true },
            { "kind": "third", "adapter_version": 1, "builtin": false,
              "manifest": { "manifest_version": 1, "kind": "third", "names": { "en": "Third" }, "protocol": "fake" } }
        ], "harnesses": [] });
        sqlx::query(
            "INSERT INTO machines (id, name, placement, platform_os, platform_arch, connector_version,
                                   protocol_version, cert_serial, cert_fingerprint, cert_not_after, facts)
             SELECT gen_random_uuid(), 'm' || g, 'connected', 'linux', 'x86_64', '0.1.0', 1, 'serial',
                    decode(md5(g::text) || md5(g::text), 'hex'), now() + interval '1 year', $2
               FROM generate_series(1, $1::bigint) AS g",
        )
        .bind(MANY)
        .bind(&facts)
        .execute(&mut *conn)
        .await
        .unwrap();
        sqlx::query(
            "INSERT INTO agents (id, name, harness_kind, descriptor)
             SELECT gen_random_uuid(), 'a' || g, 'kind' || (g % 3),
                    jsonb_build_object('capabilities', jsonb_build_object('import_neutral', g % 2 = 0))
               FROM generate_series(1, $1::bigint) AS g",
        )
        .bind(MANY)
        .execute(&mut *conn)
        .await
        .unwrap();

        let manifests = reported_manifests(&mut conn).await.unwrap();
        assert_eq!(
            manifests.len(),
            1,
            "one manifest for a kind, however many machines report it"
        );
        assert_eq!(
            registry(&mut conn).await.unwrap().kinds().count(),
            Programs::builtin().kinds().count() + 1
        );
        let neutral = crate::queries::resources::binding::neutral_import_of_kinds(&mut conn)
            .await
            .unwrap();
        assert_eq!(
            neutral.len(),
            3,
            "one row for a kind, however many profiles declare it"
        );
        let summaries = crate::queries::resources::binding::agent_kind_summaries(&mut conn)
            .await
            .unwrap();
        assert!(
            (3..=12).contains(&summaries.len()),
            "at most four profiles speak for a kind, however many there are: {}",
            summaries.len()
        );
        assert_eq!(
            crate::queries::resources::binding::machine_adapter_kinds(&mut conn)
                .await
                .unwrap()
                .len(),
            3,
            "a kind once, however many machines report it"
        );
        assert!(
            crate::queries::resources::binding::machine_harness_versions(&mut conn)
                .await
                .unwrap()
                .is_empty()
        );
    }

    #[test]
    fn what_a_built_in_adapter_reports_is_not_taken_for_a_new_program() {
        assert!(reported_in(&[reported("third", true)]).is_empty());
    }
}
