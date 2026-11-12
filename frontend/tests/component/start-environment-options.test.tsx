/** Existing/new placement and More use the server's host and use facts. */
import { act, cleanup, screen, waitFor } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { useState } from 'react';
import { afterEach, beforeEach, expect, it, vi } from 'vitest';
import type * as StartApi from '@/api/start';
import type { CreationAccessContext, StartOptions } from '@/api/start';
import { NO_PICKS, type Picks } from '@/features/start/choices';
import { useStartOptions } from '@/features/start/options';
import { Where } from '@/features/start/Where';
import type { PlaceRef } from '@/features/navigation/places';
import type { BindingEnvironment } from '@/generated/api-types';
import { i18n, setLocale } from '@/i18n';
import { environment, options } from '../unit/start-fixtures';
import { deferred } from './deferred';
import { show } from './support';

const calls = vi.hoisted(() => ({ read: vi.fn<(environmentId: string | null, context?: CreationAccessContext) => Promise<StartOptions>>() }));
vi.mock('@/api/start', async (load) => {
  const actual = await load<typeof StartApi>();
  return { ...actual, startOptionsQuery: (node: string, id: string | null, context?: CreationAccessContext) => ({ ...actual.startOptionsQuery(node, id, context), queryFn: () => calls.read(id, context) }) };
});
const place: PlaceRef = { id: 'topic', kind: 'topic', name: 'Work', parentId: null, archived: false, status: null, folder: false };
beforeEach(() => { calls.read.mockReset().mockResolvedValue(options()); });
afterEach(async () => { cleanup(); await setLocale('en'); });

function Placement({ offered }: { offered: StartOptions }) {
  const [value, setValue] = useState<string | null>('e1');
  const [requesting, setRequesting] = useState<BindingEnvironment | null>(null);
  return <Where compact place={place} options={offered} value={value} onChange={setValue} machines={[]} homeId="org"
    requesting={requesting} onRequestingChange={setRequesting} />;
}
function Fallback() {
  const [picks, setPicks] = useState<Picks>(NO_PICKS);
  const state = useStartOptions(place.id, picks, { suggest: true }, true, { threadAccess: 'private' });
  if (state.options === null || state.resolved === null) return <p role="status">Checking placement</p>;
  return <><span data-placement-ready>{state.refreshing ? 'checking' : 'ready'}</span><span data-placement-arrangement>{state.resolved.arrangement}</span><span data-placement-origin>{state.resolved.origins.environment}</span><Where compact place={place} options={state.options}
    value={state.resolved.fresh ? 'new' : state.resolved.environment?.environment.id ?? null} onChange={(environment) => setPicks((before) => ({ ...before, environment }))}
    machines={[]} homeId="org" requesting={null} onRequestingChange={() => undefined} /></>;
}

for (const locale of ['en', 'zh-CN'] as const) {
  it('keeps existing/new side by side and groups More by actual use in '+locale, async () => {
    await setLocale(locale);
    const topic = environment('e1', { isDefault: true }); topic.used = 'topic';
    const project = environment('project'); project.used = 'project';
    const elsewhere = environment('elsewhere'); elsewhere.used = 'elsewhere';
    const unused = environment('unused'); unused.used = 'unused';
    show(<Placement offered={options({ environments: [topic, project, elsewhere, unused] })} />);
    expect(screen.getByRole('radio', { name: i18n.t('start.where.useExisting') })).toBeChecked();
    expect(screen.getByRole('radio', { name: i18n.t('start.where.createNew') })).toBeEnabled();
    const more = screen.getByRole('combobox', { name: i18n.t('start.where.moreTitle') });
    const user = userEvent.setup(); await user.click(more);
    for (const use of ['project', 'elsewhere', 'unused'] as const) expect(screen.getByText(i18n.t(`start.where.used.${use}`), { exact: true })).toBeVisible();
    expect(screen.queryByRole('option', { name: topic.environment.name })).not.toBeInTheDocument();
    await user.type(more, 'project');
    expect(screen.queryByRole('option', { name: elsewhere.environment.name })).not.toBeInTheDocument();
    await user.click(screen.getByRole('option', { name: project.environment.name }));
    expect(screen.getByRole('combobox', { name: i18n.t('start.where.title') })).toHaveDisplayValue(project.environment.name);
    expect(calls.read).not.toHaveBeenCalled();
  });

  it('shows the no-host reason in Where and reads the fallback environment’s actual context in '+locale, async () => {
    await setLocale(locale);
    const offered = options({ hosts: [], environment_default: 'fresh' });
    const selected = deferred<StartOptions>();
    calls.read.mockImplementation((id) => id === null ? Promise.resolve(offered) : selected.promise);
    show(<Fallback />);
    await waitFor(() => expect(calls.read).toHaveBeenCalledWith('e1', { threadAccess: 'private' }));
    expect(document.querySelector('[data-placement-ready]')).toHaveTextContent('checking');
    await act(async () => { selected.resolve(offered); await selected.promise; });
    // The reason is the status line that is shown, and also the disabled choice's accessible description, in a hidden node.
    const noHost = i18n.t('start.where.noHost');
    await waitFor(() => expect(screen.getAllByRole('status').find((node) => node.textContent === noHost)).toBeVisible());
    expect(screen.getByRole('radio', { name: i18n.t('start.where.createNew') })).toHaveAccessibleDescription(expect.stringContaining(noHost));
    expect(screen.getByRole('radio', { name: i18n.t('start.where.useExisting') })).toBeChecked();
    expect(screen.getByRole('radio', { name: i18n.t('start.where.createNew') })).toBeDisabled();
    expect(screen.getByRole('combobox', { name: i18n.t('start.where.title') })).toHaveDisplayValue('Env e1');
    expect(document.querySelector('[data-placement-arrangement]')).toHaveTextContent('separated');
    expect(document.querySelector('[data-placement-origin]')).toHaveTextContent('suggested');
    expect(calls.read.mock.calls.map(([id]) => id)).toEqual([null, 'e1']);
  });
}
