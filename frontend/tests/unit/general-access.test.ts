/** General access presets and changing a followed holder without rebuilding the page. */
import { describe, expect, it } from 'vitest';

import { editFollowedHolderChange, forkRowChange, generalAccessChange, generalAccessOf, generalHolder, holderLevelChange, linesOf, removeHolderChange, wholeChange } from '@/features/access/model';

import { BEA, CATALOGUE, generalRole, named, pageFor, role, TOPIC } from './general-access-fixtures';

describe('general access', () => {
  it.each(['view', 'interact', 'manage'] as const)('%s at creation cuts every row and explicitly names every line up to the level', (preset) => {
    const change = generalAccessChange(CATALOGUE, 'thread', TOPIC.id, preset);
    const expected = ['chat.view', 'doc.view', 'thread.view', ...(preset === 'view' ? [] : ['chat.interact', 'doc.edit', 'thread.interact']), ...(preset === 'manage' ? ['chat.manage', 'doc.manage', 'thread.manage'] : [])];
    expect(change.rows?.filter((row) => row.add !== undefined).map((row) => row.permission).sort()).toEqual(expected.sort());
    expect(change.rows?.every((row) => row.inherit === false)).toBe(true);
    for (const row of change.rows ?? []) for (const entry of row.add ?? []) expect(entry.holder).toEqual({ kind: 'role', role: 'viewer', at: TOPIC.id });
    expect(change.rows?.map((row) => row.permission)).not.toContain('access.own');
  });

  it('has an empty cut page for Private and the existing reset for Defaults', () => {
    expect(generalAccessChange(CATALOGUE, 'thread', TOPIC.id, 'private').rows?.every((row) => row.inherit === false && row.add === undefined)).toBe(true);
    const page = pageFor();
    expect(generalAccessChange(CATALOGUE, 'thread', TOPIC.id, 'defaults', page)).toEqual(wholeChange(page, 'reset'));
  });

  it('keeps a directly added person’s level and updates an existing general entry without removing and re-adding its identifier', () => {
    const base = pageFor();
    const page = { ...base, rows: base.rows.map((row, i) => ({ ...row, inherit: false, inherited: null, entries: [named({ kind: 'user', user: BEA }, i + 10), ...(row.permission === 'thread.view' ? [named(generalRole(), 80, { expires_at: '2026-12-01T00:00:00Z' })] : [])] })) };
    const change = generalAccessChange(CATALOGUE, 'thread', TOPIC.id, 'view', page);
    expect(change.expected_revision).toBe(7);
    expect(change.rows?.flatMap((row) => row.remove ?? [])).toEqual([]);
    const view = change.rows?.find((row) => row.permission === 'thread.view');
    expect(view?.add).toBeUndefined();
    expect(view?.update).toEqual([{ entry_id: named(generalRole(), 80).id, options: {}, expires_at: null }]);
    expect(change.rows?.some((row) => row.permission === 'thread.manage')).toBe(true);
  });

  it.each(['view', 'interact', 'manage'] as const)('recognizes %s with an added person who manages it', (preset) => {
    const base = pageFor();
    const level = preset === 'view' ? ['full_view'] as const : preset === 'interact' ? ['full_view', 'full_interact'] as const : ['full_view', 'full_interact', 'full_manage'] as const;
    const held = new Set(level.flatMap((choice) => CATALOGUE.choice(choice, 'thread')));
    const page = { ...base, rows: base.rows.map((row, i) => ({ ...row, inherit: false, inherited: null, entries: [...(held.has(row.permission) ? [named(generalRole(), i + 2)] : []), ...(row.permission === 'thread.manage' ? [named({ kind: 'user', user: BEA }, 80)] : [])] })) };
    expect(generalAccessOf(page, CATALOGUE, 'thread', TOPIC.id)).toBe(preset);
  });

  it('recognizes Defaults and Private; mixed, incomplete or constrained audiences are Custom', () => {
    const base = pageFor();
    expect(generalAccessOf(base, CATALOGUE, 'thread', TOPIC.id)).toBe('defaults');
    expect(generalAccessOf({ ...base, rows: base.rows.map((row) => ({ ...row, inherit: false, inherited: null })) }, CATALOGUE, 'thread', TOPIC.id)).toBe('private');
    const mixed = { ...base, rows: base.rows.map((row) => row.permission === 'thread.interact' ? { ...row, inherit: false } : row) };
    expect(generalAccessOf(mixed, CATALOGUE, 'thread', TOPIC.id)).toBe('custom');
    const partial = { ...base, rows: base.rows.map((row) => ({ ...row, inherit: false, entries: row.permission === 'thread.view' ? [named(generalRole(), 9)] : [] })) };
    expect(generalAccessOf(partial, CATALOGUE, 'thread', TOPIC.id)).toBe('custom');
    const limited = { ...partial, rows: partial.rows.map((row) => ({ ...row, entries: row.entries.map((entry) => ({ ...entry, expires_at: '2026-12-01T00:00:00Z' })) })) };
    expect(generalAccessOf(limited, CATALOGUE, 'thread', TOPIC.id)).toBe('custom');
    expect(generalHolder(base.object.id, 'org')).toEqual({ kind: 'members', id: base.object.id });
  });
});

