/** Administrative reach never adds another person's space or an unrelated team to the activity bar. */
import { expect, it } from 'vitest';
import { railOrgs } from '@/shell/org';
import { navigationRoot, readableNavigationRoot } from '@/features/navigation/currentOrg';
import { me, org, personalHomeOf, userRef } from '../doubles/fixtures';

it('keeps the signed-in person and participating teams, independently of administrative reach', () => {
  const person = me(1, 'ada', 'Ada');
  const owner = userRef(person);
  const personal = org(personalHomeOf(person), 'ada', 'Ada', owner, { org_kind: 'personal' });
  const others = org('other', 'other', 'Other person', owner, { org_kind: 'personal' });
  const team = org('team', 'team', 'Team', owner);
  const managed = org('managed', 'managed', 'Managed team', owner, { access: { ...team.access, participating: false, administers: true } });
  for (const rows of [[personal, team], [personal, team, others, managed]]) {
    expect(railOrgs(rows, person).map((row) => row.id)).toEqual([personal.id, team.id]);
  }
  expect(railOrgs([personal, others, managed], undefined)).toEqual([]);
});

it('waits for the addressed place, without briefly opening the remembered org', () => {
  const person = me(1, 'ada', 'Ada');
  const team = org('team', 'acme', 'Acme', userRef(person));
  expect(navigationRoot('/thread/new', [team], person.personal_org_id, {})).toBeNull();
  expect(navigationRoot('/thread/new', [team], person.personal_org_id, { thread: team.id })).toBe(team.id);
  expect(navigationRoot('/topic/new', [team], person.personal_org_id, { node: team.id })).toBe(team.id);
  expect(navigationRoot('/doc/new', [team], person.personal_org_id, {})).toBeNull();
  expect(navigationRoot('/doc/new', [team], person.personal_org_id, { heading: team.id })).toBe(team.id);
  expect(navigationRoot('/org/acme', [team], person.personal_org_id, {})).toBe(team.id);
  expect(navigationRoot('/org/acme.research', [team], person.personal_org_id, { org: team.id })).toBe(team.id);
  for (const path of ['/settings/profile', '/admin/settings', '/help', '/help/approvals', '/usage', '/inbox', '/everything']) {
    expect(navigationRoot(path, [team], person.personal_org_id, {})).toBeNull();
  }
  expect(navigationRoot('/resources', [team], person.personal_org_id, {})).toBeNull();
  expect(navigationRoot('/resources/providers', [team], person.personal_org_id, {})).toBeNull();
});


it('keeps the selected org on global pages and an already displayed tree while a new place loads', () => {
  const person = me(1, 'ada', 'Ada');
  const team = org('team', 'acme', 'Acme', userRef(person));
  for (const path of ['/settings/profile', '/admin/settings', '/help', '/usage']) {
    expect(navigationRoot(path, [team], team.id, {})).toBe(team.id);
  }
  expect(navigationRoot('/thread/new', [team], team.id, {})).toBeNull();
  expect(navigationRoot('/thread/new', [team], team.id, { retained: team.id })).toBe(team.id);
  expect(navigationRoot('/thread/new', [team], team.id, { retained: team.id, thread: 'other-org' })).toBe('other-org');
});


it('a redacted breadcrumb never chooses an unreadable organization over the retained tree', () => {
  const hidden = [{ id: 'private-org', kind: 'org' as const, name: null, redacted: true }];
  expect(readableNavigationRoot(hidden, 'stale-cached-root')).toBeNull();
  expect(readableNavigationRoot([{ ...hidden[0]!, name: 'Readable org', redacted: false }])).toBe('private-org');
  expect(navigationRoot('/doc/shared', [], null, { heading: readableNavigationRoot(hidden), retained: 'readable-tree' })).toBe('readable-tree');
});
