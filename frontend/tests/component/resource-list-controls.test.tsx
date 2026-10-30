/** Criteria and order stay compact, keyboard reachable and unchanged by opening or dismissing them. */
import { cleanup, screen, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { useState } from 'react';
import { afterEach, expect, it, vi } from 'vitest';

import { ResourceListControls } from '@/features/resources/ListControls';
import { i18n, setLocale } from '@/i18n';
import { useAppearance } from '@/lib/appearance';
import { FilterBar } from '@/ui/FilterBar';
import { Input } from '@/ui/Input';
import { Select } from '@/ui/Select';
import { show } from './support';

const appearance = useAppearance.getState();
afterEach(async () => { cleanup(); useAppearance.setState(appearance); await setLocale('en'); });

function List({ changed }: { changed: (value: string | boolean) => void }) {
  const [participating, setParticipation] = useState(false);
  const [status, setStatus] = useState('any');
  const [direction, setDirection] = useState('asc');
  const statusLabel = i18n.t('providers.list.statusFilter');
  return <FilterBar wrap={false} search={<Input type="search" label={i18n.t('providers.list.search')} hideLabel />}>
    <ResourceListControls participating={participating} filtered={participating || status !== 'any'} sorted={direction !== 'asc'}
      onParticipation={(value) => { setParticipation(value); changed(value); }}
      filters={<Select label={statusLabel} value={status} onValueChange={(value) => { setStatus(value); changed(value); }} options={[
        { value: 'any', name: i18n.t('choiceCatalog.statusFilter.any.name') }, { value: 'ready', name: i18n.t('providers.status.ready') },
      ]} />}
      sorting={<Select label={i18n.t('everything.direction')} value={direction} onValueChange={(value) => { setDirection(value); changed(value); }} options={[
        { value: 'asc', name: i18n.t('inventoryChoices.everything.directions.asc.name') }, { value: 'desc', name: i18n.t('inventoryChoices.everything.directions.desc.name') },
      ]} />} />
  </FilterBar>;
}

for (const locale of ['en', 'zh-CN'] as const) for (const theme of ['light', 'dark'] as const) {
  it(`opens criteria and order without an inline control wall or an automatic write in ${locale}/${theme}`, async () => {
    await setLocale(locale); useAppearance.setState({ theme, resolvedTheme: theme });
    const user = userEvent.setup(), changed = vi.fn<(value: string | boolean) => void>();
    show(<List changed={changed} />);
    const bar = screen.getByRole('group', { name: i18n.t('ui.filterBar.label') });
    expect(within(bar).getByRole('searchbox')).toBeVisible();
    expect(within(bar).getAllByRole('button')).toHaveLength(2);
    expect(screen.queryByRole('switch')).not.toBeInTheDocument();
    expect(screen.queryByRole('combobox')).not.toBeInTheDocument();
    const filter = within(bar).getByRole('button', { name: i18n.t('resources.controls.filter') });
    filter.focus(); await user.keyboard('{Enter}');
    const criteria = screen.getByRole('dialog', { name: i18n.t('resources.controls.filter') });
    expect(within(criteria).getByRole('switch', { name: i18n.t('adminReach.focus.all') })).toBeVisible();
    expect(changed).not.toHaveBeenCalled();
    await user.click(within(criteria).getByRole('switch'));
    await user.click(within(criteria).getByRole('combobox', { name: i18n.t('providers.list.statusFilter') }));
    await user.click(screen.getByRole('option', { name: i18n.t('providers.status.ready') }));
    await user.keyboard('{Escape}');
    expect(filter).toHaveFocus();
    expect(filter).toHaveAccessibleDescription(i18n.t('resources.controls.filtered'));
    const sort = within(bar).getByRole('button', { name: i18n.t('resources.controls.sort') });
    await user.click(sort);
    await user.click(screen.getByRole('combobox', { name: i18n.t('everything.direction') }));
    await user.click(screen.getByRole('option', { name: i18n.t('inventoryChoices.everything.directions.desc.name') }));
    await user.keyboard('{Escape}');
    expect(sort).toHaveFocus();
    expect(sort).toHaveAccessibleDescription(i18n.t('resources.controls.sorted'));
    expect(changed.mock.calls.map(([value]) => value)).toEqual([true, 'ready', 'desc']);
    await user.click(filter);
    expect(screen.getByRole('switch', { name: i18n.t('adminReach.focus.only') })).toBeChecked();
    expect(screen.getByRole('combobox', { name: i18n.t('providers.list.statusFilter') })).toHaveTextContent(i18n.t('providers.status.ready'));
    await user.keyboard('{Escape}');
    expect(changed).toHaveBeenCalledTimes(3);
  });
}
