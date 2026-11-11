/** Public creation retries offer sign-in without adopting a credential or revealing a refused account. */
import { cleanup, screen, waitFor } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import type { AnchorHTMLAttributes } from 'react';
import { afterEach, beforeEach, expect, it, vi } from 'vitest';
import type * as Auth from '@/api/auth';
import type * as Router from '@tanstack/react-router';
import { ApiError } from '@/api/client';
import { Register } from '@/features/auth/Register';
import { Setup } from '@/features/auth/Setup';
import { i18n, setLocale } from '@/i18n';
import { meta } from '../doubles/fixtures';
import { show } from './support';

const calls=vi.hoisted(()=>({setup:vi.fn<typeof Auth.setUp>(),register:vi.fn<typeof Auth.register>(),
  navigate:vi.fn<(...args:unknown[])=>Promise<void>>(),meta:vi.fn<()=>Promise<ReturnType<typeof meta>>>()}));
vi.mock('@/api/auth',async(load)=>({...await load<typeof Auth>(),setUp:calls.setup,register:calls.register,
  metaQuery:{queryKey:['account-replay-meta'],queryFn:calls.meta},
  nameAvailabilityQuery:(name:string)=>({queryKey:['account-replay-name',name],queryFn:()=>Promise.resolve({available:true})}),
}));
vi.mock('@tanstack/react-router',async(load)=>({...await load<typeof Router>(),useRouter:()=>({navigate:calls.navigate}),
  Link:({to,children,className}:Pick<AnchorHTMLAttributes<HTMLAnchorElement>,'children'|'className'>&{to:string})=><a href={to} className={className}>{children}</a>,
}));
beforeEach(()=>{vi.clearAllMocks();calls.meta.mockResolvedValue(meta({registration_mode:'open',needs_setup:true}));});
afterEach(async()=>{cleanup();await setLocale('en');});
const refused=()=>new ApiError({type:'orch:error:conflict.exists',code:'conflict.exists',status:409,
  title:'Already used',detail:'The account change could not be completed',request_id:'refused-registration'},null);
async function submit(kind:'setup'|'register') {
  await userEvent.type(await screen.findByLabelText(i18n.t('auth.fields.username')),'new-account');
  await userEvent.type(screen.getByLabelText(i18n.t('auth.fields.displayName')),'New account');
  await userEvent.type(screen.getByLabelText(i18n.t('auth.fields.password'),{exact:true}),'a-valid-secret-1234');
  await userEvent.click(screen.getByRole('button',{name:i18n.t(kind==='setup'?'auth.setup.submit':'auth.register.submit')}));
}
for(const locale of ['en','zh-CN'] as const) for(const kind of ['setup','register'] as const) {
  it(`acknowledges the existing account and offers only sign-in in ${locale}/${kind}`,async()=>{
    await setLocale(locale);
    calls.setup.mockResolvedValue({signedIn:false,alreadyCreated:true});
    calls.register.mockResolvedValue({signedIn:false,alreadyCreated:true});
    show(kind==='setup'?<Setup/>:<Register invitation={null}/>);
    await submit(kind);
    expect(await screen.findByText(i18n.t('auth.creation.alreadyCreated'))).toBeVisible();
    expect(screen.getByRole('link',{name:i18n.t('auth.links.signIn')})).toHaveAttribute('href','/login');
    expect(screen.queryByRole('textbox',{name:i18n.t('auth.fields.username')})).not.toBeInTheDocument();
    expect(calls.navigate).not.toHaveBeenCalled();
    const written=kind==='setup'?calls.setup.mock.calls:calls.register.mock.calls;
    expect(written).toHaveLength(1);expect(written[0]?.[1]).toMatch(/^[0-9a-f-]{36}$/i);
  });
  it(`keeps the ordinary refusal and the same attempt key on a resend in ${locale}/${kind}`,async()=>{
    await setLocale(locale);
    calls.setup.mockRejectedValue(refused());calls.register.mockRejectedValue(refused());
    show(kind==='setup'?<Setup/>:<Register invitation={null}/>);
    await submit(kind);
    const written=()=>kind==='setup'?calls.setup.mock.calls:calls.register.mock.calls;
    await waitFor(()=>expect(written()).toHaveLength(1));
    expect(screen.queryByText(i18n.t('auth.creation.alreadyCreated'))).not.toBeInTheDocument();
    const button=screen.getByRole('button',{name:i18n.t(kind==='setup'?'auth.setup.submit':'auth.register.submit')});
    await waitFor(()=>expect(button).toBeEnabled());
    await userEvent.click(button);
    await waitFor(()=>expect(written()).toHaveLength(2));
    expect(written()[1]?.[1]).toBe(written()[0]?.[1]);
    expect(calls.navigate).not.toHaveBeenCalled();
  });
}
