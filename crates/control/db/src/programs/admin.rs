//! Bounded program and build pages, database counts and representative declarations for system administration.

use crate::{Conn, DbError};
use chrono::{DateTime, Utc};
use orch_types::Json;

/// One known kind, with at most one manifest and one representative running declaration.
#[derive(Debug, sqlx::FromRow)]
pub struct ProgramRow {
    /// Stable kind.
    pub kind: String,
    /// First active external declaration, if one exists.
    pub manifest: Option<Json>,
    /// Newest system declaration, otherwise newest declaration of any live profile.
    pub descriptor: Option<Json>,
    /// Overrides on that representative declaration, narrowed by the adapter contract before use.
    pub descriptor_overrides: Option<Json>,
    /// Count of live profiles.
    pub profiles: i64,
    /// Count of active machine reports.
    pub machines: i64,
}

/// Names are fetched alongside one displayed page, never by reading the whole registry.
pub const NAME_PAGE_SIZE: usize = 50;

/// The declarations needed to name one bounded selection of programs. Counts are deliberately omitted.
///
/// # Errors
/// The selection exceeds a page, or a database read failed.
pub async fn names(conn: &mut Conn, kinds: &[String]) -> Result<Vec<ProgramRow>, DbError> {
    if kinds.len() > NAME_PAGE_SIZE {
        return Err(
            sqlx::Error::Protocol("a program-name selection exceeds one page".to_owned()).into(),
        );
    }
    Ok(sqlx::query_as(
        "SELECT wanted.kind,k.manifest,d.descriptor,d.descriptor_overrides,0::bigint profiles,0::bigint machines
         FROM unnest($1::text[]) wanted(kind)
         LEFT JOIN agent_program_kinds k ON k.kind=wanted.kind
         LEFT JOIN LATERAL (
             SELECT descriptor,descriptor_overrides FROM agents a WHERE a.harness_kind=wanted.kind AND a.deleted_at IS NULL
                 AND a.descriptor IS NOT NULL AND a.descriptor<>'{}'::jsonb
             ORDER BY (org_id IS NULL AND machine_id IS NULL) DESC,updated_at DESC,created_at,id LIMIT 1
         ) d ON true ORDER BY wanted.kind")
        .bind(kinds).fetch_all(conn).await?)
}

/// One page of kinds, ordered by their immutable kind. `builtins` is the bounded release manifest array.
/// Search checks translated and English names as well as the kind. Only a page of declarations is held.
///
/// # Errors
/// A database read failed.
pub async fn page(
    conn: &mut Conn,
    builtins: &Json,
    after: &str,
    search: &str,
    cutoff: DateTime<Utc>,
    limit: i64,
    exact: Option<&str>,
) -> Result<Vec<ProgramRow>, DbError> {
    // SQL LIKE metacharacters stay literal; the query scope/cursor still uses the original search text.
    let search = if search.is_empty() {
        String::new()
    } else {
        format!(
            "%{}%",
            search
                .to_lowercase()
                .replace('\\', "\\\\")
                .replace('%', "\\%")
                .replace('_', "\\_")
        )
    };
    Ok(sqlx::query_as(
        "WITH kinds AS (
            SELECT kind,manifest FROM agent_program_kinds k WHERE kind>$2 AND created_at<=$4
                AND ($6::text IS NULL OR kind=$6) AND ($3='' OR search_text LIKE $3)
                AND NOT EXISTS (SELECT 1 FROM jsonb_array_elements($1) p WHERE p->>'kind'=k.kind)
            UNION ALL SELECT p->>'kind',p FROM jsonb_array_elements($1) p WHERE p->>'kind'>$2
                AND ($6::text IS NULL OR p->>'kind'=$6)
                AND ($3='' OR orch_program_search_text(p->>'kind',p->'names') LIKE $3)
         ), picked AS (
            SELECT * FROM kinds k WHERE
                EXISTS (SELECT 1 FROM jsonb_array_elements($1) p WHERE p->>'kind'=k.kind)
                OR EXISTS (SELECT 1 FROM agents a WHERE a.harness_kind=k.kind AND a.deleted_at IS NULL)
                OR EXISTS (SELECT 1 FROM agent_program_reports r WHERE r.kind=k.kind)
                OR EXISTS (SELECT 1 FROM agent_program_builds b WHERE b.kind=k.kind)
            ORDER BY kind LIMIT $5
         ) SELECT p.kind,p.manifest,d.descriptor,d.descriptor_overrides,
             (SELECT count(*) FROM agents a WHERE a.harness_kind=p.kind AND a.deleted_at IS NULL) profiles,
             (SELECT count(*) FROM agent_program_reports r WHERE r.kind=p.kind) machines
         FROM picked p LEFT JOIN LATERAL (
             SELECT descriptor,descriptor_overrides FROM agents a WHERE a.harness_kind=p.kind AND a.deleted_at IS NULL
                 AND a.descriptor IS NOT NULL AND a.descriptor<>'{}'::jsonb
             ORDER BY (org_id IS NULL AND machine_id IS NULL) DESC,updated_at DESC,created_at,id LIMIT 1
         ) d ON true ORDER BY p.kind")
        .bind(builtins).bind(after).bind(search).bind(cutoff).bind(limit).bind(exact).fetch_all(conn).await?)
}

/// A continuation boundary in the newest-first immutable build order.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct BuildPosition {
    /// Precomputed numeric semantic-version ordering.
    pub order: String,
    /// Original version, also disambiguating build metadata.
    pub version: String,
    /// System tie-break.
    pub os: String,
    /// Processor tie-break.
    pub arch: String,
}

