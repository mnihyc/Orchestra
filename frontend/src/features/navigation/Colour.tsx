/** The colour of a place's icon: one of the twelve, a colour of its own, the colour of its kind again, or one picked at random, given by whoever manages the place and the same for everybody.
 *
 * The dialog shows the place's own glyph in each of the twelve, so that a choice is seen before it is
 * made, and each is named, since a colour alone says nothing to somebody who cannot tell it apart. A
 * choice is sent at once and shown as chosen while it is on its way; one the server refuses is taken
 * back and said. "Random" picks another of the twelve than the one in use. A colour of its own is picked
 * with the small picker, seen on the glyph beside it, and given with "Use this colour"; one of the twelve
 * takes its place, and it the place of the twelve's.
 */
import { useMutation, useQuery, useQueryClient } from '@tanstack/react-query';
import { Dices, LoaderCircle } from 'lucide-react';
import { useState } from 'react';
import { useTranslation } from 'react-i18next';

import { ApiError } from '@/api/client';
import { nodeKey, nodeQuery, updateNode } from '@/api/nodes';
import { useNamedPlace } from '@/features/navigation/namedPlace';
import { kindWord, type PlaceRef } from '@/features/navigation/places';
import type { Node } from '@/generated/api-types';
import { Button } from '@/ui/Button';
import { cx } from '@/ui/cx';
import { Dialog } from '@/ui/Dialog';
import { ErrorAlert, ErrorState } from '@/ui/ErrorState';
import { isNodeColor, isOwnColor, NODE_COLOR_NAMES, NODE_COLORS, nodeColor, NodeIcon, type NodeIconKind } from '@/ui/NodeIcon';
import { SkeletonRows } from '@/ui/Skeleton';
import { Tooltip } from '@/ui/Tooltip';

/** Another of the twelve than the one in use, each as likely as the next. `random` gives a number from 0 up to 1. */
export function anotherColour(current: number | null, random: () => number = Math.random): number {
  const others = Array.from({ length: NODE_COLORS }, (_, index) => index + 1).filter((colour) => colour !== current);
  return others[Math.min(others.length - 1, Math.floor(random() * others.length))] ?? 1;
}

interface DialogProps {
  open: boolean;
  onClose: () => void;
}

export function ColourDialog({ place, open, onClose }: DialogProps & { place: PlaceRef }) {
  const { t } = useTranslation();
  const named = useNamedPlace(place);
  const node = useQuery(nodeQuery(place.id));
  return (
    <Dialog
      open={open}
      onOpenChange={(next) => !next && onClose()}
      size="sm"
      title={t('places.colour.title', { name: named })}
      description={t('places.colour.lead')}
      footer={<Button onClick={onClose}>{t('common.close')}</Button>}
    >
      {node.isPending ? (
        <SkeletonRows rows={3} className="py-2" />
      ) : node.isError ? (
        <ErrorState compact error={node.error} retrying={node.isRefetching} onRetry={() => void node.refetch()} />
      ) : (
        <Choices node={node.data} kind={kindWord(place)} />
      )}
    </Dialog>
  );
}

const SWATCH =
  'relative flex size-(--control-md) items-center justify-center rounded-md border border-border bg-surface transition-colors hover:bg-bg-muted focus-visible:outline-2 focus-visible:outline-offset-2 focus-visible:outline-focus aria-pressed:border-accent aria-pressed:bg-accent-subtle';

/** What a choice asks for: one of the twelve (null for the colour of its kind), or a colour of its own. */
type Choice = { color: number | null } | { own: string };

/** What a colour of its own starts as in the picker before one was given: a quiet grey-blue. */
const FIRST_OWN = '#4f6d8f';

