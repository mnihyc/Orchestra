/** A reachable machine still names blocked authoritative output beside its own state. */
import { screen } from '@testing-library/react';
import { afterEach, expect, it } from 'vitest';
import { MachineStatus } from '@/features/resources/machines/MachineList';
import { i18n, setLocale } from '@/i18n';
import { useAppearance } from '@/lib/appearance';
import { formatNumber } from '@/lib/format';
import { show } from './support';

const previousAppearance=useAppearance.getState();
afterEach(async () => { useAppearance.setState(previousAppearance); await setLocale('en'); });
it.each(['en','zh-CN'] as const)('groups a large waiting output count in the machine state in %s',async(locale)=>{
  await setLocale(locale);useAppearance.setState({locale});
  show(<MachineStatus machine={{status:'active',online:true,spool:{problem:'full',waiting_events:1234567}}} />);
  expect(screen.getByText(formatNumber(1234567,locale),{exact:false})).toBeVisible();
});
for (const locale of ['en', 'zh-CN'] as const) for (const problem of ['full', 'write', 'sync', 'damaged', 'event_rejected'] as const) {
  it(`names ${problem} output backpressure without pretending the machine is offline in ${locale}`, async () => {
    await setLocale(locale);
    const machine = {status:'active' as const, online:true,spool:{problem,waiting_events:3}};
    show(<MachineStatus machine={machine} />);
    expect(screen.getByText(i18n.t(`machineSpool.${problem}`, {count:3,waiting:'3'}), {exact:false})).toBeVisible();
    expect(screen.getByText(i18n.t('machines.state.online'), {exact:false})).toBeVisible();
    expect(screen.queryByText(i18n.t('machines.state.offline'))).not.toBeInTheDocument();
  });
}
it.each(['en', 'zh-CN'] as const)('marks a storage problem as last reported after the machine disconnects in %s', async (locale) => {
  await setLocale(locale);
  const machine = {status:'active' as const,online:false,spool:{problem:'full' as const,waiting_events:3}};
  show(<MachineStatus machine={machine} />);
  expect(screen.getByText(i18n.t('machineSpool.last', {state:i18n.t('machineSpool.full', {count:3,waiting:'3'})}), {exact:false})).toBeVisible();
});