/// A bounded build payload with its exact continuation position.
#[derive(Debug, sqlx::FromRow)]
pub struct BuildRow {
    /// Original version.
    pub version: String,
    /// Operating system.
    pub os: String,
    /// Processor.
    pub arch: String,
    /// Precomputed numeric version order.
    pub version_sort: String,
    /// Canonical trusted build, with its effective check status and default.
    pub payload: Json,
}

impl BuildRow {
    /// Continuation after this row.
    #[must_use]
    pub fn position(&self) -> BuildPosition {
        BuildPosition {
            order: self.version_sort.clone(),
            version: self.version.clone(),
            os: self.os.clone(),
            arch: self.arch.clone(),
        }
    }
}

/// Selection applied in SQL before a bounded build page is returned.
pub struct BuildSelection<'a> {
    /// Program.
    pub kind: &'a str,
    /// Release's fixed build array.
    pub shipped: &'a Json,
    /// Explicit system mirror trust.
    pub trust_mirror: bool,
    /// Complete native case catalogue, null when no native check is declared.
    pub expected: Option<&'a [String]>,
    /// Membership boundary shared by cursor pages.
    pub cutoff: DateTime<Utc>,
    /// Optional system filter.
    pub os: Option<&'a str>,
    /// Optional processor filter.
    pub arch: Option<&'a str>,
    /// Optional effective status filter.
    pub status: Option<&'a str>,
}

/// Reads at most `limit` immutable builds. Continuations use the exact SQL ordering, never an offset.
///
/// # Errors
/// A database read failed.
pub async fn builds(
    conn: &mut Conn,
    selection: &BuildSelection<'_>,
    after: Option<&BuildPosition>,
    limit: i64,
) -> Result<Vec<BuildRow>, DbError> {
    Ok(sqlx::query_as(
        "SELECT version,os,arch,version_sort,payload FROM orch_program_build_page($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11,$12,$13)
         ORDER BY version_sort COLLATE \"C\" DESC,version DESC,os,arch LIMIT $13",
    )
    .bind(selection.kind)
    .bind(selection.shipped)
    .bind(selection.trust_mirror)
    .bind(selection.expected)
    .bind(selection.cutoff)
    .bind(selection.os)
    .bind(selection.arch)
    .bind(selection.status)
    .bind(after.map(|p| p.order.as_str()))
    .bind(after.map(|p| p.version.as_str()))
    .bind(after.map(|p| p.os.as_str()))
    .bind(after.map(|p| p.arch.as_str()))
    .bind(limit)
    .fetch_all(conn)
    .await?)
}

