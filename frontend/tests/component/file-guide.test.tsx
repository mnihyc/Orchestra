/** File hints open the actual authored heading in both languages. */
import { cleanup, screen } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { afterEach, expect, it, vi } from 'vitest';
import { FileGuideHint } from '@/features/files/FileGuideHint';
import { FILE_GUIDE } from '@/help/files';
import { i18n } from '@/i18n';
import { show } from './support';

const calls = vi.hoisted(() => ({ open: vi.fn() }));
vi.mock('@/features/help/open', () => ({ useHelpOpener: () => calls.open }));
afterEach(async () => { cleanup(); calls.open.mockClear(); await i18n.changeLanguage('en'); });

it.each(['en', 'zh-CN'] as const)('opens the file preview heading in %s', async (language) => {
  await i18n.changeLanguage(language);
  const user = userEvent.setup();
  show(<FileGuideHint section="previews" />);
  const trigger = screen.getByRole('button', { name: i18n.t('help.hint.label', { term: i18n.t('fileMedia.help.previewsTerm') }) });
  expect(trigger).toHaveAttribute('data-help-article', FILE_GUIDE.id);
  expect(trigger).toHaveAttribute('data-help-heading', FILE_GUIDE.sections.previews[language]);
  await user.click(trigger);
  await user.click(await screen.findByRole('button', { name: i18n.t('help.hint.more') }));
  expect(calls.open).toHaveBeenCalledWith(FILE_GUIDE.id, FILE_GUIDE.sections.previews[language]);
});
