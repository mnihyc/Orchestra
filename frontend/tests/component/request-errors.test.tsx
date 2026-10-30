/** Served access requests preserve their actual refusal, draft and retry across all three entry points. */
import { cleanup, screen, waitFor } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { afterEach, beforeEach, expect, it, vi } from 'vitest';
import type * as AccessApi from '@/api/access';
import { ApiError } from '@/api/client';
import { AccessRequest } from '@/features/access/AccessRequest';
import { RequestExport } from '@/features/export/RequestExport';
import { RequestDefault } from '@/features/start/RequestDefault';
import type { MissingAccess } from '@/generated/api-types';
import { i18n, setLocale } from '@/i18n';
import { useAppearance } from '@/lib/appearance';
import { ADA, CATALOGUE, TOPIC } from '../unit/general-access-fixtures';
import { show } from './support';

const calls = vi.hoisted(() => ({ request: vi.fn<typeof AccessApi.requestAccess>() }));
vi.mock('@/api/access', async (load) => ({ ...await load<typeof AccessApi>(),
  requestAccess: (...args: Parameters<typeof AccessApi.requestAccess>) => calls.request(...args),
  permissionsQuery: { queryKey: ['request-errors-catalogue'], queryFn: () => Promise.resolve(CATALOGUE.reply) },
  accessRequestQuery: (batch: string) => ({ queryKey: ['request-errors-state', batch], queryFn: () => Promise.resolve({ batch_id: batch, approvals: [], complete: true }) }),
}));

const surfaces = [
  { kind: 'access', reason: 'access.request.reason', send: 'access.request.send', permission: 'thread.view' },
  { kind: 'export', reason: 'export.request.reason', send: 'export.request.send', permission: 'thread.export' },
  { kind: 'defaults', reason: 'start.request.reason', send: 'start.request.send', permission: 'topic.manage' },
] as const;
const appearance = useAppearance.getState();
beforeEach(() => { sessionStorage.clear(); calls.request.mockReset().mockResolvedValue({ batch_id: 'request-errors-batch', approvals: [] }); });
afterEach(async () => { cleanup(); useAppearance.setState(appearance); await setLocale('en'); });

for (const locale of ['en', 'zh-CN'] as const) for (const surface of surfaces) {
  for (const status of [404, 500] as const) for (const hasOwner of [false, true]) {
    it(`${surface.kind} shows its ${status} error with owner=${hasOwner}, retaining the reason and retry in ${locale}`, async () => {
      await setLocale(locale); useAppearance.setState({ locale });
      const code = status === 404 ? 'not_found' : 'internal';
      const error = new ApiError({ type: `orch:error:${code}`, code, status,
        title: status === 404 ? 'Not found' : 'Request failed', request_id: 'request-errors',
        detail: status === 404 ? 'The requested object was removed.' : 'The request could not be completed.',
      }, null);
      calls.request.mockRejectedValueOnce(error);
      const missing: MissingAccess = { object: { kind: 'thread', id: 'thread-id', name: 'Release', owner: hasOwner ? ADA : null },
        permissions: ['thread.view'], ask: hasOwner ? [ADA] : [], ask_more: 0, options: null, closed: null, override: false };
      show(surface.kind === 'access' ? <AccessRequest missing={[missing]} objectId="thread-id" fixed />
        : surface.kind === 'export' ? <RequestExport target={{ kind: 'thread', id: 'thread-id', name: 'Release', ...(hasOwner ? { owner: ADA } : {}) }} open onClose={vi.fn()} />
          : <RequestDefault place={{ id: TOPIC.id, kind: 'topic', name: 'Release', parentId: 'project', archived: false, status: 'open', folder: false }} owner={hasOwner ? ADA : null} open onClose={vi.fn()} />);
      const user = userEvent.setup();
      const reason = screen.getByRole('textbox', { name: i18n.t(surface.reason) });
      await user.type(reason, 'Please review access.');
      const send = screen.getByRole('button', { name: i18n.t(surface.send) });
      await user.click(send);
      const alert = await screen.findByRole('alert');
      expect(alert).toHaveTextContent(error.message);
      expect(alert).not.toHaveTextContent(/not available yet|系统暂不接受申请|暂时还不能/u);
      expect(reason).toHaveValue('Please review access.');
      await waitFor(() => expect(send).toBeEnabled());
      const expected = { items: [{ object: surface.kind === 'defaults' ? { kind: 'topic', id: TOPIC.id } : { kind: 'thread', id: 'thread-id' }, permissions: [surface.permission] }], reason: 'Please review access.' };
      expect(calls.request).toHaveBeenCalledOnce();
      expect(calls.request.mock.calls[0]?.[0]).toEqual(expected);
      await user.click(send);
      await waitFor(() => expect(calls.request).toHaveBeenCalledTimes(2));
      expect(calls.request.mock.calls[1]?.[0]).toEqual(expected);
      await waitFor(() => expect(screen.queryByRole('alert')).not.toBeInTheDocument());
      const receipt = surface.kind === 'access' ? i18n.t('access.request.complete')
        : surface.kind === 'export' ? hasOwner ? i18n.t('export.request.sent', { name: ADA.display_name }) : i18n.t('export.request.sentNoOwner')
          : hasOwner ? i18n.t('start.request.sent', { name: ADA.display_name }) : i18n.t('start.request.sentNoOwner');
      expect(await screen.findByText(receipt)).toBeVisible();
    });
  }
}

it.each([['en', true], ['en', false], ['zh-CN', true], ['zh-CN', false]] as const)('keeps the actual pending-request limit beside the retained reason in %s, count supplied=%s', async (locale, withCount) => {
  await setLocale(locale); useAppearance.setState({ locale });
  const error = new ApiError({ type: 'orch:error:access.too_many_requests', code: 'access.too_many_requests',
    status: 429, title: 'Too many requests', detail: 'The pending request limit is reached.', request_id: 'request-limit', ...(withCount ? { waiting: 10 } : {}) }, null);
  calls.request.mockRejectedValueOnce(error);
  const missing: MissingAccess = { object: { kind: 'thread', id: 'thread-id', name: 'Release', owner: ADA },
    permissions: ['thread.view'], ask: [ADA], ask_more: 0, options: null, closed: null, override: false };
  show(<AccessRequest missing={[missing]} objectId="thread-id" fixed />);
  const user = userEvent.setup();
  const reason = screen.getByRole('textbox', { name: i18n.t('access.request.reason') });
  await user.type(reason, 'Keep these words.');
  await user.click(screen.getByRole('button', { name: i18n.t('access.request.send') }));
  const alert = await screen.findByRole('alert');
  expect(alert).toHaveTextContent(withCount ? i18n.t('access.request.tooMany', { waiting: 10 }) : error.message);
  if (!withCount) expect(alert).not.toHaveTextContent(i18n.t('access.request.tooMany', { waiting: 0 }));
  expect(reason).toHaveValue('Keep these words.');
  expect(calls.request).toHaveBeenCalledOnce();
});
