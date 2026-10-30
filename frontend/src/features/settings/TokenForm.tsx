/** The form that makes an API token, for its person or for a service identity: narrow as it opens, its exact permissions shown and sent, a subtree and an end, and the maker's fresh proof in the same act.
 *
 * It opens on Only what I choose with Can view, the narrowest of the page's presets; Can interact and
 * Can manage expand the same way through the permission catalogue, and By area… ticks permissions one
 * by one. Everything I can do is a separate choice that sends `*` and says what it means. The proof is
 * the password with the code of a second step where the maker has one; a maker without a password
 * proves their linked sign-in again, and comes back to the form as they left it, the one-use proof
 * taken from the address. A token that was made already (its answer was lost) is said as such, with
 * no secret: revoke it and make another.
 */
import { useQuery } from '@tanstack/react-query';
import { useMemo, useState } from 'react';
import { useTranslation } from 'react-i18next';
import { choiceOption } from '@/i18n/choices';

import { permissionsQuery } from '@/api/access';
import { meQuery } from '@/api/auth';
import { ApiError } from '@/api/client';
import { sendEmailCode } from '@/api/users';
import { type FreshPurpose, proveAgain, takeDraft, takeFreshProof, takeFreshProofError } from '@/features/auth/freshProof';
import { PlacePicker, type PickedPlace } from '@/features/settings/PlacePicker';
import type { ApiTokenCreateBody, PermissionDef, SensitiveProof, TokenCreated } from '@/generated/api-types';
import { useNow } from '@/lib/time';
import { Button } from '@/ui/Button';
import { Checkbox } from '@/ui/Checkbox';
import { CopyButton } from '@/ui/CopyButton';
import { ErrorAlert, ErrorState } from '@/ui/ErrorState';
import { InlineAlert } from '@/ui/InlineAlert';
import { Input, PasswordInput } from '@/ui/Input';
import { DateField } from '@/ui/DateField';
import { RadioGroup, type RadioOption } from '@/ui/RadioGroup';
import { ChoiceField } from '@/ui/ChoiceField';
import { Select } from '@/ui/Select';
import { SkeletonRows } from '@/ui/Skeleton';

/** The scope that means no narrowing: the token may do whatever its person may. */
export const EVERYTHING = '*';

/** How long a token can be made to last, in days, besides a day of the maker's choosing and never. */
const LIFETIMES_DAYS = [7, 30, 90, 365] as const;

export type Lifetime = 'never' | `${(typeof LIFETIMES_DAYS)[number]}` | 'date';

const DAY_MS = 86_400_000;

/** The moment a token made now ends: the end of the chosen day for a date, `null` for never. */
export function endOfToken(lifetime: Lifetime, date: string, now: number): string | null {
  if (lifetime === 'never') return null;
  if (lifetime === 'date') {
    const end = Date.parse(`${date}T23:59:59.000Z`);
    return Number.isNaN(end) ? null : new Date(end).toISOString();
  }
  return new Date(now + Number(lifetime) * DAY_MS).toISOString();
}

/** The permissions of the catalogue under the name of their area, areas and permissions in the order the catalogue gives them. */
export function permissionsByArea(permissions: readonly PermissionDef[]): { area: string; items: PermissionDef[] }[] {
  const areas = new Map<string, PermissionDef[]>();
  for (const permission of permissions) {
    const list = areas.get(permission.area);
    if (list === undefined) areas.set(permission.area, [permission]);
    else list.push(permission);
  }
  return [...areas].map(([area, items]) => ({ area, items }));
}

/** The page's three presets. */
export type Preset = 'view' | 'interact' | 'manage';

/** The levels each preset holds: a page's Can view, Can interact (what is in a thing) and Can manage (the thing itself). */
const PRESET_LEVELS: Record<Preset, readonly string[]> = {
  view: ['view'],
  interact: ['view', 'interact', 'edit', 'use'],
  manage: ['view', 'interact', 'edit', 'use', 'manage'],
};

/** The exact permissions a preset expands to through the catalogue, sorted: rows of one single act and owning are never in a preset. */
export function presetScopes(preset: Preset, permissions: readonly PermissionDef[]): string[] {
  return permissions
    .filter((permission) => PRESET_LEVELS[preset].includes(permission.level))
    .map((permission) => permission.name)
    .sort();
}

/** What the form holds, kept in the tab while its maker proves their sign-in elsewhere. */
export interface TokenDraft {
  name: string;
  mode: 'some' | 'everything';
  how: Preset | 'area';
  chosen: string[];
  place: PickedPlace | null;
  lifetime: Lifetime;
  date: string;
  /** The service identity the token is for, where it is one. */
  serviceAccountId?: string;
}

const BLANK: TokenDraft = { name: '', mode: 'some', how: 'view', chosen: [], place: null, lifetime: '90', date: '' };

