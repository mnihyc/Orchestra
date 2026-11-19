/** One mechanism reads whole names in every theme/language; a literal folded value opens in place. */
import { act, fireEvent, screen } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { afterEach, expect, it, vi } from 'vitest';
import { DescriptionLine } from '@/features/thread/Description';
import { i18n, setLocale } from '@/i18n';
import { JsonTree, STRING_FOLDED_AT } from '@/ui/JsonTree';
import { OVERFLOW_REST_MS } from '@/ui/overflowTooltips';
import { show } from './support';

const priorTheme = document.documentElement.dataset.theme;
afterEach(async () => {
  vi.useRealTimers();
  await setLocale('en');
  if (priorTheme === undefined) delete document.documentElement.dataset.theme;
  else document.documentElement.dataset.theme = priorTheme;
});

for (const theme of ['light', 'dark'] as const) for (const locale of ['en', 'zh-CN'] as const) {
  it(`keeps an uncut description quiet and expands it on request in ${locale} and ${theme}`, async () => {
    await setLocale(locale); document.documentElement.dataset.theme = theme;
    vi.useFakeTimers();
    const toggle = vi.fn();
    const full = locale === 'en' ? 'A complete short description' : '完整的简短说明';
    const { container } = show(<DescriptionLine thread={{ description: full, description_source: 'user', auto_updated_at: null }} open={false} onToggle={toggle} mayEdit />);
    const value = container.querySelector('[data-overflow-text]');
    if (!(value instanceof HTMLElement)) throw new Error('The description original is missing.');
    // Nonzero equal dimensions model an actual uncut label, rather than an unrendered jsdom box.
    Object.defineProperties(value, { clientWidth: { value: 300 }, scrollWidth: { value: 300 }, clientHeight: { value: 20 }, scrollHeight: { value: 20 } });
    value.style.cssText = 'text-overflow:ellipsis;overflow:hidden;white-space:nowrap';
    const button = screen.getByRole('button', { name: i18n.t('thread.description.expand') });
    expect(button).toHaveTextContent(full);
    // RTL's async drain schedules its own zero-delay timeout after user-event's promise,
    // outside that event's advanceTimers callback in Vitest. Drive the timed hover synchronously.
    fireEvent.pointerOver(button, { pointerType: 'mouse' });
    act(() => { vi.advanceTimersByTime(OVERFLOW_REST_MS + 1); });
    expect(screen.queryByRole('tooltip')).not.toBeInTheDocument();
    vi.useRealTimers();
    const user = userEvent.setup();
    await user.click(button);
    expect(toggle).toHaveBeenCalledOnce();
    expect(button).toHaveFocus();
    expect(screen.queryByRole('tooltip')).not.toBeInTheDocument();
  });
  it(`reveals every line of a description when its summary receives keyboard focus in ${locale} and ${theme}`, async () => {
    await setLocale(locale); document.documentElement.dataset.theme = theme;
    const full = locale === 'en' ? 'First line\nThe rest of the complete description' : '第一行\n完整说明的其余内容';
    const { container } = show(<DescriptionLine thread={{ description: full, description_source: 'user', auto_updated_at: null }} open={false} onToggle={() => undefined} mayEdit />);
    const value = container.querySelector('[data-overflow-text]');
    if (!(value instanceof HTMLElement)) throw new Error('The description original is missing.');
    Object.defineProperties(value, { clientWidth: { value: 100 }, scrollWidth: { value: 100 }, clientHeight: { value: 20 }, scrollHeight: { value: 20 } });
    act(() => { screen.getByRole('button', { name: i18n.t('thread.description.expand') }).focus(); });
    expect(screen.getByRole('tooltip')).toHaveTextContent(full.replace('\n', ' '));
  });
  it(`reads the complete clipped name by keyboard in ${locale} and ${theme}`, async () => {
    await setLocale(locale);
    document.documentElement.dataset.theme = theme;
    const user = userEvent.setup();
    const full = locale === 'en' ? 'The complete environment name including the final words' : '环境的完整名称，包括末尾的文字';
    show(<div data-theme={theme}><span style={{ textOverflow: 'ellipsis', overflow: 'hidden', whiteSpace: 'nowrap' }} ref={(element) => {
      if (element !== null) Object.defineProperties(element, {
        clientWidth: { configurable: true, value: 100 }, scrollWidth: { configurable: true, value: 300 },
        clientHeight: { configurable: true, value: 20 }, scrollHeight: { configurable: true, value: 20 },
      });
    }}>{full}</span></div>);
    const value = screen.getByText(full);
    await user.tab();
    expect(value).toHaveFocus();
    const tooltip = screen.getByRole('tooltip');
    expect(tooltip).toHaveTextContent(full);
    expect(tooltip).toHaveClass('bg-text', 'text-bg');
    await user.keyboard('{Escape}');
    expect(screen.queryByRole('tooltip')).not.toBeInTheDocument();
    expect(value).toHaveFocus();
  });
  it(`makes the ellipsis itself open every character in ${locale} and ${theme}`, async () => {
    await setLocale(locale);
    document.documentElement.dataset.theme = theme;
    const user = userEvent.setup();
    const full = (locale === 'en' ? 'A complete value. ' : '完整的值。').repeat(STRING_FOLDED_AT);
    show(<div data-theme={theme}><JsonTree label="Configuration" value={full} /></div>);
    const expand = screen.getByRole('button', { name: i18n.t('ui.json.showAll', { count: full.length }) });
    expect(expand).toHaveTextContent('…');
    expect(expand).toHaveAttribute('data-overflow-text', full);
    await user.click(expand);
    expect(screen.queryByRole('button', { name: i18n.t('ui.json.showAll', { count: full.length }) })).not.toBeInTheDocument();
    expect(screen.getByText(`“${full}”`)).toBeVisible();
  });
}
