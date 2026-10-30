/** What Management reads out of what the server says: the span each chart covers and how stored values become lines, when a host counts as full, what an adapter says it supports, how the authority's figures are written, the days an export spans, and what an address may say. */
import { describe, expect, it } from 'vitest';

import { groupSeries, linesOf, MOST_LINES, spanBounds, SPAN_MS, THREAD_STATES } from '@/features/admin/charts';
import { daysBetween, EXPORT_MOST_DAYS, exportName, today } from '@/features/admin/exports';
import { declaredShare, fullEffect, isFull } from '@/features/admin/hosts';
import { harnessName, hasDescriptor, parseObject, patchedDrivers, supportsOf } from '@/features/admin/harnesses';
import { daysLeft, groupFingerprint } from '@/features/admin/pki';
import { readSearch, withSearch } from '@/features/admin/search';
import { isSection, SECTIONS } from '@/features/admin/sections';
import type { Amounts, MetricSeries } from '@/generated/api-types';
import { alignLines, figuresOf, largestOf, nearestIndex, sampleRows, scaleTop, withAlpha } from '@/ui/timeSeries';

const amounts = (memory_mb: number): Amounts => ({ cpu_millis: 16_000, disk_gb: 500, memory_mb });

const series = (name: string, labels: Record<string, string>, points: [string, number][]): MetricSeries => ({
  name,
  labels,
  resolution: 'minute',
  points,
  peaks: null,
});

describe('the span a chart covers', () => {
  const now = Date.parse('2026-10-12T03:41:27.000Z');

  it('ends at the minute it was asked in, so that the same minute asks the same thing', () => {
    expect(new Date(spanBounds('day', now).to).toISOString()).toBe('2026-10-12T03:41:00.000Z');
    expect(spanBounds('day', now)).toEqual(spanBounds('day', now + 20_000));
  });

  it('is as long as the reader chose, and a month is one hour short of thirty days so that hourly steps still fit', () => {
    const day = spanBounds('day', now);
    expect(day.to - day.from).toBe(24 * 3_600_000);
    const month = spanBounds('month', now);
    expect(month.to - month.from).toBe(719 * 3_600_000);
    // The server answers at an hour only where a span touches at most 720 of them.
    expect(Math.floor(month.to / 3_600_000) - Math.floor(month.from / 3_600_000) + 1).toBeLessThanOrEqual(720);
  });

  it('never asks for more days than one request is answered for at the step of a day', () => {
    const all = spanBounds('all', now);
    expect(Math.floor(all.to / 86_400_000) - Math.floor(all.from / 86_400_000) + 1).toBeLessThanOrEqual(720);
    expect(SPAN_MS.all).toBeGreaterThan(SPAN_MS.year);
  });
});

describe('the lines a chart draws', () => {
  const at = (minute: number) => new Date(Date.UTC(2026, 9, 12, 3, minute)).toISOString();

  it('adds the series that share a label value, moment by moment, and keeps each value apart', () => {
    const groups = groupSeries(
      [
        series('threads', { state: 'running', host: 'a' }, [[at(0), 2], [at(5), 3]]),
        series('threads', { state: 'running', host: 'b' }, [[at(0), 1]]),
        series('threads', { state: 'idle', host: 'a' }, [[at(0), 7]]),
        series('other', { state: 'running' }, [[at(0), 100]]),
      ],
      'threads',
      'state',
      'sum',
    );
    expect(groups.map(([value]) => value)).toEqual(['running', 'idle']);
    expect(groups[0]?.[1]).toEqual([
      [Date.parse(at(0)), 3],
      [Date.parse(at(5)), 3],
    ]);
  });

  it('averages a duration where several series have a point at one moment, and does not add it', () => {
    const [group] = groupSeries([series('turn_seconds', { h: 'a' }, [[at(0), 30]]), series('turn_seconds', { h: 'b' }, [[at(0), 50]])], 'turn_seconds', null, 'mean');
    expect(group?.[1]).toEqual([[Date.parse(at(0)), 40]]);
  });

  it('gives a state its own colour whichever of the others are there, and the states it does not know the colours after them', () => {
    const point = (value: number): [number, number][] => [[1, value]];
    const some = linesOf([['idle', point(1)], ['running', point(2)]], THREAD_STATES, (value) => value, 'Other');
    const more = linesOf([['done', point(1)], ['idle', point(1)], ['running', point(2)], ['starting-up', point(3)]], THREAD_STATES, (value) => value, 'Other');
    const slotOf = (lines: typeof some, id: string) => lines.find((line) => line.id === id)?.slot;
    expect(slotOf(some, 'running')).toBe(1);
    expect(slotOf(some, 'idle')).toBe(5);
    // Survivors keep their colour when others come and go.
    expect(slotOf(more, 'running')).toBe(slotOf(some, 'running'));
    expect(slotOf(more, 'idle')).toBe(slotOf(some, 'idle'));
    expect(slotOf(more, 'starting-up')).toBe(THREAD_STATES.length + 1);
  });

  it('adds what is past the last colour together, and never makes up a colour', () => {
    const many = Array.from({ length: 12 }, (_, at): [string, [number, number][]] => [`label-${String(at).padStart(2, '0')}`, [[1, 1]]]);
    const lines = linesOf(many, [], (value) => value, 'Everything else');
    expect(lines).toHaveLength(MOST_LINES);
    expect(lines.at(-1)).toMatchObject({ label: 'Everything else', slot: MOST_LINES });
    expect(lines.at(-1)?.points).toEqual([[1, 12 - (MOST_LINES - 1)]]);
    expect(Math.max(...lines.map((line) => line.slot))).toBeLessThanOrEqual(MOST_LINES);
  });
});

