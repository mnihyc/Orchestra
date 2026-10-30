/** Management roots, finder acts and personal frames keep the same places and destinations as their screens. */
import { screen, within } from '@testing-library/react';
import type { ComponentProps } from 'react';
import { afterEach, describe, expect, it, vi } from 'vitest';

import { Everything } from '@/features/admin/Everything';
import { commandsNow } from '@/features/finder/commands';
import { PlaceCommands } from '@/features/finder/shellCommands';
import { placeOfRow } from '@/features/navigation/places';
import { PersonalFrame } from '@/features/settings/PersonalFrame';
import { i18n } from '@/i18n';
import { treeWriter, useTreeStore } from '@/realtime/stores/tree';

import { ACME, acmeTree } from '../doubles/fixtures';
import { show } from './support';

vi.mock('@/features/navigation/NodeMenu', () => ({ useNodeMenu: () => [{ id: 'rename', label: 'Rename', onSelect: vi.fn() }] }));
vi.mock('@tanstack/react-router', () => ({
  Link: ({ to, activeOptions: _activeOptions, ...props }: ComponentProps<'a'> & { to: string; activeOptions?: unknown }) => <a href={to} {...props} />,
  useNavigate: () => vi.fn(),
  useRouter: () => ({ history: { push: vi.fn() }, routesByPath: { '/project/$projectId/settings': {} } }),
}));
afterEach(() => { useTreeStore.setState({ byOrg: new Map() }); });

describe('places in Management and finder acts', () => {
  it('opens Management Everything in the main-area inventory with system scope', () => {
    show(<Everything search={{ q: 'web' }} onSearch={vi.fn()} />);
    expect(screen.getByRole('link', { name: i18n.t('everything.openSystem') })).toHaveAttribute('href', '/everything?scope=system&q=web');
  });

  it('names the org of the repeated place in action context and in its settings command', () => {
    const rows = acmeTree();
    treeWriter.reset(ACME.org, rows, 20);
    treeWriter.reset('beta', rows.map((row) => ({ ...row, id: `beta-${row.id}`, parent_id: row.parent_id === null ? null : `beta-${row.parent_id}`, name: row.id === ACME.org ? 'Beta' : row.name })), 20);
    const project=rows.find((row)=>row.id===ACME.web);
    if(project===undefined)throw new Error('the fixture needs Web');
    show(<PlaceCommands place={placeOfRow(project)} />);
    const rename = commandsNow().find((command) => command.id === 'place.rename');
    expect(rename?.about?.(i18n.t)).toContain('Web · Acme');
    const general = commandsNow().find((command) => command.id === 'go.place-settings.general');
    expect(general?.title(i18n.t)).toContain('Web · Acme');
    expect(general?.path).toBe(`/project/${ACME.web}/settings?section=general`);
  });
});

it('gives Notifications the personal navigation and one canonical Usage destination', () => {
  show(<PersonalFrame page="notifications" lead="Where to be told"><p>Notification controls</p></PersonalFrame>);
  const navigation = within(screen.getByRole('navigation', { name: 'Your settings' }));
  expect(navigation.getByRole('link', { name: 'Notifications' })).toHaveAttribute('aria-current', 'page');
  expect(navigation.getByRole('link', { name: 'Usage and limits' })).toHaveAttribute('href', '/usage');
  expect(navigation.getByRole('link', { name: 'Security' })).toHaveAttribute('href', '/settings/security');
  expect(screen.getByRole('heading', { level: 1 })).toHaveTextContent('Notifications');
});
