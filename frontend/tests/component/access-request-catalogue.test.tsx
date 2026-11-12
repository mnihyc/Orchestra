/** No guessed access level or early request while the actual catalogue is still loading. */
import { act, fireEvent, screen, waitFor } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { afterEach, beforeEach, expect, it, vi } from 'vitest';
import type * as AccessApi from '@/api/access';
import { client } from '@/api/client';
import { AccessRequest } from '@/features/access/AccessRequest';
import type { MissingAccess, PermissionCatalogueGetReply } from '@/generated/api-types';
import { i18n, setLocale } from '@/i18n';
import { ADA, CATALOGUE, TOPIC } from '../unit/general-access-fixtures';
import { show } from './support';

const calls=vi.hoisted(()=>({catalogue:vi.fn<()=>Promise<PermissionCatalogueGetReply>>(),request:vi.fn<typeof AccessApi.requestAccess>()}));
vi.mock('@/api/access',async load=>({...await load<typeof AccessApi>(),
  permissionsQuery:{queryKey:['pending-request-catalogue'],queryFn:()=>calls.catalogue()},
  requestAccess:(...args:Parameters<typeof AccessApi.requestAccess>)=>calls.request(...args),
  accessRequestQuery:(batch:string)=>({queryKey:['pending-request',batch],queryFn:()=>Promise.resolve({batch_id:batch,approvals:[],complete:false})}),
}));
const missing:MissingAccess={object:{kind:'thread',id:TOPIC.id,name:'Work',owner:ADA},permissions:['thread.view'],ask:[ADA],ask_more:0,closed:null,override:false,options:null};
beforeEach(()=>{vi.clearAllMocks();sessionStorage.clear();client.session.setState({status:'signed_in',userId:'reader'});calls.request.mockResolvedValue({batch_id:'batch',approvals:[]});});
afterEach(async()=>{act(()=>client.session.setState({status:'signed_out',userId:null}));await setLocale('en');});

for(const locale of ['en','zh-CN'] as const) it(`waits for the catalogue before selecting or submitting a level (${locale})`,async()=>{
  await setLocale(locale);
  let answer:((value:PermissionCatalogueGetReply)=>void)|undefined;
  calls.catalogue.mockImplementation(()=>new Promise(resolve=>{answer=resolve;}));
  show(<AccessRequest missing={[missing]} objectId={TOPIC.id} />);
  const send=screen.getByRole('button',{name:i18n.t('access.request.send')});
  expect(send).toBeDisabled();
  expect(screen.getByText(i18n.t('startImplicit.checkingMissing'))).toBeVisible();
  expect(screen.queryByRole('radio')).toBeNull();
  const form=send.closest('form'); if(form===null)throw new Error('The request form is required');
  fireEvent.submit(form);
  expect(calls.request).not.toHaveBeenCalled();
  await waitFor(()=>expect(answer).toBeDefined());
  await act(()=>{answer?.(CATALOGUE.reply);return Promise.resolve();});
  await waitFor(()=>expect(send).toBeEnabled());
  const selected=screen.getByRole('radio',{name:i18n.t('access.request.levels.full_view')});
  expect(selected).toBeChecked();
  expect(selected).toHaveAccessibleDescription(expect.stringContaining(i18n.t('startImplicit.origin.suggested')));
  await userEvent.click(send);
  await waitFor(()=>expect(calls.request).toHaveBeenCalledOnce());
  expect(calls.request.mock.calls[0]?.[0].items[0]?.permissions).toContain('thread.view');
});

it('keeps a failed catalogue visible and sends nothing until its retry succeeds',async()=>{
  calls.catalogue.mockRejectedValue(new Error('Catalogue read failed'));
  show(<AccessRequest missing={[missing]} objectId={TOPIC.id} />);
  const retry=await screen.findByRole('button',{name:i18n.t('common.tryAgain')});
  const send=screen.getByRole('button',{name:i18n.t('access.request.send')});
  expect(send).toBeDisabled();
  fireEvent.submit(send.closest('form')!);
  expect(calls.request).not.toHaveBeenCalled();
  calls.catalogue.mockResolvedValue(CATALOGUE.reply);
  await userEvent.click(retry);
  await waitFor(()=>expect(send).toBeEnabled());
});

it('preserves an exact request when the served catalogue offers no grouped levels',async()=>{
  calls.catalogue.mockResolvedValue({...CATALOGUE.reply,choices:[]});
  show(<AccessRequest missing={[missing]} objectId={TOPIC.id} />);
  const send=screen.getByRole('button',{name:i18n.t('access.request.send')});
  await waitFor(()=>expect(send).toBeEnabled());
  expect(screen.queryByRole('radio')).toBeNull();
  await userEvent.click(send);
  await waitFor(()=>expect(calls.request).toHaveBeenCalledOnce());
  expect(calls.request.mock.calls[0]?.[0].items[0]?.permissions).toEqual(missing.permissions);
});

it('keeps exact fixed requests usable without an everyday-choice catalogue',async()=>{
  calls.catalogue.mockImplementation(()=>new Promise(()=>undefined));
  show(<AccessRequest missing={[missing]} objectId={TOPIC.id} fixed />);
  const send=screen.getByRole('button',{name:i18n.t('access.request.send')});
  expect(send).toBeEnabled();
  await userEvent.click(send);
  await waitFor(()=>expect(calls.request).toHaveBeenCalledOnce());
  expect(calls.request.mock.calls[0]?.[0].items[0]?.permissions).toEqual(missing.permissions);
});
