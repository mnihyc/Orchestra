/** Requests sent together can be reviewed together; every decision keeps its own confirmed outcome. */
import { useQueryClient } from '@tanstack/react-query';
import { type ReactNode, useEffect, useLayoutEffect, useRef, useState } from 'react';
import { useTranslation } from 'react-i18next';
import { useStore } from 'zustand';

import { answeredMeanwhile, approvalKey, decideApproval } from '@/api/approvals';
import { client } from '@/api/client';
import { RequestedEnd } from '@/features/access/RequestEnd';
import { DecisionStatus } from '@/features/approvals/Card';
import { decisionReason } from '@/features/approvals/current';
import { decisionReader, ownsDecisionReader } from '@/features/approvals/Decision';
import type { Approval, ApprovalDecision } from '@/generated/api-types';
import { useAppearance } from '@/lib/appearance';
import { formatNumber } from '@/lib/format';
import { Button } from '@/ui/Button';
import { Checkbox } from '@/ui/Checkbox';
import { Dialog } from '@/ui/Dialog';
import { ErrorAlert } from '@/ui/ErrorState';
import { StatusChip } from '@/ui/StateChip';
import { Table, type Column } from '@/ui/Table';
import { Textarea } from '@/ui/Textarea';

type AccessApproval = Extract<Approval, { kind: 'access' }>;
interface Props {
  items: readonly AccessApproval[];
  live: boolean;
  onSettled: (approval: Approval) => void;
  summary: (approval: AccessApproval) => ReactNode;
}

export function AccessBatch(props: Props) {
  const { t } = useTranslation();
  const reader = useStore(client.session, (state) => `${state.userId}:${state.readerVersion}:${state.status}`);
  const [review, setReview] = useState<{ reader: string; items: readonly AccessApproval[] } | null>(null);
  if (props.items.length < 2 || client.session.getState().status !== 'signed_in') return null;
  const eligible = props.items.filter((item) => item.state === 'pending' && item.can_decide);
  return <>
    <Button size="sm" variant="secondary" disabled={!props.live || eligible.length < 2}
      reason={!props.live ? t('approvals.waitForLine') : eligible.length < 2 ? t('approvals.batch.needSeveral') : undefined}
      onClick={() => setReview({ reader, items: props.items })}>{t('approvals.batch.open')}</Button>
    {review?.reader === reader && <AccessBatchDialog {...props} reviewed={review.items} onClose={() => setReview(null)} />}
  </>;
}

