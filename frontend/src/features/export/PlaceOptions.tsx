/** What an archive holds besides the rows that are ticked: the kinds, form, conversation detail, attachments, earlier versions, comments and span of days.
 *
 * The three kinds are ticked as the dialog opens. The form is what a person reads or what a program reads, both
 * the forms a single export gives. Every option stays visible; a choice about a kind that is not wanted
 * is switched off.
 */
import { useTranslation } from 'react-i18next';
import { choiceOption } from '@/i18n/choices';

import { Hint } from '@/features/help/Hint';
import type { ExportDetail } from '@/generated/api-types';
import { Checkbox } from '@/ui/Checkbox';
import { DateField } from '@/ui/DateField';
import { Select } from '@/ui/Select';
import { SegmentedControl } from '@/ui/SegmentedControl';

import { type Choices, type PlaceFormat, spanFault } from './choices';
import { Group } from './Group';

export interface PlaceOptionsProps {
  choices: Choices;
  onChange: (patch: Partial<Choices>) => void;
}

export function PlaceOptions({ choices, onChange }: PlaceOptionsProps) {
  const { t } = useTranslation();
  const fault = spanFault(choices);

  return (
    <>
      <Group label={t('export.include.label')} lead="text">
        <div className="flex flex-wrap gap-x-6 gap-y-2">
          <Checkbox label={t('export.include.conversations')} checked={choices.conversations} onCheckedChange={(on) => onChange({ conversations: on })} />
          <Checkbox label={t('export.include.chats')} checked={choices.chats} onCheckedChange={(on) => onChange({ chats: on })} />
          <Checkbox label={t('export.include.documents')} checked={choices.documents} onCheckedChange={(on) => onChange({ documents: on })} />
        </div>
      </Group>
      <Group label={t('export.format.label')} help={<Hint id="export-place-format" />}>
        <SegmentedControl<PlaceFormat>
          label={t('export.format.label')}
          value={choices.format}
          onValueChange={(format) => onChange(format === 'package' ? { format, files: true, comments: true } : { format })}
          segments={[
              { value: 'markdown', ...choiceOption(t, 'exportFormat.markdown') },
              { value: 'json', ...choiceOption(t, 'exportFormat.json') },
              { value: 'package', ...choiceOption(t, 'exportFormat.package') },
          ]}
        />
        {choices.format === 'package' && <p className="mt-1 text-sm text-text-muted">{t('importWizard.export.packageNote')}</p>}
      </Group>
      <Group label={t('export.options.label')} lead="text">
        <div className="flex min-w-0 flex-col gap-4">
          <Select<ExportDetail>
            label={t('export.more.conversations')}
            help={<Hint id="export-detail" />}
            value={choices.detail}
            onValueChange={(detail) => onChange({ detail })}
            disabled={!choices.conversations}
            fieldClassName="max-w-(--chip-width)"
            options={[
              { value: 'brief', ...choiceOption(t, 'exportDetail.brief') },
              { value: 'normal', ...choiceOption(t, 'exportDetail.normal') },
              { value: 'full', ...choiceOption(t, 'exportDetail.full') },
            ]}
          />
          <div className="flex flex-wrap gap-x-6 gap-y-2">
            <Checkbox label={t('export.more.files')} hint={t('export.more.filesAbout')} checked={choices.files} onCheckedChange={(on) => onChange({ files: on })} />
            <Checkbox
              label={t('export.more.versions')}
              hint={t('export.more.versionsAbout')}
              checked={choices.versions && choices.documents}
              disabled={!choices.documents}
              onCheckedChange={(on) => onChange({ versions: on })}
            />
            <Checkbox
              label={t('export.more.comments')}
              hint={t('export.more.commentsAbout')}
              checked={choices.comments && choices.documents}
              disabled={!choices.documents}
              onCheckedChange={(on) => onChange({ comments: on })}
            />
          </div>
          <div className="flex flex-wrap items-start gap-3">
            <DateField
              label={t('export.more.from')}
              help={<Hint id="export-span" />}
              value={choices.from}
              max={choices.to === '' ? undefined : choices.to}
              onValueChange={(value) => onChange({ from: value })}
              fieldClassName="w-[11rem]"
            />
            <DateField
              label={t('export.more.to')}
              value={choices.to}
              min={choices.from === '' ? undefined : choices.from}
              error={fault === 'order' ? t('export.more.spanOrder') : undefined}
              onValueChange={(value) => onChange({ to: value })}
              fieldClassName="w-[11rem]"
            />
          </div>
        </div>
      </Group>
    </>
  );
}
