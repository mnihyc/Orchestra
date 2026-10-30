/** An environment's applicable unit values, provenance, apply timing and editors, including commands after start.
 *
 * Read from the settings the environment resolves: each value, the place it comes from and whether the
 * reader may change it. Managed units show their template's commands read-only; connected directories
 * keep their own attach and tool settings without unit start commands.
 */
import { UnitConfiguration } from '@/features/resources/UnitConfiguration';
import { record } from '@/features/resources/UnitCommands';
import type { Environment } from '@/generated/api-types';

export function EnvironmentSettings({ environment, wantedKey }: { environment: Environment; wantedKey?: string }) {
  const facts=record(environment.facts);
  const descriptor=record(facts.environment_descriptor);
  const commands=record(descriptor.commands_after_start);
  const afterUp=record(facts.machine_policy)['environment.after_up'];
  const lifecycle=descriptor.lifecycle === 'unit' || descriptor.lifecycle === 'attach' ? descriptor.lifecycle : null;
  return (
    <div id="environment-settings">
      <UnitConfiguration compact wantedKey={wantedKey} pool="environment" id={environment.id} placement={environment.placement} runtime={environment.runtime_changes}
        lifecycle={lifecycle} resourceSupport={descriptor.resource_limits} commands={{supported:commands.supported === true,reason:typeof commands.reason === 'string' ? commands.reason : null}}
        afterUp={typeof afterUp==='boolean'?afterUp:null}
        templateId={environment.template?.id} jobId={environment.job?.id} failureDetail={environment.status_detail} place={{ name: environment.name, owner: environment.owner, access: environment.access, kind: null }} />
    </div>
  );
}
