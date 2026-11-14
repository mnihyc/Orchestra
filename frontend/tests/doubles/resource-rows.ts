/** Table facts use distinct wire rows; full records are read only by their own readers. */
import type { EverythingResource, ResourceListRow } from '../../src/generated/api-types.ts';

function pick<T, Key extends keyof T>(row: T, keys: readonly Key[]): Pick<T, Key> {
  return Object.fromEntries(keys.map((key) => [key, row[key]])) as Pick<T, Key>;
}
const object = (value: unknown): Record<string, unknown> => typeof value === 'object' && value !== null && !Array.isArray(value) ? value as Record<string, unknown> : {};

export function resourceListRow(resource: EverythingResource): ResourceListRow {
  switch (resource.kind) {
    case 'agent': {
      const row = resource.value;
      const value = { ...pick(row, ["id", "name", "address", "harness_kind", "harness", "machine", "disabled", "owner", "home", "profile", "revision", "access" , "org_id"]), list_row: true };
      value.profile = { harness_version: object(row.profile).harness_version };
      return { kind: resource.kind, value };
    }
    case 'provider': {
      const row = resource.value;
      const value = { ...pick(row, ["id", "name", "kind", "harness_kind", "harness", "address", "placement", "status", "owner", "home", "key_set", "key_machine", "machine", "status_detail", "status_at", "facts", "facts_state", "models_count", "running_threads", "runtime", "runtime_support", "revision", "access", "choice_usage" , "org_id"]), list_row: true };
      const plan = row.facts.plan;
      return { kind: resource.kind, value: { ...value,
        runtime: row.runtime === null ? null : pick(row.runtime, ['state', 'host', 'updates', 'update_required']),
        facts: { ...value.facts, spend: [], extra_usage: [], available_resets: [] },
        choice_usage: value.choice_usage == null ? value.choice_usage : { ...value.choice_usage, performance: [] },
        enabled_models_count: row.models.filter((model) => model.enabled).length,
        upstreams_count: row.upstreams.length,
        no_key_needed: row.upstreams.length > 0 && row.upstreams.every((entry) => object(entry.options).auth === 'none'),
        plan_label: plan === null ? null : row.sources.facts.descriptor?.facts?.plans[plan] ?? plan,
        source_consent: [row.sources.facts, row.sources.models, row.sources.presenter].some((source) => source?.address != null),
      } };
    }
    case 'environment': {
      const row = resource.value;
      const value = { ...pick(row, ["activity", "ports_exposed", "id", "name", "owner", "address", "placement", "status", "machine", "home", "threads_hidden", "workspace_root", "template", "host", "updates", "update_required", "status_detail", "job", "temporary", "connector_update", "removal", "remove_at", "last_activity_at", "can_declare_lost", "access" , "org_id"]), list_row: true };
      return { kind: resource.kind, value: { ...value, threads_count: row.threads.length, topics_count: new Set(row.threads.map((entry) => entry.topic.id)).size,
        start_allowed: !(row.network.required_mode === 'restricted' && row.network.mode === 'open') } };
    }
    case 'host': {
      const row = resource.value;
      const value = { ...pick(row, ["id", "name", "address", "owner", "home", "machine", "labels", "drivers", "capacity", "declared", "schedulable", "running", "revision", "access", "lost_at", "full", "usage", "offline_since", "software_update", "storages" , "org_id"]), list_row: true };
      return { kind: resource.kind, value };
    }
    case 'machine': {
      const row = resource.value;
      const value = { ...pick(row, ["id", "name", "online", "platform_os", "placement", "revision", "owner", "platform_arch", "connector_version", "roles", "status", "last_seen_at", "cert_not_after", "access", "host", "lost_at", "software_update", "role_management"]), list_row: true };
      return { kind: resource.kind, value };
    }
    case 'template': {
      const row = resource.value;
      const value = { ...pick(row, ["id", "name", "owner", "home", "address", "kind", "driver", "spec", "units", "enabled", "revision", "access" , "org_id"]), list_row: true };
      const spec = object(row.spec);
      value.spec = { image: spec.image, runtime: spec.runtime, network: { mode: object(spec.network).mode }, security: { read_only_rootfs: object(spec.security).read_only_rootfs, no_new_privileges: object(spec.security).no_new_privileges },
        ...(typeof spec.build === 'object' && spec.build !== null ? { build: {} } : {}), ...(typeof spec.archive === 'object' && spec.archive !== null ? { archive: {} } : {}) };
      return { kind: resource.kind, value };
    }
    case 'mcp_server': {
      const row = resource.value;
      const value = { ...pick(row, ["id", "name", "description", "owner", "home", "address", "machine", "environment", "imported", "spec", "enabled", "revision", "access" , "org_id"]), list_row: true };
      value.spec = { transport: object(row.spec).transport };
      return { kind: resource.kind, value };
    }
    case 'mount': {
      const row = resource.value;
      const value = { ...pick(row, ["id", "name", "address", "description", "home", "owner", "target", "read_only", "enabled", "locations", "locations_hidden", "revision", "access" , "org_id"]), list_row: true };
      return { kind: resource.kind, value };
    }
    case 'skill': {
      const row = resource.value;
      const value = { ...pick(row, ["id", "name", "description", "owner", "home", "address", "enabled", "latest", "revision", "access" , "org_id"]), list_row: true };
      return { kind: resource.kind, value };
    }
  }
}
