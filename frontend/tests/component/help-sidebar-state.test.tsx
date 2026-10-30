/** Help keeps its local choices and draft when the independent global navigation opens or closes. */
import { cleanup, screen } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { useState } from 'react';
import { afterEach, expect, it } from 'vitest';
import { ViewOpeningContext } from '@/features/workbench/viewOpening';
import { i18n, setLocale } from '@/i18n';
import { HelpPanel } from '@/ui/HelpPanel';
import { show } from './support';

function Sidebar({ shown }: { shown: boolean }) {
  return <nav hidden={!shown} aria-label="Project navigation"><a href="/org/acme">Acme</a></nav>;
}
function Reading() {
  return <HelpPanel label={i18n.t('views.help.title')} search={<input aria-label="Search the guide" />} value="articles" onValueChange={() => undefined}
    tabs={[{ value: 'articles', label: i18n.t('help.parts.articles'), panel: <input aria-label="Kept draft" defaultValue="" /> }, { value: 'keys', label: i18n.t('help.parts.keys'), panel: <p>Keyboard commands</p> }]} />;
}
function Frame() {
  const [shown, setShown] = useState(true);
  return <><button onClick={() => setShown(!shown)}>Toggle sidebar</button><ViewOpeningContext.Provider value={shown ? 'opening' : null}><Sidebar shown={shown} /></ViewOpeningContext.Provider><Reading /></>;
}
afterEach(async () => { cleanup(); document.documentElement.removeAttribute('data-theme'); await setLocale('en'); });
for (const locale of ['en', 'zh-CN'] as const) for (const theme of ['light', 'dark'] as const) it(`keeps one section navigation and the same draft while hiding the sidebar at ${locale}/${theme}`, async () => {
  await setLocale(locale); document.documentElement.setAttribute('data-theme', theme);
  const user = userEvent.setup(); show(<Frame />);
  expect(screen.getByRole('navigation', { name: 'Project navigation' })).toBeVisible();
  expect(screen.getByRole('tablist', { name: i18n.t('views.help.title') })).toBeVisible();
  expect(screen.getByRole('tabpanel', { name: i18n.t('help.parts.articles') })).toBeVisible();
  const draft = screen.getByRole('textbox', { name: 'Kept draft' });
  await user.type(draft, 'Still reading');
  await user.click(screen.getByRole('button', { name: 'Toggle sidebar' }));
  expect(screen.queryByRole('navigation')).not.toBeInTheDocument();
  expect(screen.getByRole('tablist', { name: i18n.t('views.help.title') })).toBeVisible();
  expect(screen.getByRole('textbox', { name: 'Kept draft' })).toBe(draft);
  expect(draft).toHaveValue('Still reading');
  await user.click(screen.getByRole('button', { name: 'Toggle sidebar' }));
  expect(screen.getByRole('tablist')).toBeVisible();
  expect(screen.getByRole('textbox', { name: 'Kept draft' })).toBe(draft);
  expect(draft).toHaveValue('Still reading');
});
