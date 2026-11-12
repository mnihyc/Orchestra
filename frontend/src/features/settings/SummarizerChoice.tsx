/** Model summaries retain the ordinary setting editor and expose the provider's actual option form. */
import { record } from '@/features/resources/UnitCommands';
import { ModelRequestOptions } from '@/features/models/RequestOptions';
import { ValueControl, type ValueControlProps } from './ValueControl';

export function SummarizerChoice(props: ValueControlProps) {
  const held = record(props.value);
  if (held.kind !== 'model') return <ValueControl {...props} />;
  const { params, ...configuration } = held;
  return <div className="flex min-w-0 flex-col gap-3">
    <ValueControl {...props} value={configuration} onChange={(next) => props.onChange({ ...record(next), params: params ?? {} })} />
    <ModelRequestOptions providerId={typeof held.provider_id === 'string' ? held.provider_id : ''} model={typeof held.model_name === 'string' ? held.model_name : ''} value={params ?? {}} disabled={props.disabled === true} onChange={(next) => props.onChange({ ...held, params: next })} />
  </div>;
}
