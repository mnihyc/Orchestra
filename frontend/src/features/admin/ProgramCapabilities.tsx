/** A program's declared capabilities, grouped as compact states with reasons when unavailable. */
import { useTranslation } from 'react-i18next';
import { HelpHint } from '@/ui/HelpHint';
import { StateChip } from '@/ui/StateChip';
import { StateLine } from '@/ui/StateLine';

/** One localized declaration with an explanation whenever it is unavailable. */
export type ProgramCapability =
  | { id: string; title: string; available: true }
  | { id: string; title: string; available: false; reason: string };

/** Declarations grouped by the role supplied by the program catalogue. */
export type ProgramCapabilityGroups = readonly { id: string; title: string; items: readonly ProgramCapability[] }[];

/** A missing descriptor is distinct from a reported descriptor with no capabilities. */
export function ProgramCapabilities({ groups }: { groups: ProgramCapabilityGroups | null }) {
  const { t } = useTranslation();
  if (groups === null) return <StateLine compact message={t('programCapabilities.notReported')} />;
  return <div className="flex min-w-0 flex-col gap-4">
    {groups.map((group) => <section key={group.id} aria-label={group.title} className="flex min-w-0 flex-col gap-2">
      <h3 className="font-medium">{group.title}</h3>
      {group.items.length === 0 ? <StateLine compact className="text-text-muted" message={t('programCapabilities.empty')} /> : <div className="flex min-w-0 flex-wrap items-center gap-2">
        {group.items.map((capability) => <span key={capability.id} className="inline-flex min-w-0 max-w-full items-center gap-1" data-program-capability={capability.id}>
          <StateChip state={capability.available ? 'done' : 'idle'} label={t('programCapabilities.state', { name: capability.title, state: t(capability.available ? 'programCapabilities.on' : 'programCapabilities.off') })} />
          {!capability.available && <HelpHint term={capability.title} what={capability.reason} />}
        </span>)}
      </div>}
    </section>)}
  </div>;
}
