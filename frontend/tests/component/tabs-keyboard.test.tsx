/** Scrolling tab navigation preserves the complete keyboard/panel contract and external focus. */
import { cleanup, screen, waitFor } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { useState } from 'react';
import { afterEach, expect, it } from 'vitest';
import { Tabs } from '@/ui/Tabs';
import { show } from './support';

afterEach(cleanup);
function Example({ navigation = true }: { navigation?: boolean }) {
  const [value, setValue] = useState('first');
  return <><button onClick={() => setValue('last')}>External last panel</button><Tabs navigation={navigation} label="Complete tabs" value={value} onValueChange={setValue} tabs={[
    { value: 'first', label: 'First', panel: <p>First content</p> },
    { value: 'blocked', label: 'Unavailable', disabled: true, panel: <p>Unavailable content</p> },
    { value: 'middle', label: 'Middle', panel: <p>Middle content</p> },
    { value: 'last', label: 'Last', panel: <p>Last content</p> },
  ]} /></>;
}
it('includes every enabled tab in arrows and Home/End with the right labelled panel', async () => {
  show(<Example />); const user = userEvent.setup();
  await user.click(screen.getByRole('tab', { name: 'First' })); await user.keyboard('{ArrowRight}');
  await waitFor(() => expect(screen.getByRole('tab', { name: 'Middle' })).toHaveFocus());
  expect(screen.getByRole('tab', { name: 'Unavailable' })).toBeDisabled();
  expect(screen.getByRole('tabpanel', { name: 'Middle' })).toHaveTextContent('Middle content');
  await user.keyboard('{End}'); await waitFor(() => expect(screen.getByRole('tab', { name: 'Last' })).toHaveFocus());
  expect(screen.getByRole('tabpanel', { name: 'Last' })).toHaveTextContent('Last content');
  await user.keyboard('{Home}'); await waitFor(() => expect(screen.getByRole('tab', { name: 'First' })).toHaveFocus());
  expect(screen.getAllByRole('tab', { selected: true })).toHaveLength(1);
});
it('reveals externally chosen content without stealing focus', async () => {
  show(<Example />); const control = screen.getByRole('button', { name: 'External last panel' });
  await userEvent.click(control); expect(control).toHaveFocus();
  expect(screen.getByRole('tab', { name: 'Last' })).toHaveAttribute('aria-selected', 'true');
  expect(screen.getByRole('tabpanel', { name: 'Last' })).toHaveTextContent('Last content');
});
it('keeps externally owned navigation hidden and gives the panel its own region name', () => {
  show(<Example navigation={false} />); expect(screen.queryByRole('tablist')).not.toBeInTheDocument();
  expect(screen.getByRole('region', { name: 'First' })).toHaveTextContent('First content');
});

function PageTabs() {
  const [value, setValue] = useState('overview');
  return <Tabs label="Topic sections" value={value} onValueChange={setValue} overflow="menu" tabs={[
    { value: 'overview', label: 'Overview', panel: <p>Overview content</p> },
    { value: 'threads', label: 'Threads', count: 3, panel: <p>Threads content</p> },
    { value: 'environments', label: 'Environments', count: 1, panel: <p>Environments content</p> },
    { value: 'documents', label: 'Documents', panel: <p>Documents content</p> },
    { value: 'settings', label: 'Settings', panel: <p>Settings content</p> },
  ]} />;
}
it('keeps a page\'s tabs on one row, those that do not fit under More, and the chosen one always in the row', async () => {
  // Nothing is laid out here: every tab and More is 100 px wide, and the row has 320 px (a phone's page, less its margins).
  const width = Object.getOwnPropertyDescriptor(HTMLElement.prototype, 'offsetWidth');
  const room = Object.getOwnPropertyDescriptor(Element.prototype, 'clientWidth');
  Object.defineProperty(HTMLElement.prototype, 'offsetWidth', { configurable: true, get(this: HTMLElement) { return this.matches('[data-tab-value], [data-tabs-more]') ? 100 : 0; } });
  Object.defineProperty(Element.prototype, 'clientWidth', { configurable: true, get(this: Element) { return this.matches('[data-navigation-strip]') ? 320 : 0; } });
  try {
    show(<PageTabs />); const user = userEvent.setup();
    // The chosen tab and as many as fit before More; the rest wait under More, nothing on a second row.
    expect(screen.getByRole('tab', { name: /^Overview/u })).toBeVisible();
    expect(screen.getByRole('tab', { name: /^Threads/u })).toBeVisible();
    for (const name of [/^Environments/u, /^Documents/u, /^Settings/u]) expect(screen.getByRole('tab', { name, hidden: true })).not.toBeVisible();
    await user.click(screen.getByRole('button', { name: 'More' }));
    expect(screen.getByRole('menuitemradio', { name: 'Environments (1)' })).toBeVisible();
    await user.click(screen.getByRole('menuitemradio', { name: 'Settings' }));
    // Chosen from More, it takes its place in the row with its panel; another leaves for More.
    await waitFor(() => expect(screen.getByRole('tab', { name: /^Settings/u })).toBeVisible());
    expect(screen.getByRole('tab', { name: /^Settings/u })).toHaveAttribute('aria-selected', 'true');
    expect(screen.getByRole('tabpanel', { name: /^Settings/u })).toHaveTextContent('Settings content');
    expect(screen.getByRole('tab', { name: /^Overview/u })).toBeVisible();
    expect(screen.getByRole('tab', { name: /^Threads/u, hidden: true })).not.toBeVisible();
  } finally {
    if (width !== undefined) Object.defineProperty(HTMLElement.prototype, 'offsetWidth', width);
    if (room !== undefined) Object.defineProperty(Element.prototype, 'clientWidth', room);
  }
});
