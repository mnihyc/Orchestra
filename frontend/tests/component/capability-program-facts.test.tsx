/** Program facts are plain localized words from the existing capability projection, never another resolver. */
import { cleanup, screen } from '@testing-library/react';
import { afterEach, expect, it, vi } from 'vitest';
import { CapabilityRoutes } from '@/features/inspector/CapabilityRoutes';
import { setLocale } from '@/i18n';
import { useAppearance } from '@/lib/appearance';
import program from '../../../extensions/agents/claude-code/src/capability-program.json';
import { show } from './support';

vi.mock('@/features/help/open', () => ({ useHelpOpener: () => vi.fn() }));
const base = { title: 'Capability', reason: null, program_default: true, provider: null, prices: {}, route_view: { kind: 'native-program' } };
afterEach(async () => { cleanup(); await setLocale('en'); useAppearance.getState().set({ locale: 'en', theme: 'system' }); });

for (const language of ['en', 'zh-CN'] as const) for (const theme of ['light', 'dark'] as const) {
  it(`shows the declared host limitations and actual page-reader location ${language}/${theme}`, async () => {
    await setLocale(language); useAppearance.getState().set({ locale: language, theme });
    const facts = program.access.forwarded.facts;
    show(<CapabilityRoutes value={[
      { ...base, capability: 'generation', facts: facts.filter((fact) => fact.capabilities.includes('generation')) },
      { ...base, capability: 'web_search', facts: facts.filter((fact) => fact.capabilities.includes('web_search')) },
      { ...base, capability: 'page_fetch', facts: program.facts },
    ]} />);
    for (const fact of [...facts, ...program.facts]) expect(screen.getByText(fact.text[language], { exact: true })).toBeVisible();
    expect(screen.getAllByTestId('capability-facts')).toHaveLength(3);
    expect(screen.queryByRole('combobox')).not.toBeInTheDocument();
  });
}

it('keeps unknown private members and malformed facts out of the view and renders words without HTML', () => {
  const text = '<img src="credential.invalid" onerror="secret()">';
  const { container } = show(<CapabilityRoutes value={[{ ...base, capability: 'generation', facts: [
    { id: 'safe', text: { en: text }, extensions: { secret: 'private-data' } },
    { id: 'future', future_text: 'not a declared text' },
    { id: 'wrong', text: { en: 7 } }, null,
  ] }]} />);
  expect(screen.getByText(text, { exact: true })).toBeVisible();
  expect(container.querySelector('img')).toBeNull();
  expect(container).not.toHaveTextContent('private-data');
  expect(container).not.toHaveTextContent('not a declared text');
});
