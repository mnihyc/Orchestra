/** A skill's page shows its latest version, origin and files; its managers edit the draft and publish it; a place switches skills on at their latest or a pinned version. */
import { QueryClient, QueryClientProvider } from '@tanstack/react-query';
import { cleanup, screen, waitFor, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { useState, type ReactNode } from 'react';
import type * as Router from '@tanstack/react-router';
import { afterEach, beforeEach, expect, it, vi } from 'vitest';
import { ApiError } from '@/api/client';
import type * as SkillsApi from '@/api/skills';
import { SkillScreen } from '@/features/resources/skills/Skill';
import { SkillsSettingChoice, SkillsSummary } from '@/features/settings/SkillsChoice';
import type { ResourceAccess, Skill, SkillDraft, SkillVersion, SkillVersionInfo } from '@/generated/api-types';
import { i18n, setLocale } from '@/i18n';
import { ADA } from '../doubles/chat-rows';
import { show } from './support';

const manage: ResourceAccess = { permissions: ['skill.view', 'skill.use', 'skill.manage'], administers: false, participating: true, admin_only: [], through: [{ kind: 'owner' }] };
const view: ResourceAccess = { permissions: ['skill.view'], administers: false, participating: true, admin_only: [], through: [] };
const home = { id: 'team', kind: 'org' as const, name: 'Workshop', redacted: false };
const first: SkillVersionInfo = { id: 'v1', number: 1, name: 'report-writer', description: 'Writes reports.', size: 120, file_count: 2,
  archive: { blob: 'a'.repeat(64), size: 300 }, origin: { kind: 'upload', name: 'report-writer.zip', folder: null }, created_by: ADA, created_at: '2026-10-20T10:00:00.000Z' };
const second: SkillVersionInfo = { ...first, id: 'v2', number: 2, origin: { kind: 'environment', environment_id: 'env', path: 'skills/report-writer' }, created_at: '2026-10-21T10:00:00.000Z' };
const draftInfo = { based_on: 'v2', file_count: 2, revision: 4, updated_by: ADA, updated_at: '2026-10-22T10:00:00.000Z' };
const skill: Skill = { id: 'skill', name: 'report-writer', description: 'Writes reports.', org_id: 'team', owner: ADA, home, address: 'workshop.ada.report-writer', enabled: true,
  latest: second, versions: 2, origin: second.origin, draft: draftInfo, revision: 3, access: manage };
const version: SkillVersion = { version: second, files: [
  { path: 'SKILL.md', size: 80, sha256: 'b'.repeat(64), executable: false },
  { path: 'scripts/report.sh', size: 40, sha256: 'c'.repeat(64), executable: true },
] };
const draft: SkillDraft = { info: draftInfo, files: [
  { path: 'SKILL.md', size: 80, executable: false, updated_by: ADA, updated_at: '2026-10-22T10:00:00.000Z' },
  { path: 'scripts/report.sh', size: 40, executable: true, updated_by: ADA, updated_at: '2026-10-22T10:00:00.000Z' },
] };
const texts: Record<string, string> = { 'SKILL.md': '---\nname: report-writer\ndescription: Writes reports.\n---\n', 'scripts/report.sh': '#!/bin/sh\necho report\n' };

const current = vi.hoisted(() => ({ skill: undefined as unknown as Skill }));
const calls = vi.hoisted(() => ({ put: vi.fn(), publish: vi.fn(), reimport: vi.fn(), update: vi.fn() }));
vi.mock('@/api/skills', async (load) => {
  const actual = await load<typeof SkillsApi>();
  return { ...actual,
    skillQuery: (id: string) => ({ queryKey: actual.skillKey(id), queryFn: () => Promise.resolve(current.skill), staleTime: Infinity }),
    skillsQuery: { queryKey: ['skills'], queryFn: () => Promise.resolve(Object.assign([current.skill, { ...current.skill, id: 'other', name: 'pdf-tools', access: manage }], { privateRows: [] })), staleTime: Infinity },
    skillVersionsQuery: (id: string) => ({ queryKey: actual.skillVersionsKey(id), queryFn: () => Promise.resolve({ items: [second, first], next_cursor: null }), staleTime: Infinity }),
    skillVersionQuery: (id: string, versionId: string) => ({ queryKey: ['skill-version', id, versionId], queryFn: () => Promise.resolve(version), staleTime: Infinity }),
    skillVersionFileQuery: (id: string, versionId: string, path: string) => ({ queryKey: ['skill-version-file', id, versionId, path], queryFn: () => Promise.resolve({ text: texts[path] ?? null, bytes: 1 }), staleTime: Infinity }),
    skillDraftQuery: (id: string) => ({ queryKey: actual.skillDraftKey(id), queryFn: () => Promise.resolve(draft), staleTime: Infinity }),
    skillDraftFileQuery: (id: string, path: string, revision: number) => ({ queryKey: ['skill-draft-file', id, path, revision], queryFn: () => Promise.resolve({ text: texts[path] ?? null, bytes: 1 }), staleTime: Infinity }),
    putSkillDraftFile: calls.put, publishSkillDraft: calls.publish, reimportSkill: calls.reimport, updateSkill: calls.update,
  };
});
vi.mock('@/ui/CodeView', () => ({ CodeView: ({ text, label, onChange, readOnly }: { text: string; label: string; readOnly?: boolean; onChange?: (text: string) => void }) =>
  <textarea aria-label={label} defaultValue={text} readOnly={readOnly} onChange={(event) => onChange?.(event.target.value)} /> }));
vi.mock('@tanstack/react-router', async (load) => ({ ...await load<typeof Router>(), useNavigate: () => vi.fn(), useLocation: () => ({ pathname: '/resources/skills/skill', searchStr: '' }),
  Link: ({ children, className }: { children: ReactNode; className?: string }) => <a className={className}>{children}</a> }));
vi.mock('@/features/resources/trail', () => ({ useResourceHeading: () => undefined }));
vi.mock('@/features/help/open', () => ({ useHelpOpener: () => vi.fn() }));
vi.mock('@/features/help/Hint', () => ({ Hint: () => null }));
vi.mock('@/features/resources/ResourceAccess', () => ({ ResourceAccess: () => <p>access-page</p> }));
vi.mock('@/shell/Page', async (load) => ({ ...await load<object>(), Page: ({ children }: { children: ReactNode }) => <main>{children}</main>,
  PageHeader: ({ title, actions }: { title: string; actions?: ReactNode }) => <header><h1>{title}</h1>{actions}</header> }));

function draw(content: ReactNode) {
  const client = new QueryClient({ defaultOptions: { queries: { retry: false, staleTime: Infinity }, mutations: { retry: false } } });
  return show(<QueryClientProvider client={client}>{content}</QueryClientProvider>);
}

beforeEach(() => { vi.clearAllMocks(); current.skill = skill; });
afterEach(async () => { cleanup(); await setLocale('en'); });

it.each(['en', 'zh-CN'] as const)('shows the latest version, where it came from and its files, with Update for a directory it was read from (%s)', async (locale) => {
  await setLocale(locale);
  draw(<SkillScreen id="skill" />);
  expect(await screen.findByRole('heading', { level: 1, name: 'report-writer' })).toBeVisible();
  const tabs = within(screen.getByRole('tablist')).getAllByRole('tab').map((tab) => tab.textContent);
  expect(tabs).toEqual([i18n.t('resourceDetails.overview'), i18n.t('resourceDetails.skill.versions'), i18n.t('resourceDetails.skill.draft.title'), i18n.t('agents.tabs.access')]);
  expect(screen.getByText(i18n.t('resourceDetails.skill.origin.environment', { path: 'skills/report-writer' }))).toBeVisible();
  expect(screen.getByText(i18n.t('resourceDetails.skill.fileCount', { count: 2 }), { exact: false })).toBeVisible();
  expect(await screen.findByRole('button', { name: /scripts\/report\.sh/ })).toBeVisible();
  expect(screen.getByRole('button', { name: i18n.t('resourceDetails.skill.reimport') })).toBeVisible();
});

it('offers neither the draft nor Update to whoever only sees the skill', async () => {
  current.skill = { ...skill, access: view, draft: null };
  draw(<SkillScreen id="skill" />);
  await screen.findByRole('heading', { level: 1, name: 'report-writer' });
  expect(within(screen.getByRole('tablist')).queryByRole('tab', { name: i18n.t('resourceDetails.skill.draft.title') })).not.toBeInTheDocument();
  expect(screen.queryByRole('button', { name: i18n.t('resourceDetails.skill.reimport') })).not.toBeInTheDocument();
});

it('updates from where the skill came from and says when nothing changed', async () => {
  calls.reimport.mockResolvedValue({ version: second, made: false });
  const user = userEvent.setup();
  draw(<SkillScreen id="skill" />);
  await user.click(await screen.findByRole('button', { name: i18n.t('resourceDetails.skill.reimport') }));
  expect(await screen.findByText(i18n.t('resourceDetails.skill.upToDate'))).toBeVisible();
  expect(calls.reimport).toHaveBeenCalledExactlyOnceWith('skill');
});

it('edits a text file of the draft, marks a script executable and publishes the draft at the revision it read', async () => {
  calls.put.mockImplementation((_id: string, body: { path: string }) => Promise.resolve({ ...draft, info: { ...draft.info, revision: draft.info.revision + 1 }, files: draft.files.map((file) => file.path === body.path ? { ...file } : file) }));
  calls.publish.mockResolvedValue(version);
  const user = userEvent.setup();
  function Draft() { const [tab, setTab] = useState<'overview' | 'versions' | 'draft' | 'access'>('draft'); return <SkillScreen id="skill" tab={tab} onTab={setTab} />; }
  draw(<Draft />);
  const editor = await screen.findByRole('textbox', { name: 'SKILL.md' });
  await user.type(editor, 'More steps.');
  await user.click(screen.getByRole('button', { name: i18n.t('resources.edit.save') }));
  await waitFor(() => expect(calls.put).toHaveBeenCalledTimes(1));
  expect(calls.put.mock.calls[0]?.[1]).toMatchObject({ path: 'SKILL.md', expected_revision: 4 });
  expect((calls.put.mock.calls[0]?.[1] as { text: string }).text).toContain('More steps.');

  await user.click(screen.getByRole('button', { name: /scripts\/report\.sh/ }));
  await screen.findByRole('textbox', { name: 'scripts/report.sh' });
  await user.click(screen.getByRole('switch', { name: i18n.t('resourceDetails.skill.executable') }));
  await waitFor(() => expect(calls.put).toHaveBeenCalledTimes(2));
  // Each save answers with the draft at its next revision, which the following change names.
  expect(calls.put.mock.calls[1]?.[1]).toMatchObject({ path: 'scripts/report.sh', executable: false, expected_revision: 5 });

  await user.click(screen.getByRole('button', { name: i18n.t('resourceDetails.skill.draft.publish') }));
  await waitFor(() => expect(calls.publish).toHaveBeenCalledExactlyOnceWith('skill', 5));
});

it('says why a draft is refused and what the refusal is about', async () => {
  calls.publish.mockRejectedValue(new ApiError({
    type: 'orch:error:request.value_refused', code: 'request.value_refused', title: 'Refused', status: 422, detail: 'refused', request_id: 'r1',
    errors: [{ path: 'files', message: 'The front matter gives no description' }], about: ['SKILL.md'],
  }, null));
  const user = userEvent.setup();
  draw(<SkillScreen id="skill" tab="draft" />);
  await screen.findByRole('textbox', { name: 'SKILL.md' });
  await user.click(screen.getByRole('button', { name: i18n.t('resourceDetails.skill.draft.publish') }));
  const alert = await screen.findByRole('alert');
  expect(within(alert).getByText('The front matter gives no description')).toBeVisible();
  expect(within(alert).getByText('SKILL.md')).toBeVisible();
});

it.each(['en', 'zh-CN'] as const)('switches a skill on at its latest version, pins one, and takes it out again (%s)', async (locale) => {
  await setLocale(locale);
  const user = userEvent.setup();
  const changes: unknown[] = [];
  function Control() {
    const [value, setValue] = useState<unknown>({ skill: 'latest' });
    return <><SkillsSettingChoice shape={{ type: 'json' } as never} value={value} label="Skills" onChange={(next) => { changes.push(next); setValue(next); }} /><SkillsSummary value={value} /></>;
  }
  draw(<Control />);
  const list = await screen.findByRole('list', { name: 'Skills' });
  expect(await within(list).findByText('report-writer')).toBeVisible();
  expect(screen.getByText(i18n.t('settings.compact.enabledCount', { count: 1 }))).toBeVisible();

  await user.click(within(list).getByRole('combobox', { name: 'Skills: report-writer' }));
  await user.click(await screen.findByRole('option', { name: i18n.t('resourceDetails.skill.version', { number: 1 }) }));
  expect(changes.at(-1)).toEqual({ skill: 'v1' });

  await user.click(screen.getByRole('combobox', { name: i18n.t('settings.skills.add') }));
  await user.click(await screen.findByRole('option', { name: 'pdf-tools' }));
  expect(changes.at(-1)).toEqual({ skill: 'v1', other: 'latest' });

  await user.click(screen.getByRole('button', { name: i18n.t('settings.skills.remove', { name: 'report-writer' }) }));
  expect(changes.at(-1)).toEqual({ other: 'latest' });
});