describe('a local edit of inherited holders', () => {
  it('changes a followed holder’s end in that row and keeps the other inherited holders', () => {
    const page = pageFor('document');
    const row = { ...page.rows.find((candidate) => candidate.permission === 'doc.edit')!, inherited: { from: TOPIC, holders: [role('interactor'), role('developer')] } };
    const current = { ...page, rows: page.rows.map((candidate) => candidate.permission === 'doc.edit' ? row : candidate) };
    const change = editFollowedHolderChange(current, row, role('interactor'), { expires_at: '2026-12-01T00:00:00Z' });
    expect(change.rows?.map((edit) => edit.permission)).toEqual(['doc.edit']);
    expect(change.rows?.[0]?.inherit).toBe(false);
    expect(change.rows?.[0]?.add).toContainEqual({ holder: { kind: 'role', role: 'interactor' }, expires_at: '2026-12-01T00:00:00Z' });
    expect(change.rows?.[0]?.add).toContainEqual({ holder: { kind: 'role', role: 'developer' } });
  });
  it('keeps a followed entry’s limited models and expiry when its row is frozen', () => {
    const base = pageFor('provider');
    const parent = named({ kind: 'user', user: BEA }, 80, { options: { models: ['small'] }, expires_at: '2026-12-01T00:00:00Z', note: 'Limited plan' });
    const row = { ...base.rows.find((item) => item.permission === 'provider.use')!, inherited: { from: TOPIC, holders: [parent.holder, role('developer')], entries: [parent] } };
    expect(forkRowChange(row, new Set(['role:developer@'])).add).toEqual([{ holder: { kind: 'user', id: BEA.id }, options: { models: ['small'] }, expires_at: '2026-12-01T00:00:00Z', note: 'Limited plan' }]);
  });
  it('freezes one followed row with its other holders and own entries, and sends no other row', () => {
    const base = pageFor('document');
    const row = { ...base.rows.find((item) => item.permission === 'doc.edit')!, inherited: { from: TOPIC, holders: [role('interactor'), role('developer')] }, entries: [named({ kind: 'user', user: BEA }, 80)] };
    const change = forkRowChange(row, new Set(['role:interactor@']));
    expect(change).toEqual({ permission: 'doc.edit', inherit: false, add: [{ holder: { kind: 'role', role: 'developer' } }] });
    expect(change.remove).toBeUndefined();
    expect(base.rows.find((item) => item.permission === 'doc.view')?.inherit).toBe(true);
  });

  it('removes a followed Take part holder, leaves View following and reads Custom', () => {
    const base = pageFor();
    const holder = linesOf(base, CATALOGUE, 'thread').interact[0]!;
    const change = removeHolderChange(base, holder);
    expect(change.expected_revision).toBe(7);
    expect(change.rows?.map((row) => row.permission).sort()).toEqual(['chat.interact', 'doc.edit', 'thread.interact']);
    expect(change.rows?.every((row) => row.inherit === false && row.add === undefined)).toBe(true);
    const changed = { ...base, rows: base.rows.map((row) => change.rows?.some((edit) => edit.permission === row.permission) === true ? { ...row, inherit: false, inherited: null } : row) };
    expect(generalAccessOf(changed, CATALOGUE, 'thread', TOPIC.id)).toBe('custom');
    expect(changed.rows.find((row) => row.permission === 'thread.view')?.inherit).toBe(true);
  });

  it('changes a followed holder’s level while leaving unrelated advanced rows following', () => {
    const page = pageFor();
    const holder = linesOf(page, CATALOGUE, 'thread').interact[0]!;
    const change = holderLevelChange(page, CATALOGUE, 'thread', holder, 'full_manage');
    expect(change.rows?.some((row) => row.permission === 'thread.manage' && row.inherit === false && row.add?.some((entry) => entry.holder.kind === 'role' && entry.holder.role === 'interactor') === true)).toBe(true);
    expect(change.rows?.some((row) => row.permission === 'thread.fork')).toBe(false);
    expect(change.rows?.find((row) => row.permission === 'thread.manage')?.add).toContainEqual({ holder: { kind: 'role', role: 'developer' } });
  });

  it('lowers Interactors to View without cutting the View rows they still hold through Viewers', () => {
    const page = pageFor();
    const holder = linesOf(page, CATALOGUE, 'thread').interact[0]!;
    const change = holderLevelChange(page, CATALOGUE, 'thread', holder, 'full_view');
    expect(change.rows?.map((row) => row.permission).sort()).toEqual(['chat.interact', 'doc.edit', 'thread.interact']);
    expect(change.rows?.some((row) => row.permission.endsWith('.view'))).toBe(false);
  });
});
