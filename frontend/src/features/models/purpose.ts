/** Every model chooser applies the same declared-work rule; access remains the native reader's decision. */
import type { ModelParameters, ModelPurpose, ModelPurposeInfo } from '@/generated/api-types';

export const MODEL_PURPOSES: readonly ModelPurpose[] = ['generation', 'embedding', 'image_generation', 'transcription', 'speech_generation', 'rerank', 'decision'];

/** A manager's explicit declaration replaces discovery. Unknown models stay possible and are labelled. */
export function modelPurpose(model: { parameters: ModelParameters; purpose?: ModelPurposeInfo | null }, purpose: ModelPurpose): 'supported' | 'unsupported' | 'unknown' {
  if (model.purpose?.reachable !== undefined && model.purpose.reachable !== null && !model.purpose.reachable.includes(purpose)) return 'unsupported';
  const declared = model.purpose?.declared ?? model.parameters.purposes ?? model.parameters.offered?.purposes;
  return declared === undefined || declared === null ? 'unknown' : declared.includes(purpose) ? 'supported' : 'unsupported';
}

export function modelsForPurpose<T extends { parameters: ModelParameters; purpose?: ModelPurposeInfo | null }>(models: readonly T[], purpose: ModelPurpose): T[] {
  return models.filter((model) => modelPurpose(model, purpose) !== 'unsupported');
}
