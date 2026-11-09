/** AI-12: readable resources retain applicable management controls without lending their authority. */
import { cleanup, fireEvent, screen } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { afterEach, beforeEach, expect, it, vi } from 'vitest';
import type * as Resources from '@/api/resources';
import type * as Tools from '@/api/resource-tools';
import type * as Skills from '@/api/skills';
import type * as Auth from '@/api/auth';
import type { McpServer, Mount, ResourceAccess, Skill } from '@/generated/api-types';
import { HeadingActs, managedAct } from '@/features/resources/acts';
import { useAgentActs } from '@/features/resources/agents/acts';
import { useProviderActs } from '@/features/resources/providers/acts';
import { useHostActs } from '@/features/resources/hosts/acts';
import { useEnvironmentActs } from '@/features/resources/environments/acts';
import { useTemplateActs } from '@/features/resources/templates/acts';
import { MountScreen } from '@/features/resources/mounts/Mount';
import { McpScreen } from '@/features/resources/mcp/Preset';
import { SkillScreen } from '@/features/resources/skills/Skill';
import { i18n, setLocale } from '@/i18n';
import { Menu } from '@/ui/Menu';
import { detailAccess, detailAgent, detailEnvironment, detailHome, detailHost, detailMcp, detailMount, detailTemplate, detailUnitProvider } from './resource-detail-fixtures';
import { ADA } from '../doubles/chat-rows';
import { me } from '../doubles/fixtures';
import { show } from './support';

const calls = vi.hoisted(() => ({ mount: vi.fn<() => Promise<Mount>>(), mcp: vi.fn<() => Promise<McpServer>>(), skill: vi.fn<() => Promise<Skill>>(), writes: vi.fn() }));
vi.mock('@/api/resources', async (original) => ({ ...await original<typeof Resources>(), updateAgent: calls.writes }));
vi.mock('@/api/resource-tools', async (original) => ({ ...await original<typeof Tools>(),
  mountQuery: (id: string) => ({ queryKey: ['implicit-mount', id], queryFn: calls.mount }),
  mcpQuery: (id: string) => ({ queryKey: ['implicit-mcp', id], queryFn: calls.mcp }),
  updateMount: calls.writes, deleteMount: calls.writes, updateMcp: calls.writes, deleteMcp: calls.writes,
}));
vi.mock('@/api/skills', async (original) => ({ ...await original<typeof Skills>(), skillQuery: (id: string) => ({ queryKey: ['implicit-skill', id], queryFn: calls.skill }), updateSkill: calls.writes, deleteSkill: calls.writes }));
vi.mock('@/api/auth', async (original) => ({ ...await original<typeof Auth>(), meQuery: { queryKey: ['implicit-me'], queryFn: () => Promise.resolve(me(1, 'ada', 'Ada')) } }));
vi.mock('@/features/resources/trail', () => ({ useResourceHeading: () => undefined }));
vi.mock('@/features/help/Hint', () => ({ Hint: () => null }));
vi.mock('@/features/help/open', () => ({ useHelpOpener: () => vi.fn() }));
vi.mock('@/features/resources/agents/Edit', () => ({ EditAgent: () => <div role="dialog" aria-label="Profile editor" /> }));
vi.mock('@/features/resources/ResourceAccess', () => ({ ResourceAccess: () => null }));
vi.mock('@/features/resources/mcp/Enabled', () => ({ McpEnabledIn: () => null }));
vi.mock('@/features/resources/skills/Draft', () => ({ SkillDraftPanel: () => null }));

const readonly: ResourceAccess = { ...detailAccess, permissions: ['agent.view', 'provider.view', 'host.view', 'environment.view', 'template.view', 'mount.view', 'mcp.view', 'skill.view'], through: [] };
const skill: Skill = { id: 'skill', name: 'Research skill', address: 'workshop.skill', home: detailHome, org_id: 'team', owner: ADA, availability: [], revision: 1, enabled: true, enabled_places: 0, description: '', draft: null, latest: null, origin: null, origin_machine: null, versions: 0, access: readonly };
type Kind = 'agent' | 'provider' | 'host' | 'environment' | 'template';
function Probe({ kind, allowed = false }: { kind: Kind; allowed?: boolean }) {
  const agent = useAgentActs(), provider = useProviderActs(), host = useHostActs(), environment = useEnvironmentActs(), template = useTemplateActs();
  const access = allowed ? detailAccess : readonly;
  const a = { ...detailAgent, access }, p = { ...detailUnitProvider, access }, h = { ...detailHost, access }, e = { ...detailEnvironment, machine: null, status: 'stopped' as const, access }, t = { ...detailTemplate, access };
  const choices = {
    agent: { name: a.name, row: agent.list.of(a), page: agent.page(a), dialogs: agent.dialogs },
    provider: { name: p.name, row: provider.list.of(p), page: provider.page(p), dialogs: provider.dialogs },
    host: { name: h.name, row: host.list.of(h), page: host.page(h), dialogs: host.dialogs },
    environment: { name: e.name, row: environment.list.of(e), page: environment.page(e), dialogs: environment.dialogs },
    template: { name: t.name, row: template.list.of(t), page: template.page(t), dialogs: template.dialogs },
  };
  const selected = choices[kind];
  return <><HeadingActs name={selected.name} {...selected.page} /><div data-testid="resource-row" tabIndex={0} onKeyDown={(event) => selected.row.keys[event.key]?.()}>
    <Menu label="Row actions" trigger={<button type="button">Row actions</button>} entries={selected.row.entries} />
  </div>{selected.dialogs}</>;
}
beforeEach(() => { vi.clearAllMocks(); calls.mount.mockResolvedValue({ ...detailMount, access: readonly }); calls.mcp.mockResolvedValue({ ...detailMcp, access: readonly }); calls.skill.mockResolvedValue(skill); });
afterEach(async () => { cleanup(); await setLocale('en'); });

