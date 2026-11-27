/** Names remain real links inside the shared component frame, including newly introduced group headings. */
import { createMemoryHistory, createRootRoute, createRouter, Link, RouterContextProvider } from '@tanstack/react-router';
import { act, render, screen } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { afterEach, expect, it } from 'vitest';
import { i18n, setLocale } from '@/i18n';
import { PathLink } from '@/shell/TrailLink';
import { recoveryCodesIssued, recoveryCodeUsed } from '@/features/auth/recoveryCode';
import { Toaster } from '@/ui/Toast';
import { show } from './support';

afterEach(async () => { recoveryCodesIssued(); await setLocale('en'); });
for (const locale of ['en', 'zh-CN'] as const) it(`draws a native router link without replacing its semantics (${locale})`, async () => {
  await setLocale(locale);
  const name = locale === 'en' ? 'Open Inbox' : '打开收件箱';
  show(<Link to="/inbox">{name}</Link>);
  expect(screen.getByRole('link', { name })).toHaveAttribute('href', '/inbox');
});


const isolatedRouter = () => createRouter({ routeTree: createRootRoute(), history: createMemoryHistory({ initialEntries: ['/before'] }) });
it('routes an ordinary link once with its search and fragment while retaining the original click handler', async () => {
  const router = isolatedRouter();
  let clicked = 0, prevented = false;
  show(<RouterContextProvider router={router}><PathLink href="/after?tab=files&sort=name#kept" onClick={() => { clicked += 1; }}>Go</PathLink></RouterContextProvider>);
  const stopNative = (event: MouseEvent) => { prevented = event.defaultPrevented; event.preventDefault(); };
  document.addEventListener('click', stopNative);
  try {
    await userEvent.click(screen.getByRole('link', { name: 'Go' }));
    expect(prevented).toBe(true);
    expect(router.history.location.href).toBe('/after?tab=files&sort=name#kept');
    expect(clicked).toBe(1);
  } finally { document.removeEventListener('click', stopNative); }
});

it.each([{ ctrlKey: true }, { metaKey: true }, { shiftKey: true }, { altKey: true }, { button: 1 }])('keeps a modified link click as the browser act (%j)', (gesture) => {
  const router = isolatedRouter(); let prevented = true;
  show(<RouterContextProvider router={router}><PathLink href="/after">Go</PathLink></RouterContextProvider>);
  const stopNative = (event: MouseEvent) => { prevented = event.defaultPrevented; event.preventDefault(); };
  document.addEventListener('click', stopNative);
  try {
    act(() => { screen.getByRole('link', { name: 'Go' }).dispatchEvent(new MouseEvent('click', { bubbles: true, cancelable: true, button: 0, ...gesture })); });
    expect(prevented).toBe(false); expect(router.history.location.href).toBe('/before');
  } finally { document.removeEventListener('click', stopNative); }
});

it.each([
  { href: '/after', target: '_blank' }, { href: '/after', download: true }, { href: '#part' },
  { href: 'https://outside.example.test/after' }, { href: '/api/v1/pki/ca.pem' },
])('keeps a native target outside ordinary application routing (%j)', async (anchor) => {
  const router = isolatedRouter(); let prevented = true;
  show(<RouterContextProvider router={router}><PathLink {...anchor}>Go</PathLink></RouterContextProvider>);
  const stopNative = (event: MouseEvent) => { prevented = event.defaultPrevented; event.preventDefault(); };
  document.addEventListener('click', stopNative);
  try { await userEvent.click(screen.getByRole('link', { name: 'Go' })); expect(prevented).toBe(false); expect(router.history.location.href).toBe('/before'); }
  finally { document.removeEventListener('click', stopNative); }
});

it('lets an existing panel handler own its click without adding a second navigation', async () => {
  const router = isolatedRouter(); let opened = 0;
  show(<RouterContextProvider router={router}><PathLink href="/after" onClick={(event) => { event.preventDefault(); opened += 1; }}>Go</PathLink></RouterContextProvider>);
  await userEvent.click(screen.getByRole('link', { name: 'Go' }));
  expect(opened).toBe(1); expect(router.history.location.href).toBe('/before');
});

it('opens the recovery-code reminder through the same router as the page', async () => {
  const router = isolatedRouter();
  const view = render(<RouterContextProvider router={router}><Toaster /></RouterContextProvider>);
  try {
    act(() => { recoveryCodeUsed(); });
    await userEvent.click(await screen.findByRole('link', { name: i18n.t('security.recovery.open') }));
    expect(router.history.location.href).toBe('/settings/security');
  } finally { recoveryCodesIssued(); view.unmount(); }
});
