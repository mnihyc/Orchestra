/** The form that makes or changes a channel or a destination from the fields its kind declares: its name, each field with its sentence, and what is written and never shown.
 *
 * What is asked is declared by the kind of channel (the mail server, the bot, the webhook, anything
 * installed), so this draws whatever it is given and names none of them. A secret field is a hidden
 * input, and where it has a value already the form says that leaving it empty keeps it. What the server
 * finds wrong with a field is said at that field, and what the far side refuses is said under the form
 * in the far side's own words.
 */
import { type ReactNode, useMemo, useState } from 'react';
import { useTranslation } from 'react-i18next';

import { ApiError } from '@/api/client';
import { configOf, fieldsForEdit, missingFields, secretsOf, startingValues } from '@/features/settings/notifierFields';
import type { Notifier } from '@/generated/api-types';
import { Button } from '@/ui/Button';
import { Dialog } from '@/ui/Dialog';
import { ErrorAlert } from '@/ui/ErrorState';
import { FieldForm, type FormValue, type FormValues } from '@/ui/FieldForm';
import { Input } from '@/ui/Input';
import { InlineAlert } from '@/ui/InlineAlert';
import { ChoiceLocator } from '@/ui/ChoiceLocator';

/** What a form gives back: the kind, the name, the values of the fields that are not secrets, and the secrets that were typed. */
export interface NotifierFormValue {
  kind: string;
  name: string;
  config: Record<string, unknown>;
  secrets: Record<string, string>;
}

export interface NotifierDialogProps {
  title: string;
  lead?: ReactNode;
  /** The kinds that can be chosen. */
  notifiers: readonly Notifier[];
  /** When a channel is changed: its kind, which stays as it is, and what it holds now. */
  existing?: { kind: string; name: string; config: unknown; secretsSet: readonly string[] };
  submitLabel: string;
  pending: boolean;
  error: unknown;
  /** What a kind of channel does not have: for a destination, which kinds it carries and at what level. */
  extra?: ReactNode;
  /** What the extra part still needs before the form can be sent: said once the person tries, and nothing is sent until it is gone. */
  hold?: string | null;
  onSubmit: (value: NotifierFormValue) => void;
  onClose: () => void;
}

/** The members of a request the server names, down to the field of the form each is about: `config.url` is the field `url`. */
function faultsByField(error: unknown): Record<string, string> {
  const found: Record<string, string> = {};
  if (!(error instanceof ApiError)) return found;
  for (const fault of error.faults()) {
    const name = fault.path.replace(/^(config|secrets)\./, '');
    if (!(name in found)) found[name] = fault.message;
  }
  return found;
}

export function NotifierDialog({ title, lead, notifiers, existing, submitLabel, pending, error, extra, hold = null, onSubmit, onClose }: NotifierDialogProps) {
  const { t } = useTranslation();
  const [kind, setKind] = useState(existing?.kind ?? notifiers[0]?.name ?? '');
  const notifier = notifiers.find((entry) => entry.name === kind);
  const fields = useMemo(
    () => (notifier === undefined ? [] : existing === undefined ? notifier.fields : fieldsForEdit(notifier.fields, existing.secretsSet, t('settings.channels.keepSecret'))),
    [notifier, existing, t],
  );
  const [values, setValues] = useState<FormValues>(() => startingValues(notifier?.fields ?? [], existing?.config));
  const [name, setName] = useState(existing?.name ?? '');
  const [sent, setSent] = useState(false);

  const missing = notifier === undefined ? [] : missingFields(fields, values, existing?.secretsSet ?? []);
  const nameWrong = name.trim() === '' ? t('settings.channels.nameMissing') : undefined;
  const fromServer = faultsByField(error);
  const errors: Record<string, string> = { ...fromServer };
  if (sent) for (const field of missing) errors[field] = errors[field] ?? t('settings.channels.fieldMissing');
  const placed = Object.keys(fromServer).some((field) => fields.some((entry) => entry.name === field) || field === 'name');

  const submit = () => {
    setSent(true);
    if (notifier === undefined || nameWrong !== undefined || missing.length > 0 || hold !== null || pending) return;
    onSubmit({ kind, name: name.trim(), config: configOf(fields, values), secrets: secretsOf(fields, values) });
  };

  return (
    <Dialog
      open
      onOpenChange={(next) => !next && !pending && onClose()}
      title={title}
      description={lead}
      size="lg"
      dismissible={!pending}
      footer={
        <>
          <Button disabled={pending} onClick={onClose}>
            {t('common.cancel')}
          </Button>
          <Button variant="primary" pending={pending} onClick={submit}>
            {submitLabel}
          </Button>
        </>
      }
    >
      <form
        noValidate
        className="flex flex-col gap-4 py-2"
        onSubmit={(event) => {
          event.preventDefault();
          submit();
        }}
      >
        {existing === undefined && notifiers.length > 1 && (
          <ChoiceLocator
            label={t('settings.channels.kind')}
            value={kind}
            onValueChange={(next) => {
              setKind(next);
              setValues(startingValues(notifiers.find((entry) => entry.name === next)?.fields ?? [], null));
            }}
            options={notifiers.map((entry) => ({ value: entry.name, label: entry.title, description: entry.summary }))}
          />
        )}
        {notifier !== undefined && existing === undefined && notifiers.length === 1 && <p className="text-sm text-text-muted">{notifier.summary}</p>}
        <Input
          label={t('settings.channels.name')}
          hint={t('settings.channels.nameHint')}
          value={name}
          error={sent ? (fromServer.name ?? nameWrong) : fromServer.name}
          onChange={(event) => setName(event.target.value)}
          autoComplete="off"
        />
        {notifier !== undefined && (
          <FieldForm fields={fields} values={values} errors={errors} onChange={(field: string, value: FormValue) => setValues((before) => ({ ...before, [field]: value }))} />
        )}
        {extra}
        {sent && hold !== null && <InlineAlert tone="danger">{hold}</InlineAlert>}
        {error !== null && error !== undefined && !placed && <ErrorAlert error={error} />}
      </form>
    </Dialog>
  );
}
