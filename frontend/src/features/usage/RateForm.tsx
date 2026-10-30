/** The form of one row of the rate card: the model's pattern, the prices for a million tokens, the speeds that multiply them and the steps for large prompts, each fault beside its field.
 *
 * Opened to add a rate for a model that has none, to change a row of the card, or to copy an estimate to
 * the card. Nothing is sent while a field is wrong, and what is wrong is said beside it once a first try
 * was made. A row is written by putting the whole card back with the row in it, naming the revision that
 * was read, so that two people who change the card at once are told and neither loses the other's row.
 */
import { useMutation, useQueryClient } from '@tanstack/react-query';
import { Plus, Trash2 } from 'lucide-react';
import { useState } from 'react';
import { useTranslation } from 'react-i18next';

import { ApiError } from '@/api/client';
import { RATE_SETTINGS, refreshRates, setRateSetting } from '@/api/usage';
import { type Card, cardWith, cardWithout, check, type Faults, type RateDraft, emptyStep } from '@/features/usage/rates';
import { Button } from '@/ui/Button';
import { ConfirmDialog } from '@/ui/ConfirmDialog';
import { Dialog } from '@/ui/Dialog';
import { ErrorAlert } from '@/ui/ErrorState';
import { IconButton } from '@/ui/IconButton';
import { InlineAlert } from '@/ui/InlineAlert';
import { Input } from '@/ui/Input';
import { toast } from '@/ui/Toast';

/** How the form was opened: a rate added for a model that has none, a row of the card changed, or an estimate copied to the card. */
export type RateMode = 'add' | 'edit' | 'copy';

export interface RateDialogProps {
  node: { id: string | null; name: string };
  mode: RateMode;
  /** The row to start from, and the pattern of the row it replaces; none for a row that is new. */
  initial: RateDraft;
  replacing: string | null;
  /** The card as this place writes it now, and the revision of that setting where one was written. */
  card: Card;
  revision: number | null;
  onClose: () => void;
}

/** A number of the rate: credits for a million tokens. */
function Decimal({ label, value, onChange, error, optional = false, className }: { label: string; value: string; onChange: (next: string) => void; error?: string | undefined; optional?: boolean; className?: string }) {
  return <Input label={label} optional={optional} value={value} inputMode="decimal" onChange={(event) => onChange(event.target.value)} error={error} fieldClassName={className} />;
}

