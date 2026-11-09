/** Layout sheets follow replaced chrome and keep automatic focus quiet without losing keyboard hints. */
import { act, cleanup, screen, waitFor } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { useState } from 'react';
import { Dialog } from '@/ui/Dialog';
import { afterEach, expect, it, vi } from 'vitest';
import { useChromeBottom } from '@/features/workbench/chrome';
import { i18n, setLocale } from '@/i18n';
import { Drawer } from '@/ui/Drawer';
import { IconButton } from '@/ui/IconButton';
import { show } from './support';

function ChromePosition() {
  const bottom = useChromeBottom();
  return <output aria-label="Sheet top">{bottom ?? 'unmeasured'}</output>;
}
function chrome(bottom: number) {
  const element = document.createElement('header');
  element.setAttribute('data-workbench-chrome', '');
  element.getBoundingClientRect = () => new DOMRect(0, 0, 400, bottom);
  document.body.append(element);
  return element;
}
afterEach(async () => {
  cleanup();
  for (const element of document.querySelectorAll('[data-workbench-chrome]')) element.remove();
  vi.restoreAllMocks();
  await setLocale('en');
});

for (const locale of ['en', 'zh-CN'] as const) {
  it(`follows the current chrome after desktop is replaced by phone navigation (${locale})`, async () => {
    await setLocale(locale);
    const desktop = chrome(144);
    show(<ChromePosition />);
    expect(screen.getByLabelText('Sheet top')).toHaveTextContent('144');
    await act(async () => { desktop.remove(); chrome(80); });
    await waitFor(() => expect(screen.getByLabelText('Sheet top')).toHaveTextContent('80'));
    await act(async () => { document.querySelector('[data-workbench-chrome]')?.remove(); });
    await waitFor(() => expect(screen.getByLabelText('Sheet top')).toHaveTextContent('unmeasured'));
    await act(async () => { chrome(120); });
    await waitFor(() => expect(screen.getByLabelText('Sheet top')).toHaveTextContent('120'));
  });

  it(`keeps sheet autofocus quiet and still shows a tooltip when Tab reaches the action (${locale})`, async () => {
    await setLocale(locale);
    const label = i18n.t('workbench.section.menu', { name: i18n.t('ask.title') });
    show(<Drawer open hideTitle prompt={false} modal={false} title={i18n.t('ask.title')} onOpenChange={() => undefined}>
      <IconButton label={label} action="actions" /><button>Next</button>
    </Drawer>);
    const action = screen.getByRole('button', { name: label, exact: true });
    await waitFor(() => expect(action).toHaveFocus());
    expect(screen.queryByRole('tooltip')).not.toBeInTheDocument();
    const user = userEvent.setup();
    await user.tab();
    expect(screen.getByRole('button', { name: 'Next' })).toHaveFocus();
    await user.tab({ shift: true });
    expect(action).toHaveFocus();
    expect(await screen.findByRole('tooltip')).toHaveTextContent(label);
  });
}

function RestoredAction({ label }: { label: string }) {
  const [open, setOpen] = useState(false);
  return <><IconButton label={label} action="actions" onClick={() => setOpen(true)} /><button>Next action</button>
    <Dialog title="Edit" open={open} onOpenChange={setOpen}><button onClick={() => setOpen(false)}>Done</button></Dialog>
  </>;
}
for (const locale of ['en', 'zh-CN'] as const) it(`returns the keyboard from a form quietly without changing the next Tab stop (${locale})`, async () => {
  await setLocale(locale);
  const label = i18n.t('workbench.containers.menu', { name: i18n.t('ask.title') });
  show(<RestoredAction label={label} />);
  const user = userEvent.setup(), action = screen.getByRole('button', { name: label, exact: true });
  await user.click(action); await user.click(screen.getByRole('button', { name: 'Done', exact: true }));
  await waitFor(() => expect(action).toHaveFocus());
  expect(screen.queryByRole('tooltip')).not.toBeInTheDocument();
  await user.tab(); expect(screen.getByRole('button', { name: 'Next action' })).toHaveFocus();
  await user.tab({ shift: true }); expect(await screen.findByRole('tooltip')).toHaveTextContent(label);
});
