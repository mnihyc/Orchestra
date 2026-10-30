/** Who of the people on the thread's page may go on talking to it on the new side, and the step that opens the new provider to them.
 *
 * Talking to a thread takes each person's own use of the provider it runs on, so a switch to another provider can leave
 * some of the people who talk to it without. It is no blocker, since the thread would run; it is said before anybody
 * confirms. Whoever may give that use (who manages the provider's page) is offered one tick that gives it to the work in
 * the place the plan names; for everybody else it says who decides, and that they can ask from the thread.
 */
import { useTranslation } from 'react-i18next';

import type { Dependency } from '@/generated/api-types';
import { Checkbox } from '@/ui/Checkbox';
import { InlineAlert } from '@/ui/InlineAlert';

export interface DependsProps {
  depends: readonly Dependency[];
  /** The model the result runs, which the first sentence says it needs. */
  model: string;
  /** Which of them the person ticked to open. */
  ticked: ReadonlySet<string>;
  onTick: (objectId: string, on: boolean) => void;
}

export function Depends({ depends, model, ticked, onTick }: DependsProps) {
  const { t } = useTranslation();
  if (depends.length === 0) return null;
  return (
    <div className="flex flex-col gap-2" data-depends>
      {depends.map((dependency) => {
        const provider = dependency.object.name;
        if (dependency.open === 'all') {
          return (
            <p key={dependency.object.id} className="text-base text-text-muted">
              {t('change.depends.all', { model, provider })}
            </p>
          );
        }
        const place = dependency.through?.name ?? '';
        return (
          <InlineAlert key={dependency.object.id} tone="warning" title={t('change.depends.title', { model, provider })}>
            <span className="flex flex-col gap-2">
              <span>{dependency.open === 'none' ? t('change.depends.none', { provider }) : t('change.depends.some', { provider })}</span>
              {dependency.can_give && dependency.through !== null ? (
                <Checkbox label={t('change.depends.open', { provider, place })} tooltip={t('change.depends.openAbout', { provider })} checked={ticked.has(dependency.object.id)} onCheckedChange={(on) => onTick(dependency.object.id, on)} />
              ) : (
                <span className="text-text-muted">{t('change.depends.decides', { name: dependency.object.owner?.display_name ?? '' })}</span>
              )}
            </span>
          </InlineAlert>
        );
      })}
    </div>
  );
}
