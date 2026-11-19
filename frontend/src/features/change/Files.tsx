/** Which files go along into another environment: all, none, or those that match what is written; the home directory and what was written in private views; the agent's notes.
 *
 * Where the result works in another directory or another environment, its files have to come from somewhere. The
 * choice is made here in three: everything (the default), nothing, or only what matches the patterns written, one
 * to a line, with what to leave out of that. The plan beneath says how many files and how many bytes that comes to,
 * where they are read from (live, or a view kept from a moment) and what cannot be copied.
 */
import { useTranslation } from 'react-i18next';

import type { FileChoice } from '@/features/change/choices';
import { RadioGroup } from '@/ui/RadioGroup';
import { Textarea } from '@/ui/Textarea';

export function FilesChoice({ value, onChange }: { value: FileChoice; onChange: (files: FileChoice) => void }) {
  const { t } = useTranslation();
  return (
    <div className="flex flex-col gap-2">
      <h4 className="text-sm font-medium text-text">{t('fork.files.title')}</h4>
      <RadioGroup
        variant="list"
        label={t('fork.files.title')}
        value={value.mode}
        onValueChange={(mode) => onChange(mode === 'selection' ? { mode, include: value.mode === 'selection' ? value.include : '', exclude: value.mode === 'selection' ? value.exclude : '' } : { mode })}
        options={[
          { value: 'all', name: t('changeCompact.files.all') },
          { value: 'selection', name: t('changeCompact.files.selection') },
          { value: 'none', name: t('changeCompact.files.none') },
        ]}
      />
      {value.mode === 'selection' && (
        <div className="flex flex-col gap-2">
          <Textarea label={t('fork.files.include')} hint={t('fork.files.includeHint')} rows={3} value={value.include} onChange={(event) => onChange({ ...value, include: event.target.value })} />
          <Textarea label={t('fork.files.exclude')} optional hint={t('fork.files.excludeHint')} rows={2} value={value.exclude} onChange={(event) => onChange({ ...value, exclude: event.target.value })} />
        </div>
      )}
    </div>
  );
}
