/** A thread as one file: how much of the conversation, Markdown to read or JSON for a program, a range of turns, and the three things only a person's export can add.
 *
 * One small dialog for whoever may take the thread away: what was said, with each step, or with everything each
 * step did; the form; the turns where not all; and three ticks for the agent's reasoning, what its sub-agents
 * did and the thread's chat. The file is made at once and saved by the browser; nothing is kept.
 */
import { useMutation } from '@tanstack/react-query';
import { useEffect, useId, useRef, useState } from 'react';
import { useTranslation } from 'react-i18next';
import { choiceOption } from '@/i18n/choices';

import { type ThreadFileQuery } from '@/api/exports';
import { exportViewedThreadFile } from '@/api/threadHistory';
import { captureThreadRead, type ThreadReadCapture } from '@/api/threadRead';
import { useThreadReadScope } from '@/realtime/useThreadRead';
import type { ExportDetail, ExportFormat } from '@/generated/api-types';
import { Hint } from '@/features/help/Hint';
import { Button } from '@/ui/Button';
import { Checkbox } from '@/ui/Checkbox';
import { Dialog } from '@/ui/Dialog';
import { ErrorAlert } from '@/ui/ErrorState';
import { Input } from '@/ui/Input';
import { RadioGroup } from '@/ui/RadioGroup';
import { SegmentedControl } from '@/ui/SegmentedControl';

import { Group } from './Group';
import { saveFile } from './save';
import type { ExportTarget } from './target';

export interface FileDialogProps {
  target: ExportTarget;
  open: boolean;
  onClose: () => void;
}

/** A turn or a message number as typed: a whole number from 1, or nothing. */
export function countOf(text: string): number | null | 'invalid' {
  const trimmed = text.trim();
  if (trimmed === '') return null;
  return /^\d+$/.test(trimmed) && Number(trimmed) >= 1 ? Number(trimmed) : 'invalid';
}

export function ThreadFile({ target, open, onClose }: FileDialogProps) {
  const { t } = useTranslation();
  const formId = useId();
  useThreadReadScope(open ? target.id : null);
  const captured = useRef<ThreadReadCapture | null>(null);
  useEffect(() => { captured.current = open ? captureThreadRead(target.id) : null; }, [open, target.id]);
  const [format, setFormat] = useState<ExportFormat>('markdown');
  const [detail, setDetail] = useState<ExportDetail>('normal');
  const [reasoning, setReasoning] = useState(false);
  // What its sub-agents did is in a person's export unless they say otherwise, as the server gives it.
  const [subAgents, setSubAgents] = useState(true);
  const [chat, setChat] = useState(false);
  const [from, setFrom] = useState('');
  const [to, setTo] = useState('');
  const [sent, setSent] = useState(false);

  const first = countOf(from);
  const last = countOf(to);
  const fault = (text: string, value: number | null | 'invalid'): string | undefined => (sent && value === 'invalid' ? t('export.thread.turnInvalid', { text }) : undefined);
  const backwards = typeof first === 'number' && typeof last === 'number' && last < first;

  const make = useMutation({
    mutationFn: () => {
      // What a person's export adds, in the words the server names them by.
      const include: ThreadFileQuery['include'][number][] = [];
      if (reasoning) include.push('reasoning');
      if (subAgents) include.push('sub_streams');
      if (chat) include.push('chat');
      return exportViewedThreadFile(target.id, {
        format,
        detail,
        include,
        ...(typeof first === 'number' ? { from: first } : {}),
        ...(typeof last === 'number' ? { to: last } : {}),
      }, undefined, captured.current ?? captureThreadRead(target.id));
    },
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
      title={t('export.thread.title', { name: target.name })}
      description={t('export.thread.lead')}
      dismissible={!make.isPending}
      footer={
        <>
          <Button variant="secondary" disabled={make.isPending} onClick={onClose}>
            {t('common.cancel')}
          </Button>
          <Button type="submit" form={formId} variant="primary" pending={make.isPending}>
            {t('export.thread.do')}
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
        <Group label={t('export.thread.howMuch')} help={<Hint id="export-detail" />} lead="box">
          <RadioGroup<ExportDetail>
            label={t('export.thread.howMuch')}
            variant="list"
            value={detail}
            onValueChange={setDetail}
            disabled={format === 'json'}
            options={[
              { value: 'brief', ...choiceOption(t, 'exportDetail.brief') },
              { value: 'normal', ...choiceOption(t, 'exportDetail.normal') },
              { value: 'full', ...choiceOption(t, 'exportDetail.full') },
            ]}
          />
          {format === 'json' && <p className="mt-1 text-sm text-text-muted">{t('export.thread.jsonHoldsAll')}</p>}
        </Group>
        <Group label={t('export.thread.adds')} lead="text">
          <div className="flex flex-col gap-2">
            <Checkbox label={t('export.thread.reasoning')} checked={reasoning} onCheckedChange={setReasoning} />
            <Checkbox label={t('export.thread.subAgents')} checked={subAgents} onCheckedChange={setSubAgents} />
            <Checkbox label={t('export.thread.chat')} hint={t('export.thread.chatAbout')} checked={chat} onCheckedChange={setChat} />
          </div>
        </Group>
        <Group label={t('export.thread.turns')} lead="text">
          <div className="flex flex-wrap items-start gap-3">
            <Input
              label={t('export.thread.fromTurn')}
              inputMode="numeric"
              value={from}
              error={fault(from, first)}
              onChange={(event) => setFrom(event.target.value)}
              fieldClassName="w-[8rem]"
            />
            <Input
              label={t('export.thread.toTurn')}
              inputMode="numeric"
              value={to}
              error={fault(to, last) ?? (sent && backwards ? t('export.thread.turnsBackwards') : undefined)}
              onChange={(event) => setTo(event.target.value)}
              fieldClassName="w-[8rem]"
            />
          </div>
          <p className="mt-1 text-sm text-text-muted">{t('export.thread.turnsAbout')}</p>
        </Group>
        {make.isError && <ErrorAlert error={make.error} />}
      </form>
    </Dialog>
  );
}
