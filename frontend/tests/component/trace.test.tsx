/** A person's authorized card entry and unknown retained size are readable without turning states into buttons. */
import { screen } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { describe, expect, it } from 'vitest';
import { TracePerson } from '@/features/trace/Person';
import { RemovalFacts } from '@/features/trace/KeptEnvironments';
import { memberAccess, me, userRef } from '../doubles/fixtures';
import { show } from './support';
import type { DeletedEnvironment } from '@/generated/api-types';
import { i18n } from '@/i18n';

const user = userRef(me(9, 'dora', 'Dora Developer'));
describe('a person’s card', () => {
  it('its keyboard entry stays in the selected place with the actor', async () => {
    show(<TracePerson user={user} scope={{ type: 'node', id: 'folder-id', name: 'Payments', orgId: 'org-id', kind: 'project', access: memberAccess }} allowed />); const keyboard = userEvent.setup(); await keyboard.tab(); await keyboard.keyboard('{Enter}'); const entry = screen.getByRole('link', { name: 'What they did here' }); expect(entry).toHaveAttribute('href', `/project/folder-id/settings?actor=${user.id}&include_agents=true&section=audit`); expect(entry.querySelector('svg')).toHaveAttribute('aria-hidden', 'true');
  });
  it('without audit authority it keeps the identity card and offers no trace entry', async () => {
    show(<TracePerson user={user} scope={{ type: 'system', name: 'System' }} allowed={false} />); expect(screen.getByText('Dora Developer')).toBeVisible(); await userEvent.click(screen.getByRole('button', { name: 'Dora Developer' })); expect(screen.queryByRole('link', { name: 'What they did here' })).not.toBeInTheDocument();
  });
});
it('a kept row says its size is not recorded and shows its reason, host, storage and phase as facts', () => {
  const entry: DeletedEnvironment = { environment: { id: 'env', name: 'Unmeasured cache', address: 'team.cache', owner: user, placement: 'managed', status: 'removed', ports_exposed: false }, home: { id: 'home', kind: 'project', name: 'Web', redacted: false }, host: { id: 'host', name: 'host-1', address: 'system.host-1' }, storage: 'cold', removal: { phase: 'kept', bytes: null, size_at: null, deleted_at: '2026-10-12T12:00:00Z', deleted_by: user, keep_days: 7, purge_at: '2026-10-19T12:00:00Z', purged_at: null, revision: 17, reason: 'The host has not reported the size.', can_inspect: true, can_restore: true, can_purge: false, job: null } };
  show(<RemovalFacts entry={entry} />); expect(screen.getByText(i18n.t('trace.unknown'))).toBeVisible(); expect(screen.queryByText('0 B', { exact: true })).not.toBeInTheDocument(); expect(screen.getByText('host-1')).toBeVisible(); expect(screen.getByText('cold')).toBeVisible(); expect(screen.getByText('Kept', { exact: true })).toBeVisible(); expect(screen.getByText('The host has not reported the size.')).toBeVisible(); expect(screen.queryByRole('button')).not.toBeInTheDocument();
});
