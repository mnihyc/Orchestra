/** A view retains its live models while Activity pauses the effects that read them. */
import { createContext, type ReactNode, useContext, useEffect, useLayoutEffect, useMemo, useState } from 'react';

type Hold = (key: string) => () => void;
interface Entry { key: string; hold: Hold; stop: () => void; seen: symbol; attempt: symbol }
class Models {
  private entries = new Map<symbol, Entry>();
  private generation = Symbol('models');
  private active = false;
  connected(): boolean { return this.active; }
  connect(active: boolean): void {
    if (active && !this.active) this.generation = Symbol('models');
    this.active = active;
  }
  keep(owner: symbol, key: string | null, hold: Hold): () => void {
    let entry = this.entries.get(owner);
    if (entry !== undefined && (entry.key !== key || entry.hold !== hold)) {
      this.entries.delete(owner); entry.stop(); entry = undefined;
    }
    if (key === null) return () => undefined;
    const attempt = Symbol('model');
    if (entry === undefined) { entry = { key, hold, stop: hold(key), seen: this.generation, attempt }; this.entries.set(owner, entry); }
    entry.seen = this.generation; entry.attempt = attempt;
    return () => {
      const current = this.entries.get(owner);
      if (current?.attempt !== attempt) return;
      this.entries.delete(owner); current.stop();
    };
  }
  prune(): void {
    for (const [owner, entry] of this.entries) if (entry.seen !== this.generation) { this.entries.delete(owner); entry.stop(); }
  }
  dispose(): void {
    const held = [...this.entries.values()]; this.entries.clear();
    for (const entry of held) entry.stop();
  }
}
const KeptModels = createContext<{ models: Models; active: boolean } | null>(null);

export function KeptModelScope({ active, children }: { active: boolean; children: ReactNode }) {
  const models = useMemo(() => new Models(), []);
  const value = useMemo(() => ({ models, active }), [models, active]);
  useLayoutEffect(() => { models.connect(active); }, [models, active]);
  // Reconnected children register before this parent effect; removed hidden children release their models here.
  useEffect(() => { if (active) models.prune(); }, [models, active]);
  useEffect(() => () => models.dispose(), [models]);
  return <KeptModels.Provider value={value}>{children}</KeptModels.Provider>;
}

export function useKeptModel(key: string | null, hold: Hold): void {
  const context = useContext(KeptModels);
  const models = context?.models, active = context?.active;
  const [owner] = useState(() => Symbol('model owner'));
  useEffect(() => {
    if (models === undefined) return;
    const release = models.keep(owner, key, hold);
    return () => { if (models.connected()) release(); };
  }, [models, active, owner, key, hold]);
}
