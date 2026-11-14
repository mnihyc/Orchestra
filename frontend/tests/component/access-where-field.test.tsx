/** Where on a provider's Use entry: left out shows Anywhere, names are added from the pickers and removed, a name that is gone is said so and still saves. */
import { cleanup, screen, waitFor, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { afterEach, beforeEach, expect, it, vi } from 'vitest';

import type * as EnvironmentsApi from '@/api/environments';
import type * as ResourcesApi from '@/api/resources';
import { EntryOptions } from '@/features/access/EntryOptions';
import { askedOptions } from '@/features/access/model';
import { EntryOptionFacts } from '@/features/access/ShareConsequences';
import { MissingAccessList } from '@/ui/ErrorState';
import type { Changes } from '@/features/access/changes';
import type { Environment, Host, Machine, PermissionPage, PermissionRow, WhereName } from '@/generated/api-types';
import { i18n, setLocale } from '@/i18n';
import { offeredChoice } from '@/i18n/choices';
import { ADA, CATALOGUE, named, pageFor } from '../unit/general-access-fixtures';
import { show } from './support';

const id = (kind: number, n: number) => `00000000-0000-4000-${kind.toString(16).padStart(4, '0')}-${n.toString().padStart(12, '0')}`;
const BUILD = id(0xa1, 1);
const STAGING = id(0xa1, 2);
const GONE = id(0xa1, 9);
const EDGE = id(0xa2, 1);
const LAPTOP = id(0xa3, 1);
const rows = vi.hoisted(() => ({ environments: [] as unknown[], hosts: [] as unknown[], machines: [] as unknown[] }));

vi.mock('@/api/environments', async (original) => ({ ...await original<typeof EnvironmentsApi>(),
  environmentsQuery: () => ({ queryKey: ['where-environments'], queryFn: () => Promise.resolve(rows.environments) }),
}));
vi.mock('@/api/resources', async (original) => ({ ...await original<typeof ResourcesApi>(),
  hostsQuery: () => ({ queryKey: ['where-hosts'], queryFn: () => Promise.resolve(rows.hosts) }),
  machinesQuery: { queryKey: ['where-machines'], queryFn: () => Promise.resolve(rows.machines) },
}));

const def = CATALOGUE.of('provider').find((each) => each.name === 'provider.use')!;
const page: PermissionPage = pageFor('provider');
const ENTRY_ID = named({ kind: 'user', user: ADA }, 7).id;

function rowWith(options: unknown): PermissionRow {
  const base = page.rows.find((row) => row.permission === 'provider.use')!;
  return { ...base, entries: [{ ...named({ kind: 'user', user: ADA }, 7), options: options }] };
}

/** Opens the entry's options and answers the change the save would make. */
function open(options: unknown) {
  const written: unknown[] = [];
  const changes: Changes = {
    run: (request) => { written.push((request as { make: (page: PermissionPage) => unknown }).make(page)); return Promise.resolve(true); },
    busy: false, doing: null, error: null, stale: null, again: () => Promise.resolve(true), forget: () => undefined,
  };
  show(<EntryOptions open onClose={() => undefined} def={def} row={rowWith(options)} entryId={ENTRY_ID} holder="Ada" changes={changes} />);
  return written;
}

const saved = (written: unknown[]): Record<string, unknown> | undefined => {
  const change = written.at(-1) as { rows: { update: { options?: Record<string, unknown> }[] }[] };
  return change.rows[0]?.update[0]?.options;
};

beforeEach(() => {
  rows.environments = [{ id: BUILD, name: 'Build box' }, { id: STAGING, name: 'Staging' }] satisfies Pick<Environment, 'id' | 'name'>[];
  rows.hosts = [{ id: EDGE, name: 'Edge host' }] satisfies Pick<Host, 'id' | 'name'>[];
  rows.machines = [{ id: LAPTOP, name: 'Ada laptop' }] satisfies Pick<Machine, 'id' | 'name'>[];
});
afterEach(async () => {
  cleanup();
  await setLocale('en');
});

const where = () => within(screen.getByRole('group', { name: i18n.t('access.options.where') }));

it.each(['en', 'zh-CN'] as const)('keeps an Ask-only provider grant explicit in the sharing editor and request in %s', async (locale) => {
  await setLocale(locale);
  const written = open({ purposes: ['ask'], models: ['gpt-x'] });
  expect(screen.getByRole('combobox', { name: i18n.t('access.options.purposes') })).toHaveTextContent(i18n.t('access.options.purpose.ask'));
  await userEvent.click(screen.getByRole('button', { name: i18n.t('access.options.save') }));
  await waitFor(() => expect(written).toHaveLength(1));
  expect(saved(written)).toMatchObject({ purposes: ['ask'], models: ['gpt-x'] });
  expect(askedOptions({ purposes: ['turn'], models: ['gpt-x'] })).toEqual({ purposes: ['turn'], models: ['gpt-x'] });
});

it('keeps a cleared purpose list as no purpose and names that result in the preview', async () => {
  const written = open({ purposes: ['ask'] });
  await userEvent.click(screen.getByRole('combobox', { name: i18n.t('access.options.purposes') }));
  await userEvent.click(screen.getByRole('option', { name: i18n.t('access.options.customPurposes') }));
  const group = within(screen.getByRole('group', { name: i18n.t('access.options.purposes') }));
  await userEvent.click(group.getByRole('checkbox', { name: i18n.t('access.options.purpose.ask') }));
  await userEvent.click(screen.getByRole('button', { name: i18n.t('access.options.save') }));
  await waitFor(() => expect(written).toHaveLength(1));
  expect(saved(written)).toEqual({ purposes: [] });
  cleanup();
  show(<EntryOptionFacts options={{ purposes: [] }} word={(text) => text} />);
  expect(screen.getByText('No purposes', { exact: false })).toBeVisible();
});

async function add(user: ReturnType<typeof userEvent.setup>, name: string) {
  await user.click(screen.getByRole('combobox', { name: i18n.t('access.options.whereAdd', { what: i18n.t('access.options.where') }) }));
  await user.click(await screen.findByRole('option', { name: new RegExp(name) }));
}

it('says Anywhere while nothing is named and writes no where member', async () => {
  const user = userEvent.setup();
  const written = open({ models: ['gpt-x'] });
  expect(where().getByText(i18n.t('access.options.anywhere'))).toBeVisible();
  await user.click(screen.getByRole('button', { name: i18n.t('access.options.save') }));
  await waitFor(() => expect(written).toHaveLength(1));
  expect(saved(written)).toEqual({ models: ['gpt-x'] });
});

it('writes both typed references when two names are chosen, an environment and a host', async () => {
  const user = userEvent.setup();
  const written = open({});
  await add(user, 'Build box');
  await add(user, 'Edge host');
  expect(await where().findByText('Build box')).toBeVisible();
  expect(await where().findByText('Edge host')).toBeVisible();
  expect(where().queryByText(i18n.t('access.options.anywhere'))).not.toBeInTheDocument();
  await user.click(screen.getByRole('button', { name: i18n.t('access.options.save') }));
  await waitFor(() => expect(written).toHaveLength(1));
  expect(saved(written)).toEqual({ where: [{ kind: 'environment', id: BUILD }, { kind: 'host', id: EDGE }] satisfies WhereName[] });
});

it('offers a machine beside environments and hosts, and does not offer a name twice', async () => {
  const user = userEvent.setup();
  const written = open({ where: [{ kind: 'environment', id: BUILD }] });
  await add(user, 'Ada laptop');
  await user.click(screen.getByRole('button', { name: i18n.t('access.options.save') }));
  await waitFor(() => expect(written).toHaveLength(1));
  expect(saved(written)).toEqual({ where: [{ kind: 'environment', id: BUILD }, { kind: 'machine', id: LAPTOP }] });
  await user.click(screen.getByRole('combobox', { name: i18n.t('access.options.whereAdd', { what: i18n.t('access.options.where') }) }));
  expect(screen.queryByRole('option', { name: /Build box/ })).not.toBeInTheDocument();
  expect(await screen.findByRole('option', { name: /Staging/ })).toBeVisible();
});

it('returns to left out when the last name is removed', async () => {
  const user = userEvent.setup();
  const written = open({ models: ['gpt-x'], where: [{ kind: 'environment', id: BUILD }, { kind: 'host', id: EDGE }] });
  expect(await where().findByText('Build box')).toBeVisible();
  await user.click(where().getByRole('button', { name: i18n.t('access.options.whereRemove', { name: 'Build box' }) }));
  await user.click(where().getByRole('button', { name: i18n.t('access.options.whereRemove', { name: 'Edge host' }) }));
  expect(where().getByText(i18n.t('access.options.anywhere'))).toBeVisible();
  await user.click(screen.getByRole('button', { name: i18n.t('access.options.save') }));
  await waitFor(() => expect(written).toHaveLength(1));
  expect(saved(written)).toEqual({ models: ['gpt-x'] });
});

it('shows a name that can no longer be read as gone, keeps it, and still saves', async () => {
  const user = userEvent.setup();
  const written = open({ where: [{ kind: 'environment', id: GONE }, { kind: 'environment', id: BUILD }] });
  expect(await where().findByText('Build box')).toBeVisible();
  expect(where().getByText(i18n.t('access.options.gone'))).toBeVisible();
  await user.click(screen.getByRole('button', { name: i18n.t('access.options.save') }));
  await waitFor(() => expect(written).toHaveLength(1));
  expect(saved(written)).toEqual({ where: [{ kind: 'environment', id: GONE }, { kind: 'environment', id: BUILD }] });
});

it.each(['en', 'zh-CN'] as const)('says in %s which environment a refusal is about', async (locale) => {
  await setLocale(locale);
  show(<MissingAccessList missing={[{
    object: { kind: 'provider', id: id(0xb1, 1), name: 'Team key', owner: ADA }, permissions: ['provider.use'],
    options: { where: [{ kind: 'environment', id: BUILD }] }, ask: [ADA], ask_more: 0, closed: null, override: false,
  }]} />);
  expect(await screen.findByText('Build box')).toBeVisible();
  expect(document.body).toHaveTextContent(locale === 'en' ? 'does not allow it in Build box' : '不允许它在 Build box 中运行');
});

it('has the words of the control and of the refusal reason in both languages', async () => {
  for (const locale of ['en', 'zh-CN'] as const) {
    await setLocale(locale);
    for (const key of ['access.options.where', 'access.options.anywhere', 'access.options.gone', 'access.options.addName', 'start.reason.not_allowed_where', 'access.missing.notWhere']) {
      expect(i18n.t(key), `${key} in ${locale}`).not.toBe(key);
    }
  }
});

it('says the places of an entry in the preview by name beside the other options', async () => {
  show(<EntryOptionFacts word={(text) => text} options={{ models: ['gpt-x'], where: [{ kind: 'environment', id: BUILD }, { kind: 'host', id: EDGE }] }} />);
  expect(await screen.findByText('Build box')).toBeVisible();
  expect(screen.getByText('Edge host')).toBeVisible();
  expect(screen.getByText('gpt-x')).toBeVisible();
  expect(screen.getByText(i18n.t('access.options.where'))).toBeVisible();
  expect(document.body).not.toHaveTextContent(BUILD);
});

it('leaves the places out of what a request for access carries', () => {
  expect(askedOptions({ models: ['gpt-x'], efforts: ['high'], where: [{ kind: 'environment', id: BUILD }] })).toEqual({ models: ['gpt-x'], efforts: ['high'] });
  expect(askedOptions({ models: ['gpt-x'] })).toEqual({ models: ['gpt-x'] });
});

const typeBox = (value: string) => screen.getByRole('checkbox', { name: offeredChoice(i18n.t, 'arrangement', value)?.name ?? value });
const consent = (value: string) => i18n.t(`access.options.leaves.${value}`);

it('shows the types in which the sign-in stays as what an entry that names none has, and writes nothing for them', async () => {
  const user = userEvent.setup();
  const written = open({ models: ['gpt-x'] });
  expect(typeBox('separated')).toBeChecked();
  expect(typeBox('key_separated')).toBeChecked();
  expect(typeBox('credential_shared')).not.toBeChecked();
  expect(typeBox('handed_over')).not.toBeChecked();
  expect(screen.queryByText(consent('credential_shared'))).not.toBeInTheDocument();
  expect(screen.queryByText(consent('handed_over'))).not.toBeInTheDocument();
  await user.click(screen.getByRole('button', { name: i18n.t('access.options.save') }));
  await waitFor(() => expect(written).toHaveLength(1));
  expect(saved(written)).toEqual({ models: ['gpt-x'] });
});

it('names a type in which the sign-in leaves by adding it to the staying ones, says what leaves, and returns to nothing written when it is taken off', async () => {
  const user = userEvent.setup();
  const written = open({});
  await user.click(typeBox('credential_shared'));
  expect(typeBox('credential_shared')).toBeChecked();
  expect(screen.getByText(consent('credential_shared'))).toBeVisible();
  expect(screen.queryByText(consent('handed_over'))).not.toBeInTheDocument();
  await user.click(screen.getByRole('button', { name: i18n.t('access.options.save') }));
  await waitFor(() => expect(written).toHaveLength(1));
  expect(saved(written)).toEqual({ arrangements: ['separated', 'key_separated', 'credential_shared'] });
  await user.click(typeBox('credential_shared'));
  expect(screen.queryByText(consent('credential_shared'))).not.toBeInTheDocument();
  await user.click(screen.getByRole('button', { name: i18n.t('access.options.save') }));
  await waitFor(() => expect(written).toHaveLength(2));
  expect(saved(written)).toEqual({});
});

it('keeps an entry that names its types as it names them, and says what leaves for each type in use', async () => {
  const user = userEvent.setup();
  const written = open({ arrangements: ['credential_shared', 'handed_over'] });
  expect(typeBox('separated')).not.toBeChecked();
  expect(typeBox('credential_shared')).toBeChecked();
  expect(typeBox('handed_over')).toBeChecked();
  expect(screen.getByText(consent('credential_shared'))).toBeVisible();
  expect(screen.getByText(consent('handed_over'))).toBeVisible();
  await user.click(screen.getByRole('button', { name: i18n.t('access.options.save') }));
  await waitFor(() => expect(written).toHaveLength(1));
  expect(saved(written)).toEqual({ arrangements: ['credential_shared', 'handed_over'] });
});

it('has the sentences about what leaves in both languages', async () => {
  for (const locale of ['en', 'zh-CN'] as const) {
    await setLocale(locale);
    for (const key of ['credential_shared', 'handed_over']) expect(consent(key), `${key} in ${locale}`).not.toContain('access.options');
    expect(i18n.t('access.options.leavesOther')).not.toBe('access.options.leavesOther');
  }
});
