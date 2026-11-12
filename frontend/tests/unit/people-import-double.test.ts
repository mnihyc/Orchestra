// @vitest-environment node
/** An uploaded Markdown document uses the same block reader as conversion in the fake API. */
import { expect, it } from 'vitest';
import type { DocumentImportResult } from '@/generated/api-types';
import { ACME } from '../doubles/fixtures';
import { PeopleImports } from '../doubles/people-import';
import { World } from '../doubles/world';

it('imports the title, bullets, checked task and quote without changing their types on replay', () => {
  const world = new World(), person = world.people.get('ada');
  if (person === undefined) throw new Error('The actual importing person is required');
  const imports = new PeopleImports(world);
  const bytes = Buffer.from('# Imported plan\n\nA first line.\n\n- one\n- two\n\n- [x] done\n\n> said\n');
  imports.blobs.set('uploaded-markdown', { name: 'plan.md', bytes, size: bytes.length, media_type: 'text/markdown', owner: person.me.id });
  const request = { client_id: 'markdown-import', files: [{ name: 'plan.md', blob: 'uploaded-markdown', format: 'markdown' }] };
  const imported = imports.handle(person, 'POST', `/nodes/${ACME.fixLogin}/documents/import`, new URLSearchParams(), request);
  expect(imported?.status).toBe(200);
  const result = imported?.body as DocumentImportResult;
  expect(result.entries).toHaveLength(1);
  expect(result.entries[0]?.error).toBeNull();
  const document = result.entries[0]?.document;
  expect(document?.title).toBe('Imported plan');
  const stored = document === null || document === undefined ? undefined : world.documents.docs.get(document.id);
  if (stored === undefined) throw new Error('The native document producer must retain the imported document');
  const blocks = [...stored.blocks.values()].filter((block) => !block.removed);
  expect(blocks.map((block) => block.type)).toEqual(['paragraph', 'bullet', 'bullet', 'todo', 'quote']);
  expect(blocks[3]?.props).toMatchObject({ checked: true });
  expect(blocks[3]?.content).toEqual([{ type: 'text', text: 'done' }]);
  expect(blocks[4]?.content).toEqual([{ type: 'text', text: 'said' }]);
  const count = world.documents.docs.size;
  expect(imports.handle(person, 'POST', `/nodes/${ACME.fixLogin}/documents/import`, new URLSearchParams(), request)?.body).toEqual(result);
  expect(world.documents.docs.size).toBe(count);
});
