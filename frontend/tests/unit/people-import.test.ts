/** Imported and pasted writing preserves structure and reports losses without fetching remote content. */
import { describe, expect, it } from 'vitest';

import { readHtml } from '@/features/people/html';
import { selectionFault } from '@/features/people/imports';
import { normalizeReactions, recentAfter } from '@/features/people/reactions';
import { inChat, readParams, mergeParams, filtersOf } from '@/features/search/params';

describe('safe writing conversion', () => {
  it('preserves nested lists and every table header cell', () => {
    const result = readHtml('<ul><li>Parent<ul><li>Child</li></ul></li></ul><table><thead><tr><th>Step</th><th>Owner</th></tr></thead><tbody><tr><td>Canary</td><td>Dora</td></tr></tbody></table>');
    expect(result.pieces[0]).toMatchObject({ type: 'bullet', content: [{ text: 'Parent' }], children: [{ type: 'bullet', content: [{ text: 'Child' }] }] });
    expect(result.pieces[1]).toMatchObject({ type: 'table', props: { header_row: true }, content: [{ cells: [{ content: [{ text: 'Step' }] }, { content: [{ text: 'Owner' }] }] }, { cells: [{ content: [{ text: 'Canary' }] }, { content: [{ text: 'Dora' }] }] }] });
  });
  it('leaves unsafe links as text, drops script and hands a picture on to whoever settles it', () => {
    const result = readHtml('<p><a href="javascript:alert(1)">Plain</a><script>secret()</script><img src="https://remote.invalid/tracker.png" alt="Diagram"></p>');
    expect(JSON.stringify(result.pieces)).not.toContain('javascript:');
    expect(JSON.stringify(result.pieces)).not.toContain('secret()');
    expect(result.warnings).toHaveLength(2);
    expect(result.images).toMatchObject([{ src: 'https://remote.invalid/tracker.png', alt: 'Diagram' }]);
    expect(JSON.stringify(result.pieces)).toContain('Diagram');
  });
  it('refuses a mixed archive selection and unsupported product files', () => {
    expect(selectionFault([{ name: 'one.zip' }, { name: 'two.md' }])).toBe('mixed');
    expect(selectionFault([{ name: 'word.docx' }])).toBe('unsupported');
    expect(selectionFault([{ name: 'one.md' }, { name: 'two.html' }])).toBeNull();
  });
});

it('eight distinct recent reactions keep the newest choice first and exclude custom images', () => {
  const choices = ['👍', '✅', '👀', '🎉', '😀', '😃', '😄', '😁', '😆'];
  expect(normalizeReactions([...choices, '👍', 'custom:image'])).toHaveLength(8);
  expect(recentAfter(['✅', '👍', '👀'], '👍')).toEqual(['👍', '✅', '👀']);
});

it('searching in a chat retains the query, author, dates and file/mention filters', () => {
  const id = '00000000-0000-4000-8001-000000000001';
  const params = readParams({ q: 'rollout', tab: 'documents', document_id: id, with_file: 'true', mentions_me: 'true', author: id, from: '2026-10-01' });
  const scoped = inChat(params, id, id);
  expect(scoped.tab).toBe('chats');
  expect(scoped.document_id).toBeUndefined();
  expect(filtersOf(scoped)).toMatchObject({ q: 'rollout', chat_id: id, scope: id, author: id, with_file: true, mentions_me: true, kinds: ['chat_message'], from: '2026-10-01' });
});

it('a delayed word query keeps filters chosen after the field was typed', () => {
  expect(mergeParams({ with_file: 'true', mentions_me: 'true', q: 'before' }, { q: 'before' }, { q: 'canary' })).toEqual({ q: 'canary', with_file: 'true', mentions_me: 'true' });
});