describe('lines laid on one list of moments', () => {
  it('has a value for each line at each moment, or none where a line has none', () => {
    const aligned = alignLines([
      { id: 'a', label: 'A', slot: 1, points: [[10, 1], [30, 3]] },
      { id: 'b', label: 'B', slot: 2, points: [[20, 2], [30, 4]] },
    ]);
    expect(aligned.xs).toEqual([10, 20, 30]);
    expect(aligned.ys).toEqual([
      [1, null, 3],
      [null, 2, 4],
    ]);
  });

  it('says what a line is now and the most it has been, over what it had values for', () => {
    expect(figuresOf([1, 9, null, 4, null])).toEqual({ now: 4, peak: 9 });
    expect(figuresOf([null, null])).toEqual({ now: null, peak: null });
    expect(largestOf({ xs: [1, 2], ys: [[1, null], [null, 6]] })).toBe(6);
  });

  it('puts the top of the scale a little above the largest value, and gives an empty chart a scale of its own', () => {
    expect(scaleTop(10)).toBeGreaterThan(10);
    expect(scaleTop(0)).toBe(1);
    expect(scaleTop(null)).toBe(1);
  });

  it('finds the moment nearest a point, and the table view keeps its first and last row among an even spread', () => {
    expect(nearestIndex([10, 20, 40], 26)).toBe(1);
    expect(nearestIndex([10, 20, 40], 33)).toBe(2);
    expect(nearestIndex([], 5)).toBeNull();
    const rows = sampleRows(1000, 24);
    expect(rows).toHaveLength(24);
    expect(rows[0]).toBe(0);
    expect(rows.at(-1)).toBe(999);
    expect(sampleRows(5, 24)).toEqual([0, 1, 2, 3, 4]);
  });

  it('writes a colour at a share of its strength', () => {
    expect(withAlpha('#2a78d6', 0.1)).toBe('rgba(42, 120, 214, 0.1)');
    expect(withAlpha('not a colour', 0.1)).toBe('not a colour');
  });
});

describe('when a host counts as full', () => {
  it('is when the memory its units declare reaches the share of what it reports, and a host that reports none is never judged', () => {
    expect(isFull(amounts(1000), amounts(900), 90)).toBe(true);
    expect(isFull(amounts(1000), amounts(899), 90)).toBe(false);
    expect(isFull(amounts(0), amounts(500), 90)).toBe(false);
    expect(declaredShare(amounts(2000), amounts(500))).toBe(25);
    expect(declaredShare(amounts(0), amounts(500))).toBe(0);
  });

  it('says what being full does there, and whether that is the host’s own word', () => {
    const host = { capacity: amounts(1000), declared: amounts(950) };
    expect(fullEffect({ ...host, when_full: { own: false, value: 'warn' } }, 90)).toEqual({ full: true, does: 'warn', own: false });
    expect(fullEffect({ ...host, when_full: { own: true, value: 'block' } }, 99)).toEqual({ full: false, does: 'block', own: true });
  });
});

