/** An observed destination is not a creation grant, and an unrelated response never enables it. */
import { beforeEach, expect, it, vi } from 'vitest';
import { judgeCopyDestinations, type CopyDestinationsRequest } from '@/api/copyDestinations';
import { findAct } from '@/api/acts';
const calls = vi.hoisted(() => ({ json: vi.fn<(path: string, options: unknown) => Promise<unknown>>() }));
vi.mock('@/api/trace', () => ({ traceJson: calls.json }));
beforeEach(() => vi.clearAllMocks());
const request: CopyDestinationsRequest = { subject: { kind: 'provider', id: 'source' }, intent: 'duplicate', destinations: [{ kind: 'node', id: 'destination' }] };
it('only judges the explicit visible candidates, with no native write identity or effects', async () => {
  const reply = { subject: request.subject, intent: request.intent, source_owned: false, destinations: [{ destination: request.destinations[0], cross_root: true, outcome: { state: 'allowed' } }] };
  calls.json.mockResolvedValue(reply); expect(await judgeCopyDestinations(request)).toEqual(reply);
  expect(calls.json).toHaveBeenCalledExactlyOnceWith('/copy-destinations/judge', { method: 'POST', body: request });
  expect(findAct('POST', '/copy-destinations/judge')?.changes).toEqual([]);
});
it.each(['subject', 'intent', 'destination', 'missing', 'duplicate'] as const)('rejects a %s mismatch instead of enabling a candidate', async (bad) => {
  const destination = { destination: { kind: 'node', id: bad === 'destination' ? 'foreign' : 'destination' }, cross_root: true, outcome: { state: 'allowed' } };
  calls.json.mockResolvedValue({ subject: bad === 'subject' ? { kind: 'provider', id: 'foreign' } : request.subject,
    intent: bad === 'intent' ? 'move' : request.intent, source_owned: true, destinations: bad === 'missing' ? [] : bad === 'duplicate' ? [destination, destination] : [destination] });
  await expect(judgeCopyDestinations(request)).rejects.toThrow('did not match');
});
it('preserves needs_choice without inventing a denial or an allowed result', async () => {
  const reply = { subject: request.subject, intent: request.intent, source_owned: true, destinations: [{ destination: request.destinations[0], cross_root: true, outcome: { state: 'needs_choice', choice: 'mcp_machine' } }] };
  calls.json.mockResolvedValue(reply); expect(await judgeCopyDestinations(request)).toEqual(reply);
});

it('judges every visible destination by native batches instead of dropping later choices', async () => {
  const destinations: CopyDestinationsRequest['destinations'] = Array.from({ length: 251 }, (_, at) => ({ kind: 'node', id: `place-${at}` }));
  calls.json.mockImplementation((_path, options) => {
    const batch = (options as { body: CopyDestinationsRequest }).body;
    expect(batch.destinations.length).toBeLessThanOrEqual(100);
    return Promise.resolve({ subject: batch.subject, intent: batch.intent, source_owned: false,
      destinations: batch.destinations.map((destination) => ({ destination, cross_root: true, outcome: { state: 'allowed' } })) });
  });
  const reply = await judgeCopyDestinations({ ...request, destinations });
  expect(reply.destinations.map((row) => row.destination)).toEqual(destinations);
  expect(calls.json).toHaveBeenCalledTimes(3);
});