export function RateDialog({ node, mode, initial, replacing, card, revision, onClose }: RateDialogProps) {
  const { t } = useTranslation();
  const queryClient = useQueryClient();
  const [draft, setDraft] = useState(initial);
  const [tried, setTried] = useState(false);
  const [removing, setRemoving] = useState(false);
  const faults: Faults = check(draft, Object.keys(card), replacing);
  const shown: Faults = tried ? faults : {};
  const say = (field: string): string | undefined => {
    const fault = shown[field];
    return fault === undefined ? undefined : t(fault);
  };

  const write = useMutation({
    mutationFn: (next: Card) => setRateSetting(node.id, RATE_SETTINGS.card, next, revision),
    onSuccess: (_saved, next) => {
      toast({ tone: 'success', title: Object.keys(next).length < Object.keys(card).length ? t('usage.rates.form.removed') : t('usage.rates.form.saved') });
      onClose();
    },
    onError: () => void refreshRates(queryClient),
  });
  const stale = write.error instanceof ApiError && write.error.status === 409;

  const submit = () => {
    setTried(true);
    if (Object.keys(faults).length > 0) return;
    write.mutate(cardWith(card, draft, replacing));
  };

  const set = <Member extends keyof RateDraft>(member: Member, value: RateDraft[Member]) => setDraft((before) => ({ ...before, [member]: value }));
  const title = mode === 'add' ? t('usage.rates.form.addTitle') : mode === 'copy' ? t('usage.rates.form.copyTitle') : t('usage.rates.form.editTitle');

  return (
    <>
      <Dialog
        open
        onOpenChange={(next) => !next && onClose()}
        title={title}
        description={t('usage.rates.form.about', { name: node.name })}
        size="lg"
        dismissible={!write.isPending}
        footer={
          <>
            {mode === 'edit' && replacing !== null && (
              <Button variant="ghost" className="mr-auto" disabled={write.isPending} onClick={() => setRemoving(true)}>
                {t('usage.rates.form.remove')}
              </Button>
            )}
            <Button variant="ghost" disabled={write.isPending} onClick={onClose}>
              {t('common.cancel')}
            </Button>
            <Button variant="primary" pending={write.isPending} onClick={submit}>
              {t('usage.rates.form.save')}
            </Button>
          </>
        }
      >
        <form
          className="flex flex-col gap-6"
          onSubmit={(event) => {
            event.preventDefault();
            submit();
          }}
        >
          <Input
            label={t('usage.rates.form.pattern')}
            hint={t('usage.rates.form.patternHint')}
            value={draft.pattern}
            onChange={(event) => set('pattern', event.target.value)}
            error={say('pattern')}
            spellCheck={false}
            autoCapitalize="none"
            fieldClassName="max-w-[26rem]"
            data-autofocus
          />

          <fieldset className="flex flex-col gap-2">
            <legend className="text-sm font-medium text-text">{t('usage.rates.form.prices')}</legend>
            <p className="text-sm text-text-muted">{t('usage.rates.form.pricesHint')}</p>
            <div className="grid grid-cols-2 gap-3 sm:grid-cols-4">
              <Decimal label={t('usage.rates.col.input')} value={draft.input} onChange={(next) => set('input', next)} error={say('input')} />
              <Decimal label={t('usage.rates.col.cached')} value={draft.cached} onChange={(next) => set('cached', next)} error={say('cached')} />
              <Decimal label={t('usage.rates.col.output')} value={draft.output} onChange={(next) => set('output', next)} error={say('output')} />
              <Decimal label={t('usage.rates.col.cacheWrite')} optional value={draft.cacheWrite} onChange={(next) => set('cacheWrite', next)} error={say('cacheWrite')} />
            </div>
          </fieldset>

          <fieldset className="flex flex-col gap-2">
            <legend className="text-sm font-medium text-text">{t('usage.rates.form.speeds')}</legend>
            <p className="text-sm text-text-muted">{t('usage.rates.form.speedsHint')}</p>
            {draft.tiers.map((one, at) => (
              // A row has no name of its own: its place in the list is what it is.
              <div key={at} className="flex flex-wrap items-start gap-2">
                <Input
                  label={t('usage.rates.form.speed')}
                  value={one.tier}
                  onChange={(event) => set('tiers', draft.tiers.map((row, place) => (place === at ? { ...row, tier: event.target.value } : row)))}
                  error={say(`tier-${at}`)}
                  autoCapitalize="none"
                  fieldClassName="w-[10rem]"
                />
                <Decimal
                  label={t('usage.rates.form.multiple')}
                  value={one.multiplier}
                  onChange={(next) => set('tiers', draft.tiers.map((row, place) => (place === at ? { ...row, multiplier: next } : row)))}
                  error={say(`multiplier-${at}`)}
                  className="w-[8rem]"
                />
                <IconButton className="mt-6" label={t('usage.rates.form.removeSpeed')} icon={<Trash2 aria-hidden />} onClick={() => set('tiers', draft.tiers.filter((_, place) => place !== at))} />
              </div>
            ))}
            <Button type="button" size="sm" variant="ghost" className="w-fit" icon={<Plus aria-hidden />} onClick={() => set('tiers', [...draft.tiers, { tier: '', multiplier: '' }])}>
              {t('usage.rates.form.addSpeed')}
            </Button>
          </fieldset>

          <fieldset className="flex flex-col gap-2">
            <legend className="text-sm font-medium text-text">{t('usage.rates.form.large')}</legend>
            <p className="text-sm text-text-muted">{t('usage.rates.form.largeHint')}</p>
            {draft.steps.map((step, at) => {
              const change = (member: keyof typeof step, next: string) => set('steps', draft.steps.map((row, place) => (place === at ? { ...row, [member]: next } : row)));
              return (
                <div key={at} className="flex flex-wrap items-start gap-2">
                  <Input label={t('usage.rates.form.over')} value={step.tokens} onChange={(event) => change('tokens', event.target.value)} error={say(`step-${at}-tokens`)} fieldClassName="w-[8rem]" />
                  <Decimal label={t('usage.rates.col.input')} value={step.input} onChange={(next) => change('input', next)} error={say(`step-${at}-input`)} className="w-[7rem]" />
                  <Decimal label={t('usage.rates.col.cached')} optional value={step.cached} onChange={(next) => change('cached', next)} error={say(`step-${at}-cached`)} className="w-[7rem]" />
                  <Decimal label={t('usage.rates.col.output')} value={step.output} onChange={(next) => change('output', next)} error={say(`step-${at}-output`)} className="w-[7rem]" />
                  <Decimal label={t('usage.rates.col.cacheWrite')} optional value={step.cacheWrite} onChange={(next) => change('cacheWrite', next)} error={say(`step-${at}-cacheWrite`)} className="w-[7rem]" />
                  <IconButton className="mt-6" label={t('usage.rates.form.removeStep')} icon={<Trash2 aria-hidden />} onClick={() => set('steps', draft.steps.filter((_, place) => place !== at))} />
                </div>
              );
            })}
            <Button type="button" size="sm" variant="ghost" className="w-fit" icon={<Plus aria-hidden />} onClick={() => set('steps', [...draft.steps, emptyStep()])}>
              {t('usage.rates.form.addStep')}
            </Button>
          </fieldset>

          {stale && <InlineAlert tone="warning">{t('usage.rates.form.stale')}</InlineAlert>}
          {write.isError && !stale && <ErrorAlert error={write.error} />}
        </form>
      </Dialog>

      <ConfirmDialog
        open={removing}
        onOpenChange={setRemoving}
        title={t('usage.rates.form.removeTitle', { pattern: replacing ?? '' })}
        confirmLabel={t('usage.rates.form.removeConfirm', { pattern: replacing ?? '' })}
        danger
        pending={write.isPending}
        error={removing ? write.error : undefined}
        onConfirm={() => {
          if (replacing !== null) write.mutate(cardWithout(card, replacing));
        }}
      >
        {t('usage.rates.form.removeAbout')}
      </ConfirmDialog>
    </>
  );
}
