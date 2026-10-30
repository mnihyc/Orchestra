/** Quiet bilingual navigation keeps its names, icon context and keyboard behavior while fields are edited. */
import { screen, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { LucideProvider } from 'lucide-react';
import { useState } from 'react';
import { afterEach, beforeEach, expect, it, vi } from 'vitest';

import { Section } from '@/features/navigation/Section';
import { TreeFilter } from '@/features/navigation/TreeFilter';
import { i18n, setLocale } from '@/i18n';

import { show } from './support';

let previousTheme: string | null;
beforeEach(() => { previousTheme = document.documentElement.getAttribute('data-theme'); });
afterEach(async () => {
  if (previousTheme === null) document.documentElement.removeAttribute('data-theme');
  else document.documentElement.setAttribute('data-theme', previousTheme);
  await setLocale('en');
});

function Navigation({ leave, down }: { leave: () => void; down: () => void }) {
  const [folded, setFolded] = useState(false);
  const [typed, setTyped] = useState('');
  return <LucideProvider size={16} strokeWidth={1.75}>
    <Section title={i18n.t('navigation.recent')} folded={false} onFoldedChange={() => undefined}>
      <a href="/org/acme">Acme</a>
    </Section>
    <Section title={i18n.t('navigation.projects')} folded={folded} onFoldedChange={setFolded}>
      <a href="/project/web">Web</a>
    </Section>
    <TreeFilter value={typed} onChange={setTyped} onLeave={leave} onDown={down} />
  </LucideProvider>;
}

for (const locale of ['en', 'zh-CN'] as const) for (const theme of ['light', 'dark'] as const) {
  it(`keeps each section name once and folds it from the keyboard (${locale}, ${theme})`, async () => {
    await setLocale(locale);
    document.documentElement.dataset.theme = theme;
    show(<Navigation leave={() => undefined} down={() => undefined} />);
    const user = userEvent.setup();
    for (const name of [i18n.t('navigation.recent'), i18n.t('navigation.projects')]) {
      expect(screen.getAllByText(name, { exact: true })).toHaveLength(1);
      const heading = screen.getByRole('heading', { name });
      const button = within(heading).getByRole('button', { name });
      expect(button).toHaveAttribute('aria-expanded', 'true');
    }
    const projects = screen.getByRole('button', { name: i18n.t('navigation.projects') });
    projects.focus();
    await user.keyboard('{Enter}');
    expect(projects).toHaveFocus();
    expect(projects).toHaveAttribute('aria-expanded', 'false');
    expect(document.getElementById(projects.getAttribute('aria-controls') ?? '')).toHaveAttribute('inert');
    await user.keyboard('{Enter}');
    expect(projects).toHaveAttribute('aria-expanded', 'true');
    expect(screen.getByRole('link', { name: 'Web' })).toBeVisible();
  });

  it(`keeps the padded search field and its clear action usable by keyboard (${locale}, ${theme})`, async () => {
    await setLocale(locale);
    document.documentElement.dataset.theme = theme;
    const leave = vi.fn(), down = vi.fn();
    const view = show(<Navigation leave={leave} down={down} />);
    const user = userEvent.setup();
    const field = screen.getByRole('searchbox', { name: i18n.t('navigation.filter.label') });
    expect(field).toHaveAttribute('placeholder', i18n.t('navigation.filter.placeholder'));
    await user.type(field, 'Web');
    expect(field).toHaveValue('Web');
    await user.keyboard('{ArrowDown}{Enter}');
    expect(down).toHaveBeenCalledTimes(2);
    const clear = screen.getByRole('button', { name: i18n.t('navigation.filter.clear') });
    clear.focus();
    await user.keyboard('{Enter}');
    expect(field).toHaveValue('');
    expect(leave).not.toHaveBeenCalled();
    await user.type(field, 'Acme');
    await user.keyboard('{Escape}');
    expect(field).toHaveValue('');
    expect(leave).toHaveBeenCalledOnce();
    // jsdom proves accessible interaction and actual SVG attributes; the browser measures spacing and ink.
    for (const icon of view.container.querySelectorAll('svg.lucide')) {
      expect(icon).toHaveAttribute('width', '16');
      expect(icon).toHaveAttribute('height', '16');
      expect(icon).toHaveAttribute('stroke-width', '1.75');
    }
  });
}
