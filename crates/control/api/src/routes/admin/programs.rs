//! Paged agent program administration, independent release selections and localized declaration capabilities.

use super::{admin_of, invalid, page_size, read_ahead};
use crate::middleware::requests::{ApiError, ApiResult};
use crate::registry::{Ctx, Registry};
use crate::routes::downloads::payload_dir;
use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD};
use chrono::{DateTime, Utc};
use orch_agent_programs::{Catalogue, ProgramManifest, Programs};
use orch_api_contract::admin::{
    HarnessBuild, HarnessBuildStatus, HarnessChannelSchedule, HarnessChannelsStatus,
};
use orch_api_contract::common::Page;
use orch_api_contract::program_admin::{
    AdminProgramBuildCheck, AdminProgramBuildList, AdminProgramBuildListQuery,
    AdminProgramBuildSet, AdminProgramChannelsRead, AdminProgramGet, AdminProgramList,
    AdminProgramListQuery, AdminProgramVersionFetch, AdminProgramVersionGet,
    ProgramBuildCheckReply, ProgramCapability, ProgramCapabilityGroup, ProgramChannelReadReply,
    ProgramChannelState, ProgramDetail, ProgramPlatform, ProgramSummary,
};
use orch_db::Conn;
use orch_db::programs::admin::{
    self as rows, BuildPosition, BuildSelection, PlatformRow, ProgramRow,
};
use orch_types::Json;
use serde::{Deserialize, Serialize};
use serde_json::json;

/// Registers system-administrator-only program projections.
pub fn mount(registry: &mut Registry) {
    registry
        .on(AdminProgramList, list)
        .on(AdminProgramGet, get)
        .on(AdminProgramBuildList, builds)
        .on(AdminProgramBuildSet, set_build)
        .on(AdminProgramBuildCheck, check_build)
        .on(AdminProgramVersionGet, version_get)
        .on(AdminProgramVersionFetch, version_fetch)
        .on(AdminProgramChannelsRead, channels_read);
}

async fn set_build(ctx: Ctx, (): (), body: HarnessBuild) -> ApiResult<HarnessBuild> {
    let mut build = super::catalogue::set_build(ctx.clone(), body).await?;
    let mut conn = ctx.app.db.acquire().await?;
    let now = rows::snapshot(&mut conn).await?;
    let row = rows::page(
        &mut conn,
        &builtin_manifests(),
        "",
        "",
        now,
        1,
        Some(&build.kind),
    )
    .await?
    .pop()
    .map(normalized);
    let neutral = row
        .and_then(|r| r.descriptor)
        .and_then(|d| {
            d.pointer("/capabilities/import_neutral")
                .and_then(Json::as_bool)
        })
        .unwrap_or(false);
    build.carry = Some(crate::programs::carry_of(neutral, &[]));
    Ok(build)
}
async fn check_build(ctx: Ctx, (): (), (): ()) -> ApiResult<ProgramBuildCheckReply> {
    Ok(ProgramBuildCheckReply {
        queued: super::catalogue::queue_check(ctx).await?,
    })
}

