/** The identity providers people may sign in through: the list with each one's callback address to copy, and the form that adds or changes one.
 *
 * A provider is told by its key, its issuer and the client it has made for this system. What the system
 * needs from it, and what a person who comes through it gets, are set here: the claims that give a
 * name and an address, the address domains it may bring, whether people are made on their first
 * sign-in, and the org and role they start with. The client secret is written and never read back.
 * Test looks at the provider itself, its description of itself and its signing keys, and says what it
 * found or what went wrong; it signs nobody in.
 */
import { useMutation, useQuery } from '@tanstack/react-query';
import { Ellipsis, FlaskConical, Pencil, Plus, ShieldOff, Trash } from 'lucide-react';
import { useEffect, useRef, useState } from 'react';
import { useTranslation } from 'react-i18next';
import { choiceOption } from '@/i18n/choices';

import {
  createIdentityProvider,
  deleteIdentityProvider,
  identityProvidersQuery,
  testIdentityProvider,
  updateIdentityProvider,
} from '@/api/admin';
import { ApiError } from '@/api/client';
import type { IdentityProvider, IdentityProviderInput, Role } from '@/generated/api-types';
import { useForm } from '@/lib/forms';
import { StatusTag } from '@/features/admin/StatusTag';
import { OrganizationChoice } from '@/features/admin/OrganizationChoice';
import { Button } from '@/ui/Button';
import { Checkbox } from '@/ui/Checkbox';
import { ConfirmDialog } from '@/ui/ConfirmDialog';
import { CopyButton } from '@/ui/CopyButton';
import { Dialog } from '@/ui/Dialog';
import { EmptyState } from '@/ui/EmptyState';
import { ErrorAlert, ErrorState } from '@/ui/ErrorState';
import { IconButton } from '@/ui/IconButton';
import { InlineAlert } from '@/ui/InlineAlert';
import { Input } from '@/ui/Input';
import { Menu } from '@/ui/Menu';
import { Select } from '@/ui/Select';
import { SkeletonRows } from '@/ui/Skeleton';
import { Switch } from '@/ui/Switch';
import { type Column, Table } from '@/ui/Table';
import { toast } from '@/ui/Toast';

