/** A catalogue build or adapter version never becomes an observation of an installed harness. */
import { describe, expect, it } from 'vitest';
import { profileDetection } from '@/features/resources/agents/profileDetection';
import type { MachineProgram } from '@/generated/api-types';
import { partialDescriptor } from '../doubles/harness-names';
const catalogue = { descriptor: partialDescriptor({ harness: { version: 'catalogue-build' }, arrangements: [{ type: 'key_separated' }] }) };
describe('profile observations', () => {
  it('uses actual installed versions and keeps machine policy separate from program support', () => {
    const report = profileDetection({ harnesses: [{ kind: 'custom', version: '2.3.4' }],
      policy: { 'provider.arrangements': ['separated'] } }, 'custom', catalogue);
    expect(report).toEqual({ version: '2.3.4', source: 'catalogue', arrangements: ['key_separated'], machineArrangements: ['separated'] });
  });
  it('never assumes a catalogue version was installed and preserves an explicit empty policy', () => {
    expect(profileDetection({ harnesses: [{ kind: 'custom', version: 'unknown' }], policy: { 'provider.arrangements': [] } }, 'custom', catalogue))
      .toEqual({ version: null, source: 'catalogue', arrangements: ['key_separated'], machineArrangements: [] });
  });
  it('uses a ready custom harness declaration without confusing its adapter version', () => {
    const program: MachineProgram = { kind: 'agent', name: 'registered', command: ['adapter'], origin: 'orch', ready: true,
      version: 'adapter-99', descriptor: { kind: 'custom', harness: { version: 'harness-2' }, arrangements: [] }, models: null, detail: null };
    expect(profileDetection({ programs: { entries: [program] } }, 'custom', catalogue)).toEqual({ version: 'harness-2', source: 'machine', arrangements: [], machineArrangements: null });
    expect(profileDetection({}, 'custom', catalogue, program).version).toBe('harness-2');
    expect(profileDetection({ programs: { entries: [{ ...program, ready: false }] } }, 'custom', catalogue).version).toBeNull();
  });
  it('reads older validated plugin reports but ignores a failed or unrelated program', () => {
    const descriptor = { kind: 'custom', harness: { version: 'harness-3' }, arrangements: [{ type: 'separated' }, { type: 'future-arrangement' }] };
    expect(profileDetection({ plugins: [{ kind: 'adapter', state: 'running', descriptor }] }, 'custom', undefined))
      .toEqual({ version: 'harness-3', source: 'machine', arrangements: ['separated', 'future-arrangement'], machineArrangements: null });
    expect(profileDetection({ plugins: [{ kind: 'adapter', state: 'failed', descriptor }] }, 'custom', undefined).source).toBeNull();
    expect(profileDetection({ plugins: [{ kind: 'provider_source', descriptor }] }, 'custom', undefined).version).toBeNull();
  });
  it('leaves missing and malformed observations unknown', () => {
    expect(profileDetection(null, 'custom', undefined)).toEqual({ version: null, source: null, arrangements: null, machineArrangements: null });
    expect(profileDetection({ programs: [], harnesses: [null, 'custom'], policy: { 'provider.arrangements': true } }, 'custom', { descriptor: null }).version).toBeNull();
  });
});
