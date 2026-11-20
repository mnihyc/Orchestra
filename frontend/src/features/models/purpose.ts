/** Model pickers and the provider editor follow the same purpose and endpoint evidence. */
import families from '../../../../shared/model-purpose-families.json';
import protocols from '../../../../shared/protocol-options.json';
import type { ModelParameters, ModelPurpose, ModelPurposeInfo } from '@/generated/api-types';

export const MODEL_PURPOSES: readonly ModelPurpose[] = ['generation', 'embedding', 'image_generation', 'transcription', 'speech_generation', 'rerank', 'decision'];

const nameRules = families.map(({ purpose, pattern }) => ({ purpose: purpose as ModelPurpose, pattern: new RegExp(pattern) }));
const interfaces: Record<string, { kind: string } | undefined> = protocols.protocols;

function purposeFromName(name?: string | null): ModelPurpose | undefined {
  const model = name?.trim().split('/').at(-1)?.toLowerCase();
  return model ? nameRules.find((rule) => rule.pattern.test(model))?.purpose : undefined;
}

/** The provider's actual interfaces bound the choice even after a person edits its purpose. */
export function endpointPurposes(names?: readonly string[]): ModelPurpose[] | null {
  if (names === undefined || names.length === 0) return null;
  const kinds = new Set<ModelPurpose>();
  for (const name of names) {
    const work = MODEL_PURPOSES.find((purpose) => purpose === interfaces[name]?.kind);
    if (work === undefined) return null;
    kinds.add(work);
  }
  return [...kinds];
}

/** What the model follows when the manager clears the explicit override. */
export function automaticPurposes(name: string | undefined, parameters: ModelParameters, reachable?: readonly ModelPurpose[] | null): readonly ModelPurpose[] | null {
  const offered = parameters.offered;
  if (offered?.purposes !== undefined && offered.purposes !== null) return offered.purposes;
  const inferred = purposeFromName(offered?.report?.line) ?? purposeFromName(name);
  if (inferred !== undefined) return [inferred];
  return reachable !== undefined && reachable !== null && reachable.length > 0 && !reachable.includes('generation') ? reachable : null;
}

interface ModelWork {
  model_name?: string;
  parameters: ModelParameters;
  purpose?: ModelPurposeInfo | null;
}

export function modelPurpose(model: ModelWork, purpose: ModelPurpose): 'supported' | 'unsupported' | 'unknown' {
  const reachable = model.purpose?.reachable;
  if (reachable !== undefined && reachable !== null && !reachable.includes(purpose)) return 'unsupported';
  const declared = model.parameters.purposes ?? model.purpose?.declared ?? automaticPurposes(model.model_name, model.parameters, reachable);
  return declared === null ? 'unknown' : declared.includes(purpose) ? 'supported' : 'unsupported';
}

export function modelsForPurpose<T extends ModelWork>(models: readonly T[], purpose: ModelPurpose): T[] {
  return models.filter((model) => modelPurpose(model, purpose) !== 'unsupported');
}
