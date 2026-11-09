/** The one-time reset dialog acknowledges a replay without offering an empty secret. */
import { cleanup, screen, waitFor } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { afterEach, expect, it, vi } from 'vitest';
import type * as AdminApi from '@/api/admin';
import { ResetLinkDialog } from '@/features/admin/UserDialogs';
import type { AdminUser } from '@/generated/api-types';
import { i18n, setLocale } from '@/i18n';
import { show } from './support';

const calls=vi.hoisted(()=>({make:vi.fn<(...args:Parameters<typeof AdminApi.makeResetLink>)=>Promise<unknown>>()}));
vi.mock('@/api/admin',async(load)=>({...await load<typeof AdminApi>(),makeResetLink:calls.make}));
const person:AdminUser={id:'created-user',username:'reader',display_name:'Reader',kind:'human',avatar:{text:'R',color:0},
  email:null,email_verified:false,status:'active',is_system_admin:false,totp_enabled:false,email_code_second_step:false,
  locked:[],org_invited_at:null,created_at:'2026-11-01T00:00:00Z',last_seen_at:null};
afterEach(async()=>{cleanup();vi.clearAllMocks();await setLocale('en');});

it.each(['en','zh-CN'] as const)('shows an already-shown acknowledgement with a repeatable creation key (%s)',async(locale)=>{
  await setLocale(locale);
  calls.make.mockResolvedValue({url:null,expires_at:'2026-11-08T00:30:00Z',already_created:true});
  show(<ResetLinkDialog user={person} onClose={()=>undefined}/>);
  await userEvent.click(screen.getByRole('button',{name:i18n.t('admin.users.link.make')}));
  await waitFor(()=>expect(calls.make).toHaveBeenCalledOnce());
  expect(calls.make.mock.calls[0]?.[2]).toMatch(/^[0-9a-f-]{36}$/i);
  expect(await screen.findByText(i18n.t('admin.users.link.alreadyShown'))).toBeVisible();
  expect(screen.queryByRole('textbox',{name:i18n.t('admin.users.link.url')})).not.toBeInTheDocument();
  expect(screen.queryByRole('button',{name:i18n.t('admin.users.link.copy')})).not.toBeInTheDocument();
  expect(calls.make).toHaveBeenCalledOnce();
});