async fn channels_read(ctx: Ctx, (): (), (): ()) -> ApiResult<ProgramChannelReadReply> {
    let kind = ctx.param("kind")?.to_owned();
    Ok(ProgramChannelReadReply {
        kept: super::catalogue::read_channels(ctx, Some(kind)).await?,
    })
}
async fn version_fetch(ctx: Ctx, (): (), (): ()) -> ApiResult<Vec<HarnessBuild>> {
    admin_of(&ctx)?;
    super::catalogue::fetch_exact(
        ctx,
        orch_api_contract::admin::HarnessVersionFetchBody {
            environment_id: None,
            node_id: None,
        },
    )
    .await
}
async fn version_get(ctx: Ctx, (): (), (): ()) -> ApiResult<Vec<HarnessBuild>> {
    admin_of(&ctx)?;
    let mut conn = ctx.app.db.acquire().await?;
    exact_builds(&ctx, &mut conn, ctx.param("kind")?, ctx.param("version")?).await
}
/// Reads only this version's trusted platform rows and applies its server-owned carry and download projection.
pub(super) async fn exact_builds(
    ctx: &Ctx,
    conn: &mut Conn,
    kind: &str,
    version: &str,
) -> ApiResult<Vec<HarnessBuild>> {
    let now = rows::snapshot(conn).await?;
    let row = rows::page(conn, &builtin_manifests(), "", "", now, 1, Some(kind))
        .await?
        .pop()
        .map(normalized);
    let programs = row
        .as_ref()
        .map_or_else(|| Programs::builtin().clone(), registry);
    let expected = expected(&programs, kind);
    let shipped = json!(super::catalogue::release_builds(&payload_dir(&ctx.app.config)).await);
    let selection = BuildSelection {
        kind,
        shipped: &shipped,
        trust_mirror: crate::harness_channels::trusts_mirror(conn).await?,
        expected: expected.as_deref(),
        cutoff: now,
        os: None,
        arch: None,
        status: None,
    };
    let found = rows::builds_of_version(conn, &selection, version).await?;
    let mirror = system::<String>(
        conn,
        orch_settings_contract::registry::keys::AGENTS_HARNESS_MIRROR,
    )
    .await?;
    let neutral = row
        .as_ref()
        .and_then(|r| r.descriptor.as_ref())
        .and_then(|d| d.pointer("/capabilities/import_neutral"))
        .and_then(Json::as_bool)
        .unwrap_or(false);
    found
        .into_iter()
        .map(|row| {
            let mut build: HarnessBuild =
                serde_json::from_value(row.payload).map_err(ApiError::internal)?;
            let off = build
                .check
                .as_ref()
                .map(|c| c.off.as_slice())
                .unwrap_or(&[]);
            build.carry = Some(crate::programs::carry_of(neutral, off));
            build.url = crate::harness_channels::download_url(&programs, &build, mirror.as_deref());
            Ok(build)
        })
        .collect()
}