function AccessBatchDialog({ items, reviewed, live, onSettled, summary, onClose }: Props & { reviewed: readonly AccessApproval[]; onClose: () => void }) {
  const { t } = useTranslation();
  const locale = useAppearance((state) => state.locale);
  const cache = useQueryClient();
  const [selected, setSelected] = useState<ReadonlySet<string>>(() => new Set(reviewed.filter((item) => item.state === 'pending' && item.can_decide).map((item) => item.id)));
  const [results, setResults] = useState<ReadonlyMap<string, Approval>>(new Map());
  const [errors, setErrors] = useState<ReadonlyMap<string, unknown>>(new Map());
  const [note, setNote] = useState('');
  const [busy, setBusy] = useState<'approve' | 'deny' | null>(null);
  const [working, setWorking] = useState<string | null>(null);
  const running = useRef(false);
  const mounted = useRef(true);
  const connected = useRef(live);
  const latest = useRef(items);
  useLayoutEffect(() => { connected.current = live; latest.current = items; }, [live, items]);
  useEffect(() => { mounted.current = true; return () => { mounted.current = false; }; }, []);
  const current = (item: AccessApproval): Approval | undefined => results.get(item.id) ?? items.find((row) => row.id === item.id);
  const eligible = (item: AccessApproval) => {
    const row = current(item);
    return row?.state === 'pending' && row.can_decide;
  };
  const chosen = reviewed.filter((item) => selected.has(item.id) && eligible(item));
  const unavailable = reviewed.some((item) => current(item) === undefined);
  const reason = !live ? t('approvals.waitForLine') : busy !== null ? t('ui.unavailable.deciding')
    : chosen.length === 0 ? t(unavailable ? 'approvals.batch.unavailable' : reviewed.some((item) => current(item)?.state === 'pending') ? 'approvals.batch.choose' : 'approvals.batch.finished') : undefined;
  const send = async (approve: boolean) => {
    if (running.current || !connected.current || chosen.length === 0) return;
    running.current = true;
    setBusy(approve ? 'approve' : 'deny');
    const reader = decisionReader();
    if (reader.status !== 'signed_in' || reader.userId === null) {
      running.current = false; setBusy(null); return;
    }
    try {
      for (const item of chosen) {
        if (!mounted.current || !ownsDecisionReader(reader) || !connected.current) break;
        const visible = latest.current.find((row) => row.id === item.id);
        if (visible === undefined || !visible.can_decide || visible.state !== 'pending') continue;
        const cached = cache.getQueryData<Approval>(approvalKey(item.id));
        if (cached !== undefined && cached.state !== 'pending') {
          setResults((held) => new Map(held).set(item.id, cached)); onSettled(cached); continue;
        }
        setWorking(item.id);
        setErrors((held) => { const next = new Map(held); next.delete(item.id); return next; });
        const decision: ApprovalDecision = { approve,
          ...(approve ? { permissions: item.request.permissions, ...(item.request.options === null ? {} : { options: item.request.options }),
            ...(item.request.expires_at == null ? {} : { expires_at: item.request.expires_at }) } : {}),
          ...(note.trim() === '' ? {} : { note: note.trim() }),
        };
        try {
          const result = await decideApproval(item.id, decision);
          if (!mounted.current || !ownsDecisionReader(reader)) break;
          if (result.id !== item.id) throw new Error('The decision returned another request');
          cache.setQueryData(approvalKey(item.id), result);
          setResults((held) => new Map(held).set(item.id, result)); onSettled(result);
        } catch (error) {
          if (!mounted.current || !ownsDecisionReader(reader)) break;
          const result = answeredMeanwhile(error);
          if (result?.id === item.id) {
            cache.setQueryData(approvalKey(item.id), result);
            setResults((held) => new Map(held).set(item.id, result)); onSettled(result);
          } else setErrors((held) => new Map(held).set(item.id, error));
        }
      }
    } finally {
      running.current = false;
      if (mounted.current && ownsDecisionReader(reader)) { setBusy(null); setWorking(null); }
    }
  };
  const columns: Column<AccessApproval>[] = [
    { id: 'choose', header: t('approvals.batch.select'), hideHeader: true, grow: false, cell: (item) => <Checkbox presentation="compact"
      label={<span className="sr-only">{t('approvals.batch.selectNamed', { name: current(item)?.title ?? t('approvals.batch.unavailable') })}</span>}
      checked={selected.has(item.id) && eligible(item)} disabled={busy !== null || !live || !eligible(item)}
      onCheckedChange={(on) => setSelected((held) => { const next = new Set(held); if (on) next.add(item.id); else next.delete(item.id); return next; })} /> },
    { id: 'request', header: t('approvals.batch.request'), grow: true, cell: (item) => {
      const row = current(item);
      return <div className="flex min-w-0 flex-col gap-1 py-1">
        <span className="font-medium text-text">{row?.title ?? t('approvals.batch.unavailable')}</span>
        {row?.kind === 'access' && <>{summary(row)}<RequestedEnd at={row.request.expires_at} /></>}
        {row === undefined ? null : row.state !== 'pending' ? <DecisionStatus approval={row} /> : working === item.id ? <StatusChip tone="neutral">{t('approvals.batch.deciding')}</StatusChip>
          : !row.can_decide && <span className="text-sm text-text-muted">{decisionReason(row, live, false, t)}</span>}
        {errors.has(item.id) && <ErrorAlert error={errors.get(item.id)} />}
      </div>;
    } },
  ];
  return <Dialog open title={t('approvals.batch.title')} description={t('approvals.batch.review')} size="lg" dismissible={busy === null}
    onOpenChange={(open) => { if (!open && busy === null) onClose(); }} footer={<>
      <span role="status" className="mr-auto text-sm text-text-muted">{reason ?? t('approvals.batch.selected', { number: formatNumber(chosen.length, locale) })}</span>
      <Button disabled={busy !== null} onClick={onClose}>{t('common.close')}</Button>
      <Button variant="danger-ghost" pending={busy === 'deny'} disabled={reason !== undefined} reason={reason} onClick={() => void send(false)}>{t('approvals.batch.deny')}</Button>
      <Button variant="primary" pending={busy === 'approve'} disabled={reason !== undefined} reason={reason} onClick={() => void send(true)}>{t('approvals.batch.approve')}</Button>
    </>}>
    <Table label={t('approvals.batch.title')} columns={columns} rows={reviewed} rowKey={(item) => item.id} boundedIdentity />
    <Textarea label={t('approvals.note')} rows={2} value={note} disabled={busy !== null} onChange={(event) => setNote(event.target.value)} />
  </Dialog>;
}
