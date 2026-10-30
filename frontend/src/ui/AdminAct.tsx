/** An act's authority is a returned permission fact, independent of whether View needed administration. */
import type { ReactNode } from 'react';
import { useTranslation } from 'react-i18next';
import type { AdminAccess, NodeOrSystem } from '@/generated/api-types';
import { adminAccessOf, useAdminPlaceName } from '@/ui/AsAdminMark';
import { Tooltip } from '@/ui/Tooltip';

export interface AdminActSpec {
  access: { permissions?: readonly string[]; admin_only?: readonly string[]; admin_access?: AdminAccess | null; admin_place?: NodeOrSystem | null } | null | undefined;
  permission: string | readonly string[];
  /** An ordinary View can coexist with an admin-only act, whose granting place is supplied separately. */
  of?: NodeOrSystem | null;
}

export function isAdminAct(spec: AdminActSpec | undefined): spec is AdminActSpec {
  if (spec === undefined) return false;
  const { access } = spec;
  if (access === null || access === undefined) return false;
  const permissions = typeof spec.permission === 'string' ? [spec.permission] : spec.permission;
  const only = access.admin_only ?? [];
  // If either alternative is held ordinarily, the act itself does not depend on administration.
  return permissions.some((permission) => only.includes(permission)) && !permissions.some((permission) => access.permissions?.includes(permission) === true && !only.includes(permission));
}

/** A UI binding to the act's actual permission; never a calculation of who holds it. */
export function adminSpec(value: unknown, permission: string | readonly string[]): AdminActSpec {
  if (typeof value !== 'object' || value === null) return { access: null, permission };
  const row = value as Record<string, unknown>;
  const access = row.access ?? row.can ?? row;
  if (typeof access !== 'object') return { access: null, permission };
  const permissions: unknown = 'admin_only' in access ? access.admin_only : undefined;
  const held: unknown = 'permissions' in access ? access.permissions : undefined;
  const origin: unknown = 'admin_place' in access ? access.admin_place : null;
  const administrative = adminAccessOf({ admin_access: { place: origin } });
  return { access: { permissions: Array.isArray(held) ? held.filter((one): one is string => typeof one === 'string') : [], admin_only: Array.isArray(permissions) ? permissions.filter((one): one is string => typeof one === 'string') : [], admin_access: adminAccessOf(value), admin_place: administrative?.place ?? null }, permission };
}

function Words({ spec, children }: { spec: AdminActSpec; children?: (words: string) => ReactNode }) {
  const { t } = useTranslation();
  const of = spec.of ?? spec.access?.admin_place ?? spec.access?.admin_access?.place;
  const place = useAdminPlaceName(of);
  const words = of === 'system' ? t('adminReach.act.system') : place === null ? t('adminReach.mode.name') : t('adminReach.act.place', { place });
  return children === undefined ? <span className="block text-sm text-text-muted" data-admin-act>{words}</span> : children(words);
}

export function AdminActNotice({ spec }: { spec: AdminActSpec | undefined }) {
  return isAdminAct(spec) ? <Words spec={spec} /> : null;
}

/** Adds context to an existing act without inserting a confirmation or changing what the act does. */
export function AdminActTooltip({ spec, children, content }: { spec: AdminActSpec | undefined; children: ReactNode; content?: ReactNode }) {
  return isAdminAct(spec) ? <Words spec={spec}>{(words) => <Tooltip content={<span className="flex min-w-0 flex-col gap-1">{content}<span>{words}</span></span>}>{children}</Tooltip>}</Words>
    : <Tooltip content={content}>{children}</Tooltip>;
}
