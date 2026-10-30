/** The actual root keeps its shell mounted while the next screen waits for code/data. */
import { createMemoryHistory, createRoute, createRouter, RouterProvider } from '@tanstack/react-router';
import { act, cleanup, screen } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import type { ReactNode } from 'react';
import { afterEach, expect, it, vi } from 'vitest';
import { client } from '@/api/client';
import { i18n, setLocale } from '@/i18n';
import { Route } from '@/routes/__root';
import { ScreenLoading } from '@/shell/ScreenLoading';
import { show } from './support';

vi.mock('@/shell/Shell', () => ({ Shell: ({ children }: { children: ReactNode }) => <><nav aria-label="Rail"><input aria-label="Frame draft" defaultValue="" /></nav><main>{children}</main></> }));
vi.mock('@/routes/-access', () => ({ admit: () => Promise.resolve({}) }));
vi.mock('@/shell/screenCode', () => ({ useScreenCodePrefetch: () => undefined }));
afterEach(async () => { cleanup(); await setLocale('en'); document.documentElement.removeAttribute('data-theme'); });

for (const locale of ['en', 'zh-CN'] as const) for (const theme of ['light', 'dark']) {
  it(`retains the frame and puts a skeleton only in main in ${locale}/${theme}`, async () => {
    const session = client.session.getState();
    let release: (() => void) | undefined;
    const waiting = new Promise<void>((resolve) => { release = resolve; });
    try {
      await setLocale(locale);
      document.documentElement.setAttribute('data-theme', theme);
      client.session.setState({ status: 'signed_in', userId: 'reader' });
      const ready = createRoute({ getParentRoute: () => Route, path: '/ready', component: () => <p>Ready screen</p> });
      const later = createRoute({ getParentRoute: () => Route, path: '/later', loader: () => waiting, pendingComponent: ScreenLoading, component: () => <p>Next screen</p> });
      const router = createRouter({ routeTree: Route.addChildren([ready, later]), history: createMemoryHistory({ initialEntries: ['/ready'] }), defaultPendingMs: 0, defaultPendingMinMs: 0 });
      const user = userEvent.setup();
      show(<RouterProvider router={router} />);
      await screen.findByText('Ready screen');
      const draft = screen.getByRole('textbox', { name: 'Frame draft' });
      await user.type(draft, 'Retained');
      let navigation: Promise<void> | undefined;
      await act(async () => { navigation = router.navigate({ href: '/later' }); await Promise.resolve(); });
      expect(await screen.findByRole('status', { name: i18n.t('common.loading') })).toBeVisible();
      expect(screen.getByRole('navigation', { name: 'Rail' })).toBeVisible();
      expect(screen.getByRole('textbox', { name: 'Frame draft' })).toBe(draft);
      expect(draft).toHaveValue('Retained');
      expect(document.querySelector('[data-screen-loading]')?.closest('main')).not.toBeNull();
      await act(async () => { release?.(); await navigation; });
      expect(await screen.findByText('Next screen')).toBeVisible();
      expect(screen.getByRole('textbox', { name: 'Frame draft' })).toHaveValue('Retained');
    } finally { release?.(); client.session.setState(session); }
  });
}
