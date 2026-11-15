/** The route facts are rendered in both languages without deciding availability or exposing backend data. */
import { screen, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { afterEach, expect, it, vi } from 'vitest';
import { CapabilityRoutes } from '@/features/inspector/CapabilityRoutes';
import { i18n, setLocale } from '@/i18n';
import { useAppearance } from '@/lib/appearance';
import { show } from './support';
const help = vi.hoisted(() => vi.fn());
vi.mock('@/features/help/open', () => ({ useHelpOpener: () => help }));
// The provider card owns its own current read. This suite keeps the ordinary chip as supplied text.
vi.mock('@/features/resource-facts/ResourceFacts', () => ({ ResourceChip: ({ subject }: { subject: { provider: { name: string } } }) => <span>{subject.provider.name}</span> }));

afterEach(async () => { await setLocale('en'); useAppearance.getState().set({ locale: 'en', theme: 'system' }); help.mockClear(); });
for (const language of ['en', 'zh-CN'] as const) for (const theme of ['light', 'dark'] as const) {
  it(`shows default, off, service, unavailable and future declarations in ${language}/${theme}`, async () => {
    await setLocale(language); useAppearance.getState().set({ locale: language, theme });
    const t = i18n.getFixedT(language);
    const row = { title: 'Fixture capability', reason: null, program_default: false, provider: null, prices: {} };
    show(<CapabilityRoutes value={[
      { ...row, capability: 'generation', program_default: true, route_view: { kind: 'native-program' } },
      { ...row, capability: 'web_search', route_view: { kind: 'service' }, provider: { id: 'fixture-account', title: 'Team search' }, prices: { search: { amount: '0.00075', currency: 'USD', unit: 'search' } }, route: { backend: { config: { secret: 'private-backend' } } } },
      { ...row, capability: 'page_fetch', route_view: null, reason: 'disabled', fallbacks:[{rule_id:'pages',scope:'org',when:'unavailable',provider:null,offering:null}] },
      { ...row, capability: 'image_generation', route_view: null, reason: 'current_authority_refused' },
      { ...row, capability: 'future_capability', title: 'Future capability', route_view: { kind: 'future-route' } },
      { ...row, capability: 'embeddings', route_view: { kind: 'native-model' }, provider: { id: 'fixture-router', title: 'Router account' } },
    ]} />);
    const region = screen.getByRole('region', { name: t('capabilityRoutes.title') });
    expect(within(region).getByText(t('capabilityRoutes.names.web_search'), { exact: true })).toBeVisible();
    expect(within(region).getByText(t('capabilityRoutes.routes.native-program'))).toBeVisible();
    expect(within(region).getByText(t('capabilityRoutes.programDefault'))).toBeVisible();
    expect(within(region).getByText(t('capabilityRoutes.routes.service'))).toBeVisible();
    expect(region).toHaveTextContent('Team search');
    expect(region).toHaveTextContent('0.00075');
    expect(region).toHaveTextContent(t('capabilityRoutes.reasons.disabled'));
    expect(region).toHaveTextContent(t('capabilityRoutes.reasons.current_authority_refused'));
    expect(region).toHaveTextContent('Future capability');
    expect(region).toHaveTextContent(t('capabilityRoutes.unknownRoute'));
    expect(region).toHaveTextContent('Router account');
    expect(region).toHaveTextContent(t('capabilityRoutes.routes.native-model'));
    expect(within(region).getByText(t('capabilityRoutes.noPrice'))).toBeVisible();
    expect(region).not.toHaveTextContent('private-backend');
    expect(region).toHaveTextContent('pages');
    expect(region).toHaveTextContent(t('capabilityRoutes.scopes.org'));
    expect(within(region).queryByRole('combobox')).not.toBeInTheDocument();
    await userEvent.click(within(region).getByRole('button', { name: t('capabilityRoutes.help') }));
    expect(help).toHaveBeenCalledWith('provider-web-search');
  });
}
it('omits an absent older-server projection and ignores malformed rows', () => {
  const { rerender } = show(<CapabilityRoutes value={undefined} />);
  expect(screen.queryByTestId('capability-routes')).not.toBeInTheDocument();
  rerender(<CapabilityRoutes value={[null, { capability: 'incomplete' }]} />);
  expect(screen.queryByTestId('capability-routes')).not.toBeInTheDocument();
});

it.each(['en','zh-CN'] as const)('shows a captured generation conversion with its declared limits in %s', async (language) => {
  await setLocale(language);
  const t = i18n.getFixedT(language);
  const translation = {from:'openai-responses',to:'anthropic-messages',title:'Responses / Messages',losses:{
    preserved:['Text, image inputs, tool identifiers and upstream token usage.'],
    approximated:['Custom tool grammars become a single string argument; the upstream does not enforce the grammar.'],
    dropped:['Vendor server tools are not translated; use Orch’s capability routes.'],
  }};
  const row = {capability:'generation',title:'Generation',reason:null,program_default:false,provider:null,prices:{},route_view:{kind:'native-model'}};
  const shown = show(<CapabilityRoutes value={[{...row,translation}]} />);
  const route = screen.getByRole('button',{name:t('modelTranslation.route',{from:'Responses',to:'Messages'})});
  expect(screen.queryByRole('dialog')).not.toBeInTheDocument();
  await userEvent.click(route);
  expect(screen.getByRole('dialog')).toHaveTextContent(t('modelTranslation.custom'));
  expect(screen.getByRole('dialog')).not.toHaveTextContent('private-backend');
  await userEvent.keyboard('{Escape}');
  expect(screen.queryByRole('dialog')).not.toBeInTheDocument();
  shown.rerender(<CapabilityRoutes value={[row]} />);
  expect(screen.queryByRole('button',{name:t('modelTranslation.route',{from:'Responses',to:'Messages'})})).not.toBeInTheDocument();
});
