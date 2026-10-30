/** The request an archive is asked with, and what it says of the plan: only what differs from what the server does unasked, the span of days, and how a plan reads against the limits of a place. */
import { describe, expect, it } from 'vitest';

import { isDay, OPENING_CHOICES, requestOf, spanFault, wantsNothing } from '@/features/export/choices';
import { readPlan } from '@/features/export/plan';
import type { ExportPlan } from '@/generated/api-types';

const everything = { type: 'all' } as const;

describe('the request of an archive', () => {
  it('says nothing but what it is of when nothing was changed', () => {
    expect(requestOf(everything, OPENING_CHOICES)).toEqual({ what: 'all' });
  });

  it('says only the kinds that are not wanted, and the form, the depth and the options that differ', () => {
    const request = requestOf(everything, { ...OPENING_CHOICES, conversations: false, chats: false, format: 'json', detail: 'full', files: true, from: '2026-10-01', to: '2026-10-09' });
    expect(request).toEqual({ what: 'all', parts: { conversations: false, chats: false }, format: 'json', detail: 'full', files: true, from: '2026-10-01', to: '2026-10-09' });
  });

  it('names what was chosen and leaves out the lists that name nothing', () => {
    const request = requestOf({ type: 'chosen', chosen: { nodes: ['a', 'b'], documents: [], chats: ['c'] } }, OPENING_CHOICES);
    expect(request).toEqual({ what: { nodes: ['a', 'b'], chats: ['c'] } });
  });

  it('asks for the earlier versions and the comments of documents only while documents are wanted', () => {
    const asked = requestOf(everything, { ...OPENING_CHOICES, versions: true, comments: true });
    expect(asked).toEqual({ what: 'all', versions: true, comments: true });
    const without = requestOf(everything, { ...OPENING_CHOICES, documents: false, versions: true, comments: true });
    expect(without).toEqual({ what: 'all', parts: { documents: false } });
  });

  it('sends the package choice and preserves explicit file and comment decisions', () => {
    const withContents = requestOf(everything, { ...OPENING_CHOICES, format: 'package', files: true, comments: true, versions: true });
    expect(withContents).toEqual({ what: 'all', package: true, files: true, comments: true, versions: true });
    const withoutContents = requestOf(everything, { ...OPENING_CHOICES, format: 'package', files: false, comments: false });
    expect(withoutContents).toEqual({ what: 'all', package: true, files: false, comments: false });
    const withoutDocuments = requestOf(everything, { ...OPENING_CHOICES, format: 'package', documents: false, files: true, comments: true });
    expect(withoutDocuments).toEqual({ what: 'all', parts: { documents: false }, package: true, files: true, comments: false });
  });

  it('is the same request for the same choices, whichever way they were made', () => {
    const one = JSON.stringify(requestOf(everything, { ...OPENING_CHOICES, format: 'json', files: true }));
    const other = JSON.stringify(requestOf(everything, { ...OPENING_CHOICES, files: true, format: 'json' }));
    expect(one).toBe(other);
  });

  it('knows when no kind of thing is wanted', () => {
    expect(wantsNothing({ conversations: false, chats: false, documents: false })).toBe(true);
    expect(wantsNothing({ conversations: false, chats: false, documents: true })).toBe(false);
  });
});

describe('a span of days', () => {
  it('takes a day that exists, or none', () => {
    expect(isDay('')).toBe(true);
    expect(isDay('2026-10-09')).toBe(true);
    expect(isDay('2026-02-30')).toBe(false);
    expect(isDay('09/10/2026')).toBe(false);
  });

  it('is wrong when the end is before the beginning', () => {
    expect(spanFault({ from: '2026-10-09', to: '2026-10-01' })).toBe('order');
    expect(spanFault({ from: '2026-10-01', to: '2026-10-01' })).toBeNull();
    expect(spanFault({ from: '', to: '2026-10-01' })).toBeNull();
    expect(spanFault({ from: 'x', to: '' })).toBe('day');
  });
});

const GIB = 1024 ** 3;
const MIB = 1024 ** 2;

function planOf(more: Partial<ExportPlan> = {}): ExportPlan {
  return {
    node: { id: 'web', kind: 'project', name: 'Web', redacted: false },
    counts: { places: 3, conversations: 10, chats: 2, documents: 5, files: 0 },
    bytes: 180 * MIB,
    limits: { max_bytes: 2 * GIB, max_items: 10_000, keep_hours: 24 },
    left_out: [],
    blockers: [],
    ...more,
  };
}

describe('the plan of an archive', () => {
  it('can be asked for while it is inside what the place allows, and counts the things that are limited: conversations, chats and documents', () => {
    const reading = readPlan(planOf());
    expect(reading.state).toBe('ok');
    expect(reading.items).toBe(17);
    expect(reading.overBytes).toBe(0);
    expect(reading.keepHours).toBe(24);
  });

  it('says by how much the size passes what the place allows', () => {
    const reading = readPlan(planOf({ bytes: 2 * GIB + 300 * MIB, blockers: [{ code: 'export.too_large', message: 'm' }] }));
    expect(reading.state).toBe('too-large');
    expect(reading.overBytes).toBe(300 * MIB);
    expect(reading.overItems).toBe(0);
  });

  it('says by how many things the number passes what the place allows', () => {
    const reading = readPlan(planOf({ limits: { max_bytes: 2 * GIB, max_items: 10, keep_hours: 24 } }));
    expect(reading.state).toBe('too-large');
    expect(reading.overItems).toBe(7);
  });

  it('tells a place that allows none from an archive that is only too large', () => {
    expect(readPlan(planOf({ limits: { max_bytes: 0, max_items: 10_000, keep_hours: 24 }, bytes: 1 })).state).toBe('none-allowed');
  });

  it('tells that there is nothing the caller may take away', () => {
    const reading = readPlan(planOf({ counts: { places: 1, conversations: 0, chats: 0, documents: 0, files: 0 }, bytes: 0, blockers: [{ code: 'export.nothing', message: 'm' }] }));
    expect(reading.state).toBe('nothing');
  });

  it('counts what is left out, in all and by the place it lies in, and the files that were too large apart', () => {
    const place = { id: 'topic-1', kind: 'topic' as const, name: 'Fix login', redacted: false };
    const reading = readPlan(
      planOf({
        left_out: [
          { what: 'not_exportable', count: 3, where: place },
          { what: 'not_exportable', count: 6, where: { id: 'topic-2', kind: 'topic', name: null, redacted: true } },
          { what: 'too_large', count: 2, where: place },
        ],
      }),
    );
    expect(reading.notExportable).toBe(9);
    expect(reading.notExportableIn.get('topic-1')).toBe(3);
    expect(reading.tooLarge).toBe(2);
  });
});
