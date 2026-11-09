/** Rule reads and inactive causes remain visible without lending private matcher content. */
import { cleanup, screen, waitFor, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { afterEach, expect, it, vi } from 'vitest';
import { ThreadRules } from '@/features/approvals/ThreadRules';
import type { ThreadApprovalRule } from '@/generated/api-types';
import { i18n, setLocale } from '@/i18n';
import { show } from './support';

const THREAD = '00000000-0000-7000-8000-000000000001';
const PRIVATE = 'hidden-rule-source-do-not-display';
function rule(id: string, reason: string | null, active = false): ThreadApprovalRule {
  return { id, active, reason, revision: 2, at: '2026-11-09T06:00:00Z',
    matcher: { type: 'command_exact', command: `git status ${id}`, cwd: '/workspace' },
    by: { id: 'author', kind: 'human', username: 'author', display_name: 'Rule author', avatar: { text: 'R', color: 2 } } };
}
function responses(read: () => Promise<Response>, revoke?: () => void) {
  const sent = vi.fn<typeof fetch>(async (input, init) => {
    const request = input instanceof Request ? input : new Request(input, init);
    const url = new URL(request.url);
    if (request.method === 'GET' && url.pathname === `/api/v1/threads/${THREAD}/approval-rules`) return read();
    if (request.method === 'DELETE' && url.pathname.startsWith(`/api/v1/threads/${THREAD}/approval-rules/`)) {
      revoke?.(); return new Response(null, { status: 204 });
    }
    return Response.json({ type: 'orch:error:not_found', code: 'not_found', title: 'Not found', status: 404, detail: 'Not found', request_id: 'fixture' }, { status: 404 });
  });
  vi.stubGlobal('fetch', sent);
  return sent;
}
const page = (items: ThreadApprovalRule[]) => Response.json({ items, next: null });
afterEach(async () => { cleanup(); vi.unstubAllGlobals(); await setLocale('en'); });

// EN: Thread → Parameters → Allowed for this thread → read failure → Try again.
// 中文：会话 → 参数 → 对此会话允许的操作 → 读取失败 → 重试。
for (const locale of ['en', 'zh-CN'] as const) it(`keeps the entitled rule inspector visible while checking and after a failed read, then retries only the read (${locale})`, async () => {
  await setLocale(locale);
  let finish: ((response: Response) => void) | undefined;
  const first = new Promise<Response>((resolve) => { finish = resolve; });
  const read = vi.fn<() => Promise<Response>>().mockReturnValueOnce(first).mockResolvedValueOnce(page([]));
  const sent = responses(read);
  const view = show(<ThreadRules threadId={THREAD} writable />);
  expect(screen.getByRole('region', { name: i18n.t('approvals.rules.title') })).toBeVisible();
  expect(screen.getByRole('status')).toHaveTextContent(i18n.t('approvals.rules.checking'));
  expect(screen.queryByText(i18n.t('approvals.rules.none'))).not.toBeInTheDocument();
  await waitFor(() => expect(read).toHaveBeenCalledTimes(1));
  if (finish === undefined) throw new Error('The native read was not held');
  finish(Response.json({ type: 'orch:error:source.offline', code: 'source.offline', title: 'Unavailable', status: 503, detail: 'Unavailable', request_id: 'fixture' }, { status: 503 }));
  const failure = await screen.findByRole('alert');
  expect(failure).toHaveTextContent(i18n.t('approvals.rules.unavailable'));
  expect(screen.queryByText(i18n.t('approvals.rules.none'))).not.toBeInTheDocument();
  await userEvent.setup().click(within(failure).getByRole('button', { name: i18n.t('common.tryAgain') }));
  expect(await screen.findByText(i18n.t('approvals.rules.none'))).toBeVisible();
  expect(read).toHaveBeenCalledTimes(2);
  expect(sent.mock.calls.every(([input, init]) => (input instanceof Request ? input : new Request(input, init)).method === 'GET')).toBe(true);
  view.unmount();
});

// EN: Thread → Parameters → retained rule → native inactive cause; Take back refreshes that same native read.
// 中文：会话 → 参数 → 已保存规则 → 原生失效原因；收回后重新读取同一规则。
for (const locale of ['en', 'zh-CN'] as const) it(`names taken-back and changed-setup rules while keeping unknown native details private (${locale})`, async () => {
  await setLocale(locale);
  let items = [rule('revoked', 'revoked'), rule('changed', 'binding_changed'), rule('unknown', PRIVATE), rule('active', null, true)];
  const read = vi.fn<() => Promise<Response>>(() => Promise.resolve(page(items)));
  responses(read, () => { items = items.map((row) => row.active ? { ...row, active: false, reason: 'revoked' } : row); });
  show(<ThreadRules threadId={THREAD} writable />);
  const section = await screen.findByRole('region', { name: i18n.t('approvals.rules.title') });
  expect(await within(section).findByText(i18n.t('approvals.rules.reasonRevoked'))).toBeVisible();
  expect(within(section).getByText(i18n.t('approvals.rules.reasonBindingChanged'))).toBeVisible();
  expect(within(section).getByText(i18n.t('approvals.rules.reasonUnavailable'))).toBeVisible();
  expect(section).not.toHaveTextContent(PRIVATE);
  await userEvent.setup().click(within(section).getByRole('button', { name: i18n.t('approvals.rules.takeBack') }));
  await waitFor(() => expect(within(section).getAllByText(i18n.t('approvals.rules.reasonRevoked'))).toHaveLength(2));
  expect(read).toHaveBeenCalledTimes(2);
  expect(within(section).queryByRole('button', { name: i18n.t('approvals.rules.takeBack') })).not.toBeInTheDocument();
});

for (const locale of ['en', 'zh-CN'] as const) it(`never reads rules for a non-manager or leaves cached matcher content after management is lost (${locale})`, async () => {
  await setLocale(locale);
  const read = vi.fn<() => Promise<Response>>(() => Promise.resolve(page([rule(PRIVATE, 'revoked')])));
  responses(read);
  const view = show(<ThreadRules threadId={THREAD} writable={false} />);
  expect(screen.queryByRole('region', { name: i18n.t('approvals.rules.title') })).not.toBeInTheDocument();
  expect(read).not.toHaveBeenCalled();
  view.rerender(<ThreadRules threadId={THREAD} writable />);
  expect(await screen.findByText(i18n.t('approvals.matcher.exact', { command: `git status ${PRIVATE}`, cwd: '/workspace' }))).toBeVisible();
  view.rerender(<ThreadRules threadId={THREAD} writable={false} />);
  expect(screen.queryByRole('region', { name: i18n.t('approvals.rules.title') })).not.toBeInTheDocument();
  expect(document.body).not.toHaveTextContent(PRIVATE);
});
