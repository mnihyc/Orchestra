/** The rail's Ask button closes the Ask that is on screen, and closing Ask leaves no empty bar behind (C1122). */
import { afterEach, beforeAll, expect, it } from 'vitest';
import { client } from '@/api/client';
import { askShown, closeAsk, openAsk, toggleAsk } from '@/features/ask/open';
import { readViewChoice, writeViewChoice } from '@/lib/viewChoices';
import { registerAskView } from '@/features/ask/register';
import { allTabs, tabHidden } from '@/features/workbench/layout';
import { useWorkbench } from '@/features/workbench/store';
import { registerShippedViews } from '@/features/workbench/shippedViews';
import { clearViewOpenings, showViewOpening } from '@/features/workbench/viewOpening';

const askTab = () => allTabs(useWorkbench.getState().layout).find((tab) => tab.view === 'ask');
const settled = () => new Promise<void>((resolve) => { queueMicrotask(resolve); });
let shown: (() => void) | null = null;
/** What the board does when Ask's section, docked or floating, is drawn: its slot is keyed by the tab. */
async function drawAsk(): Promise<string> {
  const tab = askTab();
  expect(tab).toBeDefined();
  shown = showViewOpening(tab?.id ?? '');
  await settled();
  return tab?.id ?? '';
}
beforeAll(() => { registerShippedViews(() => null); registerAskView(); });
afterEach(() => { shown?.(); shown = null; closeAsk(); clearViewOpenings(); });

it('knows Ask is shown from the slot of its own tab', async () => {
  openAsk();
  expect(askShown()).toBe(false);
  await drawAsk();
  expect(askShown()).toBe(true);
});

it('closes a shown Ask from the button that opened it', async () => {
  toggleAsk();
  const id = await drawAsk();
  toggleAsk();
  expect(tabHidden(useWorkbench.getState().layout, id)).toBe(true);
});

it('closes the bar that held only Ask instead of showing it empty', async () => {
  openAsk();
  const id = await drawAsk();
  const bar = (['left', 'right'] as const).find((side) => useWorkbench.getState().layout[side].containers
    .some((container) => container.sections.some((section) => section.tab.id === id)));
  expect(bar).toBeDefined();
  const side = bar ?? 'right';
  const others = useWorkbench.getState().layout[side].containers.filter((container) => !container.sections.some((section) => section.tab.id === id));
  closeAsk();
  const after = useWorkbench.getState().layout[side];
  const active = after.containers.find((container) => container.name === after.active);
  if (others.some((container) => container.sections.some((section) => !section.hidden))) {
    expect(active?.sections.some((section) => !section.hidden)).toBe(true);
  } else {
    expect(after.open).toBe(false);
  }
});

it('leaves the phone on its page when its Ask sheet is closed', () => {
  const before = client.session.getState();
  client.session.setState({ status: 'signed_in', userId: 'phone-reader', readerVersion: 1 });
  try {
    // What revealing Ask on a phone remembers for the device: its sheet shows Ask.
    writeViewChoice('phone-reader', 'global', 'mobileView', 'ask');
    writeViewChoice('phone-reader', 'global', 'mobileTab', 'ask-tab');
    closeAsk();
    expect(readViewChoice('phone-reader', 'global', 'mobileView', 'unset')).toBe('');
    expect(readViewChoice('phone-reader', 'global', 'mobileTab', 'unset')).toBe('');
    // Another sheet the person chose stays as it is.
    writeViewChoice('phone-reader', 'global', 'mobileView', 'files');
    closeAsk();
    expect(readViewChoice('phone-reader', 'global', 'mobileView', 'unset')).toBe('files');
  } finally {
    client.session.setState(before, true);
  }
});
