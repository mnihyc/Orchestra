/** Sign-in and keys: write-only keys and endpoint headers are sealed to the selected keeper;
 * native sign-ins use the harness's methods and the keeper's live home/policy permission.
 */
import { useMutation, useQuery, useQueryClient } from '@tanstack/react-query';
import { useMemo, useState } from 'react';
import { useTranslation } from 'react-i18next';

import { ApiError } from '@/api/client';
import { providerKey, providerSetupQuery, setProviderKey, setProviderSecret, updateProvider } from '@/api/resources';
import { ProviderGuideLink } from '@/features/help/ProviderGuideLink';
import { type EndpointDraft, EndpointsEditor, draftOf, endpointFault, upstreamOf } from '@/features/resources/providers/Endpoints';
import type { Provider, ProviderSetupProtocol } from '@/generated/api-types';
import { Section } from '@/shell/Page';
import { Button } from '@/ui/Button';
import { adminSpec, type AdminActSpec } from '@/ui/AdminAct';
import { ErrorAlert } from '@/ui/ErrorState';
import { PasswordInput } from '@/ui/Input';
import { toast } from '@/ui/Toast';
import { HelpHint } from '@/ui/HelpHint';
import { ProviderLogin } from './Login';

/** A secret to write: its own field and its own button, so that one is changed without the others. */
function SecretRow({ label, onWrite, button, pendingLabel, admin }: { label: string; onWrite: (value: string) => Promise<unknown>; button: string; pendingLabel?:string; admin?: AdminActSpec }) {
  const { t } = useTranslation();
  const [value, setValue] = useState('');
  const write = useMutation({
    mutationFn: () => onWrite(value),
    onSuccess: () => {
      setValue('');
      toast({ tone: 'success', title: t('providers.key.written', { name: label }) });
    },
  });
  return (
    <form
      className="flex flex-col gap-2"
      onSubmit={(event) => {
        event.preventDefault();
        if (value !== '') write.mutate();
      }}
    >
      <div className="flex flex-wrap items-end gap-2">
        <PasswordInput label={label} value={value} onChange={(event) => setValue(event.target.value)} fieldClassName="min-w-0 flex-1 basis-[15rem]" autoComplete="off" disabled={write.isPending} />
        <Button admin={admin} type="submit" variant="primary" pending={write.isPending} blocked={value === ''}>
          {write.isPending ? pendingLabel ?? button : button}
        </Button>
      </div>
      {write.error !== null && <ErrorAlert error={write.error} />}
    </form>
  );
}

function ApiKey({ provider }: { provider: Provider }) {
  const { t } = useTranslation();
  const queryClient = useQueryClient();
  const keyMachine = provider.key_machine;
  const setup = useQuery({ ...providerSetupQuery(provider.home.id), retry: false });
  const protocols: readonly ProviderSetupProtocol[] = useMemo(
    () =>
      setup.data?.protocols ??
      provider.protocols.map((name) => ({ name, title: name, agents: [], endpoint_options: [] })),
    [setup.data, provider.protocols],
  );
  const httpAllowed = setup.data?.http_allowed ?? false;
  const [drafts, setDrafts] = useState<EndpointDraft[]>(() => provider.upstreams.map((upstream) => draftOf(upstream, protocols.find((protocol) => protocol.name === upstream.protocol))));
  const [faults, setFaults] = useState<Record<string, string>>({});
  // What the rows say of themselves, without the identity each was given: the endpoints are changed when this differs from what the provider holds.
  const shape = (rows: readonly EndpointDraft[]) => JSON.stringify(rows.map((row) => [row.protocol, row.baseUrl, row.options]));
  const held = provider.upstreams.map((upstream) => draftOf(upstream, protocols.find((protocol) => protocol.name === upstream.protocol)));
  const changed = shape(drafts) !== shape(held);

  const save = useMutation({
    mutationFn: async () => {
      const made = drafts.map((draft) => upstreamOf(draft, protocols.find((protocol) => protocol.name === draft.protocol)));
      const reply = await updateProvider(provider.id, { expected_revision: provider.revision, upstreams: made.map((one) => one.upstream) });
      // What was typed for a secret header is written beside the endpoint and never into it.
      for (const one of made) for (const [name, value] of Object.entries(one.secrets)) await setProviderSecret(provider.id, name, value);
      return reply;
    },
    onSuccess: (reply) => {
      toast({ tone: 'success', title: t('providers.key.endpointsSaved'), ...(reply.warning === undefined || reply.warning === null ? {} : { description: reply.warning }) });
    },
    // Somebody changed it first: it is read again, and what is shown from then on is theirs.
    onError: (error) => {
      if (!(error instanceof ApiError && error.is('conflict.stale_revision'))) return;
      void queryClient.invalidateQueries({ queryKey: providerKey(provider.id) });
      toast({ tone: 'info', title: t('providers.key.changed') });
    },
  });

  const submit = () => {
    const found: Record<string, string> = {};
    for (const draft of drafts) {
      const fault = endpointFault(draft, httpAllowed, t);
      if (fault !== null) found[draft.id] = fault;
    }
    setFaults(found);
    if (Object.keys(found).length === 0) save.mutate();
  };
  const resourceSecrets = provider.secrets_set.filter((name) => !name.startsWith('runtime:') && !name.startsWith('source:'));

  return (
    <div className="flex flex-col gap-6">
      <Section id="provider-key" title={t('providers.key.title')} action={<HelpHint term={t('providers.key.title')} what={<ProviderGuideLink guide="provider-credential"/>}/>} lead={keyMachine === null ? t('providers.key.heldByOrch') : t('providers.key.onMachine', { machine: keyMachine.name })}>
        <SecretRow
          admin={adminSpec(provider, 'provider.manage')}
          label={t('providers.key.value')}
          button={t('providers.key.save')}
          pendingLabel={t('providerNativeLogin.readingModels')}
          onWrite={async (value) => {
            const saved=await setProviderKey(provider.id,value);
            // The reply includes the model source read; no readiness or model name is inferred.
            queryClient.setQueryData(providerKey(provider.id),saved);
          }}
        />
        {!provider.key_set && <p className="text-sm text-text-muted">{t('providers.key.notSet')}</p>}
      </Section>

      {resourceSecrets.length > 0 && (
        <Section id="provider-secrets" title={t('providers.key.secrets')} lead={t('providers.key.secretsLead')}>
          <ul className="flex flex-col gap-3">
            {resourceSecrets.map((name) => (
              <li key={name}>
                <SecretRow admin={adminSpec(provider, 'provider.manage')} label={name} button={t('providers.key.replaceSecret', { name })} onWrite={(value) => setProviderSecret(provider.id, name, value)} />
              </li>
            ))}
          </ul>
        </Section>
      )}

      <Section id="provider-endpoints" title={t('providers.key.endpoints')} lead={t('providers.key.endpointsLead')}>
        <EndpointsEditor value={drafts} onChange={setDrafts} protocols={protocols} httpAllowed={httpAllowed} errors={faults} disabled={save.isPending} />
        <div>
          <Button admin={adminSpec(provider, 'provider.manage')} variant="primary" pending={save.isPending} blocked={!changed} onClick={submit}>
            {t('providers.key.saveEndpoints')}
          </Button>
        </div>
        {save.error !== null && <ErrorAlert error={save.error} />}
      </Section>
    </div>
  );
}


export function ProviderKey({ provider }: { provider: Provider }) {
  // The tab is the provider's own: a key where it has one, a sign-in where it has a login.
  return provider.kind === 'api' ? <ApiKey key={provider.revision} provider={provider} /> : <ProviderLogin key={provider.id} provider={provider} />;
}
