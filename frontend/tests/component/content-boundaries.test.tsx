/** A rendering fault stays inside its real view, tab or dialog; neighbouring work is retained. */
import { act, cleanup, fireEvent, screen, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { CircleHelp } from 'lucide-react';
import { useState } from 'react';
import { afterEach, expect, it, vi } from 'vitest';

import { ViewHost, ViewSlot } from '@/features/workbench/Board';
import { LAYOUT_VERSION, makeGroup, type Layout, type Tab } from '@/features/workbench/layout';
import { registerView } from '@/features/workbench/views';
import { i18n, setLocale } from '@/i18n';
import { Dialog } from '@/ui/Dialog';
import { DialogBoundary } from '@/ui/DialogBoundary';
import { ConfirmDialog } from '@/ui/ConfirmDialog';
import { Drawer } from '@/ui/Drawer';
import { Palette, PaletteInput, PaletteItem, PaletteList } from '@/ui/Palette';
import { Popover } from '@/ui/Popover';
import { Tabs } from '@/ui/Tabs';
import { show } from './support';

const release: (() => void)[] = [];
afterEach(async () => { cleanup(); for (const stop of release.splice(0)) stop(); vi.restoreAllMocks(); await setLocale('en'); });

function Draft() {
  const [text, setText] = useState('');
  return <input aria-label="Neighbour draft" value={text} onChange={(event) => setText(event.target.value)} />;
}
function Fault() {
  const [broken, setBroken] = useState(false);
  if (broken) throw new Error('An isolated rendering fault');
  return <button onClick={() => setBroken(true)}>Break this part</button>;
}

it.each(['en', 'zh-CN'] as const)('keeps another workbench view and its draft alive while the failed view retries in %s', async (locale) => {
  await setLocale(locale);
  const log = vi.spyOn(console, 'error').mockImplementation(() => undefined);
  const shared = { title: 'screens.help.title', about: 'screens.help.title', icon: CircleHelp, screens: 'all', area: 'main' } as const;
  release.push(registerView({ ...shared, name: 'boundary-draft', body: Draft }), registerView({ ...shared, name: 'boundary-fault', body: Fault }));
  const draft: Tab = { id: 'boundary-draft', view: 'boundary-draft' };
  const fault: Tab = { id: 'boundary-fault', view: 'boundary-fault' };
  const layout: Layout = { version: LAYOUT_VERSION, left: { open: false, size: 240, containers: [], active: null },
    right: { open: false, size: 240, containers: [], active: null }, bottom: { open: false, size: 200, group: makeGroup() },
    main: { kind: 'split', id: 'boundary-columns', direction: 'row', children: [makeGroup([draft]), makeGroup([fault])], sizes: [50, 50] }, maximized: null };
  const user = userEvent.setup();
  show(<main><section aria-label="Draft view"><ViewSlot tab={draft} /></section><section aria-label="Fault view"><ViewSlot tab={fault} /></section><ViewHost layout={layout} /></main>);
  fireEvent.change(await screen.findByRole('textbox', { name: 'Neighbour draft' }), { target: { value: 'Work still here' } });
  await user.click(screen.getByRole('button', { name: 'Break this part' }));
  const local = within(screen.getByRole('region', { name: 'Fault view' }));
  expect(local.getByRole('alert')).toHaveTextContent(i18n.t('ui.error.partFailed'));
  expect(screen.getByRole('textbox', { name: 'Neighbour draft' })).toHaveValue('Work still here');
  expect(screen.getAllByRole('alert')).toHaveLength(1);
  await user.click(local.getByRole('button', { name: i18n.t('common.tryAgain') }));
  expect(local.getByRole('button', { name: 'Break this part' })).toBeVisible();
  expect(screen.getByRole('textbox', { name: 'Neighbour draft' })).toHaveValue('Work still here');
  expect(log).toHaveBeenCalled();
});

it('keeps the dialog title, close action and surrounding page when its body fails', async () => {
  vi.spyOn(console, 'error').mockImplementation(() => undefined);
  const user = userEvent.setup();
  function Page() {
    const [open, setOpen] = useState(true);
    return <><Draft /><Dialog open={open} onOpenChange={setOpen} title="Local dialog"><Fault /></Dialog></>;
  }
  show(<Page />);
  await user.click(screen.getByRole('button', { name: 'Break this part' }));
  const dialog = screen.getByRole('dialog', { name: 'Local dialog' });
  expect(within(dialog).getByRole('alert')).toBeVisible();
  await user.click(within(dialog).getByRole('button', { name: i18n.t('common.tryAgain') }));
  expect(within(dialog).getByRole('button', { name: 'Break this part' })).toBeVisible();
  await user.click(within(dialog).getByRole('button', { name: i18n.t('common.close') }));
  expect(screen.queryByRole('dialog')).not.toBeInTheDocument();
  expect(screen.getByRole('textbox', { name: 'Neighbour draft' })).toBeVisible();
});

it('also contains a dialog component that fails before it can draw its own frame', async () => {
  vi.spyOn(console, 'error').mockImplementation(() => undefined);
  let broken = true;
  const close = vi.fn();
  function Creator() { if (broken) throw new Error('Failed before the dialog frame'); return <p>Creator ready</p>; }
  show(<><Draft /><DialogBoundary title="Add provider" onClose={close}><Creator /></DialogBoundary></>);
  const dialog = screen.getByRole('dialog', { name: 'Add provider' });
  expect(within(dialog).getByRole('alert')).toBeVisible();
  broken = false;
  await userEvent.click(within(dialog).getByRole('button', { name: i18n.t('common.tryAgain') }));
  expect(screen.getByText('Creator ready')).toBeVisible();
  expect(close).not.toHaveBeenCalled();
  expect(screen.getByRole('textbox', { name: 'Neighbour draft' })).toBeVisible();
});

it('withdraws a confirmation’s destructive act when its review fails, retaining Retry and Cancel', async () => {
  vi.spyOn(console, 'error').mockImplementation(() => undefined);
  const confirm = vi.fn();
  const user = userEvent.setup();
  function Page() {
    const [open, setOpen] = useState(true);
    return <><Draft /><ConfirmDialog open={open} onOpenChange={setOpen} title="Delete this place?" confirmLabel="Delete place" danger onConfirm={confirm}><Fault /></ConfirmDialog></>;
  }
  show(<Page />);
  await user.click(screen.getByRole('button', { name: 'Break this part' }));
  const dialog = screen.getByRole('alertdialog', { name: 'Delete this place?' });
  expect(within(dialog).getByRole('alert')).toBeVisible();
  expect(within(dialog).queryByRole('button', { name: 'Delete place' })).not.toBeInTheDocument();
  expect(confirm).not.toHaveBeenCalled();
  await user.click(within(dialog).getByRole('button', { name: i18n.t('common.tryAgain') }));
  expect(within(dialog).getByRole('button', { name: 'Delete place' })).toBeVisible();
  await user.click(within(dialog).getByRole('button', { name: i18n.t('common.cancel') }));
  expect(screen.queryByRole('alertdialog')).not.toBeInTheDocument();
  expect(confirm).not.toHaveBeenCalled();
  expect(screen.getByRole('textbox', { name: 'Neighbour draft' })).toBeVisible();
});

it.each(['drawer', 'palette', 'popover'] as const)('isolates a %s body and keeps its surrounding application on Retry', async (kind) => {
  vi.spyOn(console, 'error').mockImplementation(() => undefined);
  function Part() {
    const [open, setOpen] = useState(true);
    const [selected, setSelected] = useState('choice');
    return kind === 'drawer' ? <Drawer open={open} onOpenChange={setOpen} title="Local container"><Fault /></Drawer>
      : kind === 'palette' ? <Palette open={open} onOpenChange={setOpen} label="Local container" value={selected} onValueChange={setSelected}>
        <PaletteInput /><PaletteList><PaletteItem value="choice">One choice</PaletteItem></PaletteList><Fault />
      </Palette> : <Popover label="Local container" open={open} onOpenChange={setOpen} trigger={<button>Open container</button>}><Fault /></Popover>;
  }
  const user = userEvent.setup();
  show(<><Draft /><Part /></>);
  await user.click(screen.getByRole('button', { name: 'Break this part' }));
  const dialog = screen.getByRole('dialog', { name: 'Local container' });
  expect(within(dialog).getByRole('alert')).toBeVisible();
  await user.click(within(dialog).getByRole('button', { name: i18n.t('common.tryAgain') }));
  expect(within(dialog).getByRole('button', { name: 'Break this part' })).toBeVisible();
  await user.keyboard('{Escape}');
  expect(screen.queryByRole('dialog')).not.toBeInTheDocument();
  expect(screen.getByRole('textbox', { name: 'Neighbour draft' })).toBeVisible();
});

it('keeps the tab strip usable and retries just its failed panel', async () => {
  vi.spyOn(console, 'error').mockImplementation(() => undefined);
  const user = userEvent.setup();
  function Panels() {
    const [tab, setTab] = useState('fault');
    return <Tabs label="Local tabs" value={tab} onValueChange={setTab} tabs={[
      { value: 'fault', label: 'Fault panel', panel: <Fault /> }, { value: 'working', label: 'Working panel', panel: <p>Other panel</p> },
    ]} />;
  }
  show(<><Draft /><Panels /></>);
  fireEvent.change(screen.getByRole('textbox', { name: 'Neighbour draft' }), { target: { value: 'Keep this' } });
  await user.click(screen.getByRole('button', { name: 'Break this part' }));
  expect(within(screen.getByRole('tabpanel', { name: 'Fault panel' })).getByRole('alert')).toBeVisible();
  await user.click(screen.getByRole('button', { name: i18n.t('common.tryAgain') }));
  expect(screen.getByRole('button', { name: 'Break this part' })).toBeVisible();
  await user.click(screen.getByRole('button', { name: 'Break this part' }));
  await user.click(screen.getByRole('tab', { name: 'Working panel' }));
  expect(screen.getByText('Other panel')).toBeVisible();
  expect(screen.queryByRole('alert')).not.toBeInTheDocument();
  expect(screen.getByRole('textbox', { name: 'Neighbour draft' })).toHaveValue('Keep this');
});

it('falls back to English and then the key, while a missing word cannot replace its neighbours', async () => {
  await setLocale('zh-CN');
  const resource: unknown = i18n.getResource('zh-CN', 'translation', 'ui.error.partFailed');
  try {
    // Remove only the translation this test owns, leaving both shipped catalogues' audits strict.
    i18n.addResource('zh-CN', 'translation', 'ui.error.partFailed', '');
    expect(i18n.t('ui.error.partFailed')).toBe('');
    const bundle: unknown = i18n.getResourceBundle('zh-CN', 'translation');
    if (typeof bundle !== 'object' || bundle === null || !('ui' in bundle) || typeof bundle.ui !== 'object' || bundle.ui === null
      || !('error' in bundle.ui) || typeof bundle.ui.error !== 'object' || bundle.ui.error === null) throw new Error('Missing test catalogue');
    Reflect.deleteProperty(bundle.ui.error, 'partFailed');
    expect(i18n.t('ui.error.partFailed')).toBe('This part could not be displayed. Try again; your other work is still open.');
    const missing = ['component', 'boundary', 'missing', 'word'].join('.');
    function MissingWord() { return <p>{i18n.t(missing, { defaultValue: missing })}</p>; }
    show(<><Draft /><MissingWord /></>);
    expect(screen.getByText(missing)).toBeVisible();
    fireEvent.change(screen.getByRole('textbox', { name: 'Neighbour draft' }), { target: { value: 'Survives missing text' } });
    await act(async () => { await setLocale('en'); });
    expect(screen.getByRole('textbox', { name: 'Neighbour draft' })).toHaveValue('Survives missing text');
    expect(screen.queryByRole('alert')).not.toBeInTheDocument();
  } finally { if (typeof resource === 'string') i18n.addResource('zh-CN', 'translation', 'ui.error.partFailed', resource); }
});
