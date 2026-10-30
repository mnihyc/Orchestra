/** Filename hits in the browser fixture retain their file details, grammar and document access. */
import { expect, it } from 'vitest';
import type { ChatMessage, SearchHitPage } from '@/generated/api-types';
import { World } from '../doubles/world';
import { ACME, id, MADE_AT, userRef } from '../doubles/fixtures';
import { toolCall } from '../doubles/thread-rows';

it.each([
  { q: 'merge_attachment_429', regex: false },
  { q: '429 "merge_attachment"', regex: false },
  { q: 'merge_attachment_\\d+\\.txt', regex: true },
])('keeps an authorized filename hit for $q and hides it from another reader', ({ q, regex }) => {
  const world = new World();
  const ada = world.people.get('ada'), grace = world.people.get('grace'), doc = world.documents.docs.get(id(7, 1));
  if (ada === undefined || grace === undefined || doc === undefined) throw new Error('The document and its readers exist');
  doc.page = 'restricted'; doc.owner = 'ada'; doc.title = 'A different manual';
  doc.files = [{ id: 'merge-file', name: 'merge_attachment_429.txt', size: 42, media_type: 'text/plain', is_text: true, added_by: null, added_at: MADE_AT }];
  const query = new URLSearchParams({ q, kinds: 'document', ...(regex ? { regex: 'true' } : {}) });
  const found = world.search.search(ada, query).body as SearchHitPage;
  const hit = found.items.find((one) => one.kind === 'document' && one.document.id === doc.id);
  expect(hit?.kind).toBe('document');
  if (hit?.kind !== 'document') throw new Error('The filename is found');
  expect(hit.block_id).toBeNull();
  expect(hit.file?.id).toBe('merge-file');
  expect(hit.file?.name).toBe('merge_attachment_429.txt');
  expect(hit.matches.length).toBeGreaterThan(0);
  expect((world.search.search(grace, query).body as SearchHitPage).items.some((one) => one.kind === 'document' && one.document.id === doc.id)).toBe(false);
});

it('applies owner and last-edited dates before a document result is paged', () => {
  const world = new World();
  const ada = world.people.get('ada'), grace = world.people.get('grace'), doc = world.documents.docs.get(id(7, 1));
  if (ada === undefined || grace === undefined || doc === undefined) throw new Error('The document and its readers exist');
  doc.owner = 'ada'; doc.title = 'Canary handbook'; doc.createdAt = '2026-01-01T00:00:00.000Z';
  doc.updatedAt = '2026-10-03T12:00:00.000Z'; doc.updatedBy = userRef(grace.me);
  const query = new URLSearchParams({ q: 'canary', kinds: 'document', owner_id: ada.me.id,
    edited_by_id: grace.me.id, date_field: 'edited', from: '2026-10-03T00:00:00.000Z', to: '2026-10-03T23:59:59.999Z', limit: '1' });
  const found = world.search.search(ada, query).body as SearchHitPage;
  expect(found.items).toHaveLength(1);
  expect(found.items[0]).toMatchObject({kind: 'document', document: {id: doc.id}});
  query.set('date_field', 'created');
  expect((world.search.search(ada, query).body as SearchHitPage).items).toHaveLength(0);
});

it('uses a chat message edit time and its author for the last-editor filter', () => {
  const world = new World();
  const ada = world.people.get('ada');
  if (ada === undefined) throw new Error('The reader exists');
  const reply = world.chats.control('/say', {node:'Fix login', as:'ada', text:'concordance', at:'2026-10-01T09:00:00.000Z'});
  const message = (reply.body as {message: ChatMessage}).message;
  message.edited_at = '2026-10-02T09:00:00.000Z';
  const query = new URLSearchParams({q:'concordance', kinds:'chat_message', edited_by_id:ada.me.id,
    date_field:'edited', from:'2026-10-02T09:00:00.000Z', to:'2026-10-02T09:00:00.000Z'});
  expect((world.search.search(ada, query).body as SearchHitPage).items).toHaveLength(1);
  query.set('date_field','created');
  expect((world.search.search(ada, query).body as SearchHitPage).items).toHaveLength(0);
});

it('does not treat a text part with media metadata as an attachment', () => {
  const world = new World();
  const ada = world.people.get('ada');
  if (ada === undefined) throw new Error('The reader exists');
  world.chats.control('/say', {node:'Fix login', as:'ada', parts:[{type:'text',text:'metadataonly',media_type:'image/png',kind:'image'}]});
  const query = new URLSearchParams({q:'metadataonly', kinds:'chat_message', file_type:'image/*'});
  expect((world.search.search(ada, query).body as SearchHitPage).items).toHaveLength(0);
});

it('finds an image carried by a tool call when files are requested', () => {
  const world = new World();
  const ada = world.people.get('ada');
  if (ada === undefined) throw new Error('The reader exists');
  const item = toolCall('image-result', {kind:'image',blob:'a'.repeat(64),media_type:'image/png',name:'chart.png',caption:null}, {title:'chartneedle'});
  world.threads.emit(ACME.mainThread, {items:[item]});
  const query = new URLSearchParams({q:'chartneedle', kinds:'tool_call', with_file:'true', file_type:'image/*'});
  expect((world.search.search(ada, query).body as SearchHitPage).items).toMatchObject([{kind:'item',item:{id:item.id}}]);
});
