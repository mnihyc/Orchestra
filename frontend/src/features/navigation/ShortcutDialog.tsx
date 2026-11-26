/** Choosing the place a shortcut is kept in: "Add shortcut…" on a document, a chat, a thread or a place, and "Move to…"
 * on a shortcut. One field, the place picker every destination uses (recent places first, a filter, the keyboard), and
 * the act; what was done is said once, with the way to undo it. A place where the person may not add documents refuses
 * it, and says why, without anything changing.
 */
import { useMutation } from '@tanstack/react-query';
import { useState } from 'react';
import { useTranslation } from 'react-i18next';

import { keepShortcut, moveShortcut, removeShortcut, type Shortcut } from '@/api/shortcuts';
import type { ShortcutTarget } from '@/features/navigation/shortcuts';
import { type PickedPlace, PlacePicker } from '@/features/settings/PlacePicker';
import { Button } from '@/ui/Button';
import { Dialog } from '@/ui/Dialog';
import { ErrorAlert } from '@/ui/ErrorState';
import { toast } from '@/ui/Toast';

interface Props {
  open: boolean;
  onClose: () => void;
}

/** The last step of a place's path: what a short line calls it. */
const lastOf = (path: string, separator: string): string => path.split(separator).at(-1) ?? path;

/** Keeps a shortcut to a thing in a place the person chooses. */
export function AddShortcutDialog({ target, open, onClose }: Props & { target: ShortcutTarget }) {
  const { t } = useTranslation();
  const [place, setPlace] = useState<PickedPlace | null>(null);
  const add = useMutation({
    mutationFn: (to: PickedPlace) => keepShortcut(to.id, { kind: target.kind, id: target.id }),
    onSuccess: (made, to) => {
      onClose();
      toast({
        tone: 'success',
        title: t('shortcuts.added', { name: target.name, place: lastOf(to.path, t('settings.place.separator')) }),
        action: { label: t('places.toast.undo'), run: () => void removeShortcut(made.id) },
      });
    },
  });
  return (
    <Dialog
      open={open}
      onOpenChange={(next) => !next && !add.isPending && onClose()}
      size="sm"
      title={t('shortcuts.addTitle', { name: target.name })}
      dismissible={!add.isPending}
      footer={
        <>
          <Button disabled={add.isPending} onClick={onClose}>
            {t('common.cancel')}
          </Button>
          <Button variant="primary" disabled={place === null} pending={add.isPending} onClick={() => place !== null && add.mutate(place)}>
            {t('shortcuts.addSubmit')}
          </Button>
        </>
      }
    >
      <div className="flex flex-col gap-4 py-2">
        <PlacePicker label={t('shortcuts.where')} value={place} onChange={(next) => { setPlace(next); add.reset(); }} />
        {add.error !== null && <ErrorAlert error={add.error} />}
      </div>
    </Dialog>
  );
}

/** Puts a shortcut in another place; its target is not touched. */
export function MoveShortcutDialog({ shortcut, name, open, onClose }: Props & { shortcut: Shortcut; name: string }) {
  const { t } = useTranslation();
  const [place, setPlace] = useState<PickedPlace | null>(null);
  const move = useMutation({
    mutationFn: (to: PickedPlace) => moveShortcut(shortcut.id, to.id),
    onSuccess: (_moved, to) => {
      onClose();
      toast({ tone: 'success', title: t('shortcuts.moved', { name, place: lastOf(to.path, t('settings.place.separator')) }) });
    },
  });
  return (
    <Dialog
      open={open}
      onOpenChange={(next) => !next && !move.isPending && onClose()}
      size="sm"
      title={t('shortcuts.moveTitle', { name })}
      dismissible={!move.isPending}
      footer={
        <>
          <Button disabled={move.isPending} onClick={onClose}>
            {t('common.cancel')}
          </Button>
          <Button variant="primary" disabled={place === null || place.id === shortcut.node_id} pending={move.isPending} onClick={() => place !== null && move.mutate(place)}>
            {t('shortcuts.moveSubmit')}
          </Button>
        </>
      }
    >
      <div className="flex flex-col gap-4 py-2">
        <PlacePicker
          label={t('shortcuts.where')}
          value={place}
          onChange={(next) => { setPlace(next); move.reset(); }}
          refusal={(candidate) => (candidate.id === shortcut.node_id ? t('shortcuts.already') : false)}
        />
        {move.error !== null && <ErrorAlert error={move.error} />}
      </div>
    </Dialog>
  );
}
