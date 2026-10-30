/** A configured model uses the resource chip without fetching a catalogue until its card opens. */
import { ResourceChip } from '@/features/resource-facts/ResourceFacts';
import { modelResource } from '@/features/resource-facts/facts';

export function CallModel({ providerId, model }: { providerId: string; model: string }) {
  return <ResourceChip subject={modelResource({ model_name: model, display_name: '', parameters: {} }, providerId)} hint={false} />;
}