for (const locale of ['en', 'zh-CN'] as const) {
  it.each(['agent', 'provider', 'host', 'environment', 'template'] as const)(`AI-12 keeps a read-only %s's known menu acts with their reason (${locale})`, async (kind) => {
    await setLocale(locale); const user = userEvent.setup(); show(<Probe kind={kind} />);
    if (kind !== 'provider') expect(screen.getByRole('button', { name: i18n.t('resources.act.editButton'), exact: true })).toHaveAttribute('aria-disabled', 'true');
    fireEvent.keyDown(screen.getByTestId('resource-row'), { key: 'F2' }); expect(screen.queryByRole('dialog')).not.toBeInTheDocument();
    await user.click(screen.getByRole('button', { name: 'Row actions' }));
    for (const word of ['rename', ...(kind === 'provider' ? [] : ['edit'])]) {
      const item = screen.getByRole('menuitem', { name: new RegExp(i18n.t(`resources.act.${word}`)) });
      expect(item).toHaveAttribute('aria-disabled', 'true'); expect(item).toHaveTextContent(i18n.t('resources.act.manageRequired'));
      await user.click(item);
    }
    expect(screen.getByRole('menuitem', { name: new RegExp(i18n.t(kind === 'environment' ? 'trace.removeEntry' : 'resources.act.delete')) })).toHaveAttribute('aria-disabled', 'true');
    expect(calls.writes).not.toHaveBeenCalled(); expect(screen.queryByRole('dialog')).not.toBeInTheDocument();
  });
  it(`AI-12 keeps direct Mount and MCP edit/delete controls visible and blocked (${locale})`, async () => {
    await setLocale(locale); const user = userEvent.setup(); const view = show(<MountScreen id={detailMount.id} />);
    const edit = await screen.findByRole('button', { name: i18n.t('resources.act.editButton'), exact: true });
    expect(edit).toHaveAttribute('aria-disabled', 'true'); await user.click(edit); expect(screen.queryByRole('dialog')).not.toBeInTheDocument();
    view.unmount(); show(<McpScreen id={detailMcp.id} />);
    expect(await screen.findByRole('button', { name: i18n.t('resources.act.editButton'), exact: true })).toHaveAttribute('aria-disabled', 'true');
    await user.click(screen.getByRole('button', { name: i18n.t('resources.act.menuOf', { name: detailMcp.name }) }));
    expect(screen.getByRole('menuitem', { name: new RegExp(i18n.t('resourceDetails.remove')) })).toHaveAttribute('aria-disabled', 'true');
    expect(calls.writes).not.toHaveBeenCalled();
  });
  it(`AI-12 keeps Skill upload and enable controls present without its Manage permission (${locale})`, async () => {
    await setLocale(locale); show(<SkillScreen id={skill.id} />);
    expect(await screen.findByRole('button', { name: i18n.t('resourceDetails.skill.upload') })).toHaveAttribute('aria-disabled', 'true');
    const toggle = screen.getByRole('switch'); expect(toggle).toBeDisabled(); expect(toggle).toHaveAttribute('aria-description', i18n.t('resources.act.manageRequired'));
    expect(calls.writes).not.toHaveBeenCalled();
  });
  it(`AI-12 preserves a permitted editor and refuses even direct invocation of a blocked act (${locale})`, async () => {
    await setLocale(locale); const run = vi.fn(), blocked = managedAct({ id: 'edit', label: 'Edit', run }, false, i18n.t('resources.act.manageRequired'));
    blocked?.run(); expect(run).not.toHaveBeenCalled();
    const user = userEvent.setup(); show(<Probe kind="agent" allowed />); await user.click(screen.getByRole('button', { name: i18n.t('resources.act.editButton'), exact: true }));
    expect(await screen.findByRole('dialog', { name: 'Profile editor' })).toBeInTheDocument();
  });
}