/** Where a form's draft is kept while its maker is away. */
export const TOKEN_DRAFT = 'token';

/** What a maker came back with from proving their sign-in again: the proof, or none where the provider gave none. */
export interface Returned {
  draft: TokenDraft;
  proof: string | null;
}

/** The draft and the proof of a maker who came back from proving their sign-in again (or came back without one), taken once; `serviceAccountId` says whose token. */
export function returnedToTokenForm(): Returned | null {
  const proof = takeFreshProof();
  const failed = proof === null && takeFreshProofError() !== null;
  if (proof === null && !failed) return null;
  const draft = takeDraft(TOKEN_DRAFT) as TokenDraft | null;
  return { draft: draft ?? BLANK, proof };
}

function ScopeChoice({ permissions, chosen, onChange }: { permissions: readonly PermissionDef[]; chosen: ReadonlySet<string>; onChange: (next: Set<string>) => void }) {
  const { t } = useTranslation();
  const groups = useMemo(() => permissionsByArea(permissions), [permissions]);
  return (
    <div data-scroll-allow="choices" className="flex max-h-[16rem] flex-col gap-3 overflow-y-auto rounded-lg border border-border p-3" role="group" aria-label={t('tokens.scopes.list')}>
      {groups.map(({ area, items }) => (
        <fieldset key={area} className="flex flex-col gap-1">
          <legend className="text-xs font-medium text-text-subtle">{area}</legend>
          {items.map((permission) => (
            <Checkbox
              key={permission.name}
              label={permission.label}
              hint={permission.allows}
              checked={chosen.has(permission.name)}
              onCheckedChange={(on) => {
                const next = new Set(chosen);
                if (on) next.add(permission.name);
                else next.delete(permission.name);
                onChange(next);
              }}
            />
          ))}
        </fieldset>
      ))}
    </div>
  );
}

export interface TokenFormProps {
  /** Makes the token with the body the form gives. */
  onMake: (body: ApiTokenCreateBody) => void;
  /** The make is under way. */
  pending: boolean;
  /** Why the last make was refused. */
  error: unknown;
  /** The service identity the token is for; none for the maker's own. */
  serviceAccountId?: string;
  /** What the maker had chosen before they left to prove their sign-in, and the proof they came back with. */
  returned?: Returned | null;
  /** The form's id, which a dialog's footer submits. */
  formId: string;
}

/** Only the guarded submit calls this: the token's end is measured at the act, outside React's render scope. */
function tokenAtSubmission(body: Omit<ApiTokenCreateBody, 'expires_at'>, lifetime: Lifetime, date: string): ApiTokenCreateBody {
  const expires = endOfToken(lifetime, date, Date.now());
  return { ...body, ...(expires === null ? {} : { expires_at: expires }) };
}

