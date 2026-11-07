/** Actual plan facts shared by thread and environment-move resource assertions. */
import type { ForkPlan, PlanMount } from '@/generated/api-types';
import { agent, arrangement, environment, options, provider } from '../unit/start-fixtures';
export function planMount(action: PlanMount['action'], name: string, id = name): PlanMount {
  return { mount: { id, name, address: 'work.owner.data' }, action, mode: action === 'detach' ? null : 'read_only', path: action === 'detach' ? null : '/data',
    copy: null, copy_unavailable: null, data_kind: null, default_fate: 'as_is', detached_because: action === 'detach' ? 'no_location' : null,
    fate: action === 'detach' ? 'detach' : 'as_is', fates: ['as_is', 'detach'], fingerprints: null, fingerprints_available: false };
}
export function retainedResources(): Pick<ForkPlan, 'mounts' | 'mcp'> {
  return { mounts: [planMount('keep', 'Shared data')], mcp: [{ id: 'research', name: 'Research', imported: true, own: false, fate: 'kept', reason: null }] };
}

export function resourcePlan(id: string): ForkPlan {
  const workspace = environment('destination').environment;
  const profile = agent('profile').agent, credential = provider('provider').provider;
  return { ...retainedResources(), preflight_id: `plan-${id}`, result: 'replace', source: { id, kind: 'thread', name: id, redacted: false },
    from: { agent: profile, provider: credential, environment: workspace },
    to: { agent: profile, provider: credential, environment: workspace, arrangement: 'separated', directory: '', model: 'm1', effort: null, isolation: arrangement('separated', 1).isolation, predicted: false },
    access: { differs: [], profile: 'managed', summary: options().access.managed },
    owner: { before: workspace.owner, after: workspace.owner }, files: null, files_at: null, memory: null, leaves: null, worktree: null,
    blockers: [], depends: [], locks: [], turn_running: false, expires_at: '2099-01-01T00:00:00Z',
    estimate: { basis: 'assumed', seconds_low: 1, seconds_high: 2 },
    options: { active_turn: 'wait', wait_minutes: 10, verify_timeout_secs: 60, keep_previous: true, stop_processes: false },
    history: { mode: 'none', method: 'none', archive_bytes: null, source_available: true, turns: 0, report: { fidelity: 'none', items_total: 0, by_kind: {}, dropped: [], warnings: [] } },
  };
}
