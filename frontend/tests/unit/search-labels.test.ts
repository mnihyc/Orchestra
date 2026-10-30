/** Repeated places in finder and search use the same short readable labels without discovering private ancestors. */
import { describe, expect, it, vi } from 'vitest';
import { i18n } from '@/i18n';
import { rowOfName, type Env } from '@/features/finder/rows';
import { wayOf } from '@/features/finder/destinations';
import { crumbsOf } from '@/features/search/hits';
import { labelOfName, labelsAlongPath, labelsOfHits, labelsOfNames } from '@/features/search/labels';
import type { NameHit, NodeRef, SearchHit } from '@/generated/api-types';

const node = (id: string, kind: NodeRef['kind'], name: string | null, redacted = false): NodeRef => ({ id, kind, name, redacted });
const name = (id: string, path: NodeRef[]): NameHit => ({ id, kind: 'project', name: 'web', href: `/project/${id}`, path, color: null, custom_color: null, tag: null, thread_attention: null, address: null, avatar: null, username: null, owner: null, archived: false, recent: false });
const named = [name('a-web', [node('acme', 'org', 'Acme')]), name('b-web', [node('beta', 'org', 'Beta')])];

describe('places named by a result', () => {
  it('two web projects say their root org first, keep their ids, and use those labels in finder rows', () => {
    const labels = labelsOfNames(named);
    expect(named.map((hit) => labelOfName(hit, labels))).toEqual(['web · Acme', 'web · Beta']);
    const env: Env = { t: i18n.getFixedT('en'), orgSlug: 'acme', exists: () => true, showCard: vi.fn(), go: vi.fn(), placeLabels: labels };
    const row = rowOfName(named[1] ?? name('', []), 'go', 'web', env);
    expect(row.title).toBe('web · Beta');
    row.run('here');
    expect(env.go).toHaveBeenCalledWith({ to: '/project/b-web' }, 'here');
  });
  it('a private ancestor contributes no name to a label', () => {
    const hit = name('private-child', [node('secret', 'org', 'Never exposed', true)]);
    expect([...labelsOfNames([hit]).values()].join(' ')).not.toContain('Never exposed');
    expect(labelOfName(hit, labelsOfNames([hit]))).toBe('web');
  });
  it('a search page adds its own readable paths as peers even before any tree was loaded', () => {
    const hits = named.map((hit): SearchHit => ({ kind: 'document', document: { id: `doc-${hit.id}`, title: 'Notes' }, block_id: null, by: null, path: [...hit.path, node(hit.id, 'project', hit.name)], snippet: 'words', matches: [] }) as unknown as SearchHit);
    const labels = labelsOfHits(hits);
    const words = { chat: 'Chat', private: () => 'Private' };
    expect(hits.map((hit) => crumbsOf(hit, words, labels))).toEqual([['Acme', 'web', 'Notes'], ['Beta', 'web', 'Notes']]);
    expect(wayOf({ id: 'note', path: [node('acme', 'org', 'Acme'), node('a-web', 'project', 'web')] }, i18n.getFixedT('en'), labels)).toBe('Acme › web');
  });

  it('qualifies a partial path once and keeps concealed ancestors concealed', () => {
    const path = [node('web', 'project', 'web'), node('login', 'topic', 'login'), node('turn', 'thread', 'Implementation')];
    const labels = new Map([['web', 'web · Acme'], ['login', 'login · Acme'], ['turn', 'Implementation · Acme']]);
    expect(labelsAlongPath(path, labels, () => 'Private')).toEqual(['web · Acme', 'login', 'Implementation']);
    expect(labelsAlongPath([node('root', 'org', 'Secret', true), ...path], labels, () => 'Private')).toEqual(['Private', 'web', 'login', 'Implementation']);
  });
});
