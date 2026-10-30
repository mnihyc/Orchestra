/** What is kept on the device for the person who is signed in: unsaved text and the viewer chosen for a kind of file, each under their id, and the text gone after seven days. */
import { beforeEach, describe, expect, it } from 'vitest';

import { countUnsaved, forgetUnsaved, keepUnsaved, readUnsaved, UNSAVED_KEPT_MS } from '@/features/files/Editing';
import { readChoices, rememberChoice } from '@/features/files/viewers';

beforeEach(() => localStorage.clear());

describe('unsaved text', () => {
  it('is kept for its person alone, put back, and removed when it is saved or discarded', () => {
    keepUnsaved('ada', 'env', 'web/a.ts', { base: 'h1', text: 'typed' }, 1_000);
    expect(readUnsaved('ada', 'env', 'web/a.ts', 2_000)).toEqual({ base: 'h1', text: 'typed', at: 1_000 });
    // Another person at the same browser never sees it.
    expect(readUnsaved('grace', 'env', 'web/a.ts', 2_000)).toBeNull();
    expect(countUnsaved('ada', 2_000)).toBe(1);
    forgetUnsaved('ada', 'env', 'web/a.ts');
    expect(readUnsaved('ada', 'env', 'web/a.ts', 2_000)).toBeNull();
  });

  it('is not kept beyond seven days, which is longer than a weekend away and short enough that forgotten text does not sit on a machine', () => {
    keepUnsaved('ada', 'env', 'a.ts', { base: null, text: 'old' }, 0);
    expect(readUnsaved('ada', 'env', 'a.ts', UNSAVED_KEPT_MS)).not.toBeNull();
    expect(countUnsaved('ada', UNSAVED_KEPT_MS + 1)).toBe(0);
    expect(readUnsaved('ada', 'env', 'a.ts', UNSAVED_KEPT_MS + 1)).toBeNull();
    // Reading it found it too old, and it was removed.
    expect(localStorage.length).toBe(0);
  });

  it('of nobody signed in is not kept at all', () => {
    keepUnsaved(null, 'env', 'a.ts', { base: null, text: 'x' });
    expect(localStorage.length).toBe(0);
    expect(readUnsaved(null, 'env', 'a.ts')).toBeNull();
  });
});

describe('the viewer chosen for a kind of file', () => {
  it('is kept for the person by the kind of file: its extension, else its whole name', () => {
    rememberChoice('ada', 'notes.md', 'code');
    rememberChoice('ada', 'Makefile', 'code');
    expect(readChoices('ada')).toEqual({ '.md': 'code', makefile: 'code' });
    expect(readChoices('grace')).toEqual({});
  });
});
