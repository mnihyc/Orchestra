/** The dialog that registers a provider from a machine: the login a harness already holds there, or an API key that is typed on the machine, with the command that types it.
 */
import { useMutation, useQuery } from '@tanstack/react-query';
import { Link, useNavigate } from '@tanstack/react-router';
import { KeyRound, LogIn } from 'lucide-react';
import { useMemo, useState } from 'react';
import { useTranslation } from 'react-i18next';
import { choiceOption } from '@/i18n/choices';

import { providerSetupQuery, registerProvider } from '@/api/resources';
import { CommandBox } from '@/features/resources/Facts';
import { HomePicker } from '@/features/resources/HomePicker';
import { type EndpointDraft, EndpointsEditor, endpointFault, newEndpoint, upstreamOf } from '@/features/resources/providers/Endpoints';
import { Hint } from '@/features/help/Hint';
import type { Machine, MachineProviderCreateBody, Provider, ProviderSetupProtocol } from '@/generated/api-types';
import { useActKey } from '@/lib/forms';
import { Button } from '@/ui/Button';
import { Dialog } from '@/ui/Dialog';
import { ErrorAlert } from '@/ui/ErrorState';
import { InlineAlert } from '@/ui/InlineAlert';
import { Input } from '@/ui/Input';
import { RadioGroup } from '@/ui/RadioGroup';
import { Select } from '@/ui/Select';
import { toast } from '@/ui/Toast';

type Uses = 'login' | 'key';

/** The formats an endpoint may speak when the system has not said which: the names the system always has. */
const FALLBACK_PROTOCOLS: readonly ProviderSetupProtocol[] = [
  { name: 'openai-responses', title: 'OpenAI Responses', agents: [], endpoint_options: [] },
  { name: 'openai-chat', title: 'OpenAI Chat Completions', agents: [], endpoint_options: [] },
  { name: 'anthropic-messages', title: 'Anthropic Messages', agents: [], endpoint_options: [] },
  { name: 'openai-embeddings', title: 'OpenAI Embeddings', agents: [], endpoint_options: [] },
];

