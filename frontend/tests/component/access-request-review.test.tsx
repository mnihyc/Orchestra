/** A focused Share form admits pending decisions only when their own review is requested. */
import { cleanup, screen, waitFor } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { afterEach, beforeEach, expect, it, vi } from 'vitest';
import type * as AccessApi from '@/api/access';
import { RequestReview } from '@/features/access/RequestCard';
import type { Approval } from '@/generated/api-types';
import { i18n, setLocale } from '@/i18n';
import { show } from './support';

const reads=vi.hoisted(()=>({read:vi.fn<(nodeId:string)=>Promise<Approval[]>>()}));
vi.mock('@/api/access',async original=>({...await original<typeof AccessApi>(),
  waitingRequestsQuery:(nodeId:string)=>({queryKey:['review-requests',nodeId],queryFn:()=>reads.read(nodeId),retry:false}),
}));

beforeEach(()=>{reads.read.mockReset().mockResolvedValue([]);});
afterEach(async()=>{cleanup();await setLocale('en');});

it.each(['en','zh-CN'] as const)('keeps decisions available without reading them before review (%s)',async(locale)=>{
  await setLocale(locale);
  const user=userEvent.setup();
  show(<RequestReview nodeId="topic" />);
  expect(reads.read).not.toHaveBeenCalled();
  expect(screen.queryByText(i18n.t('access.waiting.empty'))).not.toBeInTheDocument();
  await user.click(screen.getByRole('button',{name:i18n.t('access.waiting.review')}));
  expect(await screen.findByText(i18n.t('access.waiting.empty'))).toBeVisible();
  expect(reads.read).toHaveBeenCalledExactlyOnceWith('topic');
  await user.keyboard('{Escape}');
  expect(screen.queryByText(i18n.t('access.waiting.empty'))).not.toBeInTheDocument();
  expect(screen.getByRole('button',{name:i18n.t('access.waiting.review')})).toHaveFocus();
});

it('shows a refused review as a recoverable read rather than no pending decisions',async()=>{
  reads.read.mockRejectedValueOnce(new Error('Review unavailable'));
  const user=userEvent.setup();
  show(<RequestReview nodeId="topic" />);
  await user.click(screen.getByRole('button',{name:i18n.t('access.waiting.review')}));
  await waitFor(()=>expect(reads.read).toHaveBeenCalledOnce());
  const retry=await screen.findByRole('button',{name:i18n.t('common.tryAgain')});
  expect(screen.queryByText(i18n.t('access.waiting.empty'))).not.toBeInTheDocument();
  await user.click(retry);
  expect(await screen.findByText(i18n.t('access.waiting.empty'))).toBeVisible();
  expect(reads.read).toHaveBeenCalledTimes(2);
});
