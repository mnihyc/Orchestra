/** Temporary consent, deadline, Keep and a live export result are usable without a thread or polling. */
import { screen, waitFor } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { useState } from 'react';
import { afterEach, beforeEach, expect, it, vi } from 'vitest';
import { EnvironmentExportButton } from '@/features/resources/environments/WorkspaceExport';
import { TemporaryChoice, TemporaryStatus } from '@/features/resources/environments/Temporary';
import { EnvironmentStatusText } from '@/features/resources/environments/EnvironmentList';
import { NotificationRow } from '@/features/inbox/NotificationRow';
import type * as EnvironmentsApi from '@/api/environments';
import type { Environment, Job, Notification } from '@/generated/api-types';
import { i18n, setLocale } from '@/i18n';
import { useAppearance } from '@/lib/appearance';
import { formatDateTime } from '@/lib/time';
import { show } from './support';

const calls=vi.hoisted(()=>({keep:vi.fn<() => Promise<unknown>>(),export:vi.fn<typeof EnvironmentsApi.exportEnvironmentFiles>(),download:vi.fn<() => Promise<Blob>>(),save:vi.fn(),help:vi.fn()}));
vi.mock('@/features/help/open',()=>({useHelpOpener:()=>calls.help}));
vi.mock('@tanstack/react-router',()=>({useNavigate:()=>vi.fn()}));
vi.mock('@/api/environments',async(original)=>({...await original<object>(),keepEnvironment:calls.keep,exportEnvironmentFiles:calls.export,downloadEnvironmentExport:calls.download}));
vi.mock('@/features/export/save',()=>({saveFile:calls.save}));

const access:Environment['access']={permissions:['environment.manage'],administers:false,participating:true,admin_only:[],through:[]};
const expiresAt='2026-10-18T12:00:00.000Z';
const environment:Pick<Environment,'id'|'name'|'temporary'|'access'|'removal'>={id:'temporary-env',name:'Experiment',access,removal:null,temporary:{expires_at:expiresAt,idle_hours:24,on_archive:false}};
const finished:Job={id:'export-job',kind:'snapshot',node_id:null,environment_id:environment.id,provider_id:null,batch_id:null,state:'succeeded',
  result:{snapshot_id:'export-copy',missing_count:0},error:null,log:null,created_by:null,created_at:'2026-10-16T12:00:00.000Z',started_at:null,finished_at:'2026-10-16T12:00:01.000Z',
  progress:{phase:'finalize',percent:100,phases:[],files_done:1,files_total:1,bytes_done:12,bytes_total:12,rate_bps:null,eta_seconds:null,message:null}};

const originalAppearance=useAppearance.getState();
beforeEach(()=>{vi.clearAllMocks();calls.keep.mockReset();calls.export.mockReset();calls.download.mockReset();useAppearance.setState({locale:'en',timeZone:'UTC',timeZoneMode:'fixed'});calls.keep.mockResolvedValue({...environment,temporary:null});calls.export.mockResolvedValue(finished);calls.download.mockResolvedValue(new Blob(['copied files']));});
afterEach(async()=>{useAppearance.setState({locale:originalAppearance.locale,timeZone:originalAppearance.timeZone,timeZoneMode:originalAppearance.timeZoneMode});await setLocale('en');});

