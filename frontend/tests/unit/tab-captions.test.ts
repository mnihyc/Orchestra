// @vitest-environment node
/** Distinct view targets retain real names and receive readable, collision-safe tab captions. */
import { expect, it } from 'vitest';
import { tabCaptions } from '@/features/workbench/tabCaptions';

const instance = (name: string, number: number) => name + ' · View ' + String(number);

it('qualifies matching names with their known kind or readable path and keeps unique names unchanged', () => {
  const names = tabCaptions([
    { id: 'host', title: 'Outside Orch', kind: 'Hosts' },
    { id: 'environment', title: 'Outside Orch', kind: 'Environments' },
    { id: 'research', title: 'General', kind: 'Project', path: 'Acme / Research' },
    { id: 'infra', title: 'General', kind: 'Project', path: 'Acme / Infra' },
    { id: 'unique', title: 'Design notes' },
  ], instance);
  expect([...names.values()]).toEqual(['Outside Orch · Hosts', 'Outside Orch · Environments',
    'General · Acme / Research', 'General · Acme / Infra', 'Design notes']);
});

it('distinguishes saved view instances without stealing another tab’s actual name', () => {
  const entries = [{ id: 'overview', title: 'Environment' }, { id: 'limits', title: 'Environment' }, { id: 'named', title: 'Environment · View 1' }];
  const names = tabCaptions(entries, instance);
  expect(names.get('named')).toBe('Environment · View 1');
  expect(names.get('overview')).toBe('Environment · View 2');
  expect(names.get('limits')).toBe('Environment · View 3');
  expect(new Set(names.values()).size).toBe(entries.length);
  expect(tabCaptions(entries, instance)).toEqual(names);
});

it('keeps real names and terminates when a formatter is missing or repeats its output', () => {
  const entries = [{ id: 'a', title: 'Report' }, { id: 'b', title: 'Report' }, { id: 'c', title: 'Report' }];
  for (const format of [() => 'missing.key', (name: string, number: number) => name + ' ' + String(number % 2)]) {
    const names = tabCaptions(entries, format);
    expect(new Set(names.values()).size).toBe(entries.length);
    expect([...names.values()].every((name) => name.includes('Report'))).toBe(true);
  }
});
