/** Notification menus name the choice once and keep its inherited value distinct from the current exception. */
import type { TFunction } from 'i18next';
import type { ChatLevel, SubscriptionLevel } from '@/generated/api-types';

type Level = SubscriptionLevel | ChatLevel;

export function defaultLevelName(t: TFunction, inherited: Level | null): string {
  return t('notificationBell.defaultWithLevel', { level: inherited === null ? t('notificationBell.unknownLevel') : t(`notificationBell.level.${inherited}`) });
}

export function levelOptions<Value extends Level>(t: TFunction, levels: readonly Value[]): { value: Value; name: string }[] {
  return levels.map((value) => ({ value, name: t(`notificationBell.level.${value}`) }));
}
