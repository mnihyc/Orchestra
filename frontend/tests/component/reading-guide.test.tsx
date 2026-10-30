/** The real layout-menu hint opens the authored heading in the reader's language. */
import { screen, waitFor } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { afterEach, expect, it, vi } from 'vitest';
import { ReadingGuideMenuItem } from '@/features/help/ReadingGuide';
import { READING_GUIDE, readingGuideHeading } from '@/help/reading';
import { i18n, setLocale } from '@/i18n';
import { Menu } from '@/ui/Menu';
import { show } from './support';

const calls = vi.hoisted(() => ({ open: vi.fn() }));
vi.mock('@/features/help/open', () => ({ useHelpOpener: () => calls.open }));
afterEach(async () => { calls.open.mockClear(); await setLocale('en'); });

it.each(['en', 'zh-CN'] as const)('links the actual %s card-and-menu heading from the native menu', async (locale) => {
  await setLocale(locale);
  const user = userEvent.setup();
  show(<Menu label="Layout" trigger={<button>Layout</button>} content={<ReadingGuideMenuItem />} />);
  await user.click(screen.getByRole('button', { name: 'Layout' }));
  const item = screen.getByRole('menuitem', { name: i18n.t('readingGuide.cards') });
  expect(item).toHaveAttribute('data-help-article', READING_GUIDE.id);
  expect(item).toHaveAttribute('data-help-heading', readingGuideHeading(locale, 'cards'));
  await user.click(item);
  await waitFor(() => expect(calls.open).toHaveBeenCalledExactlyOnceWith(READING_GUIDE.id, readingGuideHeading(locale, 'cards')));
});
