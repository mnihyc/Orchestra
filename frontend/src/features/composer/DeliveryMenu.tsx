/** Queue, steer and interrupt for whoever may drive the thread; Ask on the side; broadcast choices.
 *
 * What a message does while the agent works: wait its turn in the queue, steer the turn that runs, or
 * stop that turn and go first. The one that is the usual one for this place (`collab.default_delivery`)
 * is the button; the others are in the list beside it, each with its key. One that cannot be used says why:
 * steering waits for the agent to run, an agent that takes no steering is told so, and anything that
 * depends on current state waits for the connection.
 */
import { MessageSquareDashed, MessageSquarePlus, Navigation, OctagonX } from 'lucide-react';
import type { TFunction } from 'i18next';

import type { Delivery } from '@/generated/api-types';
import type { MenuEntry } from '@/ui/Menu';

export interface DeliveryState {
  /** A turn is running. */
  active: boolean;
  /** The thread is starting: steering is for a turn that runs. */
  starting: boolean;
  /** The live connection is up. */
  live: boolean;
  /** The agent said it cannot be steered. */
  cannotSteer: boolean;
  /** A compatibility check names the build that made steering unavailable. */
  steerUnavailable?: string;
  agent: string;
}

/** Why a way of sending cannot be used now, or `false`. */
export function whyNot(delivery: Delivery, state: DeliveryState, t: TFunction): string | false {
  if (delivery === 'queue') return false;
  if (!state.live) return t('composer.reason.offline');
  if (delivery === 'steer') {
    if (state.starting) return t('composer.reason.starting', { agent: state.agent });
    if (!state.active) return t('composer.reason.nothingRuns');
    if (state.cannotSteer) return state.steerUnavailable ?? t('composer.reason.noSteer');
  }
  return false;
}

export function deliveryLabel(delivery: Delivery, t: TFunction): string {
  return delivery === 'steer' ? t('composer.deliveryLabel.steer') : delivery === 'interrupt' ? t('composer.deliveryLabel.interrupt') : t('composer.deliveryLabel.queue');
}

/** The list beside the Send button of a thread whose turn runs; with `askOnSide`, it ends with the way to ask on the side. */
export function deliveryEntries(state: DeliveryState, send: (delivery: Delivery) => void, t: TFunction, askOnSide?: () => void): MenuEntry[] {
  const entry = (delivery: Delivery, icon: React.ReactNode, shortcut: string): MenuEntry => ({
    id: delivery,
    label: deliveryLabel(delivery, t),
    icon,
    shortcut,
    description: t(`composer.deliveryAbout.${delivery}`),
    disabled: whyNot(delivery, state, t),
    onSelect: () => send(delivery),
  });
  return [
    entry('queue', <MessageSquarePlus aria-hidden />, 'Enter'),
    entry('steer', <Navigation aria-hidden />, 'Mod+Enter'),
    entry('interrupt', <OctagonX aria-hidden />, 'Mod+Shift+Enter'),
    ...(askOnSide === undefined
      ? []
      : [
          { type: 'separator', id: 'side-separator' } as MenuEntry,
          { id: 'side', label: t('side.menu.askBox'), icon: <MessageSquareDashed aria-hidden />, description: t('side.menu.askBoxAbout'), onSelect: askOnSide } as MenuEntry,
        ]),
  ];
}
