/** What a permission is no use without: Include the model for a thread, Open to the work here for a place, or a request to whoever decides.
 *
 * A thread's environment is shown here too, with where the people stand.
 * Talking to an agent needs the model it runs on; starting work in a place needs something to run on
 * and somewhere to run; reading a thread's files is looking into the environment it runs in. Those are
 * permissions on other pages, so the page that gives the first shows the second beside it, says whether the
 * people here have it, and offers to give it in the same step where the caller may give there. The model
 * and the work of a place are given by a tick here; letting people into an environment goes with naming
 * them, so its tick stands under the field that names them and this line says where the people stand.
 * Where the caller may not give, the line says who decides.
 */
import { useMutation } from '@tanstack/react-query';
import { Check, CircleDashed, CircleDot } from 'lucide-react';
import { useTranslation } from 'react-i18next';

import { type AccessKind, type AccessTarget, changePage } from '@/api/access';
import type { Dependency, HolderInput, PermissionPage } from '@/generated/api-types';
import { Checkbox } from '@/ui/Checkbox';
import { ErrorAlert } from '@/ui/ErrorState';

import { containerOf, holdsOfRow, inputOf } from './model';
import { usePermissionName } from './names';

/** The models and efforts an option names, in the words a person says them: "gpt-5 at high". */
function optionsText(options: unknown): string | null {
  if (typeof options !== 'object' || options === null) return null;
  const { models, efforts } = options as { models?: unknown; efforts?: unknown };
  const list = (value: unknown): string[] => (Array.isArray(value) ? value.filter((entry): entry is string => typeof entry === 'string') : []);
  const named = list(models).join(', ');
  const levels = list(efforts).join(', ');
  return named === '' ? null : levels === '' ? named : `${named} · ${levels}`;
}

const MARKS = { all: Check, some: CircleDot, none: CircleDashed } as const;

/** A subscription's profile is given to the same people or work its Use row already names. */
export function providerHolders(page: PermissionPage): HolderInput[] {
  const home = containerOf(page);
  const holders = [...page.owners.map((entry) => entry.holder), ...page.rows
    .filter((row) => row.permission === 'provider.use' || row.permission === 'provider.manage')
    .flatMap((row) => holdsOfRow(row).map((entry) => entry.holder))];
  const found = new Map<string, HolderInput>();
  for (const holder of holders) {
    let input = inputOf(holder);
    if (input.kind === 'role' && input.at === undefined && home !== null && typeof home !== 'string') input = { ...input, at: home.id };
    found.set(JSON.stringify(input), input);
  }
  return [...found.values()];
}

function Needed({ dependency, page, target }: { dependency: Dependency; page: PermissionPage; target: AccessTarget }) {
  const { t } = useTranslation();
  const permissionName = usePermissionName();
  const other = { kind: dependency.object.kind as AccessKind, id: dependency.object.id };
  const holders: HolderInput[] = target.kind === 'provider' ? providerHolders(page) : [{ kind: 'node', id: page.object.id }];
  const give = useMutation({
    mutationFn: () =>
      changePage(other, {
        rows: [
          {
            permission: dependency.permission,
            add: holders.map((holder) => ({ holder, ...(dependency.options === null || dependency.options === undefined ? {} : { options: dependency.options }) })),
          },
        ],
      }),
    // The other page has changed, and this page says whether the people here now have it: the registry of acts asks for both again.
  });
  const Mark = MARKS[dependency.open];
  const options = optionsText(dependency.options);
  const owner = dependency.object.owner?.display_name ?? null;
  const here = dependency.through?.name ?? target.name;
  const threadDependency = dependency.for.startsWith('thread.');

  return (
    <li className="flex flex-col gap-1">
      <p className="flex items-start gap-2 text-base text-text">
        <Mark className="mt-0.5 shrink-0 text-text-muted" aria-hidden />
        <span className="min-w-0 wrap-anywhere">
          {dependency.object.kind === 'provider'
            ? t(threadDependency ? 'access.goes.needsModel' : 'access.goes.needsWork', { what: options ?? permissionName(dependency.permission), name: dependency.object.name })
            : t('access.goes.needs', { for: permissionName(dependency.for), permission: permissionName(dependency.permission), name: dependency.object.name })}
          {options !== null && dependency.object.kind !== 'provider' && <span className="text-text-muted"> ({options})</span>}
        </span>
      </p>
      <div className="pl-6">
        {dependency.can_give ? (
          <Checkbox
            label={t(dependency.open === 'all' ? 'access.goes.included' : 'access.goes.include', { name: here })}
            hint={dependency.open === 'some' ? t('access.goes.some') : undefined}
            checked={dependency.open === 'all' || give.isPending}
            disabled={dependency.open === 'all' || give.isPending}
            onCheckedChange={(on) => on && give.mutate()}
          />
        ) : (
          <p className="text-sm text-text-muted">
            {dependency.open === 'all'
              ? t('access.goes.hasIt')
              : owner === null
                ? t('access.goes.ask')
                : t('access.goes.decides', { name: owner })}
          </p>
        )}
        {give.error !== null && <ErrorAlert error={give.error} className="mt-1" />}
      </div>
    </li>
  );
}

/** The environment a thread runs in: whose files, terminal, browsers and displays they are, and whether the people here look into it. */
function EnvironmentLine({ dependencies }: { dependencies: readonly Dependency[] }) {
  const { t } = useTranslation();
  const looking = dependencies.find((entry) => entry.for === 'thread.view') ?? dependencies[0];
  const changing = dependencies.filter((entry) => entry.for === 'thread.manage');
  if (looking === undefined) return null;
  const owner = looking.object.owner?.display_name ?? null;
  const mayLetIn = dependencies.some((entry) => entry.can_give);
  const Mark = MARKS[looking.open];
  return (
    <li className="flex flex-col gap-1">
      <p className="flex items-start gap-2 text-base text-text">
        <Mark className="mt-0.5 shrink-0 text-text-muted" aria-hidden />
        <span className="min-w-0 wrap-anywhere">{t('access.doors.theirs', { name: looking.object.name })}</span>
      </p>
      <p className="pl-6 text-sm text-text-muted">
        {t(`access.doors.looking.${looking.open}`, { name: looking.object.name })}
        {changing.length > 0 && changing.every((entry) => entry.open !== 'all') && ` ${t('access.doors.changing', { name: looking.object.name })}`}
      </p>
      {!mayLetIn && owner !== null && <p className="pl-6 text-sm text-text-muted">{t('access.doors.decides', { name: owner })}</p>}
    </li>
  );
}

export function GoesWith({ page, target }: { page: PermissionPage; target: AccessTarget }) {
  const { t } = useTranslation();
  if (page.depends.length === 0) return null;
  const environment = page.object.kind === 'thread' ? page.depends.filter((entry) => entry.object.kind === 'environment') : [];
  const others = page.depends.filter((entry) => !environment.includes(entry));
  return (
    <section aria-label={t('access.goes.title')} className="flex flex-col gap-2">
      <h3 className="text-sm font-medium text-text">{t('access.goes.title')}</h3>
      <ul className="flex flex-col gap-3">
        {others.map((dependency) => (
          <Needed key={`${dependency.for}:${dependency.object.id}:${dependency.permission}`} dependency={dependency} page={page} target={target} />
        ))}
        {environment.length > 0 && <EnvironmentLine dependencies={environment} />}
      </ul>
    </section>
  );
}
