// @vitest-environment node
/** A screen has one named guide entry per article/anchor; distinct sections keep distinct names. */
import { describe, expect, it } from 'vitest';
import { checkGuideEntries } from '@/help/entryCheck';

describe('the guide entries of a screen', () => {
  it('rejects the same article and anchor twice, even when the labels differ', () => {
    expect(checkGuideEntries([{ article: 'resources', heading: 'templates', name: 'Templates guide' }, { article: 'resources', heading: 'templates', name: 'Read more' }])).toEqual(['help.destination_repeated: resources#templates']);
  });
  it('rejects two different articles with one accessible name in either language', () => {
    for (const name of ['How this works', '工作原理']) {
      expect(checkGuideEntries([{ article: 'ports', heading: null, name }, { article: 'capabilities', heading: null, name }])).toContain(`help.name_repeated: ${name.toLowerCase()}`);
    }
  });
  it('compares names after whitespace/case normalization and treats a missing anchor as the article itself', () => {
    expect(checkGuideEntries([{ article: 'resources', heading: null, name: 'Templates guide' }, { article: 'resources', heading: '', name: '  TEMPLATES  guide  ' }])).toEqual(['help.destination_repeated: resources#', 'help.name_repeated: templates guide']);
  });
  it('allows differently named sections of the same article, but refuses an unnamed entry', () => {
    expect(checkGuideEntries([{ article: 'resources', heading: 'templates', name: 'Templates guide' }, { article: 'resources', heading: 'mounts', name: 'Mounts guide' }])).toEqual([]);
    expect(checkGuideEntries([{ article: 'resources', heading: null, name: ' ' }])).toEqual(['help.name_missing: resources#']);
  });
});
