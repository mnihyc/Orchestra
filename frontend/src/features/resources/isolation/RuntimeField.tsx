/** The runtime field of a template's form: a choice among the runtimes the place's hosts report for the driver, and a name typed by one who knows the host has it. */
import { useQuery } from '@tanstack/react-query';
import { useState, type ReactNode } from 'react';
import { useTranslation } from 'react-i18next';

import { hostsQuery } from '@/api/resources';
import { Input } from '@/ui/Input';
import { Select, type SelectOption } from '@/ui/Select';
import { classNamed, classesOfDriver, namesOfDriver, useIsolationWords } from './runtimes';

/** Nothing chosen: the host's own default applies. */
const UNSET = '\u0000host-default';
/** A name nobody listed. */
const OTHER = '\u0000other-runtime';

export interface RuntimeFieldProps {
  driver: string;
  label: string;
  hideLabel?: boolean;
  value: unknown;
  onChange: (value: unknown) => void;
  disabled: boolean;
  error?: string | undefined;
  help?: ReactNode;
  aside?: ReactNode;
  placeholder?: string | undefined;
}

export function RuntimeField({ driver, label, hideLabel = false, value, onChange, disabled, error, help, aside, placeholder }: RuntimeFieldProps) {
  const { t } = useTranslation();
  const words = useIsolationWords();
  const hosts = useQuery(hostsQuery(null));
  const [typing, setTyping] = useState(false);
  const classes = classesOfDriver(hosts.data ?? [], driver);
  const listed = new Set(classes.flatMap((runtime) => [runtime.name, ...(runtime.aliases ?? [])]));
  const names = namesOfDriver(hosts.data ?? [], driver).filter((name) => !listed.has(name));
  const current = typeof value === 'string' ? value : '';
  const known = current !== '' && (classNamed(classes, current) !== undefined || names.includes(current));
  const mode = typing || (current !== '' && !known) ? OTHER : current === '' ? UNSET : current;
  const options: SelectOption<string>[] = [
    { value: UNSET, label: placeholder ?? t('templates.editor.unset'), name: placeholder ?? t('templates.editor.unset') },
    ...classes.map((runtime) => ({ value: runtime.name, label: runtime.title, name: runtime.title, description: words.hint(runtime.title, runtime.boundary) })),
    ...names.map((name) => ({ value: name, label: name, name })),
    { value: OTHER, label: t('isolation.select.other'), name: t('isolation.select.other'), description: t('isolation.select.otherLead') },
  ];
  return (
    <div className="flex min-w-0 flex-col gap-2">
      <Select
        label={label}
        hideLabel={hideLabel}
        value={mode}
        disabled={disabled}
        error={error}
        help={help}
        aside={aside}
        onValueChange={(next) => {
          if (next === OTHER) { setTyping(true); return; }
          setTyping(false);
          onChange(next === UNSET ? undefined : next);
        }}
        options={options}
      />
      {mode === OTHER && <Input label={t('isolation.select.otherLabel')} value={current} disabled={disabled} autoComplete="off" onChange={(event) => onChange(event.target.value.trim() === '' ? undefined : event.target.value.trim())} />}
    </div>
  );
}
