/** The shared row keeps an editable value compact, explanations asked for, and unfinished editors safe. */
import { useEffect, useState } from 'react';
import { fireEvent, screen, waitFor } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { afterEach, expect, it, vi } from 'vitest';
import { i18n, setLocale } from '@/i18n';
import { SettingRow } from '@/ui/SettingRow';
import { SettingEditor } from '@/ui/SettingEditor';
import { Input } from '@/ui/Input';
import { Switch } from '@/ui/Switch';
import { HelpHint } from '@/ui/HelpHint';
import { show } from './support';

vi.mock('@/features/help/open', () => ({ useHelpOpener: () => vi.fn() }));
afterEach(async () => { await setLocale('en'); });

it.each(['en', 'zh-CN'] as const)('shows one description line and reserves its question for supplementary information in %s', async (locale) => {
  await setLocale(locale);
  const user = userEvent.setup();
  const name = locale === 'en' ? 'Keep awake' : '保持唤醒';
  const description = locale === 'en' ? 'Keep this runtime available.' : '保持此运行环境可用。';
  const extra = locale === 'en' ? 'Applies at the next start.' : '下次启动时生效。';
  const onChange = vi.fn();
  show(<ul><SettingRow name={name} description={description} help={<HelpHint term={name} what={extra} />}
    value={<Switch label={<span className="sr-only">{name}</span>} checked onCheckedChange={onChange} />} /></ul>);
  expect(screen.getAllByText(description)).toHaveLength(1);
  expect(screen.queryByText(extra)).not.toBeInTheDocument();
  expect(screen.getAllByText(name).filter((element) => element.closest('.sr-only') === null)).toHaveLength(1);
  expect(screen.getAllByRole('switch')).toHaveLength(1);
  await user.click(screen.getByRole('switch', { name }));
  expect(onChange).toHaveBeenCalledExactlyOnceWith(false);
  await user.click(screen.getByRole('button', { name: i18n.t('help.hint.label', { term: name }) }));
  expect(screen.getByText(extra)).toBeVisible();
  expect(screen.getAllByText(description)).toHaveLength(1);
  await user.keyboard('{Escape}');
  expect(screen.queryByText(extra)).not.toBeInTheDocument();
  expect(screen.getByText(description)).toBeVisible();
});

it('shows a reader the effective value and the actual source link rather than a disabled editing form', () => {
  show(<ul><SettingRow name="Memory" value="8 GiB" readOnly
    reason={<a href="/resources/machines/world-box?section=settings">Set on world-box</a>} /></ul>);
  expect(screen.getByText('8 GiB')).toBeVisible();
  expect(screen.getByRole('link', { name: 'Set on world-box' })).toHaveAttribute('href', '/resources/machines/world-box?section=settings');
  expect(screen.queryByRole('textbox')).not.toBeInTheDocument();
  expect(screen.queryByRole('switch')).not.toBeInTheDocument();
});

it('opens lazily, preserves unfinished input on Escape and pauses a closed editor’s effects', async () => {
  const start = vi.fn();
  const stop = vi.fn();
  function Draft() {
    const [value, setValue] = useState('');
    useEffect(() => { start(); return () => { stop(); }; }, []);
    return <Input label="Configuration" value={value} onChange={(event) => setValue(event.target.value)} />;
  }
  const user = userEvent.setup();
  show(<ul><SettingEditor name="Configuration" summary="Empty" label="Edit Configuration"><Draft /></SettingEditor></ul>);
  expect(start).not.toHaveBeenCalled();
  expect(screen.queryByRole('textbox')).not.toBeInTheDocument();
  const trigger = screen.getByRole('button', { name: 'Edit Configuration' });
  await user.click(trigger);
  await waitFor(() => expect(start).toHaveBeenCalledTimes(1));
  const field = screen.getByRole('textbox', { name: 'Configuration' });
  fireEvent.change(field, { target: { value: '{"still writing":' } });
  field.focus();
  await user.keyboard('{Escape}');
  expect(trigger).toHaveFocus();
  expect(trigger).toHaveAttribute('aria-expanded', 'false');
  expect(screen.queryByRole('textbox')).not.toBeInTheDocument();
  await waitFor(() => expect(stop).toHaveBeenCalledTimes(1));
  await user.click(trigger);
  expect(screen.getByRole('textbox', { name: 'Configuration' })).toHaveValue('{"still writing":');
  expect(trigger).toHaveAttribute('aria-expanded', 'true');
});

it('begins an inline edit in its value, and repeated Escape restores the summary and pencil focus', async () => {
  function Value() {
    const [value, setValue] = useState('safe: On');
    return <Input label="Configuration" value={value} onChange={(event) => setValue(event.target.value)} />;
  }
  const user = userEvent.setup();
  show(<ul><SettingEditor name="Configuration" placement="inline" summary="safe: On" label="Edit Configuration"><Value /></SettingEditor></ul>);
  const pencil = screen.getByRole('button', { name: 'Edit Configuration' });
  for (const opening of ['first', 'again']) {
    expect(pencil, opening).toHaveAccessibleDescription('safe: On');
    await user.click(pencil);
    expect(screen.getByRole('textbox', { name: 'Configuration' }), opening).toHaveFocus();
    expect(pencil, opening).toHaveAttribute('aria-expanded', 'true');
    await user.keyboard('{Escape}');
    expect(screen.queryByRole('textbox', { name: 'Configuration' }), opening).not.toBeInTheDocument();
    expect(pencil, opening).toHaveFocus();
    expect(pencil, opening).toHaveAttribute('aria-expanded', 'false');
    expect(pencil, opening).toHaveAccessibleDescription('safe: On');
  }
});
