/** Hidden path steps expand, and a pre-shortened select keeps its full original in both languages/themes. */
import { screen } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import type { ReactNode } from 'react';
import { afterEach, expect, it, vi } from 'vitest';
import { PlaceChain } from '@/features/navigation/PlaceChain';
import type { NodeRef } from '@/generated/api-types';
import { i18n, setLocale } from '@/i18n';
import { Select } from '@/ui/Select';
import { show } from './support';

vi.mock('@/features/navigation/places', () => ({
  PlaceLink: ({ place, children, className }: { place: NodeRef; children: ReactNode; className?: string }) => <a href={`/project/${place.id}`} className={className}>{children}</a>,
  usePlaceLookup: () => ({ pathTo: () => [] }),
}));
const priorTheme = document.documentElement.dataset.theme;
afterEach(async () => {
  await setLocale('en');
  if (priorTheme === undefined) delete document.documentElement.dataset.theme;
  else document.documentElement.dataset.theme = priorTheme;
});

for (const theme of ['light', 'dark'] as const) for (const locale of ['en', 'zh-CN'] as const) {
  it(`opens every permitted hidden path step with Enter in ${locale} and ${theme}`, async () => {
    await setLocale(locale); document.documentElement.dataset.theme = theme;
    const user = userEvent.setup();
    const places: NodeRef[] = ['Team', 'Checkout', 'Payments', 'Current'].map((name) => ({ id: name, kind: 'project', name, redacted: false }));
    places.splice(2, 0, { id: 'private', kind: 'project', name: 'Never disclose this', redacted: true });
    show(<PlaceChain places={places} />);
    const expand = screen.getByRole('button', { name: i18n.t('ui.breadcrumbs.folded') });
    expect(expand).toHaveTextContent('…');
    expect(expand).toHaveAttribute('aria-expanded', 'false');
    expect(expand.getAttribute('data-overflow-text')).not.toContain('Never disclose this');
    expect(screen.queryByRole('link', { name: 'Checkout' })).not.toBeInTheDocument();
    expand.focus(); await user.keyboard('{Enter}');
    expect(screen.getByRole('link', { name: 'Checkout' })).toHaveAttribute('href', '/project/Checkout');
    expect(screen.getByRole('link', { name: 'Payments' })).toHaveAttribute('href', '/project/Payments');
    expect(screen.getByText(i18n.t('folders.private'))).toBeVisible();
    expect(screen.queryByText('Never disclose this')).not.toBeInTheDocument();
    expect(screen.queryByRole('button', { name: i18n.t('ui.breadcrumbs.folded') })).not.toBeInTheDocument();
  });
  it(`reads a select's pre-shortened value on keyboard focus in ${locale} and ${theme}`, async () => {
    await setLocale(locale); document.documentElement.dataset.theme = theme;
    const user = userEvent.setup();
    const original = locale === 'en' ? 'The whole message used to name this turn, including its last words' : '用于命名这一轮的完整消息，包括最后的文字';
    const { container } = show(<Select label={i18n.t('change.history.upTo')} value="one" onValueChange={() => undefined}
      options={[{ value: 'one', label: 'Short…', fullLabel: original }]} />);
    const value = container.querySelector('[data-overflow-text]');
    if (!(value instanceof HTMLElement)) throw new Error('The selected original is not recorded.');
    Object.defineProperties(value, {
      clientWidth: { value: 100 }, scrollWidth: { value: 100 }, clientHeight: { value: 20 }, scrollHeight: { value: 20 },
    });
    await user.tab();
    expect(screen.getByRole('combobox')).toHaveFocus();
    expect(screen.getByRole('tooltip')).toHaveTextContent(original);
    expect(screen.queryByRole('listbox')).not.toBeInTheDocument();
  });
}