/// Per-platform totals and server-selected newest checked/default builds; no complete build list leaves SQL.
#[derive(Debug, sqlx::FromRow)]
pub struct PlatformRow {
    /// Program owning this platform.
    pub kind: String,
    /// Newest known version on this platform.
    pub newest_version: Option<String>,
    /// Effective result of that newest build.
    pub newest_status: Option<String>,
    /// Platform system.
    pub os: String,
    /// Platform processor.
    pub arch: String,
    /// Immutable identity count.
    pub builds: i64,
    /// Newest checked build.
    pub newest_checked: Option<String>,
    /// Current default.
    pub default_version: Option<String>,
}

/// Aggregates platform selections without reading the build payloads into application memory.
///
/// # Errors
/// A database read failed.
pub async fn platforms(
    conn: &mut Conn,
    programs: &Json,
    shipped: &Json,
    trust_mirror: bool,
    cutoff: DateTime<Utc>,
) -> Result<Vec<PlatformRow>, DbError> {
    Ok(sqlx::query_as(
        "SELECT p.kind,s.os,s.arch,c.*
         FROM jsonb_to_recordset($1) p(kind text,expected text[])
         CROSS JOIN (VALUES ('linux','x86_64'),('linux','aarch64'),('macos','x86_64'),
             ('macos','aarch64'),('windows','x86_64'),('windows','aarch64')) s(os,arch)
         CROSS JOIN LATERAL (
             SELECT count(*) builds,
                 split_part(max((version_sort||chr(1)||version) COLLATE \"C\"),chr(1),2) newest_version,
                 split_part(max((version_sort||chr(1)||version||chr(1)||(payload->>'status')) COLLATE \"C\"),chr(1),3) newest_status,
                 split_part(max((version_sort||chr(1)||version) COLLATE \"C\")
                     FILTER (WHERE payload->>'status'='checked'),chr(1),2) newest_checked,
                 split_part(max((version_sort||chr(1)||version) COLLATE \"C\")
                     FILTER (WHERE payload->>'is_default'='true'),chr(1),2) default_version
             FROM orch_program_build_rows(p.kind,$2,$3,p.expected,$4) b
             WHERE b.os=s.os AND b.arch=s.arch HAVING count(*)>0
         ) c ORDER BY p.kind,s.os,s.arch")
        .bind(programs).bind(shipped).bind(trust_mirror).bind(cutoff).fetch_all(conn).await?)
}

/// The platform builds of exactly one version; bounded by the declared system/processor combinations.
///
/// # Errors
/// The database read failed.
pub async fn builds_of_version(
    conn: &mut Conn,
    selection: &BuildSelection<'_>,
    version: &str,
) -> Result<Vec<BuildRow>, DbError> {
    Ok(sqlx::query_as("SELECT version,os,arch,version_sort,payload FROM orch_program_build_rows($1,$2,$3,$4,$5) WHERE version=$6 ORDER BY os,arch LIMIT 6")
        .bind(selection.kind).bind(selection.shipped).bind(selection.trust_mirror).bind(selection.expected)
        .bind(selection.cutoff).bind(version).fetch_all(conn).await?)
}

/// Database observation time for the upper bound shared by continuation pages.
///
/// # Errors
/// The database clock could not be read.
pub async fn snapshot(conn: &mut Conn) -> Result<DateTime<Utc>, DbError> {
    Ok(sqlx::query_scalar("SELECT clock_timestamp()")
        .fetch_one(conn)
        .await?)
}

/// Exact canonical source rows of one version: three systems, two architectures and two stored sources.
///
/// # Errors
/// A database read failed.
pub async fn stored_version(
    conn: &mut Conn,
    kind: &str,
    version: &str,
) -> Result<Vec<(String, Json)>, DbError> {
    Ok(sqlx::query_as("SELECT setting_key,payload FROM agent_program_builds WHERE kind=$1 AND version=$2 ORDER BY os,arch,priority DESC LIMIT 12")
        .bind(kind).bind(version).fetch_all(conn).await?)
}

#[cfg(test)]
#[allow(clippy::unwrap_used, reason = "a test fails by panicking")]
mod tests {
    use super::*;
    use serde_json::json;
    #[tokio::test]
    async fn translated_program_search_tracks_reports_and_treats_pattern_symbols_literally() {
        let test = orch_testkit::database::TestDb::fresh().await;
        let mut conn = test.pool.acquire().await.unwrap();
        let machine =
            crate::queries::resources::testing::insert_machine(&mut conn, "reported", "connected")
                .await
                .unwrap();
        let facts = json!({"adapters":[{"kind":"third-search","builtin":false,"manifest":{
            "manifest_version":1,"kind":"third-search","protocol":"fake",
            "names":{"en":"A%_program","zh-CN":"中文程序"}
        }}]});
        sqlx::query("UPDATE machines SET facts=$2 WHERE id=$1")
            .bind(machine)
            .bind(&facts)
            .execute(&mut *conn)
            .await
            .unwrap();
        let cutoff = snapshot(&mut conn).await.unwrap();
        for search in ["A%_", "中文程序"] {
            let found = page(&mut conn, &json!([]), "", search, cutoff, 3, None)
                .await
                .unwrap();
            assert_eq!(found.len(), 1);
            assert_eq!(found[0].kind, "third-search");
            assert_eq!(found[0].machines, 1);
        }
        for search in ["Axxprogram", "en", "zh-CN", "\"", "{"] {
            assert!(
                page(&mut conn, &json!([]), "", search, cutoff, 3, None)
                    .await
                    .unwrap()
                    .is_empty(),
                "search must match names or kinds, not locale keys or JSON syntax: {search}"
            );
        }
        let builtin = json!([{"kind":"built-in","names":{"en":"Opaque","zh-CN":"内置程序"}}]);
        assert!(
            page(&mut conn, &builtin, "", "zh-CN", cutoff, 3, None)
                .await
                .unwrap()
                .is_empty()
        );
        assert_eq!(
            page(&mut conn, &builtin, "", "Opaque", cutoff, 3, None)
                .await
                .unwrap()
                .len(),
            1
        );
        let changed = json!({"adapters":[{"kind":"third-search","builtin":false,"manifest":{
            "manifest_version":1,"kind":"third-search","protocol":"fake","names":{"en":"Renamed"}
        }}]});
        sqlx::query("UPDATE machines SET facts=$2 WHERE id=$1")
            .bind(machine)
            .bind(changed)
            .execute(&mut *conn)
            .await
            .unwrap();
        let cutoff = snapshot(&mut conn).await.unwrap();
        assert!(
            page(&mut conn, &json!([]), "", "中文程序", cutoff, 3, None)
                .await
                .unwrap()
                .is_empty()
        );
        assert_eq!(
            page(&mut conn, &json!([]), "", "Renamed", cutoff, 3, None)
                .await
                .unwrap()
                .len(),
            1
        );
        sqlx::query("UPDATE machines SET deleted_at=now() WHERE id=$1")
            .bind(machine)
            .execute(&mut *conn)
            .await
            .unwrap();
        assert!(
            page(&mut conn, &json!([]), "", "Renamed", cutoff, 3, None)
                .await
                .unwrap()
                .is_empty()
        );
    }

    #[tokio::test]
    async fn build_pages_are_bounded_and_trusted_precedence_filters_and_version_order_agree() {
        let test = orch_testkit::database::TestDb::fresh().await;
        let mut conn = test.pool.acquire().await.unwrap();
        sqlx::query("INSERT INTO settings(key,value) SELECT 'harness_build:third:1.0.'||g||':linux:x86_64',
            jsonb_build_object('kind','third','version','1.0.'||g,'os','linux','arch','x86_64',
                'source','admin','checksum_origin','admin','status',CASE WHEN g%2=0 THEN 'checked' ELSE 'failed' END,
                'is_default',g=7,'url','https://example.test/build','sha256',repeat('a',64)) FROM generate_series(0,119) g")
            .execute(&mut *conn).await.unwrap();
        let shipped = json!([{"kind":"third","version":"1.0.2","os":"linux","arch":"x86_64","source":"release","status":"checked","is_default":true}]);
        let selection = BuildSelection {
            kind: "third",
            shipped: &shipped,
            trust_mirror: false,
            expected: None,
            cutoff: snapshot(&mut conn).await.unwrap(),
            os: None,
            arch: None,
            status: None,
        };
        let mut after = None;
        let mut versions = Vec::new();
        let mut pages = 0;
        loop {
            let page = builds(&mut conn, &selection, after.as_ref(), 7)
                .await
                .unwrap();
            assert!(page.len() <= 7);
            pages += 1;
            if page.is_empty() {
                break;
            }
            after = page.last().map(BuildRow::position);
            versions.extend(page.iter().map(|b| b.version.clone()));
        }
        assert_eq!(versions.len(), 120);
        assert_eq!(pages, 19);
        assert_eq!(versions[0], "1.0.119");
        assert_eq!(versions[119], "1.0.0");
        let mut unique = versions.clone();
        unique.sort();
        unique.dedup();
        assert_eq!(unique.len(), 120);
        let stats = platforms(
            &mut conn,
            &json!([{"kind":"third","expected":null}]),
            &shipped,
            false,
            selection.cutoff,
        )
        .await
        .unwrap();
        assert_eq!(stats.len(), 1);
        assert_eq!(stats[0].builds, 120);
        assert_eq!(stats[0].newest_checked.as_deref(), Some("1.0.118"));
        assert_eq!(stats[0].default_version.as_deref(), Some("1.0.7"));
        let checked = BuildSelection {
            status: Some("checked"),
            ..selection
        };
        let page = builds(&mut conn, &checked, None, 5).await.unwrap();
        assert_eq!(page.len(), 5);
        assert!(page.iter().all(|b| b.payload["status"] == "checked"));
    }
    #[tokio::test]
    async fn old_interface_checks_and_malformed_native_reports_never_become_checked() {
        let test = orch_testkit::database::TestDb::fresh().await;
        let mut conn = test.pool.acquire().await.unwrap();
        let expected = vec!["startup".to_owned()];
        for (report, valid) in [
            (json!({}), false),
            (json!({"check":{"stage":"complete","cases":{}}}), false),
            (
                json!({"check":{"stage":"complete","cases":[{"name":"startup","passed":"true","elapsed_ms":1}]}}),
                false,
            ),
            (
                json!({"check":{"stage":"complete","cases":[{"name":"startup","passed":true,"elapsed_ms":"1"}]}}),
                false,
            ),
            (
                json!({"check":{"stage":"complete","cases":[{"name":"startup","passed":true,"elapsed_ms":1}]}}),
                true,
            ),
            (
                json!({"check":{"stage":"complete","cases":[{"name":"startup","passed":true,"elapsed_ms":-1}]}}),
                false,
            ),
            (
                json!({"check":{"stage":"complete","cases":[{"name":"startup","passed":true,"elapsed_ms":"bad"}]}}),
                false,
            ),
            (
                json!({"check":{"stage":"complete","cases":[{"name":"startup","passed":true,"elapsed_ms":1,"error":"failed"}]}}),
                false,
            ),
        ] {
            let actual: bool = sqlx::query_scalar("SELECT orch_program_has_native_proof($1,$2)")
                .bind(report)
                .bind(&expected)
                .fetch_one(&mut *conn)
                .await
                .unwrap();
            assert_eq!(actual, valid);
        }
        let ordered:Vec<String>=sqlx::query_scalar("SELECT v FROM unnest(ARRAY['1.0.0-rc.10','1.0.0-rc.2','1.0.0','1.0.9','1.0.10']) v ORDER BY orch_program_version_order(v) COLLATE \"C\" DESC")
            .fetch_all(&mut *conn).await.unwrap();
        assert_eq!(
            ordered,
            ["1.0.10", "1.0.9", "1.0.0", "1.0.0-rc.10", "1.0.0-rc.2"]
        );
        let numeric: bool = sqlx::query_scalar("SELECT orch_program_version_order('1.0.0-rc.100000000000000000000') COLLATE \"C\">orch_program_version_order('1.0.0-rc.99999999999999999999') COLLATE \"C\"")
            .fetch_one(&mut *conn).await.unwrap();
        assert!(
            numeric,
            "arbitrarily long numeric prerelease identifiers keep numeric order"
        );
        for malformed in [
            "01.0.0",
            "18446744073709551616.0.0",
            "1.0.0-rc..1",
            "1.0.0-01",
        ] {
            let fallback: String = sqlx::query_scalar("SELECT orch_program_version_order($1)")
                .bind(malformed)
                .fetch_one(&mut *conn)
                .await
                .unwrap();
            assert!(
                fallback.starts_with("0!"),
                "malformed semantic version uses deterministic fallback"
            );
        }
    }
}
