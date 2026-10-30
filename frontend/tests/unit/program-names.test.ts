/** Program labels use the supplied language and never derive a name from a kind. */
import { expect, it } from 'vitest';
import { programName, programNameIn, programNamesIndex } from '@/features/resources/programNames';
import type { HarnessKindInfo } from '@/generated/api-types';

it('uses a third program’s localized name and falls back to its declared English name', () => {
  const program = { kind: 'research-agent', name: 'Research assistant', names: { 'zh-CN': '研究助理' } };
  expect(programName(program, 'zh-CN')).toBe('研究助理');
  expect(programName(program, 'en')).toBe('Research assistant');
  expect(programName(program, 'fr')).toBe('Research assistant');
});

it('does not turn an absent name or translation into an identifier', () => {
  expect(programName(null, 'en')).toBeNull();
  expect(programName({ kind: 'private-kind', name: '', names: {} }, 'en')).toBeNull();
  const program = { kind: 'research-agent', name: 'Research assistant', names: { 'zh-CN': '  ' } };
  expect(programName(program, 'zh-CN')).toBe('Research assistant');
});

it('keeps the declared English name when the payload omits an empty translation map', () => {
  const payload = { harness: { kind: 'research-agent', name: 'Research assistant' } };
  expect(programNameIn(payload, 'en')).toBe('Research assistant');
  expect(programNameIn(payload, 'zh-CN')).toBe('Research assistant');
  expect(programNameIn({ harness: { kind: 'private-kind', name: '' } }, 'en')).toBeNull();
});

it('shares one name index across a thousand observers of a hundred thousand programs', () => {
  let reads = 0;
  const programs: HarnessKindInfo[] = Array.from({ length: 100_000 }, (_, index) => ({
    kind: `program-${index}`, title: 'Older title', descriptor: null, builds: [],
    ...{ program: null, harness: { kind: `program-${index}`, get name() { reads += 1; return `Declared program ${index}`; }, names: { 'zh-CN': `声明程序 ${index}` } } },
  }));
  const names = programNamesIndex(programs, 'en');
  const indexedReads = reads;
  expect(indexedReads).toBeGreaterThan(0);
  for (let observer = 0; observer < 1000; observer += 1) {
    expect(programNamesIndex(programs, 'en')).toBe(names);
    expect(names.get('program-99999')).toBe('Declared program 99999');
  }
  expect(reads).toBe(indexedReads);
  expect(programNamesIndex(programs, 'zh-CN').get('program-99999')).toBe('声明程序 99999');
});
