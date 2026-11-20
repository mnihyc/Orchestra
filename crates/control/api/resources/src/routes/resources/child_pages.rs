//! Child directories page only an already admitted native resource projection.
use crate::middleware::requests::{ApiError, ApiResult};
use orch_api_contract::common::ChildCollectionQuery;
use orch_api_contract::mounts::Mount;
use orch_api_contract::resources::{Host, Machine};

pub(super) trait Children: Sized {
    fn child_page(self, query: &ChildCollectionQuery, source: &str) -> ApiResult<Self>;
}

fn chosen(query: &ChildCollectionQuery) -> ApiResult<Option<&str>> {
    if query.collection.is_none()
        && (query.q.is_some() || query.limit.is_some() || query.cursor.is_some())
    {
        return Err(ApiError::invalid(
            "collection",
            "choose the child directory to read",
        ));
    }
    Ok(query.collection.as_deref())
}
fn wrong() -> ApiError {
    ApiError::invalid("collection", "is not a child directory of this resource")
}

impl Children for Machine {
    fn child_page(mut self, query: &ChildCollectionQuery, source: &str) -> ApiResult<Self> {
        let Some(kind) = chosen(query)? else {
            return Ok(self);
        };
        let query = query.page();
        let source = format!("{source}:{kind}");
        macro_rules! directory {
            ($field:ident) => {{
                let page = crate::collection::snapshot(
                    &query,
                    &source,
                    self.$field,
                    |row| (row.name.to_lowercase(), row.id),
                    |row| row.name.clone(),
                    false,
                )?;
                self.$field = page.items;
                self.next_cursor = page.next_cursor;
            }};
        }
        match kind {
            "agents" => directory!(agents),
            "providers" => directory!(providers),
            "environments" => directory!(environments),
            _ => return Err(wrong()),
        }
        if kind != "agents" {
            self.agents.clear();
        }
        if kind != "providers" {
            self.providers.clear();
        }
        if kind != "environments" {
            self.environments.clear();
        }
        self.environment_sandboxes.clear();
        Ok(self)
    }
}
impl Children for Host {
    fn child_page(mut self, query: &ChildCollectionQuery, source: &str) -> ApiResult<Self> {
        let Some(kind) = chosen(query)? else {
            return Ok(self);
        };
        let query = query.page();
        let source = format!("{source}:{kind}");
        match kind {
            "kept_layers" => {
                let page = crate::collection::snapshot(
                    &query,
                    &source,
                    self.kept_layers,
                    |row| {
                        (
                            row.environment
                                .as_ref()
                                .map(|value| value.name.to_lowercase())
                                .unwrap_or_default(),
                            row.environment_id,
                            row.generation.saturating_neg(),
                        )
                    },
                    |row| {
                        row.environment
                            .as_ref()
                            .map(|value| value.name.clone())
                            .unwrap_or_default()
                    },
                    false,
                )?;
                self.kept_layers = page.items;
                self.next_cursor = page.next_cursor;
            }
            "kept_data" => {
                let page = crate::collection::snapshot(
                    &query,
                    &source,
                    self.kept_data,
                    |row| {
                        (
                            row.name.as_deref().unwrap_or("").to_lowercase(),
                            row.environment_id,
                        )
                    },
                    |row| {
                        format!(
                            "{} {} {}",
                            row.name.as_deref().unwrap_or(""),
                            row.home
                                .as_ref()
                                .filter(|value| !value.redacted)
                                .and_then(|value| value.name.as_deref())
                                .unwrap_or(""),
                            row.storages
                                .iter()
                                .filter_map(|value| value.as_deref())
                                .collect::<Vec<_>>()
                                .join(" ")
                        )
                    },
                    false,
                )?;
                self.kept_data = page.items;
                self.next_cursor = page.next_cursor;
            }
            _ => return Err(wrong()),
        }
        if kind != "kept_data" {
            self.kept_data.clear();
        }
        if kind != "kept_layers" {
            self.kept_layers.clear();
        }
        Ok(self)
    }
}
impl Children for Mount {
    fn child_page(mut self, query: &ChildCollectionQuery, source: &str) -> ApiResult<Self> {
        let Some(kind) = chosen(query)? else {
            return Ok(self);
        };
        let query = query.page();
        let source = format!("{source}:{kind}");
        match kind {
            "locations" => {
                let page = crate::collection::snapshot(
                    &query,
                    &source,
                    self.locations,
                    |row| (row.machine.name.to_lowercase(), row.machine.id),
                    |row| {
                        format!(
                            "{} {} {} {} {}",
                            row.machine.name,
                            row.host
                                .as_ref()
                                .map(|value| value.name.as_str())
                                .unwrap_or(""),
                            row.source.as_deref().unwrap_or(""),
                            row.storage.as_deref().unwrap_or(""),
                            row.created_by.display_name
                        )
                    },
                    false,
                )?;
                self.locations = page.items;
                self.next_cursor = page.next_cursor;
            }
            "kept" => {
                let page = crate::collection::snapshot(
                    &query,
                    &source,
                    self.kept,
                    |row| {
                        (
                            row.kept_at.get().timestamp_micros().saturating_neg(),
                            row.id,
                        )
                    },
                    |row| {
                        format!(
                            "{} {} {} {} {}",
                            row.machine.name,
                            row.host
                                .as_ref()
                                .map(|value| value.name.as_str())
                                .unwrap_or(""),
                            row.source.as_deref().unwrap_or(""),
                            row.storage.as_deref().unwrap_or(""),
                            row.kept_by.display_name
                        )
                    },
                    false,
                )?;
                self.kept = page.items;
                self.next_cursor = page.next_cursor;
            }
            "lost_locations" => {
                let page = crate::collection::snapshot(
                    &query,
                    &source,
                    self.lost_locations,
                    |row| {
                        (
                            row.lost_at.get().timestamp_micros().saturating_neg(),
                            row.id,
                        )
                    },
                    |row| {
                        format!(
                            "{} {} {}",
                            row.machine_name,
                            row.host_name.as_deref().unwrap_or(""),
                            row.storage.as_deref().unwrap_or("")
                        )
                    },
                    false,
                )?;
                self.lost_locations = page.items;
                self.next_cursor = page.next_cursor;
            }
            _ => return Err(wrong()),
        }
        if kind != "locations" {
            self.locations.clear();
        }
        if kind != "kept" {
            self.kept.clear();
        }
        if kind != "lost_locations" {
            self.lost_locations.clear();
        }
        Ok(self)
    }
}
