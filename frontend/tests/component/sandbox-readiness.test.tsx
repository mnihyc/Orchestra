/** Sandbox readiness says which boundary is missing and offers only authorized recovery links. */
import { screen, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import type { ReactNode } from 'react';
import { describe, expect, it, vi } from 'vitest';
import { MachineSandboxReadiness, SandboxReadinessNotice } from '@/features/resources/SandboxReadiness';
import type { EnvironmentSandboxReadiness } from '@/generated/api-types';
import { i18n, setLocale } from '@/i18n';
import { show } from './support';

vi.mock('@tanstack/react-router', () => ({
  Link: ({ to, params, search, children }: { to:string; params:{id:string}; search?:{tab:string}; children:ReactNode }) =>
    <a href={`${to.replace('$id', params.id)}${search ? `?tab=${search.tab}` : ''}`}>{children}</a>,
}));

function readiness():EnvironmentSandboxReadiness {
  const named = { executor_ref: { kind: 'orch-exec', title: { en: 'Orch tools', 'zh-CN': 'Orch 工具' }, tool_set: null } };
  return {
    environment:{id:'environment',name:'Acme workspace',placement:'connected',status:'running',ports_exposed:false,address:'ada/work',
      owner:{id:'ada',username:'ada',display_name:'Ada',kind:'human',avatar:{text:'A',color:1}}},
    machine:{id:'machine',name:'world-box',placement:'connected',platform_os:'linux',online:true},
    policy_manage:true,
    sandbox:{...named,ready:false,requested:'open',widest:'workspace',source:'machine_policy',machine_limit:'workspace',template_limit:null,
      executor:'orch-exec',enforceable:['open'],agent_modes:[],reason:null},
  };
}

describe('sandbox readiness before a thread exists', () => {
  it('names the machine ceiling, the actual tools and the permitted act', async () => {
    await setLocale('en');
    show(<SandboxReadinessNotice value={readiness()} />);
    expect(screen.getByRole('alert')).toHaveTextContent('Acme workspace: tool access needs attention');
    expect(screen.getByRole('alert')).toHaveTextContent("world-box cannot restrict Orch tools. Full access uses the environment's own protection.");
    expect(screen.getByRole('alert')).toHaveTextContent('at most Workspace (machine policy)');
    expect(screen.getByRole('alert')).toHaveTextContent("Allow Full access in the machine's maximum tool access policy");
    expect(screen.getByRole('link',{name:'Open world-box policy'})).toHaveAttribute('href','/resources/machines/machine?tab=policy');
  });

  it('keeps the explanation for a reader without offering machine management', async () => {
    await setLocale('en');
    const value=readiness();
    value.policy_manage=false;
    show(<SandboxReadinessNotice value={value} />);
    expect(screen.getByRole('alert')).toHaveTextContent('world-box');
    expect(screen.queryByRole('link',{name:'Open world-box policy'})).not.toBeInTheDocument();
    expect(screen.getByRole('link',{name:'Open environment'})).toHaveAttribute('href','/resources/environments/environment');
  });

  it('distinguishes a template limit from the machine and gives its own act', async () => {
    await setLocale('en');
    const value=readiness();
    value.sandbox={...value.sandbox,source:'template',widest:'read_only',machine_limit:'open',template_limit:'read_only'};
    show(<SandboxReadinessNotice value={value} />);
    expect(screen.getByRole('alert')).toHaveTextContent('at most Read only (template limit)');
    expect(screen.getByRole('alert')).toHaveTextContent("Allow Full access in the environment template's maximum tool access");
    expect(screen.queryByRole('link',{name:'Open world-box policy'})).not.toBeInTheDocument();
  });

  it('keeps a narrower place request explicit even when the machine allows Full access', async () => {
    await setLocale('en');
    const value=readiness();
    value.sandbox={...value.sandbox,requested:'workspace',widest:'open',machine_limit:'open'};
    show(<SandboxReadinessNotice value={value} />);
    expect(screen.getByRole('alert')).toHaveTextContent('This place asks for Workspace');
    expect(screen.getByRole('alert')).toHaveTextContent("Allow Full access in this place's Tool access setting");
    expect(screen.queryByRole('link',{name:'Open world-box policy'})).not.toBeInTheDocument();
  });

  it('shows readiness after the machine permits the supported mode, without a phantom template', async () => {
    await setLocale('en');
    const value=readiness();
    value.sandbox={...value.sandbox,ready:true,widest:'open',machine_limit:'open'};
    show(<SandboxReadinessNotice value={value} />);
    expect(screen.getByRole('status')).toHaveTextContent('Acme workspace: tool access ready');
    expect(screen.queryByRole('alert')).not.toBeInTheDocument();
    expect(screen.queryByRole('link')).not.toBeInTheDocument();
    expect(screen.queryByText(/template/)).not.toBeInTheDocument();
  });

  it('translates the capability, limit and recovery act in Chinese', async () => {
    await setLocale('zh-CN');
    show(<SandboxReadinessNotice value={readiness()} />);
    expect(screen.getByRole('alert')).toHaveTextContent('world-box');
    expect(screen.getByRole('alert')).toHaveTextContent('Acme workspace：工具权限需要处理');
    expect(screen.getByRole('alert')).toHaveTextContent('在机器的最高工具权限策略中允许完全访问');
    expect(screen.getByRole('alert')).not.toHaveTextContent('sandboxReadiness.');
    expect(screen.getByRole('alert')).not.toHaveTextContent('machine_policy');
    expect(screen.getByRole('alert')).not.toHaveTextContent('cannot confine commands');
    expect(screen.getAllByRole('link')).toHaveLength(2);
    await setLocale('en');
  });
});

for (const locale of ['en', 'zh-CN'] as const) it(`names a third program's reported executor in ${locale}`, async () => {
  await setLocale(locale);
  try {
    const value = readiness();
    const named = { executor_ref: { kind: 'research-exec', title: { en: 'Research tools', 'zh-CN': '研究工具' }, tool_set: null } };
    value.sandbox = { ...value.sandbox, ...named, executor: 'research-exec' };
    show(<SandboxReadinessNotice value={value} />);
    expect(screen.getByRole('alert')).toHaveTextContent(locale === 'en' ? 'Research tools' : '研究工具');
    expect(screen.getByRole('alert')).not.toHaveTextContent('research-exec');
  } finally { await setLocale('en'); }
});

for (const locale of ['en', 'zh-CN'] as const) it(`keeps one dismissible readiness line and reveals a changed refusal (${locale})`, async () => {
  await setLocale(locale);
  const value = readiness(), user = userEvent.setup();
  value.sandbox = { ...value.sandbox, ready: true, widest: 'open', machine_limit: 'open' };
  const view = show(<MachineSandboxReadiness values={[value, value]} />);
  const list = screen.getByRole('list', { name: i18n.t('sandboxReadiness.summary') });
  expect(within(list).getAllByRole('listitem')).toHaveLength(1);
  expect(within(list).getByText(i18n.t('sandboxReadiness.modeReady', { mode: i18n.t('sandboxReadiness.modes.open.name') }))).toBeVisible();
  expect(screen.queryByText(/world-box cannot/)).not.toBeInTheDocument();
  expect(screen.queryByRole('status')).not.toBeInTheDocument();
  await user.click(screen.getByRole('button', { name: i18n.t('sandboxReadiness.dismiss', { name: value.environment.name }) }));
  expect(screen.queryByRole('list', { name: i18n.t('sandboxReadiness.summary') })).not.toBeInTheDocument();
  view.rerender(<MachineSandboxReadiness values={[structuredClone(value)]} />);
  expect(screen.queryByRole('list', { name: i18n.t('sandboxReadiness.summary') })).not.toBeInTheDocument();
  const refused = { ...value, sandbox: { ...value.sandbox, ready: false, widest: 'workspace' as const, machine_limit: 'workspace' as const } };
  view.rerender(<MachineSandboxReadiness values={[refused, value]} />);
  expect(screen.getAllByRole('listitem')).toHaveLength(1);
  expect(screen.getByText(i18n.t('sandboxReadiness.modeBlocked', { mode: i18n.t('sandboxReadiness.modes.open.name') }))).toBeVisible();
  await user.click(screen.getByRole('button', { name: i18n.t('help.hint.label', { term: value.environment.name }) }));
  expect(await screen.findByRole('link', { name: i18n.t('sandboxReadiness.openPolicy', { name: 'world-box' }) })).toHaveAttribute('href', '/resources/machines/machine?tab=policy');
  await setLocale('en');
});
