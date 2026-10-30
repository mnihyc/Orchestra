/** Empty choices open the same creation form and retain the unfinished starter. */
import { screen,waitFor } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import type { ComponentProps,ReactNode } from 'react';
import { afterEach,expect,it,vi } from 'vitest';
import type { AddProviderDialog } from '@/features/resources/providers/AddProvider';
import { ResourceCreationActions } from '@/features/resources/ResourceCreation';
import { i18n,setLocale } from '@/i18n';
import { planProvider } from '../unit/provider-plan-fixtures';
import { show } from './support';

vi.mock('@tanstack/react-router',()=>({Link:({children,target}:{children:ReactNode;target?:string})=><a href="/resources/providers/plan-provider?tab=key" target={target}>{children}</a>}));
vi.mock('@/features/resources/providers/AddProvider',()=>({AddProviderDialog:({onClose,onCreated}:ComponentProps<typeof AddProviderDialog>)=><div role="dialog"><button onClick={()=>{onCreated?.(planProvider());onClose();}}>Create the provider</button></div>}));
afterEach(async()=>{await setLocale('en');});
it.each(['en','zh-CN'] as const)('preserves the input and rereads choices only on its explicit act in %s',async(locale)=>{
  await setLocale(locale);const t=i18n.getFixedT(locale);const user=userEvent.setup();const read=vi.fn();
  show(<><textarea aria-label="Unsent message" defaultValue="My next task"/><ResourceCreationActions onReadChoices={read}/></>);
  expect(screen.getAllByRole('button',{name:t('resourceCreation.providers')})).toHaveLength(1);
  expect(screen.getAllByRole('button',{name:t('resourceCreation.agents')})).toHaveLength(1);
  await user.click(screen.getByRole('button',{name:t('resourceCreation.providers')}));
  await user.click(await screen.findByRole('button',{name:'Create the provider'}));
  await waitFor(()=>expect(screen.queryByRole('dialog')).not.toBeInTheDocument());
  expect(screen.getByRole('textbox',{name:'Unsent message'})).toHaveValue('My next task');
  expect(screen.getByRole('link',{name:t('resourceCreation.finishProvider')})).toHaveAttribute('target','_blank');
  expect(read).not.toHaveBeenCalled();
  await user.click(screen.getByRole('button',{name:t('resourceCreation.readChoices')}));
  expect(read).toHaveBeenCalledExactlyOnceWith('providers');
});