const ROLES: readonly Role[] = ['viewer', 'interactor', 'developer', 'admin'];
const KEY = /^[a-z][a-z0-9-]{1,31}$/;
const NO_ORG = '-';
/** An issuer: an https address, or an http one on this very machine for a provider in development; no query or fragment. */
const ISSUER = /^(https:\/\/[^?#\s]+|http:\/\/(localhost|127\.0\.0\.1|\[::1\])(:\d+)?(\/[^?#\s]*)?)$/;

/** A list written in a field, as the list it is: split at commas and white space, with nothing empty in it. */
const listOf = (text: string): string[] => text.split(/[\s,]+/).filter((part) => part !== '');

interface FormProps {
  provider: IdentityProvider | null;
  onClose: () => void;
}

function ProviderForm({ provider, onClose }: FormProps) {
  const { t } = useTranslation();
  const form = useForm(
    {
      key: provider?.key ?? '',
      name: provider?.name ?? '',
      issuer: provider?.issuer ?? '',
      client_id: provider?.client_id ?? '',
      client_secret: '',
      scopes: (provider?.scopes ?? ['openid', 'email', 'profile']).join(' '),
      claim_subject: provider?.claim_subject ?? 'sub',
      claim_email: provider?.claim_email ?? 'email',
      claim_name: provider?.claim_name ?? 'name',
      claim_username: provider?.claim_username ?? 'preferred_username',
      domains: (provider?.allowed_email_domains ?? []).join(' '),
    },
    {
      key: (value) => (KEY.test(value) ? null : t('admin.idp.keyRule')),
      issuer: (value) => (ISSUER.test(value.trim()) ? null : t('admin.idp.issuerRule')),
      client_id: (value) => (value.trim() === '' ? t('admin.idp.clientRule') : null),
      client_secret: (value) => (provider === null && value === '' ? t('admin.idp.secretRule') : null),
    },
  );
  const [autoRegister, setAutoRegister] = useState(provider?.auto_register ?? false);
  const [enabled, setEnabled] = useState(provider?.enabled ?? true);
  const [org, setOrg] = useState(provider?.default_org_id ?? NO_ORG);
  const [role, setRole] = useState<Role>(provider?.default_role ?? 'viewer');
  const [placed, setPlaced] = useState(false);

  const save = useMutation({
    mutationFn: (values: typeof form.values) => {
      const body: IdentityProviderInput = {
        key: values.key,
        name: values.name.trim() === '' ? values.key : values.name.trim(),
        issuer: values.issuer.trim(),
        client_id: values.client_id.trim(),
        ...(values.client_secret === '' ? {} : { client_secret: values.client_secret }),
        scopes: listOf(values.scopes),
        claim_subject: values.claim_subject.trim(),
        claim_email: values.claim_email.trim(),
        claim_name: values.claim_name.trim(),
        claim_username: values.claim_username.trim(),
        allowed_email_domains: listOf(values.domains),
        auto_register: autoRegister,
        default_org_id: org === NO_ORG ? null : org,
        default_role: org === NO_ORG ? null : role,
        enabled,
      };
      return provider === null ? createIdentityProvider(body) : updateIdentityProvider(provider.id, { ...body, expected_revision: provider.revision });
    },
    onSuccess: (saved) => {
      toast({ tone: 'success', title: t(provider === null ? 'admin.idp.added' : 'admin.idp.saved', { name: saved.name }) });
      onClose();
    },
    onError: (error) => setPlaced(form.refuse(error)),
  });

  const stale = save.error instanceof ApiError && save.error.is('conflict.stale_revision');

  return (
    <Dialog
      open
      size="lg"
      onOpenChange={(open) => !open && !save.isPending && onClose()}
      title={provider === null ? t('admin.idp.addTitle') : t('admin.idp.editTitle', { name: provider.name })}
      description={t('admin.idp.formLead')}
      dismissible={!save.isPending}
      footer={
        <>
          <Button disabled={save.isPending} onClick={onClose}>
            {t('common.cancel')}
          </Button>
          <Button variant="primary" type="submit" form="admin-idp-form" pending={save.isPending}>
            {provider === null ? t('admin.idp.add') : t('admin.idp.save')}
          </Button>
        </>
      }
    >
      <form id="admin-idp-form" className="flex flex-col gap-4 py-2" noValidate onSubmit={form.submit((values) => save.mutate(values))}>
        <div className="grid gap-4 sm:grid-cols-2">
          <Input label={t('admin.idp.key')} hint={t('admin.idp.keyHint')} autoComplete="off" autoCapitalize="none" mono {...form.field('key')} />
          <Input label={t('admin.idp.nameField')} optional autoComplete="off" {...form.field('name')} />
        </div>
        <Input label={t('admin.idp.issuer')} hint={t('admin.idp.issuerHint')} autoComplete="off" autoCapitalize="none" mono {...form.field('issuer')} />
        <div className="grid gap-4 sm:grid-cols-2">
          <Input label={t('admin.idp.clientId')} autoComplete="off" mono {...form.field('client_id')} />
          <Input
            label={t('admin.idp.secret')}
            type="password"
            autoComplete="new-password"
            hint={provider?.client_secret_set === true ? t('admin.idp.secretKept') : t('admin.idp.secretHint')}
            {...form.field('client_secret')}
          />
        </div>
        <Input label={t('admin.idp.scopes')} hint={t('admin.idp.scopesHint')} autoComplete="off" mono {...form.field('scopes')} />
        <fieldset className="flex flex-col gap-3 rounded-lg border border-border p-3">
          <legend className="px-1 text-sm font-medium text-text">{t('admin.idp.claims')}</legend>
          <p className="text-sm text-text-muted">{t('admin.idp.claimsHint')}</p>
          <div className="grid gap-3 sm:grid-cols-2">
            <Input label={t('admin.idp.claimSubject')} autoComplete="off" mono {...form.field('claim_subject')} />
            <Input label={t('admin.idp.claimEmail')} autoComplete="off" mono {...form.field('claim_email')} />
            <Input label={t('admin.idp.claimName')} autoComplete="off" mono {...form.field('claim_name')} />
            <Input label={t('admin.idp.claimUsername')} autoComplete="off" mono {...form.field('claim_username')} />
          </div>
        </fieldset>
        <fieldset className="flex flex-col gap-3 rounded-lg border border-border p-3">
          <legend className="px-1 text-sm font-medium text-text">{t('admin.idp.newPeople')}</legend>
          <Input label={t('admin.idp.domains')} optional hint={t('admin.idp.domainsHint')} autoComplete="off" mono {...form.field('domains')} />
          <Checkbox label={t('admin.idp.autoRegister')} hint={t('admin.idp.autoRegisterHint')} checked={autoRegister} onCheckedChange={setAutoRegister} />
          <div className="grid gap-3 sm:grid-cols-2">
            <OrganizationChoice value={org === NO_ORG ? null : org} onChange={(next) => setOrg(next ?? NO_ORG)} disabled={save.isPending} />
            <Select
              label={t('admin.idp.defaultRole')}
              value={role}
              disabled={org === NO_ORG}
              onValueChange={setRole}
              options={ROLES.map((one) => ({ value: one, ...choiceOption(t, `accessRole.${one}`) }))}
            />
          </div>
        </fieldset>
        <Switch label={t('admin.idp.enabled')} hint={t('admin.idp.enabledHint')} checked={enabled} onCheckedChange={setEnabled} />
        {save.error !== null && !placed && <ErrorAlert error={save.error} />}
        {stale && <p className="text-sm text-text-muted">{t('admin.idp.stale')}</p>}
      </form>
    </Dialog>
  );
}

/** What a test found in the issuer's description of itself: the issuer it names and how many keys it signs with. */
function foundIn(metadata: unknown): { issuer: string | null; keys: number } {
  if (typeof metadata !== 'object' || metadata === null) return { issuer: null, keys: 0 };
  const described = metadata as Record<string, unknown>;
  return {
    issuer: typeof described.issuer === 'string' ? described.issuer : null,
    keys: typeof described.signing_keys === 'number' ? described.signing_keys : 0,
  };
}

/** The test of one provider, asked when the dialog opens and again on demand: what was found, or the words of what went wrong. */
function ProviderTest({ provider, onClose }: { provider: IdentityProvider; onClose: () => void }) {
  const { t } = useTranslation();
  const test = useMutation({ mutationFn: () => testIdentityProvider(provider.id) });
  const asked = useRef(false);
  const { mutate } = test;
  useEffect(() => {
    // Asked once when the dialog opens, however often the screen is drawn.
    if (asked.current) return;
    asked.current = true;
    mutate();
  }, [mutate]);
  const reply = test.isPending ? undefined : test.data;
  const found = foundIn(reply?.issuer_metadata);
  return (
    <Dialog
      open
      onOpenChange={(open) => !open && onClose()}
      title={t('admin.idp.testTitle', { name: provider.name })}
      description={t('admin.idp.testLead')}
      footer={
        <>
          <Button onClick={onClose}>{t('common.close')}</Button>
          <Button variant="primary" pending={test.isPending} onClick={() => test.mutate()}>
            {t('admin.idp.testAgain')}
          </Button>
        </>
      }
    >
      <div className="flex flex-col gap-3 py-2">
        {test.isPending && (
          <p role="status" className="text-sm text-text-muted">
            {t('admin.idp.testing')}
          </p>
        )}
        {test.error !== null && <ErrorAlert error={test.error} />}
        {reply?.ok === true && (
          <InlineAlert tone="success">
            {t('admin.idp.found', { issuer: found.issuer ?? provider.issuer, count: found.keys })}
          </InlineAlert>
        )}
        {reply?.ok === false && (
          <InlineAlert tone="danger" title={t('admin.idp.notFound')}>
            <span className="wrap-anywhere">{reply.error ?? t('admin.idp.notFoundUnsaid')}</span>
          </InlineAlert>
        )}
      </div>
    </Dialog>
  );
}

export function IdentityProviders() {
  const { t } = useTranslation();
  const providers = useQuery(identityProvidersQuery);
  const [editing, setEditing] = useState<IdentityProvider | 'new' | null>(null);
  const [removing, setRemoving] = useState<IdentityProvider | null>(null);
  const [testing, setTesting] = useState<IdentityProvider | null>(null);

  const remove = useMutation({
    mutationFn: (provider: IdentityProvider) => deleteIdentityProvider(provider.id),
    onSuccess: (_none, provider) => {
      toast({ tone: 'success', title: t('admin.idp.removed', { name: provider.name }) });
      setRemoving(null);
    },
  });

  const columns: Column<IdentityProvider>[] = [
    {
      id: 'name',
      header: t('admin.idp.name'),
      cell: (provider) => (
        <span className="flex min-w-0 flex-col gap-1">
          <span className="flex flex-wrap items-center gap-2">
            <span className="font-medium text-text">{provider.name}</span>
            <span className="font-mono text-sm text-text-subtle">{provider.key}</span>
            {provider.enabled ? <StatusTag tone="accent">{t('admin.idp.on')}</StatusTag> : <StatusTag>{t('admin.idp.off')}</StatusTag>}
          </span>
          <span className="flex items-center gap-1 text-sm text-text-muted">
            <span className="min-w-0 font-mono wrap-anywhere">{provider.callback_url}</span>
            <CopyButton value={provider.callback_url} label={t('admin.idp.copyCallback', { name: provider.name })} />
          </span>
        </span>
      ),
    },
    { id: 'issuer', header: t('admin.idp.issuer'), from: 'md', cell: (provider) => <span className="font-mono text-sm wrap-anywhere">{provider.issuer}</span> },
    {
      id: 'people',
      header: t('admin.idp.newPeople'),
      from: 'lg',
      cell: (provider) => (provider.auto_register ? t('admin.idp.madeOnFirst') : t('admin.idp.knownOnly')),
    },
    {
      id: 'menu',
      header: t('admin.users.actions'),
      hideHeader: true,
      align: 'end',
      cell: (provider) => (
        <Menu
          label={t('admin.idp.menuOf', { name: provider.name })}
          align="end"
          entries={[
            { id: 'edit', label: t('admin.idp.edit'), icon: <Pencil aria-hidden />, onSelect: () => setEditing(provider) },
            { id: 'test', label: t('admin.idp.test'), icon: <FlaskConical aria-hidden />, onSelect: () => setTesting(provider) },
            { type: 'separator', id: 'apart' },
            { id: 'remove', label: t('admin.idp.remove'), icon: <Trash aria-hidden />, danger: true, onSelect: () => setRemoving(provider) },
          ]}
          trigger={<IconButton size="sm" label={t('admin.idp.menuOf', { name: provider.name })} icon={<Ellipsis aria-hidden />} />}
        />
      ),
    },
  ];

  return (
    <div className="flex flex-col gap-3">
      <div className="flex flex-wrap items-center gap-3">
        <p className="min-w-0 flex-1 text-sm text-text-muted">{t('admin.idp.callbackNote')}</p>
        <Button variant="primary" icon={<Plus aria-hidden />} onClick={() => setEditing('new')}>
          {t('admin.idp.addTitle')}
        </Button>
      </div>
      {providers.isPending ? (
        <SkeletonRows rows={2} />
      ) : providers.isError ? (
        <ErrorState compact error={providers.error} retrying={providers.isRefetching} onRetry={() => void providers.refetch()} />
      ) : providers.data.length === 0 ? (
        <EmptyState compact icon={<ShieldOff aria-hidden />} title={t('admin.idp.none')} description={t('admin.idp.noneHint')} />
      ) : (
        <Table label={t('admin.signIn.providers.title')} columns={columns} rows={providers.data} rowKey={(provider) => provider.id} />
      )}
      {editing !== null && <ProviderForm key={editing === 'new' ? 'new' : editing.id} provider={editing === 'new' ? null : editing} onClose={() => setEditing(null)} />}
      {testing !== null && <ProviderTest key={testing.id} provider={testing} onClose={() => setTesting(null)} />}
      <ConfirmDialog
        open={removing !== null}
        onOpenChange={(open) => !open && setRemoving(null)}
        title={t('admin.idp.removeTitle', { name: removing?.name ?? '' })}
        confirmLabel={t('admin.idp.removeConfirm', { name: removing?.name ?? '' })}
        danger
        pending={remove.isPending}
        error={remove.error ?? undefined}
        onConfirm={() => removing !== null && remove.mutate(removing)}
      >
        {t('admin.idp.removeBody')}
      </ConfirmDialog>
    </div>
  );
}
