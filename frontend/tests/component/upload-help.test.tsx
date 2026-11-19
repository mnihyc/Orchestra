/** The shared question opens its exact localized guide section without sending a draft. */
import { fireEvent, screen } from '@testing-library/react';
import { afterEach, expect, it, vi } from 'vitest';
import { UploadHelp } from '@/features/composer/UploadHelp';
import { UPLOAD_HELP, uploadHelpHeading, type UploadHelpTopic } from '@/help/uploads';
import { i18n, setLocale } from '@/i18n';
import { show } from './support';

const opened = vi.hoisted(() => vi.fn());
vi.mock('@/features/help/open', () => ({ useHelpOpener: () => opened }));
afterEach(async () => { opened.mockClear(); await setLocale('en'); });

for (const language of ['en', 'zh-CN'] as const) for (const topic of Object.keys(UPLOAD_HELP) as UploadHelpTopic[]) {
  it(`opens ${topic} at its ${language} heading`, async () => {
    await setLocale(language);
    show(<UploadHelp topic={topic} />);
    const question = screen.getByRole('button', { name: i18n.t('help.hint.label', { term: i18n.t(UPLOAD_HELP[topic].title) }) });
    fireEvent.click(question);
    fireEvent.click(await screen.findByRole('button', { name: i18n.t('help.hint.more') }));
    expect(opened).toHaveBeenCalledExactlyOnceWith('paste-and-upload', uploadHelpHeading(topic));
  });
}
