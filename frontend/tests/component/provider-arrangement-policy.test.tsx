/** A provider manager cannot enable a way its credential's machine forbids. */
import { cleanup, screen } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { afterEach, expect, it, vi } from 'vitest';
import { updateProvider } from '@/api/resources';
import type * as ResourcesApi from '@/api/resources';
import { ProviderArrangements } from '@/features/resources/providers/Arrangements';
import type { ArrangementType } from '@/generated/api-types';
import { i18n, setLocale } from '@/i18n';
import { planProvider } from '../unit/provider-plan-fixtures';
import { show } from './support';

vi.mock('@/features/help/Hint',()=>({Hint:()=>null}));
vi.mock('@/api/resources',async(load)=>({...await load<typeof ResourcesApi>(),updateProvider:vi.fn()}));
const kinds:ArrangementType[]=['separated','key_separated','credential_shared','handed_over'];
afterEach(async()=>{cleanup();vi.clearAllMocks();await setLocale('en');});
it.each(['en','zh-CN'] as const)('shows the keeper restriction and prevents an unsupported enable in %s',async(locale)=>{
  await setLocale(locale);const user=userEvent.setup();
  const provider=planProvider({arrangements:kinds.map((type)=>({type,allowed:type==='separated'||type==='key_separated',available:type==='separated'||type==='key_separated',puts:'',form:null}))});
  show(<ProviderArrangements provider={provider}/>);
  expect(screen.getByText(i18n.t('providers.arrangements.machineRestricted'))).toBeVisible();
  for(const kind of ['credential_shared','handed_over'] as const){
    const control=screen.getByRole('checkbox',{name:i18n.t(`providers.arrangements.types.${kind}.name`)});
    expect(control).toBeDisabled();expect(control).not.toBeChecked();
    await user.click(control);
  }
  expect(updateProvider).not.toHaveBeenCalled();
  expect(screen.queryByRole('alertdialog')).not.toBeInTheDocument();
});
