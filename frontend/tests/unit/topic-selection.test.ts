/** Topic range selection, per-row authority and confirmed-success-only inverses stay distinct from idle and uncertain writes. */
import { describe, expect, it } from 'vitest';

import { ApiError } from '@/api/client';
import { confirmedInverses, selectRange, statusBatch, statusBlockers, type StatusResult } from '@/features/project/selection';
import type { Topic } from '@/generated/api-types';

const row = (id: string, permissions = ['topic.interact']): Topic => ({ id, name: id, status: 'open', revision: 4, archived: false, access: { permissions } } as Topic);
const refused = new ApiError({ type: 'orch:error:authz.denied', code: 'authz.denied', title: 'Missing permission', detail: 'topic.interact is required', status: 403, request_id: '' }, null);

describe('loaded topic rows', () => {
  it('Shift selects the loaded range and never invents an unloaded row', () => {
    expect([...selectRange(new Set(['earlier']), ['a', 'b', 'c'], 'a', 'c', true, true)]).toEqual(['earlier', 'a', 'b', 'c']);
    expect([...selectRange(new Set(), ['a', 'b'], 'gone', 'b', true, true)]).toEqual(['b']);
  });

  it('checks each topic’s own permission and lifecycle, rather than a parent role', () => {
    expect(statusBlockers([row('allowed'), row('reader', []), { ...row('archived'), archived: true }], 'done').map((one) => [one.topic.id, one.reason])).toEqual([['reader', 'permission'], ['archived', 'archived']]);
  });

  it('reports confirmed success, positive refusal and lost reply separately', async () => {
    const progress: StatusResult[] = [];
    const result = await statusBatch([row('ok'), row('no'), row('lost')], 'done', (topic, status) => topic.id === 'ok' ? Promise.resolve({ ...topic, status, revision: 5 }) : Promise.reject(topic.id === 'no' ? refused : new Error('The reply was lost')), (one) => progress.push(one));
    expect(result.map((one) => one.state)).toEqual(['succeeded', 'refused', 'uncertain']);
    expect(progress.filter((one) => one.state === 'pending').map((one) => one.before.id)).toEqual(['ok', 'no', 'lost']);
    expect(confirmedInverses(result).map((one) => [one.before.id, one.after.revision])).toEqual([['ok', 5]]);
  });

  it('an observed uncertain write supplies no inverse, even when its status looks right', () => {
    expect(confirmedInverses([{ before: row('lost'), state: 'uncertain', observed: { ...row('lost'), status: 'done', revision: 5 } }])).toEqual([]);
  });
});
