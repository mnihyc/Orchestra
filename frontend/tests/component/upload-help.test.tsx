/** The shared question opens its exact localized guide section without sending a draft. */
import { fireEvent, screen, waitFor, within } from '@testing-library/react';
import { afterEach, expect, it, vi } from 'vitest';
import { UploadHelp } from '@/features/composer/UploadHelp';
import { Dialog } from '@/ui/Dialog';
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

it.each(['en', 'zh-CN'] as const)('keeps one attachment guide across visible composers and promotes the next owner (%s)', async (language) => {
  await setLocale(language);
  const view = show(<><section data-testid="first"><UploadHelp topic="message-files" /></section><section data-testid="second"><UploadHelp topic="message-files" /></section></>);
  const name = i18n.t('help.hint.label', { term: i18n.t(UPLOAD_HELP['message-files'].title) });
  await waitFor(() => expect(screen.getAllByRole('button', { name })).toHaveLength(1));
  expect(within(screen.getByTestId('first')).getByRole('button', { name })).toBeVisible();
  view.rerender(<><section data-testid="first" hidden><UploadHelp topic="message-files" /></section><section data-testid="second"><UploadHelp topic="message-files" /></section></>);
  await waitFor(() => expect(within(screen.getByTestId('second')).getByRole('button', { name })).toBeVisible());
  expect(screen.getAllByRole('button', { name })).toHaveLength(1);
  fireEvent.click(screen.getByRole('button', { name }));
  fireEvent.click(await screen.findByRole('button', { name: i18n.t('help.hint.more') }));
  expect(opened).toHaveBeenCalledExactlyOnceWith('paste-and-upload', uploadHelpHeading('message-files'));
});

it.each(['en', 'zh-CN'] as const)('shares a guide across nonmodal reading surfaces and gives an actual prompt its own active entry (%s)', async language => {
  await setLocale(language);
  const surfaces = <><div role="dialog" aria-label="Thread reading surface"><UploadHelp topic="message-files" /></div><div role="dialog" aria-label="Chat reading surface"><UploadHelp topic="message-files" /></div></>;
  const view = show(surfaces);
  const name = i18n.t('help.hint.label', { term: i18n.t(UPLOAD_HELP['message-files'].title) });
  await waitFor(() => expect(screen.getAllByRole('button', { name })).toHaveLength(1));
  view.rerender(<>{surfaces}<Dialog open title="Attachment prompt" onOpenChange={() => undefined}><UploadHelp topic="message-files" /></Dialog></>);
  const prompt = screen.getByRole('dialog', { name: 'Attachment prompt' });
  await waitFor(() => expect(within(prompt).getByRole('button', { name })).toBeVisible());
  expect(screen.getAllByRole('button', { name })).toHaveLength(1);
});
