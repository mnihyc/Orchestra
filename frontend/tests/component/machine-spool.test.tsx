/** A reachable machine still names blocked authoritative output beside its own state. */
import { screen } from '@testing-library/react';
import { afterEach, expect, it } from 'vitest';
import { MachineStatus } from '@/features/resources/machines/MachineList';
import { i18n, setLocale } from '@/i18n';
import { show } from './support';

afterEach(async () => { await setLocale('en'); });
for (const locale of ['en', 'zh-CN'] as const) for (const problem of ['full', 'write', 'sync', 'damaged', 'event_rejected'] as const) {
  it(`names ${problem} output backpressure without pretending the machine is offline in ${locale}`, async () => {
    await setLocale(locale);
    const machine = {status:'active' as const, online:true,spool:{problem,waiting_events:3}};
    show(<MachineStatus machine={machine} />);
    expect(screen.getByText(i18n.t(`machineSpool.${problem}`, {count:3}), {exact:false})).toBeVisible();
    expect(screen.getByText(i18n.t('machines.state.online'), {exact:false})).toBeVisible();
    expect(screen.queryByText(i18n.t('machines.state.offline'))).not.toBeInTheDocument();
  });
}
it.each(['en', 'zh-CN'] as const)('marks a storage problem as last reported after the machine disconnects in %s', async (locale) => {
  await setLocale(locale);
  const machine = {status:'active' as const,online:false,spool:{problem:'full' as const,waiting_events:3}};
  show(<MachineStatus machine={machine} />);
  expect(screen.getByText(i18n.t('machineSpool.last', {state:i18n.t('machineSpool.full', {count:3})}), {exact:false})).toBeVisible();
});
