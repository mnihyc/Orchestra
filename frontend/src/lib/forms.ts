/** The state of a form: what was typed, what is wrong with it, and when to say so.
 *
 * A field is checked when its reader leaves it and when the form is sent, never while they are still
 * typing in it for the first time. Sending a form that has something wrong sends nothing and puts
 * the focus on the first field to mend. What the server finds wrong is shown at the field it names,
 * until that field is changed.
 */
import { type ChangeEvent, type SyntheticEvent, useCallback, useRef, useState } from 'react';

import { ApiError } from '@/api/client';
import { newActKey } from '@/lib/requests';

/**
 * Whether a pointer is pressed at the moment. A field that is left because a button was pressed is
 * checked only once the press is over: what is wrong appears under the field and moves everything
 * below it, and a button that moves away under the pointer between press and release is not pressed.
 */
let pointerDown = false;
if (typeof document !== 'undefined') {
  document.addEventListener('pointerdown', () => (pointerDown = true), true);
  document.addEventListener('pointerup', () => (pointerDown = false), true);
  document.addEventListener('pointercancel', () => (pointerDown = false), true);
}

/** Runs `act` now, or after the press that is under way has done what it does. */
function afterPress(act: () => void): void {
  if (!pointerDown) {
    act();
    return;
  }
  const later = () => {
    document.removeEventListener('pointerup', later, true);
    document.removeEventListener('pointercancel', later, true);
    // The click of a press follows its release at once; what moves the page comes after it.
    setTimeout(act, 0);
  };
  document.addEventListener('pointerup', later, true);
  document.addEventListener('pointercancel', later, true);
}

/** For each field, what is wrong with its value in words, or `null` when nothing is. */
export type Rules<Values> = { [Name in keyof Values]?: (value: string, values: Values) => string | null };

export interface FieldBinding {
  name: string;
  value: string;
  error: string | undefined;
  onChange: (event: ChangeEvent<HTMLInputElement | HTMLTextAreaElement>) => void;
  onBlur: () => void;
  ref: (element: HTMLElement | null) => void;
}

export interface Form<Values extends Record<string, string>> {
  values: Values;
  /** What a control of the field needs: its value, its handlers and what is wrong with it. */
  field: (name: keyof Values & string) => FieldBinding;
  /** Sets a value from outside a control. */
  set: (name: keyof Values & string, value: string) => void;
  /** Makes the handler of the form's `submit`: checks every field, and runs `send` when all hold. */
  submit: (send: (values: Values) => void) => (event: SyntheticEvent) => void;
  /**
   * Shows what the server found wrong at the fields it names. Answers whether it placed anything:
   * what it could not place is the form's to say as a whole.
   */
  refuse: (error: unknown) => boolean;
  /** Puts the focus on a field. */
  focus: (name: keyof Values & string) => void;
}

export function useForm<Values extends Record<string, string>>(initial: Values, rules: Rules<Values>): Form<Values> {
  const [values, setValues] = useState(initial);
  const [left, setLeft] = useState<Partial<Record<keyof Values, true>>>({});
  const [sent, setSent] = useState(false);
  const [refused, setRefused] = useState<Partial<Record<keyof Values, string>>>({});
  const elements = useRef(new Map<string, HTMLElement>());

  const wrong = (name: keyof Values & string, among: Values): string | null =>
    rules[name]?.(among[name] ?? '', among) ?? null;

  const set = useCallback((name: keyof Values & string, value: string) => {
    setValues((before) => ({ ...before, [name]: value }));
    // What the server said about the old value says nothing about the new one.
    setRefused((before) => (name in before ? { ...before, [name]: undefined } : before));
  }, []);

  const focus = useCallback((name: keyof Values & string) => {
    elements.current.get(name)?.focus();
  }, []);

  const field = (name: keyof Values & string): FieldBinding => {
    const shown = refused[name] ?? (sent || left[name] === true ? wrong(name, values) : null);
    return {
      name,
      value: values[name] ?? '',
      error: shown ?? undefined,
      onChange: (event) => set(name, event.target.value),
      onBlur: () => afterPress(() => setLeft((before) => (before[name] === true ? before : { ...before, [name]: true }))),
      ref: (element) => {
        if (element) elements.current.set(name, element);
        else elements.current.delete(name);
      },
    };
  };

  const submit = (send: (values: Values) => void) => (event: SyntheticEvent) => {
    event.preventDefault();
    setSent(true);
    const first = (Object.keys(values) as (keyof Values & string)[]).find((name) => wrong(name, values) !== null);
    if (first !== undefined) {
      focus(first);
      return;
    }
    send(values);
  };

  const refuse = (error: unknown): boolean => {
    if (!(error instanceof ApiError)) return false;
    const placed: Partial<Record<keyof Values, string>> = {};
    for (const fault of error.faults()) {
      if (fault.path in values && !(fault.path in placed)) placed[fault.path as keyof Values] = fault.message;
    }
    const names = Object.keys(placed) as (keyof Values & string)[];
    if (names.length === 0) return false;
    setRefused(placed);
    const first = (Object.keys(values) as (keyof Values & string)[]).find((name) => names.includes(name));
    if (first !== undefined) focus(first);
    return true;
  };

  return { values, field, set, submit, refuse, focus };
}

/**
 * The key of one act of a person that creates something. The same key goes with every attempt of the
 * same act, so that a second press after an answer was lost makes nothing twice; an act whose content
 * was changed since is another act and gets a key of its own.
 */
export function useActKey(): (content: unknown) => string {
  const held = useRef<{ key: string; content: string } | null>(null);
  return (content) => {
    const text = JSON.stringify(content);
    if (held.current?.content !== text) held.current = { key: newActKey(), content: text };
    return held.current.key;
  };
}
