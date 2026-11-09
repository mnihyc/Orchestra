/** EN/ZH: helper references keep the current ordinary access result and the actor's private default. */
import { cleanup, screen } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { afterEach, beforeEach, expect, it, vi } from 'vitest';
import type * as References from '@/api/references';
import type { TypedReference } from '@/features/people/references';
import { CapabilityHelper } from '@/features/timeline/tools/CapabilityHelper';
import { CapabilityFailure } from '@/features/timeline/tools/CapabilityFailure';
import type { ReferencePreview } from '@/generated/api-types';
import { i18n, setLocale } from '@/i18n';
import { ADA } from '../doubles/thread-scenes';
import { ACME } from '../doubles/fixtures';
import { show } from './support';

const fixture = vi.hoisted(() => ({ preview: null as ReferencePreview | null }));
vi.mock('@/api/references', async (load) => ({
  ...await load<typeof References>(),
  referencePreviewQuery: (reference: TypedReference) => ({
    queryKey: ['capability-helper-reference', reference.id],
    queryFn: () => {
      if (fixture.preview === null) throw new Error('The fixture must declare its current preview');
      return Promise.resolve(fixture.preview);
    },
    staleTime: Infinity,
  }),
}));

beforeEach(() => {
  fixture.preview = { kind: 'node', id: ACME.depsThread, node_kind: 'thread', title: 'Readable helper', description: 'Current helper description',
    place: null, updated_at: null, updated_by: null, part: null, file: null };
});
afterEach(async () => { cleanup(); await setLocale('en'); });
function result() {
  return { helper: { thread_id: ACME.depsThread, actor_id: ADA.id, actor: ADA,
    actor_name: 'This legacy text must not replace the person chip',
    program_label: { en: 'Codex on ChatGPT', 'zh-CN': 'Codex（ChatGPT 登录）' }, requester_thread_id: ACME.mainThread } };
}

it.each(['en', 'zh-CN'] as const)('opens the ordinary current helper card and names its real owner in %s', async (locale) => {
  await setLocale(locale);
  const data = result();
  show(<CapabilityHelper result={{ structuredContent: data }} />);
  const open = await screen.findByRole('link', { name: 'Readable helper' });
  expect(open).toHaveAttribute('href', `/thread/${ACME.depsThread}`);
  expect(screen.getByText(data.helper.program_label[locale])).toBeVisible();
  expect(screen.getByRole('button', { name: ADA.display_name })).toBeVisible();
  expect(screen.queryByText(data.helper.actor_name)).not.toBeInTheDocument();
  await userEvent.setup().click(open);
  expect(await screen.findByText('Current helper description')).toBeVisible();
  expect(screen.getByRole('button', { name: i18n.t('people.reference.open') })).toBeEnabled();
});

it.each(['en', 'zh-CN'] as const)('keeps a private helper label without revealing an unreadable title in %s', async (locale) => {
  await setLocale(locale);
  if (fixture.preview === null) throw new Error('Missing reference fixture');
  fixture.preview = { ...fixture.preview, title: '', description: '', unavailable: { deleted: false,
    object: { kind: 'thread', id: ACME.depsThread }, permission: 'thread.view', requestable: false, pending: null } };
  const data = result();
  show(<CapabilityHelper result={{ content: [{ type: 'text', text: JSON.stringify(data) }] }} />);
  const locked = await screen.findByRole('button', { name: i18n.t('timeline.capabilityHelper') });
  expect(locked).toHaveAttribute('data-reference-state', 'no-access');
  expect(screen.getByText(data.helper.program_label[locale])).toBeVisible();
  expect(screen.queryByRole('link', { name: 'Readable helper' })).not.toBeInTheDocument();
  await userEvent.setup().click(locked);
  expect(await screen.findByText(i18n.t('people.reference.noAccess'))).toBeVisible();
  expect(screen.queryByText('Current helper description')).not.toBeInTheDocument();
});

it.each(['en', 'zh-CN'] as const)('keeps a deleted helper factual without stale target metadata in %s', async (locale) => {
  await setLocale(locale);
  if (fixture.preview === null) throw new Error('Missing reference fixture');
  fixture.preview = { ...fixture.preview, title: '', description: '', unavailable: { deleted: true,
    object: { kind: 'thread', id: ACME.depsThread }, permission: 'thread.view', requestable: false, pending: null } };
  show(<CapabilityHelper result={JSON.stringify(result())} />);
  const deleted = await screen.findByRole('button', { name: i18n.t('people.reference.deleted') });
  expect(deleted).toHaveAttribute('data-reference-state', 'deleted');
  await userEvent.setup().click(deleted);
  expect(await screen.findByRole('button', { name: i18n.t('people.reference.open') })).toBeDisabled();
  expect(screen.queryByText('Current helper description')).not.toBeInTheDocument();
});

it('does not invent a helper or a person from malformed or unrelated tool output', () => {
  for (const value of [null, {}, '{broken', { helper: { thread_id: '' } }, { images: [] }]) {
    const view = show(<CapabilityHelper result={value} />);
    expect(view.container.querySelector('[data-capability-helper]')).toBeNull();
    view.unmount();
  }
});

it.each(['en', 'zh-CN'] as const)('shows an explicit native failure with its real allowance and reset in %s', async (locale) => {
  await setLocale(locale);
  const reset = '2050-10-15T13:00:00Z';
  const data = { ...result(), state: 'failed', isError: true, failure: { capability: 'image_generation', code: 'rate_limited', message: 'Reported native image allowance reached',
    window: 'image allowance', resets_at: reset, retryable: true } };
  const view = show(<><CapabilityFailure result={{ structuredContent: data }} /><CapabilityHelper result={{ structuredContent: data }} /></>);
  const alert = screen.getByRole('alert');
  expect(alert).toHaveTextContent(i18n.t('capabilityBackend.failed'));
  expect(alert).toHaveTextContent(data.failure.message);
  expect(alert).toHaveTextContent(i18n.t('capabilityBackend.allowance', { window: data.failure.window }));
  expect(view.container.querySelector('time')).toHaveAttribute('datetime', reset);
  expect(await screen.findByRole('link', { name: 'Readable helper' })).toHaveAttribute('href', `/thread/${ACME.depsThread}`);
});

it.each(['en', 'zh-CN'] as const)('uses the program-owned refusal words without making a successful image in %s', async (locale) => {
  await setLocale(locale);
  const text = { en: 'Image generation is not offered on Free', 'zh-CN': 'Free 套餐不提供图像生成' };
  show(<CapabilityFailure result={{ state: 'failed', isError: true, failure: { code: 'unavailable', message: text.en, text, reason: 'fixture-program-fact' } }} />);
  expect(screen.getByRole('alert')).toHaveTextContent(text[locale]);
  expect(screen.queryByRole('img')).not.toBeInTheDocument();
  expect(document.querySelector('time')).toBeNull();
});

it('does not turn incomplete or unrelated tool data into a reported failure', () => {
  for (const result of [{ state: 'ready', isError: true, failure: { code: 'unavailable', message: 'No' } }, { state: 'failed', failure: { code: 'unavailable', message: 'No' } }, { state: 'failed', isError: true, failure: { code: '', message: 'No' } }]) {
    const view = show(<CapabilityFailure result={result} />);
    expect(view.queryByRole('alert')).not.toBeInTheDocument();
    view.unmount();
  }
});
