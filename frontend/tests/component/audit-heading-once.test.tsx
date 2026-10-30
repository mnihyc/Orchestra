/** UA-07: Management supplies the system audit heading; standalone scoped trails keep theirs. */
import { cleanup, screen } from '@testing-library/react';
import { afterEach, beforeEach, expect, it, vi } from 'vitest';
import { Audit } from '@/features/admin/Audit';
import { Trail } from '@/features/trace/Trail';
import { i18n, setLocale } from '@/i18n';
import { useAppearance } from '@/lib/appearance';
import { show } from './support';

vi.mock('@tanstack/react-router', () => ({ useLocation: () => ({ pathname: '/admin/audit', search: {}, searchStr: '', hash: '' }), useNavigate: () => vi.fn(), useRouter: () => ({ history: { replace: vi.fn() } }) }));
vi.mock('@/api/trace', async (original) => ({ ...await original<object>(),
  trailQuery: () => ({ queryKey: ['one-audit-heading'], initialPageParam: null, queryFn: () => Promise.resolve({ items: [], next_cursor: null }), getNextPageParam: () => undefined }),
  selectedPersonQuery: () => ({ queryKey: ['one-audit-person'], queryFn: () => Promise.resolve(null) }),
}));
vi.mock('@/features/trace/Filters', () => ({ TraceFilters: () => null }));
vi.mock('@/lib/viewChoices', () => ({ useViewChoice: (_view: string, _key: string, value: unknown) => [value, vi.fn()] }));
const modes = [['en', 'light'], ['en', 'dark'], ['zh-CN', 'light'], ['zh-CN', 'dark']] as const;
const appearance = useAppearance.getState();
const previousTheme = document.documentElement.dataset.theme;
const wasDark = document.documentElement.classList.contains('dark');
const matchMedia = Object.getOwnPropertyDescriptor(window, 'matchMedia');
beforeEach(() => {
  Object.defineProperty(window, 'matchMedia', { configurable: true, writable: true, value: vi.fn((media: string) => ({
    matches: true, media, onchange: null, addListener: vi.fn(), removeListener: vi.fn(),
    addEventListener: vi.fn(), removeEventListener: vi.fn(), dispatchEvent: vi.fn(() => true),
  })) });
});
afterEach(async () => {
  cleanup();
  if (matchMedia === undefined) Reflect.deleteProperty(window, 'matchMedia');
  else Object.defineProperty(window, 'matchMedia', matchMedia);
  useAppearance.setState(appearance);
  document.documentElement.classList.toggle('dark', wasDark);
  if (previousTheme === undefined) delete document.documentElement.dataset.theme;
  else document.documentElement.dataset.theme = previousTheme;
  await setLocale('en');
});

it.each(modes)('keeps one Management heading and the standalone trail heading in %s/%s', async (locale, theme) => {
  await setLocale(locale);
  useAppearance.setState({ theme, resolvedTheme: theme });
  document.documentElement.dataset.theme = theme;
  document.documentElement.classList.toggle('dark', theme === 'dark');
  const title = i18n.t('admin.sections.audit.title');
  const view = show(<><h1>{title}</h1><Audit /></>);
  expect(await screen.findByText(i18n.t('trace.empty'))).toBeVisible();
  expect(screen.getAllByRole('heading')).toHaveLength(1);
  expect(screen.getAllByRole('heading', { name: title })).toHaveLength(1);
  expect(screen.queryByRole('heading', { name: i18n.t('trace.title', { scope: i18n.t('trace.systemTrail') }) })).not.toBeInTheDocument();
  view.unmount();
  const scope = { type: 'system' as const, name: i18n.t('trace.systemTrail') };
  show(<Trail scope={scope} />);
  expect(screen.getByRole('heading', { name: i18n.t('trace.title', { scope: scope.name }) })).toBeVisible();
});
