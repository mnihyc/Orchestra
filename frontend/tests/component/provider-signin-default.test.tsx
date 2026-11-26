/** P5: the screen-native flow wins even over an older cached token-import recommendation. */
import { cleanup, screen, waitFor, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { afterEach, expect, it, vi } from 'vitest';
import { startProviderLogin, retryProvider } from '@/api/provider-plan';
import type * as LoginApi from '@/api/provider-plan';
import { ProviderLogin } from '@/features/resources/providers/Login';
import type { LoginMethod, ProviderTrial } from '@/generated/api-types';
import { i18n, setLocale } from '@/i18n';
import { planProvider } from '../unit/provider-plan-fixtures';
import { show } from './support';
import { harnessName } from '../doubles/harness-names';

const words = (locale: string) => locale === 'zh-CN'
  ? { code: '使用设备码连接', codeSummary: '打开服务页面并输入设备码', token: '导入访问令牌', tokenSummary: '使用已有令牌', field: '访问令牌', help: '从你的账户取得令牌', requires: '需要有效账户' }
  : { code: 'Connect with a device code', codeSummary: 'Open the service page and enter a device code', token: 'Import an access token', tokenSummary: 'Use an existing token', field: 'Access token', help: 'Get a token from your account', requires: 'An active account is required' };
const methods = ():LoginMethod[] => {
  const text = words(i18n.language);
  return [
    {id:'setup_token',title:text.token,summary:text.tokenSummary,kind:'import',yields:'token',recommended:true,lifetime_secs:31536000,
      requires:text.requires,fields:[{name:'token',label:text.field,type:'secret',required:true,default:null,choices:null,help:text.help,max_bytes:null}]},
    {id:'browser_code',title:text.code,summary:text.codeSummary,kind:'interactive',yields:'login',recommended:false,lifetime_secs:null,requires:null,fields:[]},
  ];
};
const idle={state:'idle' as const,method:null,shows:[],asks:null,expires_at:null,error:null};
const trial = vi.hoisted(() => ({ read: vi.fn<() => Promise<ProviderTrial | null>>() }));
vi.mock('@/api/resources',async (loadOriginal) => ({ ...await loadOriginal<object>(),providerSetupQuery:()=>({queryKey:['signin-preference-setup'],queryFn:()=>Promise.resolve({subscriptions:[{agent:{id:'profile'},methods:methods()}]})})}));
vi.mock('@/api/provider-plan',async (load)=>({ ...await load<typeof LoginApi>(),
  loginStateQuery:()=>({queryKey:['signin-preference-progress'],queryFn:()=>Promise.resolve(idle)}),
  startProviderLogin:vi.fn(()=>Promise.resolve(idle)),answerProviderLogin:vi.fn(),cancelProviderLogin:vi.fn(),retryProvider:vi.fn(),
  providerTrialQuery:(id:string)=>({queryKey:['provider-trial',id],queryFn:trial.read,retry:false}),
}));
const provider=planProvider({harness_kind:'claude-code',agent:{id:'profile',name:'Claude Code',address:'acme.claude',harness_kind:'claude-code', harness: harnessName('claude-code'),machine:null,disabled:false},
  runtime_support:{lifecycle:'attach',after_up:false,commands_after_start:{supported:true,reason:null},sign_in:{home_owned:true,policy_allowed:false,allowed:true}}});
afterEach(async()=>{cleanup();vi.clearAllMocks();await setLocale('en');});

it.each(['en', 'zh-CN'] as const)('opens the actual trial record without another sign-in check and hides it after management is lost in %s', async (locale) => {
  await setLocale(locale);
  const observed: ProviderTrial = { state: 'reserved', at: '2026-11-15T00:00:00Z', input_ref: null, thread_ref: null, reserved_for: null };
  trial.read.mockResolvedValue(observed);
  vi.mocked(retryProvider).mockResolvedValue({ ...provider, trial: observed });
  const view = show(<ProviderLogin provider={provider} />);
  expect(trial.read).not.toHaveBeenCalled();
  await userEvent.click(await screen.findByRole('button', { name: i18n.t('providerRecovery.checkSignIn') }));
  await waitFor(() => expect(retryProvider).toHaveBeenCalledExactlyOnceWith(provider.id));
  expect(trial.read).not.toHaveBeenCalled();
  await userEvent.click(screen.getByRole('button', { name: i18n.t('common.details') }));
  const details = await screen.findByRole('dialog', { name: i18n.t('common.details') });
  await within(details).findByText(i18n.t('providerRecovery.trialHidden'));
  expect(trial.read).toHaveBeenCalledTimes(1);
  expect(retryProvider).toHaveBeenCalledTimes(1);
  view.rerender(<ProviderLogin provider={{ ...provider, access: { ...provider.access, permissions: ['provider.view'] } }} />);
  expect(screen.queryByRole('dialog', { name: i18n.t('common.details') })).not.toBeInTheDocument();
});

it.each(['en','zh-CN'] as const)('starts the preselected code flow with no imported secret in %s',async(locale)=>{
  await setLocale(locale);const user=userEvent.setup();show(<ProviderLogin provider={provider}/>);
  const picker=await screen.findByRole('combobox',{name:i18n.t('providerLogin.method')});
  await waitFor(()=>expect(picker).toHaveValue(words(locale).code));
  expect(screen.getByText(words(locale).codeSummary)).toBeVisible();
  expect(screen.queryByLabelText(words(locale).field)).not.toBeInTheDocument();
  expect(screen.queryByText(words(locale).help)).not.toBeInTheDocument();
  expect(startProviderLogin).not.toHaveBeenCalled();
  await user.click(screen.getByRole('button',{name:i18n.t('common.continue')}));
  await waitFor(()=>expect(startProviderLogin).toHaveBeenCalledExactlyOnceWith(provider.id,'browser_code',{}));
});

it.each(['en','zh-CN'] as const)('keeps a deliberate token import available without silently changing the choice in %s',async(locale)=>{
  await setLocale(locale);const user=userEvent.setup();show(<ProviderLogin provider={provider}/>);
  await user.click(await screen.findByRole('combobox',{name:i18n.t('providerLogin.method')}));
  await user.click(screen.getByRole('option',{name:words(locale).token}));
  const token=screen.getByLabelText(words(locale).field);
  expect(token).toHaveAttribute('type','password');
  expect(screen.getByRole('combobox',{name:i18n.t('providerLogin.method')})).toHaveValue(words(locale).token);
  expect(screen.getByText(words(locale).requires)).toBeVisible();
  expect(screen.getByRole('button',{name:i18n.t('common.continue')})).toBeDisabled();
  expect(startProviderLogin).not.toHaveBeenCalled();
  await user.type(token,'example-secret');
  await user.click(screen.getByRole('button',{name:i18n.t('common.continue')}));
  await waitFor(()=>expect(startProviderLogin).toHaveBeenCalledExactlyOnceWith(provider.id,'setup_token',{token:'example-secret'}));
});

it.each(['en','zh-CN'] as const)('uses a third program’s declared sign-in words in %s',async(locale)=>{
  await setLocale(locale);
  const third = { ...provider, harness_kind:'research-agent', agent:provider.agent === null ? null : { ...provider.agent, harness_kind:'research-agent', name:'Research assistant' } };
  show(<ProviderLogin provider={third}/>);
  const picker=await screen.findByRole('combobox',{name:i18n.t('providerLogin.method')});
  await waitFor(()=>expect(picker).toHaveValue(words(locale).code));
  expect(screen.getByText(words(locale).codeSummary)).toBeVisible();
});
