/** The template settings page uses the same runtime chooser as the schema editor, including an expert's name. */
import { screen } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { useState } from 'react';
import { expect, it, vi } from 'vitest';
import { SchemaForm } from '@/features/resources/templates/SchemaSettings';
import type { Schema } from '@/features/resources/templates/schema';
import type { RuntimeClass } from '@/generated/api-types';
import { i18n } from '@/i18n';
import { show } from './support';

const runtime: RuntimeClass = {
  name: 'runsc', title: 'gVisor', boundary: 'user_space_kernel', capabilities: {},
  live_changes: { gateway_given: [], resources: false, mounts: false, capabilities: [] },
  network_egress: [], recreate_preserves_files: false, keeps_software_on_stop: false,
  memory_overhead_mb: 64, limits: [],
};
vi.mock('@/api/resources', async (original) => ({
  ...await original<object>(),
  hostsQuery: () => ({ queryKey: ['isolation-form-hosts'], queryFn: () => Promise.resolve([
    { drivers: [{ name: 'docker', runtimes: [runtime], runtime_classes: [runtime.name] }] },
  ]) }),
}));
const schema: Schema = { type: 'object', properties: {
  runtime: { type: 'string', title: 'Runtime', 'x-runtime': true },
} };

it('chooses a reported runtime and retains an expert name through the actual settings form', async () => {
  const changed = vi.fn();
  function Form() {
    const [spec, setSpec] = useState<Record<string, unknown>>({});
    return <SchemaForm compact schema={schema} spec={spec} driver="docker" onChange={(next) => { changed(next); setSpec(next); }} />;
  }
  show(<Form />);
  const user = userEvent.setup();
  const choice = await screen.findByRole('combobox', { name: 'Runtime' });
  expect(screen.queryByRole('textbox', { name: 'Runtime' })).not.toBeInTheDocument();
  await user.click(choice);
  await user.click(await screen.findByRole('option', { name: /gVisor/ }));
  expect(changed).toHaveBeenLastCalledWith({ runtime: 'runsc' });
  await user.click(choice);
  await user.click(screen.getByRole('option', { name: i18n.t('isolation.select.other') }));
  const expert = screen.getByRole('textbox', { name: i18n.t('isolation.select.otherLabel') });
  expect(expert).toHaveValue('runsc');
  await user.clear(expert);
  await user.type(expert, 'custom-runtime');
  expect(changed).toHaveBeenLastCalledWith({ runtime: 'custom-runtime' });
});

it('keeps runtime values read-only when the template cannot be managed', () => {
  show(<SchemaForm schema={schema} spec={{ runtime: 'custom-runtime' }} driver="docker" readOnly onChange={vi.fn()} />);
  expect(screen.getByText('custom-runtime')).toBeVisible();
  expect(screen.queryByRole('combobox', { name: 'Runtime' })).not.toBeInTheDocument();
  expect(screen.queryByRole('textbox', { name: 'Runtime' })).not.toBeInTheDocument();
});