it.each(['en','zh-CN'] as const)('keeps the Temporary explanation visible and its exact cleanup timing accessible (%s)',async(locale)=>{
  await setLocale(locale);const user=userEvent.setup();
  function Consent(){const [checked,setChecked]=useState(false);return <TemporaryChoice checked={checked} onChange={setChecked} />;}
  show(<Consent />);
  const control=screen.getByRole('switch',{name:i18n.t('temporary.name')});
  expect(screen.getByText(i18n.t('temporary.description'))).toBeVisible();
  await user.click(control);expect(control).toBeChecked();
  expect(screen.queryByText(i18n.t('temporary.consent'))).not.toBeInTheDocument();
  // A click dismisses the pointer's current tooltip. Leave and re-enter to ask for the new state.
  await user.unhover(control);
  await user.hover(control);
  expect(await screen.findByRole('tooltip')).toHaveTextContent(i18n.t('temporary.consent'));
  expect(control).toHaveAccessibleDescription(`${i18n.t('temporary.description')} ${i18n.t('temporary.consent')}`);
  expect(document.querySelector('[data-choice-selected-description]')).toBeNull();
  await user.click(screen.getByRole('button',{name:i18n.t('taskGuides.open',{title:i18n.t('taskGuides.temporaryEnvironments')})}));
  expect(calls.help).toHaveBeenCalledWith('temporary-environments');
  expect(control).toBeChecked();
});

it.each(['en','zh-CN'] as const)('shows a single lifetime row with Keep and exports the complete workspace (%s)',async(locale)=>{
  await setLocale(locale);useAppearance.setState({locale});const user=userEvent.setup();
  const view=show(<TemporaryStatus environment={environment} />);
  expect(view.container.querySelectorAll('[data-temporary-environment]')).toHaveLength(1);
  const when=formatDateTime(expiresAt,{locale,timeZone:'UTC'});
  expect(screen.getByText(i18n.t('temporary.status',{when}))).toBeVisible();
  await user.click(screen.getByRole('button',{name:i18n.t('temporary.keep')}));
  await waitFor(()=>expect(calls.keep).toHaveBeenCalledOnce());
  await user.click(screen.getByRole('button',{name:i18n.t('temporary.export')}));
  await user.click(await screen.findByRole('button',{name:i18n.t('temporary.download')}));
  await waitFor(()=>expect(calls.save).toHaveBeenCalledWith(expect.any(Blob),'Experiment.zip'));
  expect(calls.export).toHaveBeenCalledOnce();expect(calls.download).toHaveBeenCalledWith('export-copy');
});

it.each(['en','zh-CN'] as const)('distinguishes the idle limit from the warning before deletion (%s)',async(locale)=>{
  await setLocale(locale);const user=userEvent.setup();
  const consent=show(<TemporaryChoice checked onChange={vi.fn()}/>);
  await user.hover(screen.getByRole('switch',{name:i18n.t('temporary.name')}));
  const warning=await screen.findByRole('tooltip');
  expect(warning).toHaveTextContent(locale==='en'?'idle limit':'空闲期限');
  expect(warning).toHaveTextContent(locale==='en'?'at least an hour':'至少提前一小时');
  consent.unmount();
  show(<TemporaryStatus environment={{...environment,temporary:{expires_at:null,idle_hours:24,on_archive:false}}}/>);
  expect(screen.getByText(i18n.t('temporary.status',{when:i18n.t('temporary.afterIdle',{count:24})}))).toBeVisible();
  expect(calls.keep).not.toHaveBeenCalled();expect(calls.export).not.toHaveBeenCalled();
});

it('a lasting environment has no temporary label or duplicate lifecycle actions',()=>{
  show(<section aria-label="Environment lifetime"><TemporaryStatus environment={{...environment,temporary:null}} /></section>);
  // The support wrapper also renders the independent notification region.
  expect(screen.getByRole('region',{name:'Environment lifetime'})).toBeEmptyDOMElement();
});

it('explains the irreversible removal boundary instead of offering a misleading Keep or Export',()=>{
  const removal:NonNullable<Environment['removal']>={phase:'removing',deleted_at:null,deleted_by:null,keep_days:0,purge_at:null,purged_at:null,bytes:null,size_at:null,reason:null,revision:1,can_inspect:false,can_restore:false,can_purge:false,job:null};
  show(<><EnvironmentStatusText environment={{status:'running',removal}}/><TemporaryStatus environment={{...environment,removal}} /></>);
  expect(screen.getAllByText(i18n.t('environments.lifecycle.removing'),{exact:true})).toHaveLength(1);
  expect(screen.queryByText(i18n.t('environments.status.running'),{exact:true})).not.toBeInTheDocument();
  expect(screen.queryByRole('button',{name:'Keep'})).not.toBeInTheDocument();
  expect(screen.queryByRole('button',{name:'Export files'})).not.toBeInTheDocument();
});

