/** The environment's network says what is verified and offers recovery only to its managers. */
import { screen } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { describe, expect, it } from 'vitest';

import { useChangeHost } from '@/features/change/target';
import { EnvironmentNetwork, OpenEgressNotice } from '@/features/resources/environments/Network';
import type { NetworkState } from '@/generated/api-types';

import { show } from './support';

function network(): NetworkState {
  return { mode: 'restricted', required_mode: 'restricted', network_mode: 'bridge', networks: [], proxy_url: 'http://10.199.0.2:3128',
    allowed_hosts: [], sources: [], revision: 'r2', always: [{ authority: 'gateway.orch.internal', protocol: 'wss', port: 9443, purpose: 'gateway' }],
    always_revision: 'a1', always_source: 'orch_configuration', state: 'ready', reason: null, last_denied: null };
}

describe('the environment network', () => {
  it('names the outbound publication path exactly where egress is open', () => {
    const { rerender } = show(<OpenEgressNotice network={{ mode: 'open' }} />);
    expect(screen.getByText(/publish itself through its own outbound tunnel/)).toBeVisible();
    rerender(<OpenEgressNotice network={{ mode: 'restricted' }} />);
    expect(screen.queryByText(/publish itself through its own outbound tunnel/)).not.toBeInTheDocument();
  });
  it('shows Only Orch and its safe trusted authority to a reader below Manage', () => {
    show(<EnvironmentNetwork environment={{ id: 'environment', name: 'Build', job: null, network: network(), access: { permissions: ['browser.view'] } }} manage={false} />);
    expect(screen.getByRole('status')).toHaveTextContent('Ready');
    expect(screen.getByRole('status')).not.toHaveAttribute('aria-label');
    expect(screen.getByText('Only Orch')).toBeVisible();
    expect(screen.getByText('gateway.orch.internal:9443')).toBeVisible();
    expect(screen.queryByRole('button', { name: 'Retry' })).not.toBeInTheDocument();
    expect(screen.queryByText('Manage allowed hosts')).not.toBeInTheDocument();
  });

  it('names a kept-stopped failure and opens a reviewed change when its manager retries', async () => {
    const user = userEvent.setup();
    show(<EnvironmentNetwork environment={{ id: 'environment', name: 'Build', job: null, access: { permissions: ['environment.manage'] },
      network: { ...network(), mode: 'open', state: 'unavailable', reason: 'The host could not verify its restriction.' } }} manage />);
    const status = screen.getByText('Kept stopped');
    expect(status).toHaveAttribute('role', 'status');
    expect(status).toBeVisible();
    expect(screen.getByText(/publish itself through its own outbound tunnel/)).toBeVisible();
    expect(screen.getByText('Requested: Restricted')).toBeVisible();
    await user.click(screen.getByRole('button', { name: 'Retry' }));
    expect(useChangeHost.getState().current?.target).toEqual({ kind: 'environment', id: 'environment', name: 'Build' });
    expect(useChangeHost.getState().current?.options.kind).toBe('update');
  });

  it('explains a thread-only view without treating hidden host lists as Only Orch', () => {
    show(<EnvironmentNetwork environment={{ id: 'environment', name: 'Build', job: null, network: { ...network(), always: [] }, access: { permissions: ['access.manage'] } }} manage={false} />);
    expect(screen.getByRole('status')).toHaveTextContent('Ready');
    expect(screen.getByText('Network details require a permission on this environment.')).toBeVisible();
    expect(screen.queryByText('Only Orch')).not.toBeInTheDocument();
    expect(screen.queryByText('Other allowed hosts')).not.toBeInTheDocument();
  });
});
