/** A chat as one file: Markdown to read or JSON for a program, a range of messages, and whether the attachments its messages carry go with it, which makes an archive.
 *
 * One small dialog for whoever may take the chat away. A chat is written as the page shows it numbered, so the
 * range is asked for by message numbers; with the attachments it is an archive of the chat and the attachments beside it.
 */
import { useMutation } from '@tanstack/react-query';
import { useId, useState } from 'react';
import { useTranslation } from 'react-i18next';
import { choiceOption } from '@/i18n/choices';

import { exportChatFile } from '@/api/exports';
import type { ExportFormat } from '@/generated/api-types';
import { Hint } from '@/features/help/Hint';
import { Button } from '@/ui/Button';
import { Checkbox } from '@/ui/Checkbox';
import { Dialog } from '@/ui/Dialog';
import { ErrorAlert } from '@/ui/ErrorState';
import { Input } from '@/ui/Input';
import { SegmentedControl } from '@/ui/SegmentedControl';

import { Group } from './Group';
import { saveFile } from './save';
import { countOf, type FileDialogProps } from './ThreadFile';

export function ChatFile({ target, open, onClose }: FileDialogProps) {
  const { t } = useTranslation();
  const formId = useId();
  const [format, setFormat] = useState<ExportFormat>('markdown');
  const [files, setFiles] = useState(false);
  const [from, setFrom] = useState('');
  const [to, setTo] = useState('');
  const [sent, setSent] = useState(false);

  const first = countOf(from);
  const last = countOf(to);
  const fault = (text: string, value: number | null | 'invalid'): string | undefined => (sent && value === 'invalid' ? t('export.chat.numberInvalid', { text }) : undefined);
  const backwards = typeof first === 'number' && typeof last === 'number' && last < first;

  const make = useMutation({
    mutationFn: () =>
      exportChatFile(target.id, {
        format,
        files: files ? 'include' : 'omit',
        ...(typeof first === 'number' ? { from: first } : {}),
        ...(typeof last === 'number' ? { to: last } : {}),
      }),
    onSuccess: (file) => {
      saveFile(file.blob, file.name);
      onClose();
    },
  });

  const submit = () => {
    setSent(true);
    if (first === 'invalid' || last === 'invalid' || backwards) return;
    make.mutate();
  };

  return (
    <Dialog
      open={open}
      onOpenChange={(next) => !next && !make.isPending && onClose()}
      title={t('export.chat.title', { name: target.name })}
      description={t('export.chat.lead')}
      dismissible={!make.isPending}
      footer={
        <>
          <Button variant="secondary" disabled={make.isPending} onClick={onClose}>
            {t('common.cancel')}
          </Button>
          <Button type="submit" form={formId} variant="primary" pending={make.isPending}>
            {t('export.chat.do')}
          </Button>
        </>
      }
    >
      <form
        id={formId}
        className="flex flex-col gap-4"
        onSubmit={(event) => {
          event.preventDefault();
          submit();
        }}
      >
        <Group label={t('export.format.label')} help={<Hint id="export-format" />}>
          <SegmentedControl<ExportFormat>
            startsKeyboard
            label={t('export.format.label')}
            value={format}
            onValueChange={setFormat}
            segments={[
              { value: 'markdown', ...choiceOption(t, 'exportFormat.markdown') },
              { value: 'json', ...choiceOption(t, 'exportFormat.json') },
            ]}
          />
        </Group>
        <Group label={t('export.chat.holds')} lead="text">
          <Checkbox label={t('export.chat.files')} hint={t('export.chat.filesAbout')} checked={files} onCheckedChange={setFiles} />
        </Group>
        <Group label={t('export.chat.messages')} lead="text">
          <div className="flex flex-wrap items-start gap-3">
            <Input
              label={t('export.chat.fromMessage')}
              inputMode="numeric"
              value={from}
              error={fault(from, first)}
              onChange={(event) => setFrom(event.target.value)}
              fieldClassName="w-[8rem]"
            />
            <Input
              label={t('export.chat.toMessage')}
              inputMode="numeric"
              value={to}
              error={fault(to, last) ?? (sent && backwards ? t('export.chat.messagesBackwards') : undefined)}
              onChange={(event) => setTo(event.target.value)}
              fieldClassName="w-[8rem]"
            />
          </div>
          <p className="mt-1 text-sm text-text-muted">{t('export.chat.messagesAbout')}</p>
        </Group>
        {make.isError && <ErrorAlert error={make.error} />}
      </form>
    </Dialog>
  );
}
