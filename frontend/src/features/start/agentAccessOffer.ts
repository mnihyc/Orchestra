/** The admitted creation preview supplies preset and bounds; this hook owns only an unsubmitted UI choice. */
import { useState } from 'react';
import { useStore } from 'zustand';
import { client } from '@/api/client';
import type { AgentAccessAvailability, AgentAccessChoice, AgentAccessWhere, ShareParts } from '@/generated/api-types';
import { agentAccessValue } from '@/features/access/AgentAccessChoice';
import { ADVANCED, advancedGiven, hasAgentParts, creationAreas, creationChoice, mayGiveParts, sameParts, stepOf, withStep } from '@/features/access/agentShares';

const NONE: AgentAccessChoice = { where: 'nothing', parts: {} };
const WHERE: readonly AgentAccessWhere[] = ['nothing', 'thread', 'topic', 'project'];

function creationOffer(value: unknown): AgentAccessAvailability | null {
  if (typeof value !== 'object' || value === null || !('preset' in value) || !('choices' in value) || !Array.isArray(value.choices)) return null;
  const preset = agentAccessValue(value.preset);
  const choices = value.choices.map((entry: unknown) => agentAccessValue(entry));
  if (preset === null || choices.some((entry) => entry === null) || choices.length !== WHERE.length) return null;
  const valid = choices.filter((entry): entry is AgentAccessChoice => entry !== null);
  if (WHERE.some((where) => valid.filter((entry) => entry.where === where).length !== 1)) return null;
  return { preset, choices: valid };
}

function availableChoice(preset: AgentAccessChoice, bounds: ShareParts): AgentAccessChoice {
  let parts: ShareParts = {};
  const areas = creationAreas(preset.where);
  for (const area of areas) parts = withStep(parts, area, Math.min(stepOf(area, preset.parts), stepOf(area, bounds)));
  if (areas.includes('timeline')) for (const row of ADVANCED) {
    if (advancedGiven(preset.parts, row) && advancedGiven(bounds, row)
      && (parts.timeline !== 'manage' || preset.parts[row] === true)) parts[row] = true;
  }
  return hasAgentParts(parts) ? creationChoice(preset.where, parts) : NONE;
}

/** This is only the unsubmitted form value; server authority stays in the active query reply. */
export function useAgentAccessSelection(at: string, initial?: AgentAccessChoice, initialPreset?: AgentAccessChoice) {
  const actor = useStore(client.session, (state) => state.userId);
  const readerVersion = useStore(client.session, (state) => state.readerVersion ?? 0);
  const [picked, setPicked] = useState<{ at: string; actor: string | null; readerVersion: number; value: AgentAccessChoice; automaticPreset?:AgentAccessChoice } | null>(() => initial === undefined ? null : { at, actor, readerVersion, value: initial, automaticPreset:initialPreset });
  const current = picked?.actor === actor && picked.readerVersion === readerVersion && picked.at === at ? picked : null;
  return { chosen:current?.value ?? null, automaticPreset:current?.automaticPreset,
    onChange: (next: AgentAccessChoice) => setPicked({ at, actor, readerVersion, value: next }),
    adopt:(next:AgentAccessChoice,preset:AgentAccessChoice)=>setPicked({at,actor,readerVersion,value:next,automaticPreset:preset}),
    reset: () => setPicked(null) };
}

interface Selection {
  chosen:AgentAccessChoice|null;
  automaticPreset?:AgentAccessChoice;
  onChange:(next:AgentAccessChoice)=>void;
  reset:()=>void;
}

/** Fork/adopt can form their existing request from the choice before reading metadata from that same query. */
export function offeredAgentAccess(selection: Selection, { offer, pending, enabled = true, error = null, retry }: {
  offer: AgentAccessAvailability | null | undefined; pending: boolean; enabled?: boolean; error?: unknown; retry: () => void;
}) {
  const chosen = selection.chosen;
  const facts = creationOffer(offer);
  const preset = facts?.preset ?? NONE;
  const requested = selection.automaticPreset ?? preset;
  const available = (where: AgentAccessWhere) => where === 'nothing' ? {} : facts?.choices.find((entry) => entry.where === where)?.parts;
  const maximum = available(preset.where);
  const value = chosen ?? (maximum === undefined ? NONE : availableChoice(preset, maximum));
  const bounds = available(value.where);
  const allowed = value.where === 'nothing' || bounds !== undefined && mayGiveParts(creationChoice(value.where, value.parts).parts, bounds);
  const unavailable = enabled && !pending && facts === null;
  const blocked = enabled && !pending && chosen !== null && !allowed;
  return { value, preset:requested, available, error, unavailable, blocked, pending: enabled && pending,
    ready: enabled && !pending && allowed,
    reduced: (chosen === null || selection.automaticPreset !== undefined) && facts !== null && (value.where !== requested.where || !sameParts(value.parts, requested.parts)),
    onChange: selection.onChange,
    onReachChange: undefined, onClose: undefined,
    reset: selection.reset, retry,
  };
}

export function useOfferedAgentAccess({ at, initial, initialPreset, ...source }: {
  at: string; initial?: AgentAccessChoice; initialPreset?:AgentAccessChoice; offer: AgentAccessAvailability | null | undefined;
  pending: boolean; enabled?: boolean; error?: unknown; retry: () => void;
}) { return offeredAgentAccess(useAgentAccessSelection(at, initial,initialPreset), source); }
