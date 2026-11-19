/** Real shared markup and choice stylesheet: the browser companion measures the resulting boxes. */
import { cleanup, screen, waitFor } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { useState } from 'react';
import { readFileSync } from 'node:fs';
import { resolve } from 'node:path';
import { afterEach, expect, it } from 'vitest';
import { i18n, setLocale } from '@/i18n';
import { choiceCopy } from '@/i18n/choices';
import { Choice } from '@/ui/Choice';
import { Menu } from '@/ui/Menu';
import { SegmentedControl } from '@/ui/SegmentedControl';
import { show } from './support';

const styles: HTMLStyleElement[] = [];
// Vitest's css-disable transform replaces CSS imports, including ?raw, with an empty module.
// Read the real authored stylesheet so these cascade assertions cannot accidentally test no CSS.
// The runner's package root is frontend; Vite rewrites import.meta.url to an HTTP URL in jsdom.
const choiceStyles = readFileSync(resolve('src/styles/choices.css'), 'utf8');
function stylesheet(dark: boolean) {
  // Resolve variables for jsdom's stylesheet engine, not a second set of component selectors.
  // The token contrast suite and real-browser layout auditor separately measure both shipped themes.
  const colours: Record<string, string> = { surface: dark ? '#111111' : '#ffffff', 'control-border': dark ? '#bbbbbb' : '#444444',
    'control-selected': dark ? '#eeeeee' : '#111111', 'control-on-selected': dark ? '#111111' : '#ffffff',
    'bg-muted': dark ? '#333333' : '#dddddd',
    'surface-raised': dark ? '#222222' : '#ffffff', 'segment-selected': dark ? '#444444' : '#ffffff', text: dark ? '#eeeeee' : '#111111',
    'text-muted': dark ? '#cccccc' : '#555555', 'shadow-sm': '0 1px 2px #0002', 'text-on-accent': '#ffffff' };
  const style = document.createElement('style');
  style.textContent = choiceStyles.replace(/var\(--([\w-]+)\)/g, (original: string, token: string) => colours[token] ?? original);
  document.head.append(style); styles.push(style);
  expect(style.sheet).not.toBeNull();
  expect(style.sheet?.cssRules.length).toBeGreaterThan(0);
}
afterEach(async () => { cleanup(); for (const style of styles.splice(0)) style.remove(); await setLocale('en'); });

for (const locale of ['en', 'zh-CN'] as const) for (const dark of [false, true]) {
  it(`keeps name-sized flex items and the chosen ground/words in ${locale}/${dark ? 'dark' : 'light'}`, async () => {
    await setLocale(locale); stylesheet(dark);
    const first = locale === 'en' ? 'Environment' : '环境';
    const second = locale === 'en' ? 'Workspace' : '工作区';
    function Filter() {
      const [value, setValue] = useState('environment');
      return <SegmentedControl label="Kind" value={value} onValueChange={setValue} segments={[
        { value: 'environment', name: first, description: locale === 'en' ? 'Work in an environment.' : '在环境中工作。' },
        { value: 'workspace', name: second, description: locale === 'en' ? 'Open its workspace.' : '打开其工作区。' },
      ]} />;
    }
    show(<Filter />);
    const group = screen.getByRole('radiogroup', { name: 'Kind' });
    expect(group).toHaveAttribute('data-segmented');
    const environment = screen.getByRole('radio', { name: first });
    const workspace = screen.getByRole('radio', { name: second });
    const words = screen.getByText(first);
    // The measured 18px regression was flex-1 on this wrapper and wrap-anywhere on this name.
    // jsdom has no geometry: selector-layout.spec measures at least the entire label plus padding.
    expect(environment).toHaveClass('flex-auto');
    expect(words.parentElement).toHaveClass('flex-auto');
    expect(words).toHaveClass('break-words');
    expect(environment).toBeChecked();
    expect(getComputedStyle(environment).backgroundColor).toBe(dark ? 'rgb(68, 68, 68)' : 'rgb(255, 255, 255)');
    expect(getComputedStyle(words).color).toBe(dark ? 'rgb(238, 238, 238)' : 'rgb(17, 17, 17)');
    await userEvent.click(workspace);
    expect(workspace).toBeChecked();
    expect(environment).not.toBeChecked();
    expect(getComputedStyle(workspace).backgroundColor).toBe(dark ? 'rgb(68, 68, 68)' : 'rgb(255, 255, 255)');
    expect(getComputedStyle(environment).backgroundColor).toBe('rgba(0, 0, 0, 0)');
  });

  it(`marks a chosen menu radio with its check rather than a filled row in ${locale}/${dark ? 'dark' : 'light'}`, async () => {
    await setLocale(locale); stylesheet(dark);
    const user = userEvent.setup();
    const light = choiceCopy(i18n.t, 'theme.light').name;
    const night = choiceCopy(i18n.t, 'theme.dark').name;
    show(<Menu label="Theme menu" trigger={<button>Theme menu</button>} entries={[{ type: 'radio', id: 'theme', label: 'Theme', value: 'light', onValueChange: () => undefined,
      options: [{ value: 'light', name: light }, { value: 'dark', name: night }] }]} />);
    await user.click(screen.getByRole('button', { name: 'Theme menu' }));
    const chosen = screen.getByRole('menuitemradio', { name: light });
    const other = screen.getByRole('menuitemradio', { name: night });
    other.focus();
    await waitFor(() => {
      expect(other).toHaveFocus();
      expect(other).toHaveAttribute('data-highlighted');
      expect(chosen).not.toHaveAttribute('data-highlighted');
    });
    expect(chosen).toBeChecked();
    expect(chosen.querySelector('svg.lucide-check')).not.toBeNull();
    expect(other.querySelector('svg.lucide-check')).toBeNull();
    expect(getComputedStyle(chosen).backgroundColor).toBe('rgba(0, 0, 0, 0)');
  });

  it(`keeps highlighted picker words readable on a neutral highlighted row in ${locale}/${dark ? 'dark' : 'light'}`, async () => {
    await setLocale(locale); stylesheet(dark);
    const name = locale === 'en' ? 'Workspace' : '工作区';
    const description = locale === 'en' ? 'Open the selected workspace.' : '打开所选工作区。';
    show(<div role="listbox" aria-label="Palette"><Choice presentation="picker" name={name} description={description}>
      {(content, control) => <button {...control} role="option" aria-selected className="choice-option" data-highlighted>{content}</button>}
    </Choice></div>);
    const row = screen.getByRole('option', { name });
    expect(row).toHaveAttribute('data-highlighted');
    expect(row).toHaveAccessibleDescription(description);
    const expected = dark ? 'rgb(238, 238, 238)' : 'rgb(17, 17, 17)';
    expect(getComputedStyle(screen.getByText(name)).color).toBe(expected);
    expect(getComputedStyle(screen.getByText(description)).color).toBe(expected);
    expect(getComputedStyle(row).backgroundColor).toBe(dark ? 'rgb(51, 51, 51)' : 'rgb(221, 221, 221)');
  });
}
