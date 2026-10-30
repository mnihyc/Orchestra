/** Private document identities never become content references in an editor, tree or export chooser. */
import { describe, expect, it } from 'vitest';
import { documentPage } from '@/api/documents';

describe('a document list with administrative names', () => {
  it('keeps private rows apart and preserves the page continuation', () => {
    const page = documentPage({ items: [
      { id: 'readable', title: 'The readable guide', content_hidden: false },
      { id: 'hidden', parent_id: 'place', kind: 'document', name: 'Private guide', owner: null, content_hidden: true, private: true },
    ], next_cursor: 'private:2' });
    expect(page.items.map((doc) => doc.title)).toEqual(['The readable guide']);
    expect(page.items.privateRows.map((doc) => doc.name)).toEqual(['Private guide']);
    expect(page.next_cursor).toBe('private:2');
    expect(page.items.every((doc) => doc.id !== 'hidden')).toBe(true);
  });
});
