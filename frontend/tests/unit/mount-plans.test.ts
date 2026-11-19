/** Mount plan sentences tell people the mode and losses before they confirm a fork or move. */
import { describe, expect, it } from 'vitest';

import { copyLines, filesLines, mcpLines, mountLines } from '@/features/change/plan';
import type { FilesPlan, PlanMount } from '@/generated/api-types';
import { i18n } from '@/i18n';

describe('mounts in a plan', () => {
  const t = i18n.t.bind(i18n);
  const reference = { id: 'mount-1', name: 'dataset', address: 'team.dataset' };
  const row = (mount: Pick<PlanMount, 'action' | 'path' | 'mode' | 'detached_because'> & Partial<PlanMount>): PlanMount => ({
    mount: reference, fate: 'as_is', default_fate: 'as_is', fates: ['detach', 'as_is', 'copy'], copy_unavailable: null, copy: null, data_kind: null, fingerprints_available: false, fingerprints: null, ...mount,
  });

  it('never presents an unfinished target count as an empty location', () => {
    const mount = row({ action: 'keep', path: '/mnt/data', mode: 'read_only', detached_because: null,
      fate: 'copy', copy: { measured: false, file_count: 0, total_bytes: 0, measurement: { job_id: 'source-count', measured_at: null },
        location: { made: false, kind: 'volume', source: null, storage: null }, into: null, replace: true,
        holds: { measured: false, file_count: 0, total_bytes: 0, measurement: { job_id: 'target-count', measured_at: null } } } });
    const lines = copyLines([mount], 'en', t);
    expect(lines.every((line) => !line.text.includes('0 files'))).toBe(true);
    expect(lines.map((line) => line.size?.reference.job_id)).toEqual(['source-count', 'target-count']);
    expect(lines.every((line) => line.size?.measured === false)).toBe(true);
  });

  it('says every attachment mode and the reason for every detachment', () => {
    for (const [mode, words] of [['read_only', 'read-only'], ['private', 'private view'], ['shared', 'shared and writable']] as const) {
      const line = mountLines([row({ action: 'keep', path: '/mnt/data', mode, detached_because: null })], t)[0];
      expect(line?.text).toBe(`dataset stays attached at /mnt/data (${words})`);
    }
    for (const reason of ['no_location', 'driver', 'refused', 'asked'] as const) {
      const mount = row({ action: 'detach', path: null, mode: null, detached_because: reason });
      expect(mountLines([mount], t)[0]).toMatchObject({ tone: 'warning', text: `dataset will be detached: ${t(`mountPlan.detachReasons.${reason}`)}` });
    }
  });

  it('says whether private changes travel and why they stay behind', () => {
    const files: FilesPlan = { source: 'live', source_as_of: '2026-10-14T10:00:00Z', file_count: 0, total_bytes: 0,
      bytes_to_upload: 0, bytes_to_download: 0, missing: { count: 0, bytes: 0, sample: [] }, path_map: { from: '/workspace', to: '/workspace' }, home: null, file_states: null,
      private_views: [{ mount: reference, measured: true, file_count: 1, total_bytes: 30, carried: true, left_because: null }] };
    expect(filesLines(files, 'en', t).find((line) => line.id === 'view-mount-1')?.text).toBe('Private changes in dataset go along');
    expect(filesLines(files, 'en', t).find((line) => line.id === 'view-size-mount-1')?.text).toContain('1 files, 30 B');
    files.private_views[0] = { ...files.private_views[0]!, carried: false, left_because: 'other_mode' };
    expect(filesLines(files, 'en', t).find((line) => line.id === 'view-mount-1')).toMatchObject({ tone: 'warning', text: 'Private changes in dataset stay behind: the target attaches this mount in another mode' });
    files.private_views[0] = { ...files.private_views[0], measured: false, file_count: 0, total_bytes: 0 };
    expect(filesLines(files, 'en', t).find((line) => line.id === 'view-size-mount-1')?.text).toBe('The old host could not measure private changes in dataset');
  });

  it('shows mandatory recorded states when the other home contents stay behind and refuses an unmeasured store', () => {
    const files: FilesPlan = { source: 'live', source_as_of: '2026-10-14T10:00:00Z', file_count: 0, total_bytes: 0,
      bytes_to_upload: 4096, bytes_to_download: 4096, missing: { count: 0, bytes: 0, sample: [] }, path_map: { from: '/workspace', to: '/workspace' },
      home: null, private_views: [], file_states: { roots: 2, objects: 3, total_bytes: 4096, journals: 1, complete: true } };
    expect(filesLines(files, 'en', t).find((line) => line.id === 'file-states')).toMatchObject({ tone: 'plain', text: 'Recorded file states always go along: 2 saved roots, 4 KiB' });
    files.file_states = { ...files.file_states!, complete: false };
    expect(filesLines(files, 'en', t).find((line) => line.id === 'file-states')).toMatchObject({ tone: 'warning', text: 'The recorded file-state store could not be measured; this move cannot start' });
  });

  it('says what a copy carries, where it lands and what the person still has to choose there', () => {
    const copy = { measured: true, file_count: 3, total_bytes: 8192, location: { made: true, kind: 'volume' as const, source: 'orch-mount-1-abc', storage: null }, holds: null, into: null, replace: false };
    expect(copyLines([row({ action: 'keep', path: '/mnt/data', mode: 'read_only', detached_because: null, fate: 'copy', copy })], 'en', t)).toEqual([
      { id: 'copy-mount-1', tone: 'plain', text: 'The data of dataset is copied (3 files, 8 KiB) into a new location, orch-mount-1-abc', resources: [{ label: 'dataset', subject: { kind: 'mount', mount: reference } }] },
    ]);
    const onData = { ...copy, location: { ...copy.location, made: false }, holds: { measured: true, file_count: 2, total_bytes: 2048 }, replace: true };
    expect(copyLines([row({ action: 'keep', path: '/mnt/data', mode: 'read_only', detached_because: null, fate: 'copy', copy: onData })], 'en', t)[1]).toMatchObject({ tone: 'warning', text: 'dataset already has data there (2 files, 2 KiB): choose Replace or Keep both' });
    expect(copyLines([row({ action: 'keep', path: '/mnt/data', mode: 'read_only', detached_because: null, fate: 'copy', copy: { ...onData, into: 'replace' } })], 'en', t)[1]).toMatchObject({ tone: 'plain', text: 'What is there now (2 files, 2 KiB) is kept until it is purged' });
    expect(copyLines([row({ action: 'keep', path: '/mnt/data', mode: 'read_only', detached_because: null })], 'en', t)).toEqual([]);
  });

  it('says what each MCP server becomes there, with why where it is lost or cannot start', () => {
    const lines = mcpLines([
      { id: 'a', name: 'tmux', own: false, imported: false, fate: 'new', reason: null },
      { id: 'b', name: 'local-db', own: false, imported: true, fate: 'lost', reason: 'its imported definition belongs to another machine' },
    ], t);
    expect(lines[0]).toMatchObject({ tone: 'plain', text: 'The MCP server tmux is there too' });
    expect(lines[1]).toMatchObject({ tone: 'warning', text: 'The MCP server local-db will not be there: its imported definition belongs to another machine' });
  });
});