/** The fields of a new token and its proof. */
export function TokenForm({ onMake, pending, error, serviceAccountId, returned = null, formId }: TokenFormProps) {
  const { t } = useTranslation();
  const now = useNow();
  const me = useQuery(meQuery);
  const catalogue = useQuery(permissionsQuery);
  const permissions = catalogue.data?.permissions ?? [];
  const start = returned?.draft ?? BLANK;
  const [name, setName] = useState(start.name);
  const [mode, setMode] = useState<'some' | 'everything'>(start.mode);
  const [how, setHow] = useState<Preset | 'area'>(start.how);
  const [chosen, setChosen] = useState<Set<string>>(new Set(start.chosen));
  const [place, setPlace] = useState<PickedPlace | null>(start.place);
  const [lifetime, setLifetime] = useState<Lifetime>(start.lifetime);
  const [date, setDate] = useState(start.date);
  const [password, setPassword] = useState('');
  const [code, setCode] = useState('');
  const [codeWay, setCodeWay] = useState<'totp' | 'email_code' | 'recovery_code'>('totp');
  const [mailed, setMailed] = useState<'idle' | 'sending' | 'sent' | 'failed'>('idle');
  const [sent, setSent] = useState(false);
  const identityProof = returned?.proof ?? null;
  // Back from the provider without a proof: the form says so, and offers to prove the sign-in again.
  const proofFailed = returned !== null && returned.proof === null;

  const scopes = mode === 'everything' ? [EVERYTHING] : how === 'area' ? [...chosen].sort() : presetScopes(how, permissions);
  const nameWrong = name.trim() === '' ? t('tokens.new.nameMissing') : undefined;
  const scopesWrong = mode === 'some' && scopes.length === 0 ? t('tokens.new.scopesMissing') : undefined;
  const dateWrong = lifetime === 'date' && Number.isNaN(Date.parse(`${date}T23:59:59.000Z`)) ? t('tokens.new.dateMissing') : undefined;
  const person = me.data;
  const hasPassword = person?.has_password ?? true;
  const app = person?.totp_enabled ?? false;
  const byMail = person?.email_code_second_step ?? false;
  const recoveryAvailable = error instanceof ApiError && error.extra('auth.second_step_required')?.methods.includes('recovery_code') === true;
  const linked = person?.identities[0];
  const proofWrong = hasPassword ? (password === '' ? t('tokens.new.passwordMissing') : undefined) : identityProof === null ? t('tokens.new.proveFirst') : undefined;
  const wrong = nameWrong ?? scopesWrong ?? dateWrong ?? proofWrong;

  const draft = (): TokenDraft => ({
    name,
    mode,
    how,
    chosen: [...chosen],
    place,
    lifetime,
    date,
    ...(serviceAccountId === undefined ? {} : { serviceAccountId }),
  });
  const proveSignIn = () => {
    const purpose: FreshPurpose = 'token_create';
    if (linked !== undefined) proveAgain(linked.idp_key, purpose, TOKEN_DRAFT, draft(), serviceAccountId);
  };
  const mailCode = () => {
    setCodeWay('email_code');
    setMailed('sending');
    sendEmailCode({}).then(
      () => setMailed('sent'),
      () => setMailed('failed'),
    );
  };
  const submit = () => {
    setSent(true);
    if (wrong !== undefined || pending) return;
    const typed = code.trim();
    const proof: SensitiveProof = {
      ...(hasPassword ? { password } : identityProof === null ? {} : { identity_proof: identityProof }),
      ...(typed === '' ? {} : codeWay === 'recovery_code' ? { recovery_code: typed } : codeWay === 'totp' && app ? { totp: typed } : { email_code: typed }),
    };
    onMake(tokenAtSubmission({
      name: name.trim(),
      scopes,
      ...(place === null ? {} : { node_id: place.id }),
      proof,
    }, lifetime, date));
  };
  const limit = error instanceof ApiError && error.is('token.lifetime_exceeded') ? (error.problem.max_days as number | undefined) : undefined;
  const scopeOptions: RadioOption<'some' | 'everything'>[] = [
    { value: 'some', ...choiceOption(t, 'tokenScope.some') },
    { value: 'everything', ...choiceOption(t, 'tokenScope.everything') },
  ];

  if (catalogue.isPending || me.isPending) return <SkeletonRows rows={4} />;
  if (catalogue.isError) return <ErrorState compact error={catalogue.error} retrying={catalogue.isRefetching} onRetry={() => void catalogue.refetch()} />;
  return (
    <form
      id={formId}
      noValidate
      className="flex flex-col gap-4 py-2"
      onSubmit={(event) => {
        event.preventDefault();
        submit();
      }}
    >
      <Input
        label={t('tokens.new.name')}
        hint={t('tokens.new.nameHint')}
        value={name}
        error={sent ? nameWrong : undefined}
        onChange={(event) => setName(event.target.value)}
        autoComplete="off"
      />
      <ChoiceField label={t('tokens.new.mayDo')} options={scopeOptions}><RadioGroup
        label={t('tokens.new.mayDo')}
        value={mode}
        onValueChange={setMode}
        options={scopeOptions}
      /></ChoiceField>
      {mode === 'some' && (
        <div className="flex flex-col gap-2">
          <RadioGroup
            label={t('tokens.presets.label')}
            value={how}
            onValueChange={setHow}
            options={[
              { value: 'view', ...choiceOption(t, 'tokenPreset.view') },
              { value: 'interact', ...choiceOption(t, 'tokenPreset.interact') },
              { value: 'manage', ...choiceOption(t, 'tokenPreset.manage') },
              { value: 'area', ...choiceOption(t, 'tokenPreset.area') },
            ]}
          />
          {how === 'area' && <ScopeChoice permissions={permissions} chosen={chosen} onChange={setChosen} />}
          {sent && scopesWrong !== undefined && <p className="text-sm text-danger-text">{scopesWrong}</p>}
          {scopes.length > 0 && (
            <details className="text-sm text-text-muted">
              <summary className="cursor-default rounded-md select-none hover:text-text">{t('tokens.scopes.exact', { count: scopes.length })}</summary>
              <ul className="mt-1 flex flex-col gap-1 font-mono text-xs">
                {scopes.map((scope) => (
                  <li key={scope}>{scope}</li>
                ))}
              </ul>
            </details>
          )}
        </div>
      )}
      {mode === 'everything' && <InlineAlert tone="warning">{t('tokens.scopes.everythingWarn')}</InlineAlert>}
      <PlacePicker label={t('tokens.new.where')} hint={t('tokens.new.whereHint')} value={place} onChange={setPlace} />
      <div className="grid grid-cols-1 gap-4 sm:grid-cols-2">
        <Select
          label={t('tokens.new.lasts')}
          value={lifetime}
          onValueChange={setLifetime}
          options={[
            ...LIFETIMES_DAYS.map((days) => ({ value: String(days) as Lifetime, ...choiceOption(t, 'tokenLifetime.days', { duration: t('tokens.lifetime.days', { count: days }) }) })),
            { value: 'date', ...choiceOption(t, 'tokenLifetime.date'), description: t('tokens.lifetime.date') },
            { value: 'never', ...choiceOption(t, 'tokenLifetime.never') },
          ]}
        />
        {lifetime === 'date' && (
          <DateField
            label={t('tokens.new.endsOn')}
            value={date}
            min={new Date(now).toISOString().slice(0, 10)}
            error={sent ? dateWrong : undefined}
            onValueChange={setDate}
          />
        )}
      </div>
      {lifetime === 'never' && <InlineAlert tone="warning">{t('tokens.new.neverWarn')}</InlineAlert>}
      {limit !== undefined && <InlineAlert tone="warning">{t('tokens.new.tooLong', { count: limit })}</InlineAlert>}
      <fieldset className="flex flex-col gap-3 rounded-lg border border-border p-3">
        <legend className="px-1 text-sm font-medium text-text">{t('tokens.new.proofTitle')}</legend>
        {hasPassword ? (
          <PasswordInput
            label={t('tokens.new.password')}
            hint={t('tokens.new.passwordHint')}
            value={password}
            error={sent && password === '' ? proofWrong : undefined}
            onChange={(event) => setPassword(event.target.value)}
            autoComplete="current-password"
          />
        ) : identityProof === null ? (
          <div className="flex flex-col gap-2">
            {proofFailed && <InlineAlert tone="warning">{t('tokens.new.proveFailed')}</InlineAlert>}
            <p className="text-sm text-text-muted">
              {linked === undefined ? t('tokens.new.proveNone') : t('tokens.new.proveLead', { name: linked.idp_name })}
            </p>
            <div>
              <Button size="sm" blocked={linked === undefined} onClick={proveSignIn}>
                {t('tokens.new.prove')}
              </Button>
            </div>
            {sent && proofWrong !== undefined && <p className="text-sm text-danger-text">{proofWrong}</p>}
          </div>
        ) : (
          <InlineAlert tone="success">{t('tokens.new.proved')}</InlineAlert>
        )}
        {(app || byMail) && (
          <div className="flex flex-col gap-2">
            <Input
              label={codeWay === 'recovery_code' ? t('auth.second.recoveryLabel') : codeWay === 'totp' && app ? t('tokens.new.code') : t('tokens.new.mailedCode')}
              hint={codeWay === 'totp' && app ? t('tokens.new.codeHint') : undefined}
              value={code}
              onChange={(event) => setCode(event.target.value)}
              autoComplete="one-time-code"
              inputMode={codeWay === 'totp' && app ? 'numeric' : 'text'}
              mono
            />
            {app && (recoveryAvailable || codeWay === 'recovery_code') && <div><Button size="sm" variant="ghost" disabled={pending} onClick={() => {
              setCodeWay(codeWay === 'recovery_code' ? 'totp' : 'recovery_code'); setCode('');
            }}>{codeWay === 'recovery_code' ? t('auth.second.use.totp') : t('auth.second.use.recovery_code')}</Button></div>}
            {byMail && (
              <div>
                <Button size="sm" variant="ghost" pending={mailed === 'sending'} onClick={mailCode}>
                  {app ? t('secondStep.mailInstead') : t('secondStep.mailMe')}
                </Button>
              </div>
            )}
            {mailed === 'sent' && <InlineAlert tone="success">{t('secondStep.mailSent')}</InlineAlert>}
          </div>
        )}
      </fieldset>
      {error !== null && error !== undefined && <ErrorAlert error={error} />}
    </form>
  );
}

/** A token just made, shown once; or, where the same creation was answered again, that it was made and has no secret to give. */
export function TokenMade({ made }: { made: TokenCreated }) {
  const { t } = useTranslation();
  if (made.token === null) {
    return <InlineAlert tone="warning" title={t('tokens.new.alreadyTitle', { name: made.info.name })}>{t('tokens.new.already')}</InlineAlert>;
  }
  return (
    <div className="flex flex-col gap-4 py-2">
      <InlineAlert tone="warning">{t('tokens.new.onlyOnce')}</InlineAlert>
      <div className="flex items-end gap-2">
        <Input
          label={t('tokens.new.secret', { name: made.info.name })}
          value={made.token}
          readOnly
          mono
          fieldClassName="flex-1"
          onFocus={(event) => event.currentTarget.select()}
        />
        <CopyButton value={made.token} label={t('tokens.new.copy')} className="mb-1" />
      </div>
    </div>
  );
}
