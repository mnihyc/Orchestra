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