function Choices({ node, kind }: { node: Node; kind: NodeIconKind }) {
  const { t } = useTranslation();
  const queryClient = useQueryClient();
  // What was asked for and is not answered yet: shown as chosen meanwhile.
  const [asked, setAsked] = useState<Choice | undefined>(undefined);
  const heldOwn = isOwnColor(node.custom_color) ? node.custom_color.toLowerCase() : null;
  const held = heldOwn === null && isNodeColor(node.color) ? node.color : null;
  const [draft, setDraft] = useState(heldOwn ?? FIRST_OWN);

  const give = useMutation({
    mutationFn: (choice: Choice) =>
      updateNode(node.id, {
        ...('own' in choice ? { custom_color: choice.own } : { color: choice.color }),
        expected_revision: (queryClient.getQueryData<Node>(nodeKey(node.id)) ?? node).revision,
      }),
    onMutate: (choice) => setAsked(choice),
    onError: (error) => {
      // Somebody changed the place first: how it stands now is read again before anything is tried again.
      if (error instanceof ApiError && error.is('conflict.stale_revision')) void queryClient.invalidateQueries({ queryKey: nodeKey(node.id) });
    },
    onSettled: () => setAsked(undefined),
  });

  const chosenOwn = asked === undefined ? heldOwn : 'own' in asked ? asked.own : null;
  const chosen = asked === undefined ? held : 'own' in asked ? null : asked.color;
  const choose = (colour: number | null) => {
    if (give.isPending || (colour === chosen && chosenOwn === null)) return;
    give.mutate({ color: colour });
  };
  const picked = draft.toLowerCase();

  return (
    <div className="flex flex-col gap-4 py-2">
      <div role="group" aria-label={t('places.colour.palette')} className="grid grid-cols-6 justify-items-center gap-2">
        {NODE_COLOR_NAMES.map((name, index) => {
          const colour = index + 1;
          const label = t(`places.colour.names.${name}`);
          return (
            <Tooltip key={name} content={label}>
              <button type="button" aria-label={label} aria-pressed={chosenOwn === null && chosen === colour} className={SWATCH} onClick={() => choose(colour)}>
                <NodeIcon kind={kind} color={colour} size={16} />
                {give.isPending && asked !== undefined && !('own' in asked) && asked.color === colour && <LoaderCircle size={16} className="absolute top-0.5 right-0.5 animate-spin text-text-subtle" aria-hidden />}
              </button>
            </Tooltip>
          );
        })}
      </div>
      <div className="flex flex-wrap items-center gap-2">
        <Button
          icon={<NodeIcon kind={kind} color={null} />}
          aria-pressed={chosen === null && chosenOwn === null}
          className={cx(chosen === null && chosenOwn === null && 'border-accent bg-accent-subtle')}
          pending={give.isPending && asked !== undefined && !('own' in asked) && asked.color === null}
          onClick={() => choose(null)}
        >
          {t('places.colour.asKind')}
        </Button>
        {/* Another than the one the icon is drawn in now: from the colour of its kind, that colour itself is not among those that can come. */}
        <Button icon={<Dices aria-hidden />} disabled={give.isPending} onClick={() => choose(anotherColour(nodeColor(kind, chosen)))}>
          {t('places.colour.random')}
        </Button>
      </div>
      <div className="flex flex-wrap items-center gap-2">
        <label className="flex items-center gap-2 text-sm text-text">
          <input
            type="color"
            value={picked}
            onChange={(event) => setDraft(event.target.value)}
            className="h-(--control-md) w-12 cursor-pointer rounded-md border border-border bg-surface p-1"
          />
          {t('places.colour.own')}
        </label>
        <NodeIcon kind={kind} own={picked} size={16} />
        <Button
          aria-pressed={chosenOwn !== null && chosenOwn === picked}
          className={cx(chosenOwn !== null && chosenOwn === picked && 'border-accent bg-accent-subtle')}
          pending={give.isPending && asked !== undefined && 'own' in asked}
          disabled={give.isPending}
          onClick={() => {
            if (chosenOwn !== picked) give.mutate({ own: picked });
          }}
        >
          {t('places.colour.useOwn')}
        </Button>
      </div>
      {give.error !== null && <ErrorAlert error={give.error} />}
    </div>
  );
}
