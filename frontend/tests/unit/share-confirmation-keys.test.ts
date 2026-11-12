/** Consent changes the HTTP request; a lost answer keeps that request and the original native identity. */
import { afterEach, beforeEach, expect, it, vi } from 'vitest';
import type * as Client from '@/api/client';
import { installOwnerReadReview } from '@/api/ownerReadConfirmation';
import { beginShareAttempt, shareWithAgents } from '@/api/share';
import type { OwnerReadDisclosure, ShareWithAgents } from '@/generated/api-types';
import { NetworkError } from '@/lib/requests';
import { bodyOf, type HttpTransport } from '../doubles/http-wire';

const calls=vi.hoisted(()=>({post:vi.fn<HttpTransport>()}));
vi.mock('@/api/client',async(original)=>{
  const actual=await original<typeof Client>();
  return {...actual,api:{...actual.api,POST:calls.post}};
});
const firstKey='00000000-0000-4000-8000-000000000081';
const body:ShareWithAgents={client_id:firstKey,what:{node_id:'source'},to:[{thread_id:'recipient'}],mode:'live',parts:{timeline:'read'}};
const disclosure=(count:number):OwnerReadDisclosure=>({count,threads:1,confirmation:`review-${count}`});
const refused=(count:number)=>({error:{type:'orch:error:conflict.confirmation_required',code:'conflict.confirmation_required',title:'Confirm sharing',status:409,detail:'Confirm sharing.',request_id:'review',owner_reads:disclosure(count)},response:new Response(null,{status:409})});
const accepted=()=>({data:{shares:[]},response:new Response(null,{status:201})});
const header=(index:number,name:string)=>new Headers(calls.post.mock.calls[index]?.[1].headers).get(name);
let uninstall:(()=>void)|undefined;
beforeEach(()=>vi.clearAllMocks());
afterEach(()=>{uninstall?.();uninstall=undefined;});

it('mints only after confirmation and retains the new key and body through a lost answer',async()=>{
  const gate:{answer?:(confirmed:boolean)=>void}={};
  const review=vi.fn((_:OwnerReadDisclosure)=>new Promise<boolean>(answer=>{gate.answer=answer;}));
  uninstall=installOwnerReadReview(review,()=>gate.answer?.(false));
  const lost=new NetworkError('failed');
  calls.post.mockResolvedValueOnce(refused(2)).mockRejectedValueOnce(lost).mockResolvedValue(accepted());
  const attempt=beginShareAttempt(firstKey);
  const pending=shareWithAgents(body,attempt);
  const failed=expect(pending).rejects.toBe(lost);
  await vi.waitFor(()=>expect(review).toHaveBeenCalledTimes(1));
  expect(calls.post).toHaveBeenCalledTimes(1);
  expect(attempt.key).toBe(firstKey);
  gate.answer?.(true);
  await failed;
  const confirmedKey=attempt.key;
  expect(confirmedKey).not.toBe(firstKey);
  expect(header(1,'Idempotency-Key')).toBe(confirmedKey);
  expect(header(1,'orch-owner-reads-confirmed')).toBe('review-2');
  await shareWithAgents(body,attempt);
  expect(header(2,'Idempotency-Key')).toBe(confirmedKey);
  expect(header(2,'orch-owner-reads-confirmed')).toBe('review-2');
  expect(review).toHaveBeenCalledTimes(1);
  for(const call of calls.post.mock.calls) expect(bodyOf(call)).toEqual(body);
});

it('cancel keeps the retained refusal and a later confirmation starts a fresh HTTP request',async()=>{
  const gate:{answer?:(confirmed:boolean)=>void}={};
  const review=vi.fn<(value:OwnerReadDisclosure)=>Promise<boolean>>()
    .mockResolvedValueOnce(false).mockImplementationOnce(()=>new Promise<boolean>(answer=>{gate.answer=answer;}));
  uninstall=installOwnerReadReview(review,()=>gate.answer?.(false));
  calls.post.mockResolvedValueOnce(refused(2)).mockResolvedValue(accepted());
  const attempt=beginShareAttempt(firstKey);
  await expect(shareWithAgents(body,attempt)).rejects.toMatchObject({problem:{code:'conflict.confirmation_required'}});
  expect(attempt.key).toBe(firstKey);
  const pending=shareWithAgents(body,attempt);
  await vi.waitFor(()=>expect(review).toHaveBeenCalledTimes(2));
  expect(calls.post,'a cached refusal is reviewed without replaying its burned key').toHaveBeenCalledTimes(1);
  gate.answer?.(true);
  await pending;
  expect(header(1,'Idempotency-Key')).not.toBe(firstKey);
  expect(bodyOf(calls.post.mock.calls[1]).client_id).toBe(firstKey);
});

it('a changed count requires another explicit confirmation and another HTTP key',async()=>{
  const review=vi.fn<(value:OwnerReadDisclosure)=>Promise<boolean>>().mockResolvedValue(true);
  uninstall=installOwnerReadReview(review,()=>undefined);
  calls.post.mockResolvedValueOnce(refused(1)).mockResolvedValueOnce(refused(2)).mockResolvedValue(accepted());
  const attempt=beginShareAttempt(firstKey);
  await shareWithAgents(body,attempt);
  expect(review.mock.calls.map(([value])=>value.count)).toEqual([1,2]);
  expect(new Set([header(0,'Idempotency-Key'),header(1,'Idempotency-Key'),header(2,'Idempotency-Key')]).size).toBe(3);
  expect(header(1,'orch-owner-reads-confirmed')).toBe('review-1');
  expect(header(2,'orch-owner-reads-confirmed')).toBe('review-2');
  for(const call of calls.post.mock.calls) expect(bodyOf(call).client_id).toBe(firstKey);
});