describe('what an adapter says it supports', () => {
  it('is read from its descriptor, member by member, and a member it does not hold is said as not held', () => {
    expect(supportsOf({ capabilities: { fork_native: true, input_images: false, steer: true, agent_terminal: 'view', phases: true, memory: 'directory' } })).toEqual({
      fork: 'yes',
      images: 'no',
      steering: 'yes',
      terminal: 'view',
      phases: 'yes',
      memory: 'directory',
    });
    expect(supportsOf({ capabilities: { memory: 'none' } })).toMatchObject({ phases: 'unsaid', fork: 'unsaid', terminal: 'unsaid', memory: 'none' });
    expect(supportsOf(null)).toMatchObject({ memory: 'unsaid', steering: 'unsaid' });
  });

  it('has declared a descriptor only when it has said something: an adapter that has not yet run has none to read', () => {
    expect(hasDescriptor({ harness: { name: 'Codex' } })).toBe(true);
    expect(hasDescriptor(null)).toBe(false);
    expect(hasDescriptor({})).toBe(false);
    expect(hasDescriptor([])).toBe(false);
    expect(hasDescriptor('codex')).toBe(false);
  });

  it('tells the three kinds of memory apart, and knows no other', () => {
    for (const memory of ['directory', 'home', 'none'] as const) expect(supportsOf({ capabilities: { memory } }).memory).toBe(memory);
    expect(supportsOf({ capabilities: { memory: 'everything' } }).memory).toBe('unsaid');
  });

  it('takes the name an adapter gives itself, and the drivers a patch is for', () => {
    expect(harnessName({ harness: { name: 'Claude Code' } })).toBe('Claude Code');
    expect(harnessName({ harness: {} })).toBeNull();
    expect(patchedDrivers({ podman: {}, docker: {} })).toEqual(['docker', 'podman']);
  });

  it('reads a JSON object a person typed, an empty field as an empty object, and nothing else', () => {
    expect(parseObject('{"docker":{"privileged":true}}')).toEqual({ ok: true, value: { docker: { privileged: true } } });
    expect(parseObject('  ')).toEqual({ ok: true, value: {} });
    expect(parseObject('[1]').ok).toBe(false);
    expect(parseObject('{oops').ok).toBe(false);
  });
});

describe('the authority’s figures', () => {
  it('writes a fingerprint in the pairs a machine shows when it enrolls', () => {
    expect(groupFingerprint('a3f1c2d4')).toBe('A3:F1:C2:D4');
    expect(groupFingerprint('')).toBe('');
  });

  it('counts whole days left, and none once the moment has passed', () => {
    const now = Date.parse('2026-10-12T00:00:00.000Z');
    expect(daysLeft('2026-10-25T12:00:00.000Z', now)).toBe(13);
    expect(daysLeft('2026-10-01T00:00:00.000Z', now)).toBe(0);
  });
});

describe('the span of an export', () => {
  it('counts the days from one day to another, negative where the second is before the first', () => {
    expect(daysBetween('2026-10-01', '2026-10-31')).toBe(30);
    expect(daysBetween('2026-10-05', '2026-10-01')).toBe(-4);
    expect(Number.isNaN(daysBetween('', '2026-10-01'))).toBe(true);
  });

  it('holds a year and a day at most, which is what a file may span', () => {
    expect(EXPORT_MOST_DAYS).toBe(366);
    expect(daysBetween('2025-10-12', '2026-10-12') + 1).toBeLessThanOrEqual(EXPORT_MOST_DAYS);
  });

  it('names the day it is today in UTC and the file after its span and its format', () => {
    expect(today(0, Date.parse('2026-10-12T23:59:00Z'))).toBe('2026-10-12');
    expect(today(-30, Date.parse('2026-10-12T00:10:00Z'))).toBe('2026-09-12');
    expect(exportName('2026-09-12', '2026-10-12', 'jsonl')).toBe('orch-audit-2026-09-12-to-2026-10-12.jsonl');
  });
});

describe('what an address may say', () => {
  it('keeps what is one of the things it may be and drops the rest', () => {
    expect(readSearch({ q: ' ada ', status: 'suspended', admins: 'true', span: 'week', from: '2026-10-01', to: 'tomorrow', kind: 'provider', bogus: 1 })).toEqual({
      q: 'ada',
      status: 'suspended',
      admins: true,
      span: 'week',
      from: '2026-10-01',
      kind: 'provider',
    });
    expect(readSearch({ status: 'gone', span: 'decade', kind: 'both' })).toEqual({});
  });

  it('leaves out a member set to nothing, and keeps the rest', () => {
    expect(withSearch({ q: 'ada', status: 'active' }, { q: '', span: 'month' })).toEqual({ status: 'active', span: 'month' });
    expect(withSearch({ q: 'ada' }, { q: undefined })).toEqual({});
  });

  it('keeps an agent program and its selected tab in its address', () => {
    expect(readSearch({ program: ' research-agent ', programTab: 'capabilities' })).toEqual({ program: 'research-agent', programTab: 'capabilities' });
    expect(readSearch({ program: 'research-agent', programTab: 'unknown' })).toEqual({ program: 'research-agent' });
    expect(withSearch({ program: 'research-agent', programTab: 'settings' }, { program: undefined })).toEqual({});
  });

  it('names every working section once, including system resource and access editors', () => {
    const ids = SECTIONS.map((section) => section.id);
    expect(new Set(ids).size).toBe(ids.length);
    expect(isSection('overview')).toBe(true);
    expect(isSection('settings')).toBe(true);
    for (const section of ['email', 'templates', 'mcp', 'defaults', 'rates']) expect(isSection(section)).toBe(true);
    expect(isSection('nothing')).toBe(false);
  });
});
