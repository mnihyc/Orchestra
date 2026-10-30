/** Complete, readable native rows used by enrollment's cache and screen proofs. */
import type { Host, Machine } from '../../src/generated/api-types';

export function enrollmentMachine(id: string, online: boolean): Machine {
  return {
    id, name: 'build-vm', online, platform_os: 'linux', placement: 'connected',
    access: { permissions: ['machine.view', 'machine.manage'], administers: false, through: [{ kind: 'owner' }], admin_only: [], participating: true },
    owner: null, platform_arch: 'x86_64', connector_version: '1.0.0', protocol_version: 1,
    roles: ['environment', 'agent'], status: 'active', last_seen_at: null, last_ip: null,
    cert_not_after: '2030-01-01T00:00:00Z', facts: {}, providers: [], agents: [], environments: [], environment_sandboxes: [],
  };
}

export function enrollmentHost(id: string, online: boolean): Host {
  return {
    id, name: 'build-node', address: 'build-node', owner: null, org_id: null, home: null,
    machine: { id: 'node-machine-id', name: 'build-node', online, platform_os: 'linux', placement: 'connected' },
    labels: [], drivers: [], ports: null, ports_revision: 1, ports_proposal: null,
    capacity: { cpu_millis: 16_000, memory_mb: 65_536, disk_gb: 1_000 },
    declared: { cpu_millis: 0, memory_mb: 0, disk_gb: 0 }, schedulable: true,
    when_full: { own: false, value: 'warn' }, storages: [], running: { environment: 0, provider: 0 }, mine: 0, revision: 1,
    access: { permissions: ['host.view', 'host.manage'], administers: false, through: [], admin_only: [], participating: true },
  };
}
