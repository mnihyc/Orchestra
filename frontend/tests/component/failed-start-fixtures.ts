/** Full fake-world contracts for a failed new environment with its original first input still pending. */
import type { Job, Thread, ThreadInput } from '@/generated/api-types';
import { ACME } from '../doubles/fixtures';
import { threadInput } from '../doubles/thread-rows';
import { World } from '../doubles/world';

export function failedStart(): { thread: Thread; input: ThreadInput; job: Job } {
  const world = new World();
  const person = world.people.get('ada');
  const row = world.places.rowOf(ACME.mainThread);
  if (person === undefined || row === undefined) throw new Error('the fake world must supply Ada and her thread');
  const native = world.threads.thread(person, row);
  if (native.environment === null) throw new Error('the fake thread must have its environment reference');
  const thread: Thread = { ...native, name: 'New thread', state: 'blocked', starting_step: 'environment', lock: null, current_turn_id: null,
    blocked: { reason: 'environment_unavailable', detail: "the environment failed: files: the file job's environment changed", sandbox: null },
    pending_inputs: 1, pending_approvals: 0,
    environment: { ...native.environment, id: 'failed-environment', name: 'Main', status: 'failed' },
    environment_permissions: ['environment.view', 'environment.manage'],
    source: { kind: 'directory', directory: '', repository_url: null, branch: null, head: null, state: 'pending', job: null, reason: null },
    access: { ...native.access, permissions: ['thread.view', 'thread.interact', 'thread.manage'] },
  };
  const input: ThreadInput = { ...threadInput('first-input', 'hello from scenario 39', { author: thread.owner, position: '1' }), thread_id: thread.id, seq_updated: thread.seq };
  const job: Job = { id: 'retry-job', kind: 'environment_provision', environment_id: 'failed-environment', node_id: null, provider_id: null, batch_id: null,
    state: 'queued', result: null, error: null, log: null, created_by: thread.owner, created_at: '2026-10-15T23:00:00Z', started_at: null, finished_at: null,
    progress: { phase: 'files', percent: 0, phases: [], files_done: 0, files_total: null, bytes_done: 0, bytes_total: null, rate_bps: null, eta_seconds: null, message: null } };
  return { thread, input, job };
}
