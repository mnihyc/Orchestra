// @vitest-environment node
/** A page share review uses the fake world's ordinary producer and forgets its trial before another reader asks. */
import { expect, it } from 'vitest';
import type { PermissionPage, SharePreview } from '../../src/generated/api-types.ts';
import { ACME } from '../doubles/fixtures.ts';
import { Refusal, World } from '../doubles/world.ts';

it('shows the real proposed entries and keeps every ordinary page and sent-change record as it stood',() => {
  const world=new World();const ada=world.people.get('ada')!;const recipient=world.people.get('dora')!;
  const query=new URLSearchParams();
  const read=() => world.access.handle(ada,'GET',`/nodes/${ACME.fixLogin}/permissions`,query,{})?.body as PermissionPage;
  const before=structuredClone(read());const sent=world.access.sent.length;
  const preview=world.access.handle(ada,'POST',`/nodes/${ACME.fixLogin}/share/preview`,query,{kind:'page',change:{name:[{holders:[{kind:'user',id:recipient.me.id}],choice:'full_view'}]}})?.body as SharePreview;
  expect(preview.allowed).toBe(true);
  expect(preview.rows.some((row) => row.after.some((entry) => entry.holder.kind === 'user' && entry.holder.user.id === recipient.me.id))).toBe(true);
  expect(read()).toEqual(before);
  expect(world.access.sent.length).toBe(sent);
});

it('does not give a person who cannot read the private thing a preview',() => {
  const world=new World();const ada=world.people.get('ada')!;const outsider=world.people.get('zoe')!;const query=new URLSearchParams();
  world.access.handle(ada,'PATCH',`/nodes/${ACME.fixLogin}/permissions`,query,{whole:'private'});
  expect(() => world.access.handle(outsider,'POST',`/nodes/${ACME.fixLogin}/share/preview`,query,{kind:'page',change:{}})).toThrow(Refusal);
});
