/** Copied links and references retain the source and part across every menu entrance. */
import { describe, expect, it } from 'vitest';
import { linkedObject, linkTo, referenceTo, type LinkKind } from '@/lib/links';
import { referenceOfAddress } from '@/features/people/references';
import { contentLocation, contentTab, parseContentAddress } from '@/features/workbench/contentAddress';
import type { ReferencePreview } from '@/generated/api-types';

const ID = '0199d1a8-7f3c-7a1e-8b3a-1c2d3e4f5e03';
const PART = '0199d1a8-7f3c-7a1e-8b3a-1c2d3e4f5e04';

describe('public object addresses', () => {
  it.each<[LinkKind, string]>([
    ['org', '/org'], ['project', '/project'], ['topic', '/topic'], ['thread', '/thread'],
    ['document', '/doc'], ['chat', '/chat'], ['person', '/u'], ['document_container', '/document-folder'],
    ['provider', '/resources/providers'], ['machine', '/resources/machines'], ['host', '/resources/hosts'],
    ['environment', '/resources/environments'], ['agent', '/resources/agents'], ['template', '/resources/templates'],
    ['mcp_server', '/resources/mcp'], ['mount', '/resources/mounts'], ['skill', '/resources/skills'],
  ])('addresses %s through its existing screen', (kind, path) => {
    const address = new URL(linkTo({ kind, id: ID }));
    expect(address.pathname).toBe(`${path}/${ID}`);
    expect(linkedObject(address.href)).toEqual({ kind, id: ID });
  });

  it('encodes a username and opens a group in its own org settings', () => {
    const person = linkTo({ kind: 'person', id: ID, username: 'a+b' });
    expect(new URL(person).pathname).toBe('/u/a%2Bb');
    expect(linkedObject(person)).toEqual({ kind: 'person', id: 'a+b' });
    const group = new URL(linkTo({ kind: 'group', id: ID, orgId: PART }));
    expect(group.pathname).toBe(`/org/${PART}/settings/groups`);
    expect(group.searchParams.get('group')).toBe(ID);
  });

  it('retains the continuation and all selected items, retaining every item identity', () => {
    const url = new URL(linkTo({ kind: 'thread', id: ID }, { items: [PART, ID], continuation: PART }));
    expect(url.searchParams.get('item')).toBe(PART);
    expect(url.searchParams.get('items')).toBe(`${PART},${ID}`);
    expect(url.searchParams.get('continuation')).toBe(PART);
    expect(url.search).not.toContain('thread-window');
  });

  it('keeps saved page queries and fragments when copied from a tab', () => {
    const url = new URL(linkTo({ href: `/thread/${ID}?continuation=${PART}&item=${ID}#output` }));
    expect(url.searchParams.get('continuation')).toBe(PART);
    expect(url.searchParams.get('item')).toBe(ID);
    expect(url.hash).toBe('#output');
  });

  it('round-trips selected workspace lines, JSON-looking paths, and the actual source thread', () => {
    for (const path of ['web/a b.ts', '123', '[draft]', '中文/界面.ts']) {
      const url = new URL(linkTo({ kind: 'environment_file', id: ID, threadId: PART, path }, { line: 3, endLine: 8 }));
      const parsed = parseContentAddress(url.pathname + url.search);
      expect(parsed).toEqual({ view: 'file', environmentId: ID, threadId: PART, path, line: 3, endLine: 8 });
      if (parsed === null) throw new Error('Expected file address');
      const tab = { ...contentTab(parsed), id: 'local-tab', pinned: false };
      expect(contentLocation(tab)).toEqual(parsed);
      expect(referenceOfAddress(url.pathname + url.search)).toEqual({ kind: 'environment_file', id: ID, path, line: 3 });
    }
  });

  it('keeps an attachment identity when its document or chat address is dragged into a message', () => {
    for (const route of ['doc', 'chat']) expect(referenceOfAddress(`/${route}/${ID}?file=${PART}&line=7&endLine=9`))
      .toEqual({ kind: 'file', id: PART, line: 7 });
  });

  it('rejects an inverted or ownerless line range', () => {
    const base = `/workbench?view=file&environment=${ID}&path=a.ts`;
    expect(parseContentAddress(`${base}&endLine=8`)).toBeNull();
    expect(parseContentAddress(`${base}&line=8&endLine=3`)).toBeNull();
    expect(() => linkTo({ kind: 'file', id: ID })).toThrow('owning document or chat');
  });
});

describe.each(['Rollout [plan]', '发布[计划]'])('agent reference: %s', (title) => {
  const preview: ReferencePreview = {
    kind: 'document', id: ID, node_kind: null, title, description: '', place: null,
    updated_at: null, updated_by: null,
    part: { part: { block: PART }, number: 3, anchor: PART, text: '', by: null },
  };

  it('uses a stable block for the web and its numbered part for the agent', () => {
    const url = new URL(linkTo(preview));
    expect(url.searchParams.get('block')).toBe(PART);
    expect(referenceTo(preview)).toBe(`[${title.replaceAll('[', '\\[').replaceAll(']', '\\]')}](id:${ID}#3)`);
    expect(referenceOfAddress(url.pathname + url.search)).toEqual({ kind: 'document', id: ID, block: PART });
  });

  it('keeps a file path with spaces and its first selected line in the written grammar', () => {
    expect(referenceTo({ kind: 'environment_file', id: ID, path: 'web/a b.ts', name: title }, { line: 7, endLine: 9 }))
      .toContain(`](<id:${ID}/web/a b.ts#7>)`);
  });

  it('uses a message preview number without substituting its web identifier', () => {
    const message = { ...preview, kind: 'chat' as const, part: { part: { message: 5 }, number: 5, anchor: PART, text: '', by: null } };
    expect(new URL(linkTo(message)).searchParams.get('message')).toBe(PART);
    expect(referenceTo(message)).toContain(`](id:${ID}#5)`);
  });
});