#[derive(Serialize, Deserialize)]
struct Cursor {
    scope: Json,
    cutoff: DateTime<Utc>,
    kind: Option<String>,
    build: Option<BuildPosition>,
}
fn decode(text: Option<&str>, scope: Json, now: DateTime<Utc>) -> ApiResult<Cursor> {
    let Some(text) = text else {
        return Ok(Cursor {
            scope,
            cutoff: now,
            kind: None,
            build: None,
        });
    };
    let bad = || invalid("cursor", "is not a cursor of this program list and filters");
    if text.len() > 8192 {
        return Err(bad());
    }
    let bytes = URL_SAFE_NO_PAD.decode(text).map_err(|_| bad())?;
    let value: Cursor = serde_json::from_slice(&bytes).map_err(|_| bad())?;
    if value.scope != scope || value.cutoff > now {
        return Err(bad());
    }
    Ok(value)
}
fn encode(cursor: &Cursor) -> ApiResult<String> {
    Ok(URL_SAFE_NO_PAD.encode(serde_json::to_vec(cursor).map_err(ApiError::internal)?))
}
fn builtin_manifests() -> Json {
    json!(Programs::builtin().programs().collect::<Vec<_>>())
}
pub(super) fn registry(row: &ProgramRow) -> Programs {
    let base = Programs::builtin().clone();
    row.manifest
        .clone()
        .and_then(|value| serde_json::from_value::<ProgramManifest>(value).ok())
        .and_then(|manifest| base.with_program(manifest).ok())
        .unwrap_or(base)
}
fn expected(programs: &Programs, kind: &str) -> Option<Vec<String>> {
    programs
        .program(kind)
        .filter(|p| p.checks.interface.is_some())
        .map(|p| p.checks.cases().map(|(name, _)| name.to_owned()).collect())
}
fn selected(rows: &[ProgramRow]) -> Json {
    json!(
        rows.iter()
            .map(|row| json!({"kind":row.kind,"expected":expected(&registry(row),&row.kind)}))
            .collect::<Vec<_>>()
    )
}
async fn system<T: serde::de::DeserializeOwned>(
    conn: &mut Conn,
    key: &str,
) -> ApiResult<Option<T>> {
    orch_settings::get(conn, orch_settings::Place::System, key)
        .await
        .map_err(ApiError::internal)
}
async fn read_state(conn: &mut Conn) -> ApiResult<Json> {
    Ok(
        orch_db::queries::settings::row(conn, None, "harness_channels:read")
            .await?
            .map(|r| r.value)
            .unwrap_or(Json::Null),
    )
}
fn channel_state(programs: &Programs, kind: &str, state: &Json, hours: i64) -> ProgramChannelState {
    if programs
        .program(kind)
        .is_none_or(|p| p.builds.channels.is_empty())
    {
        return ProgramChannelState::None;
    }
    if hours <= 0 {
        return ProgramChannelState::Disabled;
    }
    if state.get("last_attempt_at").and_then(Json::as_str)
        > state.get("last_read_at").and_then(Json::as_str)
    {
        return ProgramChannelState::Failed;
    }
    if state.get("last_read_at").is_some_and(|v| !v.is_null()) {
        ProgramChannelState::Ready
    } else {
        ProgramChannelState::Unread
    }
}
fn version_order(a: &str, b: &str) -> std::cmp::Ordering {
    match (
        semver::Version::parse(a.trim_start_matches('v')),
        semver::Version::parse(b.trim_start_matches('v')),
    ) {
        (Ok(a), Ok(b)) => a.cmp(&b),
        (Ok(_), Err(_)) => std::cmp::Ordering::Greater,
        (Err(_), Ok(_)) => std::cmp::Ordering::Less,
        _ => a.cmp(b),
    }
}
fn channel_hours_default() -> i64 {
    orch_settings_contract::registry::get(
        orch_settings_contract::registry::keys::AGENTS_HARNESS_CHANNEL_HOURS,
    )
    .and_then(|entry| entry.default_value())
    .and_then(|value| value.as_i64())
    .unwrap_or(0)
}
pub(super) fn normalized(mut row: ProgramRow) -> ProgramRow {
    row.descriptor = row
        .descriptor
        .as_ref()
        .and_then(|value| {
            serde_json::from_value::<orch_agent_contract::descriptor::AgentDescriptor>(
                value.clone(),
            )
            .ok()
        })
        .and_then(|descriptor| {
            descriptor
                .with_overrides(row.descriptor_overrides.as_ref().unwrap_or(&json!({})))
                .ok()
        })
        .and_then(|descriptor| serde_json::to_value(descriptor).ok());
    row
}
fn summary(
    row: &ProgramRow,
    platforms: &[PlatformRow],
    shipped: &[HarnessBuild],
    state: &Json,
    hours: i64,
) -> ApiResult<ProgramSummary> {
    let programs = registry(row);
    let platforms: Vec<_> = platforms.iter().filter(|p| p.kind == row.kind).collect();
    let newest = platforms
        .iter()
        .filter_map(|p| p.newest_version.as_ref())
        .max_by(|a, b| version_order(a, b));
    let statuses: Vec<_> = platforms
        .iter()
        .filter(|p| p.newest_version.as_ref() == newest)
        .filter_map(|p| p.newest_status.as_deref())
        .collect();
    let check = ["failed", "checking", "untested", "checked"]
        .into_iter()
        .find(|s| statuses.contains(s))
        .map(|s| serde_json::from_value::<HarnessBuildStatus>(json!(s)))
        .transpose()
        .map_err(ApiError::internal)?;
    Ok(ProgramSummary {
        harness: crate::programs::harness_name(&programs, &row.kind, row.descriptor.as_ref()),
        builtin: Programs::builtin().program(&row.kind).is_some(),
        protocol: programs.program(&row.kind).map(|p| p.protocol.clone()),
        profiles: row.profiles,
        machines: row.machines,
        platforms: platforms
            .iter()
            .map(|p| {
                Ok(ProgramPlatform {
                    os: serde_json::from_value(json!(p.os)).map_err(ApiError::internal)?,
                    arch: serde_json::from_value(json!(p.arch)).map_err(ApiError::internal)?,
                    shipped: shipped
                        .iter()
                        .find(|b| {
                            b.kind == row.kind
                                && json!(b.os) == json!(p.os)
                                && json!(b.arch) == json!(p.arch)
                        })
                        .map(|b| b.version.clone()),
                    newest: p.newest_version.clone(),
                    newest_checked: p.newest_checked.clone(),
                    default_version: p.default_version.clone(),
                    builds: p.builds,
                })
            })
            .collect::<ApiResult<Vec<_>>>()?,
        channel: channel_state(&programs, &row.kind, state, hours),
        check,
    })
}
async fn list(ctx: Ctx, query: AdminProgramListQuery, (): ()) -> ApiResult<Page<ProgramSummary>> {
    admin_of(&ctx)?;
    let q = query.q.as_deref().unwrap_or("").trim();
    if q.chars().count() > 200 {
        return Err(invalid("q", "contains at most 200 characters"));
    }
    let mut conn = ctx.app.db.acquire().await?;
    let now = rows::snapshot(&mut conn).await?;
    let mut cursor = decode(
        query.cursor.as_deref(),
        json!({"list":"programs","q":q}),
        now,
    )?;
    let size = page_size(query.limit);
    let mut found = rows::page(
        &mut conn,
        &builtin_manifests(),
        cursor.kind.as_deref().unwrap_or(""),
        q,
        cursor.cutoff,
        read_ahead(size),
        None,
    )
    .await?;
    let more = found.len() > size;
    found.truncate(size);
    let found: Vec<_> = found.into_iter().map(normalized).collect();
    let shipped = super::catalogue::release_builds(&payload_dir(&ctx.app.config)).await;
    let trust = crate::harness_channels::trusts_mirror(&mut conn).await?;
    let platforms = rows::platforms(
        &mut conn,
        &selected(&found),
        &json!(shipped),
        trust,
        cursor.cutoff,
    )
    .await?;
    let hours = system::<i64>(
        &mut conn,
        orch_settings_contract::registry::keys::AGENTS_HARNESS_CHANNEL_HOURS,
    )
    .await?
    .unwrap_or_else(channel_hours_default);
    let state = read_state(&mut conn).await?;
    let items = found
        .iter()
        .map(|row| summary(row, &platforms, &shipped, &state, hours))
        .collect::<ApiResult<Vec<_>>>()?;
    cursor.kind = found.last().map(|p| p.kind.clone());
    Ok(Page {
        items,
        next_cursor: if more { Some(encode(&cursor)?) } else { None },
    })
}
fn claimed(value: &Json) -> bool {
    match value {
        Json::Bool(v) => *v,
        Json::Null => false,
        Json::String(v) => !matches!(v.as_str(), "none" | "no" | "unknown" | "uncontrolled" | ""),
        Json::Array(v) => !v.is_empty(),
        Json::Object(v) => v.values().any(claimed),
        Json::Number(v) => v.as_i64().is_some_and(|n| n > 0),
    }
}
fn capabilities(descriptor: Option<&Json>, off: &[String]) -> Option<Vec<ProgramCapabilityGroup>> {
    let descriptor = descriptor?;
    let catalogue = Catalogue::builtin();
    Some(
        catalogue
            .capability_groups
            .iter()
            .map(|group| ProgramCapabilityGroup {
                id: group.id.clone(),
                title: group.title.clone(),
                items: group
                    .items
                    .iter()
                    .map(|entry| {
                        let supported = entry.path.split('|').any(|choice| {
                            let (path, member) = choice
                                .split_once(':')
                                .map_or((choice, None), |(p, m)| (p, Some(m)));
                            descriptor.pointer(path).is_some_and(|value| {
                                member.map_or_else(
                                    || claimed(value),
                                    |m| {
                                        value.as_array().is_some_and(|v| {
                                            v.iter().any(|v| v.as_str() == Some(m))
                                        })
                                    },
                                )
                            })
                        });
                        let disabled = off.contains(&entry.id)
                            || (entry.id == "carry" && off.iter().any(|v| v == "import_neutral"));
                        ProgramCapability {
                            id: entry.id.clone(),
                            title: entry.title.clone(),
                            available: supported && !disabled,
                            reason: if disabled {
                                Some(catalogue.off_by_check.clone())
                            } else if !supported {
                                Some(catalogue.not_supported.clone())
                            } else {
                                None
                            },
                        }
                    })
                    .collect(),
            })
            .collect(),
    )
}
async fn get(ctx: Ctx, (): (), (): ()) -> ApiResult<ProgramDetail> {
    admin_of(&ctx)?;
    let kind = ctx.param("kind")?;
    let mut conn = ctx.app.db.acquire().await?;
    let now = rows::snapshot(&mut conn).await?;
    let row = rows::page(&mut conn, &builtin_manifests(), "", "", now, 1, Some(kind))
        .await?
        .pop()
        .ok_or_else(ApiError::not_found)?;
    let row = normalized(row);
    let programs = registry(&row);
    let shipped = super::catalogue::release_builds(&payload_dir(&ctx.app.config)).await;
    let trust = crate::harness_channels::trusts_mirror(&mut conn).await?;
    let platforms = rows::platforms(
        &mut conn,
        &selected(std::slice::from_ref(&row)),
        &json!(shipped),
        trust,
        now,
    )
    .await?;
    let versions = system::<Json>(
        &mut conn,
        orch_settings_contract::registry::keys::AGENTS_HARNESS_VERSIONS,
    )
    .await?
    .unwrap_or(json!({}));
    let hours = system::<i64>(
        &mut conn,
        orch_settings_contract::registry::keys::AGENTS_HARNESS_CHANNEL_HOURS,
    )
    .await?
    .unwrap_or_else(channel_hours_default);
    let state = read_state(&mut conn).await?;
    // These controls edit system policy. Its status is scoped to this program; it never resolves every environment.
    let choice = versions
        .get(kind)
        .and_then(|v| v.get("use"))
        .and_then(Json::as_str)
        .unwrap_or("shipped");
    let wanted = matches!(choice, "newest" | "newest_checked");
    let channels = HarnessChannelsStatus {
        last_read_at: state
            .get("last_read_at")
            .cloned()
            .filter(|v| !v.is_null())
            .map(serde_json::from_value)
            .transpose()
            .map_err(ApiError::internal)?,
        interval_hours: hours,
        requested: if wanted {
            vec![kind.to_owned()]
        } else {
            vec![]
        },
        reason: if hours <= 0 {
            HarnessChannelSchedule::Disabled
        } else if choice == "exact" {
            HarnessChannelSchedule::Pinned
        } else if !wanted {
            HarnessChannelSchedule::Shipped
        } else if programs
            .program(kind)
            .is_none_or(|p| p.builds.channels.is_empty())
        {
            HarnessChannelSchedule::NoChannel
        } else {
            HarnessChannelSchedule::Newer
        },
    };
    let expected = expected(&programs, kind);
    let shipped_json = json!(shipped);
    let selection = BuildSelection {
        kind,
        shipped: &shipped_json,
        trust_mirror: trust,
        expected: expected.as_deref(),
        cutoff: now,
        os: None,
        arch: None,
        status: None,
    };
    let descriptor_version = row
        .descriptor
        .as_ref()
        .and_then(|d| d.pointer("/harness/version"))
        .and_then(Json::as_str);
    let mut off = Vec::new();
    if let Some(version) = descriptor_version {
        // A declaration describes one exact build, not whichever newer version happens to exist.
        off = rows::builds_of_version(&mut conn, &selection, version)
            .await?
            .iter()
            .flat_map(|b| {
                b.payload
                    .pointer("/check/off")
                    .and_then(Json::as_array)
                    .into_iter()
                    .flatten()
            })
            .filter_map(Json::as_str)
            .map(str::to_owned)
            .collect();
    }
    let effective = row.descriptor.clone().and_then(|d| {
        serde_json::from_value::<orch_agent_contract::descriptor::AgentDescriptor>(d).ok()
    });
    let summary = summary(&row, &platforms, &shipped, &state, hours)?;
    let mirror = system::<String>(
        &mut conn,
        orch_settings_contract::registry::keys::AGENTS_HARNESS_MIRROR,
    )
    .await?;
    let neutral = effective
        .as_ref()
        .is_some_and(|d| d.capabilities.import_neutral);
    let mut resolved_builds = Vec::new();
    for platform in &summary.platforms {
        let version = match choice {
            "exact" => versions
                .get(kind)
                .and_then(|v| v.get("version"))
                .and_then(Json::as_str),
            "newest" => platform.newest.as_deref(),
            "newest_checked" => platform
                .newest_checked
                .as_deref()
                .or(platform.default_version.as_deref()),
            _ => platform.shipped.as_deref(),
        };
        if let Some(version) = version {
            let candidates = rows::builds_of_version(&mut conn, &selection, version).await?;
            for candidate in candidates {
                let mut build: HarnessBuild =
                    serde_json::from_value(candidate.payload).map_err(ApiError::internal)?;
                if build.os == platform.os && build.arch == platform.arch {
                    let off = build
                        .check
                        .as_ref()
                        .map(|c| c.off.as_slice())
                        .unwrap_or(&[]);
                    build.carry = Some(crate::programs::carry_of(neutral, off));
                    build.url =
                        crate::harness_channels::download_url(&programs, &build, mirror.as_deref());
                    resolved_builds.push(build);
                    break;
                }
            }
        }
    }
    Ok(ProgramDetail {
        summary,
        resolved_builds,
        program: programs
            .program(kind)
            .map(|p| crate::programs::program_info(&programs, p, effective.as_ref())),
        capabilities: capabilities(row.descriptor.as_ref(), &off),
        descriptor: row.descriptor,
        versions,
        channels,
    })
}
async fn builds(
    ctx: Ctx,
    query: AdminProgramBuildListQuery,
    (): (),
) -> ApiResult<Page<HarnessBuild>> {
    admin_of(&ctx)?;
    let kind = ctx.param("kind")?;
    let scope =
        json!({"list":"builds","kind":kind,"os":query.os,"arch":query.arch,"status":query.status});
    let mut conn = ctx.app.db.acquire().await?;
    let now = rows::snapshot(&mut conn).await?;
    let mut cursor = decode(query.cursor.as_deref(), scope, now)?;
    let size = page_size(query.limit);
    let row = rows::page(&mut conn, &builtin_manifests(), "", "", now, 1, Some(kind))
        .await?
        .pop()
        .ok_or_else(ApiError::not_found)?;
    let row = normalized(row);
    let programs = registry(&row);
    let expected = expected(&programs, kind);
    let shipped = json!(super::catalogue::release_builds(&payload_dir(&ctx.app.config)).await);
    let word = |v: Json| v.as_str().map(str::to_owned);
    let os = query.os.and_then(|v| word(json!(v)));
    let arch = query.arch.and_then(|v| word(json!(v)));
    let status = query.status.and_then(|v| word(json!(v)));
    let selection = BuildSelection {
        kind,
        shipped: &shipped,
        trust_mirror: crate::harness_channels::trusts_mirror(&mut conn).await?,
        expected: expected.as_deref(),
        cutoff: cursor.cutoff,
        os: os.as_deref(),
        arch: arch.as_deref(),
        status: status.as_deref(),
    };
    let mut found = rows::builds(
        &mut conn,
        &selection,
        cursor.build.as_ref(),
        read_ahead(size),
    )
    .await?;
    let more = found.len() > size;
    found.truncate(size);
    cursor.build = found.last().map(rows::BuildRow::position);
    let mirror = system::<String>(
        &mut conn,
        orch_settings_contract::registry::keys::AGENTS_HARNESS_MIRROR,
    )
    .await?;
    let neutral = row
        .descriptor
        .as_ref()
        .and_then(|d| d.pointer("/capabilities/import_neutral"))
        .and_then(Json::as_bool)
        .unwrap_or(false);
    let items = found
        .into_iter()
        .map(|row| {
            let mut build: HarnessBuild =
                serde_json::from_value(row.payload).map_err(ApiError::internal)?;
            let off = build
                .check
                .as_ref()
                .map(|c| c.off.as_slice())
                .unwrap_or(&[]);
            build.carry = Some(crate::programs::carry_of(neutral, off));
            build.url = crate::harness_channels::download_url(&programs, &build, mirror.as_deref());
            Ok(build)
        })
        .collect::<ApiResult<Vec<_>>>()?;
    Ok(Page {
        items,
        next_cursor: if more { Some(encode(&cursor)?) } else { None },
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn a_cursor_binds_the_list_search_program_and_filters() {
        let now = Utc::now();
        let scope = json!({"list":"builds","kind":"third","os":"linux"});
        let cursor = Cursor {
            scope: scope.clone(),
            cutoff: now,
            kind: None,
            build: Some(BuildPosition {
                order: "1".into(),
                version: "1.0.0".into(),
                os: "linux".into(),
                arch: "x86_64".into(),
            }),
        };
        let encoded = encode(&cursor).unwrap();
        assert!(decode(Some(&encoded), scope, now).is_ok());
        assert!(decode(Some(&encoded), json!({"list":"programs"}), now).is_err());
        assert!(decode(Some("not base64!"), Json::Null, now).is_err());
    }
    #[test]
    fn missing_declarations_are_not_reported_and_checks_cannot_turn_a_capability_back_on() {
        assert!(capabilities(None, &[]).is_none());
        let descriptor = json!({"capabilities":{"steer":true,"resume":false,"usage_context":"size_only","approvals":["user_input"]},"sandbox_modes":["open"]});
        let groups = capabilities(Some(&descriptor), &["steer".into()]).unwrap();
        let get = |id: &str| {
            groups
                .iter()
                .flat_map(|g| &g.items)
                .find(|c| c.id == id)
                .unwrap()
        };
        assert!(!get("steer").available);
        assert!(get("steer").reason.is_some());
        assert!(!get("resume").available);
        assert!(get("questions").available);
        assert!(get("usage_context").available);
    }
}