it('an active environment shows idle time, and a reader sees no management action',()=>{
  show(<TemporaryStatus environment={{...environment,access:{...access,permissions:['environment.view']},temporary:{expires_at:null,idle_hours:6,on_archive:true}}} />);
  expect(screen.getByText(i18n.t('temporary.statusThread',{when:i18n.t('temporary.afterIdle',{count:6})}))).toBeVisible();
  expect(screen.queryByRole('button',{name:'Keep'})).not.toBeInTheDocument();
  expect(screen.queryByRole('button',{name:'Export files'})).not.toBeInTheDocument();
});

it.each(['en','zh-CN'] as const)('a failed export says what happened and permits another job without claiming a download (%s)',async(locale)=>{
  await setLocale(locale);useAppearance.setState({locale});
  const diagnostic='Reconnect this machine and export again.';
  const user=userEvent.setup();calls.export.mockResolvedValueOnce({...finished,state:'failed',result:null,error:{code:'export_scan_failed',message:diagnostic,phase:'scan',undone:true}})
    .mockResolvedValueOnce({...finished,id:'export-retry-job'});
  show(<EnvironmentExportButton id={environment.id} name={environment.name} />);
  await user.click(screen.getByRole('button',{name:i18n.t('temporary.export')}));
  // Unknown job diagnostics use the shared localized recovery sentence, never raw program text.
  expect(await screen.findByText(i18n.t('ui.error.operationFailed'))).toBeVisible();
  expect(screen.queryByText(diagnostic)).not.toBeInTheDocument();
  expect(screen.queryByRole('button',{name:i18n.t('temporary.download')})).not.toBeInTheDocument();
  expect(calls.download).not.toHaveBeenCalled();
  await user.click(screen.getByRole('button',{name:i18n.t('temporary.retry')}));
  expect(await screen.findByRole('button',{name:i18n.t('temporary.download')})).toBeVisible();
  expect(calls.export).toHaveBeenCalledTimes(2);
  const firstKey=calls.export.mock.calls[0]?.[1];
  expect(firstKey).toEqual(expect.any(String));
  expect(calls.export.mock.calls[1]?.[1]).not.toBe(firstKey);
});

it.each(['en','zh-CN'] as const)('offers Keep and Export directly in the owners warning, with one result after Keep (%s)',async(locale)=>{
  await setLocale(locale);const user=userEvent.setup();
  const data={reason:'temporary',environment_id:environment.id,subject:environment.name,expires_at:'2026-10-18T12:00:00.000Z'};
  const notice:Notification={id:'removal-warning',kind:'environment.attention',title:'Experiment will be removed',body:'',data,
    details:{v:1,action:'environment.attention',parameters:data},node:null,thread_id:null,chat_id:null,approval_id:null,url:`/resources/environments/${environment.id}`,
    created_at:'2026-10-18T11:00:00.000Z',read_at:null,inbox_seq:1,group:null,waiting_for_me:false};
  show(<ul><NotificationRow row={notice} /></ul>);
  expect(screen.getByRole('button',{name:i18n.t('temporary.export')})).toBeVisible();
  await user.click(screen.getByRole('button',{name:i18n.t('temporary.keep')}));
  expect(await screen.findByText(i18n.t('temporary.kept'))).toBeVisible();
  expect(screen.queryByText(i18n.t('temporary.noticeTitle',{name:environment.name}))).not.toBeInTheDocument();
  expect(screen.queryByRole('button',{name:i18n.t('temporary.export')})).not.toBeInTheDocument();
  expect(calls.keep).toHaveBeenCalledOnce();
});
