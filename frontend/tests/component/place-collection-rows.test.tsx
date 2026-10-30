/** Compact place rows keep their complete names and lazy details without per-folder opening requests. */
import { screen } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import type { ComponentProps, ReactNode } from 'react';
import { afterEach, expect, it, vi } from 'vitest';
import { nodeQuery } from '@/api/nodes';
import { DepartmentCard, ProjectCard } from '@/features/org/ProjectCards';
import { FolderCards } from '@/features/project/FolderCards';
import type { Org, Project } from '@/generated/api-types';
import { i18n, setLocale } from '@/i18n';
import { useAppearance } from '@/lib/appearance';
import { ACME, acmeTree, me, memberAccess, org, userRef } from '../doubles/fixtures';
import { show } from './support';

vi.mock('@tanstack/react-router', () => ({ Link: ({ to, params, children, ...anchor }: ComponentProps<'a'> & { to: string; params: { slug: string } }) =>
  <a {...anchor} href={to.replace('$slug', params.slug)}>{children}</a>,
}));
vi.mock('@/api/nodes', async (original) => ({ ...await original<object>(), nodeQuery: vi.fn((id: string) => ({
  queryKey: ['lazy-folder-detail', id], queryFn: () => Promise.reject(new Error('opening a list must use its held names')),
})) }));
vi.mock('@/features/navigation/NodeDialogs', () => ({ openNodeDialog: vi.fn() }));
vi.mock('@/features/navigation/NodeMenu', () => ({ NodeMenuButton: ({ place }: { place: { name: string } }) =>
  <button aria-label={i18n.t('places.menu.of', { name: place.name })} />,
}));
vi.mock('@/features/navigation/places', async (original) => ({ ...await original<object>(),
  PlaceLink: ({ place, children, className }: { place: { id: string }; children: ReactNode; className?: string }) =>
    <a href={`/project/${place.id}`} className={className}>{children}</a>,
}));
vi.mock('@/realtime/stores/tree', async (original) => ({ ...await original<object>(), useOrgTree: () => ({
  nodes: new Map(acmeTree().map((node) => [node.id, node])), loaded: true, error: null,
}) }));
vi.mock('@/features/topic/TagPicker', () => ({ TagPicker: () => null }));
const base = org(ACME.research, 'acme.research', 'Research', userRef(me(1, 'ada', 'Ada')), {
  parent_id: ACME.org, description: 'The full research description is available on request.', member_count: 0,
  access: { ...memberAccess, permissions: ['org.view'] },
});
const project: Project = { ...base, kind: 'project', id: ACME.web, name: 'Release plan with a complete name on several phone lines', parent_id: base.id,
  topic_count: 3, access: { ...base.access, permissions: ['project.view'] } };
const stats = { running: 0, waiting: 0, approvals: 0, blocked: 0, folders: 0, openTopics: 3, lastActivity: null };
const appearance = useAppearance.getState();
afterEach(async () => { vi.clearAllMocks(); useAppearance.setState(appearance); await setLocale('en'); });

for (const locale of ['en', 'zh-CN'] as const) for (const theme of ['light', 'dark'] as const) {
  it(`renders held folders without reading each folder (${locale}/${theme})`, async () => {
    await setLocale(locale); useAppearance.setState({ theme, resolvedTheme: theme });
    show(<FolderCards projectId={ACME.checkout} orgId={ACME.org} expected={1} filters={{ q: '', tags: 0, attention: null, archived: false }} />);
    expect(screen.getByRole('link', { name: 'Payments' })).toHaveAttribute('href', `/project/${ACME.payments}`);
    expect(nodeQuery).not.toHaveBeenCalled();
  });

  it(`keeps project and department descriptions out of their rows until requested (${locale}/${theme})`, async () => {
    await setLocale(locale); useAppearance.setState({ theme, resolvedTheme: theme });
    const department: Org = { ...base };
    const view = show(<><ProjectCard project={project} stats={stats} /><DepartmentCard org={department} stats={stats} /></>);
    expect(screen.getByRole('link', { name: project.name })).toHaveAttribute('href', `/project/${project.id}`);
    expect(screen.getByRole('link', { name: department.name })).toHaveAttribute('href', '/org/acme.research');
    expect(screen.queryByText(department.address)).not.toBeInTheDocument();
    expect(screen.queryByText(base.description)).not.toBeInTheDocument();
    const details = screen.getByRole('button', { name: i18n.t('places.details.named', { name: department.name }) });
    await userEvent.click(details);
    expect(await screen.findByText(base.description)).toBeVisible();
    expect(screen.queryByRole('button', { name: i18n.t('places.details.edit') })).not.toBeInTheDocument();
    expect(screen.getByText(i18n.t('places.details.ask', { name: base.owner.display_name }))).toBeVisible();
    expect(view.container.querySelectorAll('article')).toHaveLength(2);
  });
}
