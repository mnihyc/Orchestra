/** Grouped kind controls keep each choice visible once and preserve choices in the other areas. */
import { cleanup, screen, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { useState } from 'react';
import { afterEach, expect, it } from 'vitest';

import { ALL_KINDS, KIND_AREA, KIND_AREAS, KIND_LOOK } from '@/features/inbox/kinds';
import { NotificationKinds } from '@/features/settings/personal/NotificationKinds';
import type { NotificationKind } from '@/generated/api-types';
import { i18n, setLocale } from '@/i18n';
import { show } from './support';

afterEach(async () => { cleanup(); await setLocale('en'); });

function Choices() {
  const [disabled, setDisabled] = useState<NotificationKind[]>(['access.requested', 'machine.offline']);
  return <NotificationKinds disabledKinds={disabled} onChange={setDisabled} />;
}

for (const locale of ['en', 'zh-CN'] as const) {
  it(`groups every kind once and changes only the chosen kind in ${locale}`, async () => {
    await setLocale(locale);
    show(<><nav><a href="/org/acme">{i18n.t('navigation.projects')}</a></nav><Choices /></>);
    expect(screen.getAllByText(i18n.t('navigation.projects'), { exact: true })).toHaveLength(1);
    expect(screen.getAllByRole('group')).toHaveLength(KIND_AREAS.length);
    expect(screen.getAllByRole('checkbox')).toHaveLength(ALL_KINDS.length);
    for (const kind of ALL_KINDS) {
      const area = screen.getByRole('group', { name: i18n.t(`notifications.kindAreas.${KIND_AREA[kind]}`) });
      expect(within(area).getByRole('checkbox', { name: i18n.t(KIND_LOOK[kind].label) })).toBeVisible();
    }
    const access = screen.getByRole('checkbox', { name: i18n.t(KIND_LOOK['access.requested'].label) });
    const machine = screen.getByRole('checkbox', { name: i18n.t(KIND_LOOK['machine.offline'].label) });
    const turn = screen.getByRole('checkbox', { name: i18n.t(KIND_LOOK['turn.completed'].label) });
    expect(access).not.toBeChecked(); expect(machine).not.toBeChecked(); expect(turn).toBeChecked();
    await userEvent.click(access);
    expect(access).toBeChecked(); expect(machine).not.toBeChecked(); expect(turn).toBeChecked();
    await userEvent.click(turn);
    expect(turn).not.toBeChecked(); expect(access).toBeChecked(); expect(machine).not.toBeChecked();
  });
}
