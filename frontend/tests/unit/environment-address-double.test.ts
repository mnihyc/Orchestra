// @vitest-environment node
/** The fake page, list, binding reference and finder use one environment address, as the API does. */
import { expect, it } from 'vitest';

import type { Environment, NameSearchPage } from '../../src/generated/api-types.ts';
import { ACME } from '../doubles/fixtures.ts';
import { World } from '../doubles/world.ts';

it('resolves the same short address from every environment surface without changing its display name',async () => {
  const world=new World();const ada=world.people.get('ada')!;
  const record=world.resources.environments.find((entry)=>entry.owner==='ada')!;
  record.name='Acme workspace';record.home=ACME.research;
  const address='acme.research.ada.acme-workspace';
  const read=world.resources.handle(ada,'GET',`/environments/${record.id}`,new URLSearchParams(),{})?.body as Environment;
  expect(read.name).toBe('Acme workspace');expect(read.address).toBe(address);
  expect(world.resources.environmentRef(record).address).toBe(address);
  const list=world.resources.handle(ada,'GET','/environments',new URLSearchParams(),{})?.body as Environment[];
  expect(list.find((entry)=>entry.id===record.id)?.address).toBe(address);
  const found=(await world.search.handle(ada,'GET','/search/names',new URLSearchParams({q:address,kinds:'environment'}),{}))?.body as NameSearchPage;
  expect(found.items.find((entry)=>entry.id===record.id)).toMatchObject({name:'Acme workspace',address});
});
