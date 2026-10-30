/** Workspace-only exports keep their own immutable bytes and deliver the same job and Inbox result as the server. */
import { randomUUID } from 'node:crypto';
import type { Job } from '../../src/generated/api-types.ts';
import { zipFilesOf } from './export.ts';
import { userRef } from './fixtures.ts';
import { type Answer, type Person, Refusal, type World } from './world.ts';

export class TemporaryWorld {
  private jobs = new Map<string, { owner: string; job: Job; snapshot: string; bytes: Uint8Array; name: string }>();
  private snapshots = new Map<string, { owner: string; bytes: Uint8Array }>();
  private requests = new Map<string, Job>();
  private world: World;
  constructor(world: World) { this.world = world; }

  handle(person: Person, method: string, path: string, _query: URLSearchParams, _body: Record<string, unknown>, key?: string): Answer | null {
    let found: RegExpExecArray | null;
    if (method === 'POST' && (found = /^\/environments\/([^/]+)\/files\/export$/.exec(path))) {
      const id = found[1] ?? '';
      const resource = this.world.resources.inventoryView(person, 'environment', id);
      if (resource?.kind !== 'environment') throw new Refusal(404, 'not_found', 'there is no such environment');
      const environment = resource.value;
      if (!environment.access.permissions.includes('environment.manage')) throw new Refusal(403, 'authz.denied', 'manage this environment to export its workspace');
      const request = `${person.me.id}:${id}:${key ?? randomUUID()}`;
      const prior = this.requests.get(request);
      if (prior !== undefined) return { status: 202, body: prior };
      const snapshot = randomUUID();
      const at = new Date().toISOString();
      const job: Job = { id: randomUUID(), kind: 'snapshot', node_id: null, environment_id: id, provider_id: null, batch_id: null,
        state: 'running', progress: { phase: 'Copying workspace', percent: 0, phases: [], files_done: 0, files_total: null, bytes_done: 0, bytes_total: null, rate_bps: null, eta_seconds: null, message: null },
        result: null, error: null, log: null, created_by: userRef(person.me), created_at: at, started_at: at, finished_at: null };
      this.jobs.set(job.id, { owner: person.me.username, job, snapshot, bytes: zipFilesOf(this.world.workspace.copyFiles(id)), name: environment.name });
      this.requests.set(request, job);
      return { status: 202, body: job };
    }
    if (method === 'GET' && (found = /^\/jobs\/([^/]+)$/.exec(path))) {
      const held = this.jobs.get(found[1] ?? '');
      if (held === undefined) return null;
      if (held.owner !== person.me.username) throw new Refusal(404, 'not_found', 'there is no such export');
      if (held.job.state === 'running') {
        held.job = { ...held.job, state: 'succeeded', finished_at: new Date().toISOString(), progress: { ...held.job.progress, percent: 100 }, result: { snapshot_id: held.snapshot, missing_count: 0 } };
        this.snapshots.set(held.snapshot, { owner: held.owner, bytes: held.bytes });
        this.world.notifications.add(held.owner, { kind: 'job.finished', title: 'Workspace export finished', url: '/inbox', data: { workspace_export: true, snapshot_id: held.snapshot, subject: held.name, state: 'succeeded' } });
      }
      return { status: 200, body: held.job };
    }
    if (method === 'GET' && (found = /^\/snapshots\/([^/]+)\/archive$/.exec(path))) {
      const held = this.snapshots.get(found[1] ?? '');
      if (held?.owner !== person.me.username) throw new Refusal(404, 'not_found', 'there is no such export');
      return { status: 200, bytes: held.bytes, headers: { 'Content-Type': 'application/zip', 'Content-Disposition': 'attachment; filename="workspace.zip"' } };
    }
    return null;
  }
}
