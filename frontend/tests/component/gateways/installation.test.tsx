/** Installer selection stays separate from consent and is passed literally to both shells. */
import { useState } from 'react';
import { screen, waitFor } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { afterEach, expect, it, vi } from 'vitest';
import { GatewayChoice } from '@/features/gateways/GatewayChoice';
import { commandFor } from '@/features/resources/machines/Enrollment';
import type { EnrollmentToken } from '@/generated/api-types';
import { i18n } from '@/i18n';
import { show } from '../support';
import { catalogue, look, modes, restore } from './fixtures';

afterEach(restore);
it.each(modes)('uses Automatic and short advertised labels in $locale/$theme at $width', async (mode) => {
  await look(mode);
  const changed = vi.fn();
  function Choice() { const [value, setValue] = useState('automatic'); return <GatewayChoice gateways={catalogue.gateways} value={value} onChange={(next) => { changed(next); setValue(next); }} />; }
  show(<Choice />);
  expect(screen.getByRole('combobox', { name: i18n.t('regionalGateways.gateway') })).toHaveTextContent(i18n.t('regionalGateways.automatic'));
  expect(screen.getByRole('button', { name: i18n.t('help.hint.label', { term: i18n.t('regionalGateways.gateway') }) }))
    .toHaveAttribute('data-help-heading', 'choose-a-gateway');
  expect(changed).not.toHaveBeenCalled();
  await userEvent.click(screen.getByRole('combobox', { name: i18n.t('regionalGateways.gateway') }));
  await userEvent.click(screen.getByRole('option', { name: 'Asia tools', exact: true }));
  await waitFor(() => expect(changed).toHaveBeenCalledExactlyOnceWith('asia'));
});

it('preserves a removed manual gateway instead of silently returning to Automatic', () => {
  const changed = vi.fn();
  show(<GatewayChoice gateways={catalogue.gateways} value="removed" onChange={changed} />);
  expect(screen.getByRole('combobox', { name: i18n.t('regionalGateways.gateway') })).toHaveTextContent(i18n.t('regionalGateways.unavailable', { id: 'removed' }));
  expect(changed).not.toHaveBeenCalled();
});

const token: EnrollmentToken = { token: 'synthetic-enrollment.pin', expires_at: '2026-11-07T00:00:00Z', commands: { linux: 'INSTALL', macos: 'INSTALL', windows: 'INSTALL' } };
it.each(['linux', 'macos', 'windows'] as const)('adds the explicit gateway flag without changing consent or the token (%s)', (os) => {
  const copy = structuredClone(token);
  const command = commandFor(token, os, { foreground: false, instance: '', node: false, gateway: 'asia' });
  expect(command).toBe(os === 'windows' ? 'INSTALL -Gateway asia' : 'INSTALL --gateway asia');
  expect(commandFor(token, os)).toBe('INSTALL');
  expect(token).toEqual(copy);
});

it('quotes operator-defined gateway IDs as literal arguments before a foreground start', () => {
  const posix = commandFor(token, 'linux', { foreground: true, instance: '', node: false, gateway: "Asia'$(touch example)" });
  expect(posix).toContain("--gateway 'Asia'\\''$(touch example)' &&");
  const windows = commandFor(token, 'windows', { foreground: false, instance: '', node: false, gateway: "Asia'; Write-Output example" });
  expect(windows).toBe("INSTALL -Gateway 'Asia''; Write-Output example'");
  expect(commandFor(token, 'linux', { foreground: false, instance: '', node: true, gateway: 'automatic' })).toBe('INSTALL --gateway automatic');
});
