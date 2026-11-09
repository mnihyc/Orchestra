/** Native image support is the server's declared route and reason, not a browser inference. */
import { screen, within } from '@testing-library/react';
import { afterEach, expect, it, vi } from 'vitest';
import { CapabilityRoutes } from '@/features/inspector/CapabilityRoutes';
import { i18n, setLocale } from '@/i18n';
import { show } from './support';

vi.mock('@/features/help/open', () => ({ useHelpOpener: () => vi.fn() }));
afterEach(async () => { await setLocale('en'); });

for (const locale of ['en', 'zh-CN'] as const) {
  it(`shows the declared sign-in/Free requirement and preserves current authority in ${locale}`, async () => {
    await setLocale(locale);
    const row = { capability: 'image_generation', title: 'Image generation', reason: 'program_support_not_applicable', detail: 'native_image_sign_in_required',
      program_default: true, route_view: null, provider: null, prices: {} };
    const view = show(<CapabilityRoutes value={[row]} />);
    const region = screen.getByRole('region', { name: i18n.t('capabilityRoutes.title') });
    expect(region).toHaveTextContent(locale === 'en' ? 'ChatGPT sign-in required; unavailable on Free' : '需要 ChatGPT 登录；免费套餐不提供');
    expect(within(region).queryByRole('combobox')).not.toBeInTheDocument();
    view.rerender(<CapabilityRoutes value={[{ ...row, reason: 'current_authority_refused', detail: null }]} />);
    expect(region).toHaveTextContent(i18n.t('capabilityRoutes.reasons.current_authority_refused'));
    expect(region).not.toHaveTextContent('ChatGPT');
    view.rerender(<CapabilityRoutes value={[{ ...row, detail: 'constructor' }]} />);
    expect(region).toHaveTextContent(i18n.t('capabilityRoutes.reasons.program_support_not_applicable'));
    expect(region).not.toHaveTextContent('constructor');
  });
}
