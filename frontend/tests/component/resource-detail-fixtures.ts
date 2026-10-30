/** Typed resource projections for detail-page composition; operations are proved by their native suites. */
import type { Agent, Environment, Host, Machine, McpServer, Mount, Provider, ResourceAccess, Template } from '@/generated/api-types';
import type { Schema } from '@/features/resources/templates/schema';
import { ADA } from '../doubles/chat-rows';
import { planProvider } from '../unit/provider-plan-fixtures';
import { harnessName } from '../doubles/harness-names';

export const detailAccess: ResourceAccess = { permissions: ['agent.view', 'agent.manage', 'environment.view', 'environment.manage', 'host.view', 'host.manage', 'machine.view', 'machine.manage', 'template.view', 'template.manage', 'mcp.view', 'mcp.manage', 'mount.view', 'mount.manage'], administers: false, participating: true, admin_only: [], through: [{ kind: 'owner' }] };
export const detailHome = { id: 'team', kind: 'org' as const, name: 'Workshop', redacted: false };
export const detailMachineRef = { id: 'machine', name: 'Build machine', placement: 'managed' as const, online: true, platform_os: 'linux' as const };
export const detailSchema: Schema = { type: 'object', properties: {
  image: { type: 'string', title: 'Image reference' },
  resources: { type: 'object', title: 'Limits', properties: Object.fromEntries(Array.from({ length: 27 }, (_, index) => [`field_${index}`, { type: 'integer', title: `Worker limit ${index + 1}`, default: index + 1 }])) },
}, additionalProperties: true };

export const detailAgent: Agent = { id: 'agent', name: 'Research profile', address: 'workshop.research', home: detailHome, org_id: 'team', owner: ADA,
  machine: null, harness_kind: 'codex', harness: harnessName('codex'), profile: { effort: 'low' }, disabled: false, revision: 1, access: detailAccess,
  created_at: '2026-10-15T12:00:00Z', updated_at: '2026-10-15T12:00:00Z', descriptor_overridden: [],
  descriptor: { harness: { name: 'Research harness', status: 'supported', version: '1.2.3', supported_range: '>=1' },
    capabilities: { diagnostic_canary: 'diagnostic-canary' }, parameters: Array.from({ length: 27 }, (_, index) => ({ key: `worker_field_${index}`, type: 'integer', title: `Worker option ${index + 1}`, default: index + 1, scopes: ['org', 'project', 'topic', 'thread'], description: 'An adapter-declared option.' })) } };

export const detailMachine: Machine = { ...detailMachineRef, owner: ADA, platform_arch: 'x86_64', connector_version: '1.2.3', protocol_version: 1,
  roles: ['environment', 'agent', 'provider', 'node'], status: 'active', last_seen_at: null, last_ip: null, cert_not_after: '2027-10-15T12:00:00Z',
  providers: [], agents: [], environments: [], environment_sandboxes: [], access: detailAccess,
  facts: { diagnostic_canary: 'diagnostic-canary', hostname: 'build-system' } };

export const detailHost: Host = { id: 'host', name: 'Build host', address: 'workshop.builder', home: detailHome, org_id: 'team', owner: ADA,
  machine: detailMachineRef, capacity: { cpu_millis: 8000, memory_mb: 16384, disk_gb: 200 }, declared: { cpu_millis: 1000, memory_mb: 2048, disk_gb: 20 },
  labels: ['linux'], running: { environment: 1, provider: 1 }, mine: 1, schedulable: true, revision: 1, access: detailAccess,
  drivers: [{ name: 'sample', title: 'Example driver', creates: ['environment', 'provider'], runtime_classes: [] }], storages: [],
  ports: null, ports_proposal: null, ports_revision: 0, when_full: { value: 'warn', own: false } };

export const detailEnvironment: Environment = { id: 'environment', name: 'Research workspace', address: 'workshop.workspace', home: detailHome, org_id: 'team', owner: ADA, access: detailAccess,
  placement: 'managed', machine: detailMachineRef, host: { id: detailHost.id, name: detailHost.name, address: detailHost.address }, template: null,
  status: 'running', status_detail: null, job: null, generation: 1, revision: 1, seq: 1, config: {}, config_trust: [], default_for: [],
  workspace_root: '/workspace', workspace_storage: null, remote_write: true, terminal: true, browser: false, vnc: false,
  reach: 0, threads: [], threads_hidden: 0, mounts: [], mcp_found: [], exec: { machine_offers: [], offers: [], orch_tools: false },
  facts: { diagnostic_canary: 'diagnostic-canary' }, granted_capabilities: [], index: 'full', index_reason: null, manifest_at: null,
  idle_stop_at: null, last_activity_at: null, unused_since: null, remove_at: null, remove_reason: null, removal: null,
  update_required: false, updates: [], ports_exposed: false,
  support: { looked_at: null, browser: { programs: [], window: false, playwright_cli: null, sandbox: null }, vnc: { servers: [], window_manager: null } },
  network: { mode: 'open', required_mode: 'open', network_mode: 'bridge', proxy_url: null, allowed_hosts: [], sources: [], networks: [], revision: '1', always: [], always_revision: '1', always_source: 'orch_configuration', state: 'ready', reason: null, last_denied: null } };

export const detailTemplate: Template = { id: 'template', name: 'Research runtime', address: 'workshop.runtime', home: detailHome, org_id: 'team', owner: ADA,
  driver: 'sample', kind: 'environment', enabled: true, units: 0, description: 'A runtime for research work.', spec: { image: 'research:stable', extension: { keep: true } }, revision: 1, access: detailAccess };
export const detailMcp: McpServer = { id: 'mcp', name: 'research-tools', address: 'workshop.tools', home: detailHome, org_id: 'team', owner: ADA,
  description: 'Tools for research.', machine: null, environment: null, imported: null, spec: { transport: 'stdio', command: ['research-tools'], diagnostic_canary: 'diagnostic-canary' }, enabled: true, revision: 1, access: detailAccess };
export const detailMount: Mount = { id: 'mount', name: 'research-data', address: 'workshop.data', home: detailHome, org_id: 'team', owner: ADA,
  description: 'Shared research data.', target: '/mnt/research', read_only: true, enabled: true, default_fate: 'as_is', locations: [], locations_hidden: 0, kept: [], revision: 1, access: detailAccess };

/** A ready model connection whose node uses an opaque connector name; people see its actual host. */
const unitProviderBase = planProvider();
export const detailUnitProvider: Provider = planProvider({ id: 'unit-provider', name: 'Research model connection', kind: 'api', key_set: true,
  facts: { ...unitProviderBase.facts, credential: null }, facts_state: { ...unitProviderBase.facts_state, credential: 'empty' },
  upstreams: [{ protocol: 'openai_responses', base_url: 'https://models.example.test/v1', options: {} }],
  machine: { ...detailMachineRef, id: 'unit-connector', name: '9fca895b060d' },
  runtime_support: { lifecycle: 'unit', commands_after_start: null, after_up: false },
  runtime: { state: 'running', host: { id: detailHost.id, name: detailHost.name, address: detailHost.address }, driver: 'sample',
    template: { id: detailTemplate.id, name: detailTemplate.name, owner: detailTemplate.owner }, generation: 2,
    network: detailEnvironment.network, updates: [], update_required: false, last_active_at: null, idle_stop_at: null, renew_at: null, job: null } });