export function RegisterProviderDialog({ machine, open, onClose }: { machine: Machine; open: boolean; onClose: () => void }) {
  const { t } = useTranslation();
  const navigate = useNavigate();
  const key = useActKey();
  const [uses, setUses] = useState<Uses>(machine.agents.length > 0 ? 'login' : 'key');
  const [agentId, setAgentId] = useState(machine.agents[0]?.id ?? '');
  const [name, setName] = useState('');
  const [nameError, setNameError] = useState<string | undefined>(undefined);
  const [home, setHome] = useState('');
  const setup = useQuery({ ...providerSetupQuery(home), enabled: open && home !== '' });
  // The key is typed on the machine, so a header whose value is a secret has no place in this request.
  const protocols = useMemo(
    () => (setup.data?.protocols ?? FALLBACK_PROTOCOLS).map((protocol) => ({ ...protocol, endpoint_options: protocol.endpoint_options.filter((field) => field.type !== 'secret_map') })),
    [setup.data],
  );
  const [endpoints, setEndpoints] = useState<EndpointDraft[]>(() => [newEndpoint(FALLBACK_PROTOCOLS[0])]);
  const [endpointErrors, setEndpointErrors] = useState<Record<string, string>>({});
  const [made, setMade] = useState<Provider | null>(null);
  const httpAllowed = setup.data?.http_allowed ?? false;

  const register = useMutation({
    mutationFn: (body: MachineProviderCreateBody) => registerProvider(machine.id, body, key(body)),
    onSuccess: (provider) => {
      toast({ tone: 'success', title: t('machines.provider.done', { name: provider.name }) });
      if (provider.kind === 'api') {
        setMade(provider);
        return;
      }
      onClose();
      void navigate({ to: '/resources/providers/$id', params: { id: provider.id } });
    },
  });

  const submit = () => {
    const trimmed = name.trim();
    setNameError(trimmed === '' ? t('machines.provider.nameEmpty') : undefined);
    if (trimmed === '' || home === '') return;
    if (uses === 'login') {
      if (agentId === '') return;
      register.mutate({ home_id: home, kind: 'subscription', name: trimmed, agent_id: agentId });
      return;
    }
    const faults: Record<string, string> = {};
    for (const row of endpoints) {
      const fault = endpointFault(row, httpAllowed, t);
      if (fault !== null) faults[row.id] = fault;
    }
    setEndpointErrors(faults);
    if (Object.keys(faults).length > 0) return;
    register.mutate({
      home_id: home,
      kind: 'api',
      name: trimmed,
      upstreams: endpoints.map((row) => upstreamOf(row, protocols.find((protocol) => protocol.name === row.protocol)).upstream),
    });
  };

  const pending = register.isPending;
  return (
    <Dialog
      open={open}
      onOpenChange={(next) => !next && !pending && onClose()}
      size="lg"
      title={made === null ? t('machines.provider.title', { machine: machine.name }) : t('machines.provider.madeTitle', { name: made.name })}
      description={made === null ? t('machines.provider.lead') : undefined}
      dismissible={!pending}
      footer={
        made === null ? (
          <>
            <Button disabled={pending} onClick={onClose}>
              {t('common.cancel')}
            </Button>
            <Button variant="primary" pending={pending} blocked={home === '' || (uses === 'login' && agentId === '')} onClick={submit}>
              {t('machines.provider.submit')}
            </Button>
          </>
        ) : (
          <Button variant="primary" asChild>
            <Link to="/resources/providers/$id" params={{ id: made.id }} onClick={onClose}>
              {t('machines.provider.open', { name: made.name })}
            </Link>
          </Button>
        )
      }
    >
      {made === null ? (
        <div className="flex flex-col gap-4 py-2">
          <span className="flex items-center gap-1 text-sm font-medium text-text">
            {t('machines.provider.uses')}
            <Hint id="machine-provider-source" />
          </span>
          <RadioGroup
            label={t('machines.provider.uses')}
            value={uses}
            onValueChange={setUses}
            options={[
              {
                value: 'login',
                ...choiceOption(t, 'providerRegistration.login'),
                description: t('machines.provider.loginLead'),
                icon: <LogIn />,
                disabled: machine.agents.length === 0,
                ...(machine.agents.length === 0 ? { note: t('machines.provider.loginNone') } : {}),
              },
              { value: 'key', ...choiceOption(t, 'providerRegistration.key'), description: t('machines.provider.keyLead'), icon: <KeyRound /> },
            ]}
          />
          <Input label={t('machines.provider.name')} value={name} error={nameError} onChange={(event) => setName(event.target.value)} autoComplete="off" />
          <HomePicker value={home} onChange={setHome} />
          {uses === 'login' ? (
            <Select
              label={t('machines.provider.profile')}
              hint={t('machines.provider.profileHint')}
              value={agentId}
              onValueChange={setAgentId}
              options={machine.agents.map((agent) => ({ value: agent.id, label: agent.name, description: agent.harness_kind }))}
            />
          ) : (
            <div className="flex flex-col gap-2">
              <span className="text-sm font-medium text-text">{t('machines.provider.endpoints')}</span>
              <EndpointsEditor value={endpoints} onChange={setEndpoints} protocols={protocols} httpAllowed={httpAllowed} errors={endpointErrors} disabled={pending} />
              <p className="text-sm text-text-muted">{t('machines.provider.keyWhere', { machine: machine.name })}</p>
            </div>
          )}
          {register.error !== null && <ErrorAlert error={register.error} />}
        </div>
      ) : (
        <div className="flex flex-col gap-3 py-2">
          <InlineAlert tone="info" title={t('machines.provider.typeKey', { machine: machine.name })}>
            {t('machines.provider.typeKeyLead')}
          </InlineAlert>
          <CommandBox command={`orch-connector provider set-key --provider ${made.id}`} label={t('machines.provider.copy')} />
          <p className="text-sm text-text-muted">{t('machines.provider.orHere')}</p>
        </div>
      )}
    </Dialog>
  );
}
