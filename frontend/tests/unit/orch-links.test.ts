/** Every link consumer uses deployment identity before interpreting a resource or its parts. */
import { afterEach, beforeEach, expect, it } from 'vitest';
import { queryClient } from '@/api/query';
import { placeOfAddress } from '@/features/chat/markup';
import { referenceOfAddress } from '@/features/people/references';
import { carriedLinks } from '@/features/sharing/linkPlan';
import { parseContentAddress } from '@/features/workbench/contentAddress';
import { resolveArticleLink } from '@/help/links';
import { droppedReference } from '@/lib/drop';
import { linkedObject, linkTo } from '@/lib/links';
import { publicOrchLink, readOrchLink } from '@/lib/orchLinks';

const ID = '00000000-0000-4000-8000-000000000041';
const BLOCK = '00000000-0000-4000-8000-000000000042';
const metadata = {
  public_url: 'https://public.orch.test',
  alternate_urls: ['https://inside.orch.test:8443', 'http://192.0.2.15:8080', 'https://[2001:db8::15]'],
  link_endpoints: [{ url: 'wss://edge.orch.test/link' }],
};
beforeEach(() => { queryClient.setQueryData(['meta'], metadata); });
afterEach(() => { queryClient.removeQueries({ queryKey: ['meta'], exact: true }); });

it.each([location.origin, metadata.public_url, ...metadata.alternate_urls, 'https://edge.orch.test'])('recognizes exactly the declared address %s across resource consumers', (origin) => {
  const href = `${origin}/doc/${ID}?block=${BLOCK}#section`;
  expect(readOrchLink(href)?.href).toBe(`${location.origin}/doc/${ID}?block=${BLOCK}#section`);
  expect(linkedObject(href)).toEqual({ kind: 'document', id: ID });
  expect(referenceOfAddress(href)).toEqual({ kind: 'document', id: ID, block: BLOCK });
  expect(placeOfAddress(href, location.origin)).toEqual({ kind: 'doc', id: ID, path: `/doc/${ID}?block=${BLOCK}#section` });
  expect(carriedLinks([{ type: 'text', text: `[Plan](${href})` }])).toMatchObject([{ kind: 'document', id: ID, label: 'Plan' }]);
  expect(parseContentAddress(href)).toMatchObject({ view: 'page', href: `/doc/${ID}?block=${BLOCK}#section` });
  expect(resolveArticleLink(`${origin}/help/first-steps#read`)).toEqual({ article: 'first-steps', heading: 'read' });
  const transfer = { getData: (type: string) => type === 'text/uri-list' ? href : '' } as Pick<DataTransfer, 'getData'>;
  expect(droppedReference(transfer as DataTransfer)).toBe(`[${ID}](id:${ID})`);
});

it.each([
  'https://elsewhere.test', 'https://public.orch.test.evil.test', 'https://public-orch.test',
  'https://public.orch.test:8443', 'http://192.0.2.15', 'http://192.0.2.16:8080',
  'https://[2001:db8::16]', 'https://public.orch.test@evil.test', 'https://someone@public.orch.test',
])('keeps the foreign or ambiguous origin %s out of every resource consumer', (origin) => {
  const href = `${origin}/doc/${ID}`;
  expect(readOrchLink(href)).toBeNull();
  expect(linkedObject(href)).toBeNull();
  expect(referenceOfAddress(href)).toBeNull();
  expect(placeOfAddress(href, location.origin)).toBeNull();
  expect(carriedLinks([{ type: 'text', text: `[Plan](${href})` }])).toEqual([]);
  expect(parseContentAddress(href)).toBeNull();
});

it.each(['bad%escape', 'a%2Fb', 'a%5Cb', '%00', '%0A'])('does not turn malformed or encoded separators into a resource ID: %s', (id) => {
  const href = `${metadata.public_url}/doc/${id}`;
  expect(linkedObject(href)).toBeNull();
  expect(placeOfAddress(href, location.origin)).toBeNull();
  expect(referenceOfAddress(href)).toBeNull();
});

it('normalizes host case/default ports without trusting a different scheme or an embedded backslash', () => {
  expect(readOrchLink(`https://PUBLIC.ORCH.TEST:443/doc/${ID}`)?.pathname).toBe(`/doc/${ID}`);
  expect(readOrchLink(`http://public.orch.test/doc/${ID}`)).toBeNull();
  expect(readOrchLink(`https://elsewhere.test\\@public.orch.test/doc/${ID}`)).toBeNull();
  expect(readOrchLink(`//elsewhere.test/doc/${ID}`)).toBeNull();
});

it('retains the copied chat message query and lets an explicit fragment select its part', () => {
  expect(referenceOfAddress(`${metadata.public_url}/chat/${ID}?message=4`)).toEqual({ kind: 'chat', id: ID, message: 4 });
  expect(referenceOfAddress(`${metadata.public_url}/chat/${ID}?message=4#message=7`)).toEqual({ kind: 'chat', id: ID, message: 7 });
});

it('copies the public address with every selector and reads the current cache after metadata changes', () => {
  expect(linkTo({ kind: 'document', id: ID }, { block: BLOCK })).toBe(`${metadata.public_url}/doc/${ID}?block=${BLOCK}`);
  expect(linkTo({ href: `${metadata.alternate_urls[0]}/doc/${ID}?block=${BLOCK}#section` })).toBe(`${metadata.public_url}/doc/${ID}?block=${BLOCK}#section`);
  expect(linkTo({ kind: 'environment_file', id: ID, path: 'src/a b.ts' }, { line: 3, endLine: 8 }).startsWith(`${metadata.public_url}/workbench?`)).toBe(true);
  expect(publicOrchLink(`https://elsewhere.test/doc/${ID}`)).toBe(`https://elsewhere.test/doc/${ID}`);
  queryClient.setQueryData(['meta'], { public_url: '' });
  expect(readOrchLink(`${metadata.public_url}/doc/${ID}`)).toBeNull();
  expect(linkTo({ kind: 'document', id: ID })).toBe(`${location.origin}/doc/${ID}`);
});
