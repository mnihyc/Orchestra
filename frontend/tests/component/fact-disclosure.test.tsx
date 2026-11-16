/** One subject's facts use the same opening and inline body, without starting work. */
import { cleanup, screen, waitFor } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { afterEach, expect, it, vi } from 'vitest';
import { FactDisclosure, FactDisclosureBody, type FactGroup } from '@/ui/FactDisclosure';
import { i18n, setLocale } from '@/i18n';
import { show } from './support';

afterEach(async () => { cleanup(); await setLocale('en'); });
const facts: FactGroup[] = [{ key: 'last', kind: 'last_result', facts: [{ key: 'producer', label: 'Producer', value: 'Previous model' }] }];

for (const locale of ['en', 'zh-CN'] as const) {
  it(`keeps routine facts quiet and opens them from the keyboard with focus returning on Escape (${locale})`, async () => {
    await setLocale(locale);
    const read = vi.fn();
    const page = show(<FactDisclosure groups={facts} onOpenChange={(open) => { if (open) read(); }} />);
    const button = screen.getByRole('button', { name: locale === 'en' ? 'Details' : '详情' });
    expect(screen.queryByText('Previous model')).not.toBeInTheDocument();
    expect(read).not.toHaveBeenCalled();
    button.focus();
    await userEvent.keyboard('{Enter}');
    const dialog = await screen.findByRole('dialog', { name: i18n.t('common.details') });
    expect(dialog).toHaveTextContent(locale === 'en' ? 'Last result' : '上次结果');
    expect(dialog).toHaveTextContent('Previous model');
    expect(read).toHaveBeenCalledTimes(1);
    expect(dialog.querySelector('[data-fact-disclosure-body]')).toHaveFocus();
    await userEvent.keyboard('{Escape}');
    await waitFor(() => expect(button).toHaveFocus());
    expect(page.container.querySelector('[data-fact-disclosure-fragment]')).toBeNull();
    expect(read).toHaveBeenCalledTimes(1);
  });

  it(`keeps missing facts distinct from measured zero and absent groups (${locale})`, async () => {
    await setLocale(locale);
    show(<FactDisclosure fragment="Result discarded" groups={[null, { key: 'empty', kind: 'next_refresh', facts: [] },
      { key: 'outcome', kind: 'recorded_outcome', facts: [{ key: 'unknown', label: 'Recorded usage', value: null }, { key: 'zero', label: 'Omitted turns', value: '0' }] }]} />);
    expect(screen.getByText('Result discarded')).toBeVisible();
    await userEvent.click(screen.getByRole('button', { name: i18n.t('common.details') }));
    expect(await screen.findByText(locale === 'en' ? 'Not reported' : '尚未报告')).toBeVisible();
    expect(screen.getByText('0', { exact: true })).toBeVisible();
    expect(screen.queryByText(locale === 'en' ? 'Next refresh' : '下次刷新')).not.toBeInTheDocument();
    expect(screen.getAllByRole('button', { name: i18n.t('common.details') })).toHaveLength(1);
  });

  it(`keeps a long value and its trigger reachable in the narrow view (${locale})`, async () => {
    await setLocale(locale);
    const before = window.innerWidth;
    Object.defineProperty(window, 'innerWidth', { configurable: true, value: 400 });
    try {
      show(<FactDisclosure fragment="Shortened" groups={[{ key: 'source', kind: 'current_setting', facts: [
        { key: 'source', label: 'Source', value: 'A very long readable source name '.repeat(8), source: <a href="/admin/settings?setting=summary.targets">System default</a> },
      ] }]} />);
      const trigger = screen.getByRole('button', { name: i18n.t('common.details') });
      expect(trigger).toBeEnabled();
      trigger.focus(); await userEvent.keyboard(' ');
      const body = await screen.findByRole('dialog', { name: i18n.t('common.details') });
      expect(body).toHaveTextContent('A very long readable source name');
      expect(screen.getByRole('link', { name: 'System default' })).toHaveAttribute('href', '/admin/settings?setting=summary.targets');
      expect(body.querySelector('dd')).toHaveClass('wrap-anywhere');
      await userEvent.keyboard('{Escape}');
      await waitFor(() => expect(trigger).toHaveFocus());
    } finally { Object.defineProperty(window, 'innerWidth', { configurable: true, value: before }); }
  });
}

it('uses the same body in an already open owner without a second popup or request', () => {
  const page = show(<FactDisclosureBody groups={facts} />);
  expect(screen.getByText('Previous model')).toBeVisible();
  expect(screen.queryByRole('button')).not.toBeInTheDocument();
  expect(page.container.querySelector('[data-fact-disclosure-body]')).not.toBeNull();
});

it('does not open a nested identity card just because the fact body opens', async () => {
  const readCard = vi.fn();
  show(<FactDisclosure groups={[{ key: 'result', facts: [{ key: 'model', label: 'Model', value: <button onFocus={readCard}>Readable model</button> }] }]} />);
  await userEvent.click(screen.getByRole('button', { name: i18n.t('common.details') }));
  expect(await screen.findByRole('button', { name: 'Readable model' })).toBeVisible();
  expect(readCard).not.toHaveBeenCalled();
  await userEvent.tab();
  expect(screen.getByRole('button', { name: 'Readable model' })).toHaveFocus();
  expect(readCard).toHaveBeenCalledTimes(1);
});

it('has no empty opening when no authorized fact group is supplied', () => {
  show(<FactDisclosure groups={[null, undefined]} />);
  expect(screen.queryByRole('button')).not.toBeInTheDocument();
});
