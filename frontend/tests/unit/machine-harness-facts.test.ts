/** Confirmed programs, discoveries and searched-empty reports are distinct native facts. */
import { expect, it } from 'vitest';
import { harnessPolicyDelegated, readMachineFacts, registrableHarnesses } from '@/features/resources/machines/facts';

const confirmed = { kind: 'codex', path: '/opt/codex', version: '0.164.0', daemon: true };
const found = { kind: 'claude-code', path: '/opt/claude', version: 'unknown', daemon: false };

it('keeps confirmed and found programs separate and retains the native unknown version', () => {
  expect(readMachineFacts({ harnesses: [confirmed], discovered_harnesses: [found], searched_harness_kinds: ['codex', 'claude-code'] }))
    .toMatchObject({ harnesses: [confirmed], discoveredHarnesses: [found], searchedHarnessKinds: ['codex', 'claude-code'] });
});

it('does not turn absent discovery into a successful empty search', () => {
  expect(readMachineFacts(null)).toMatchObject({ harnesses: [], discoveredHarnesses: null, searchedHarnessKinds: null });
  expect(readMachineFacts({ harnesses: [], discovered_harnesses: [], searched_harness_kinds: ['codex'] }))
    .toMatchObject({ harnesses: [], discoveredHarnesses: [], searchedHarnessKinds: ['codex'] });
  expect(readMachineFacts({ discovered_harnesses: [], searched_harness_kinds: [] }))
    .toMatchObject({ discoveredHarnesses: [], searchedHarnessKinds: [] });
});

it('rejects malformed discovery containers and entries without inventing a program', () => {
  expect(readMachineFacts({ discovered_harnesses: {}, searched_harness_kinds: 'codex' }))
    .toMatchObject({ discoveredHarnesses: null, searchedHarnessKinds: null });
  expect(readMachineFacts({ discovered_harnesses: [found, { path: '/missing-kind' }, null], searched_harness_kinds: ['codex', 2] }))
    .toMatchObject({ discoveredHarnesses: [found], searchedHarnessKinds: ['codex'] });
});

for (const key of ['agent.harnesses', 'agent.*', '*']) {
  it(`accepts the reported ${key} handover for native programs`, () => {
    expect(harnessPolicyDelegated(readMachineFacts({ policy: { 'policy.delegated': [key] } }))).toBe(true);
  });
}

it('keeps an explicitly local harness value even under wildcard handover', () => {
  expect(harnessPolicyDelegated(readMachineFacts({ policy: { 'policy.delegated': ['*'], 'policy.local': { 'agent.harnesses': [] } } }))).toBe(false);
});

it('does not substitute registry or environment handover for native harness handover', () => {
  for (const policy of [null, {}, { 'policy.delegated': ['agent.programs', 'environment.*'] }]) {
    expect(harnessPolicyDelegated(readMachineFacts({ policy }))).toBe(false);
  }
});

it('does not make a merely found program available to agent profile creation', () => {
  const facts = readMachineFacts({ harnesses: [confirmed], discovered_harnesses: [found], adapters: [{ kind: 'codex' }, { kind: 'claude-code' }] });
  expect(registrableHarnesses(facts).map(({ kind, path }) => ({ kind, path }))).toEqual([{ kind: confirmed.kind, path: confirmed.path }]);
});
