import { useEffect } from "react";
import type { InjectionSettings } from "../../shared/types";
import { useStoryStore } from "../story/store";
import { useContextStore } from "./store";

export function InjectionPanel() {
  const storyId = useStoryStore((state) => state.activeStoryId);
  const state = useContextStore((state) => storyId ? state.stories[storyId] : undefined);
  const load = useContextStore((state) => state.loadInjection);
  const save = useContextStore((state) => state.saveInjection);
  const saveNote = useContextStore((state) => state.saveNote);
  const setNoteDraft = useContextStore((state) => state.setNoteDraft);

  useEffect(() => { if (storyId) load(storyId); }, [storyId, load]);

  if (!storyId) return <p className="py-1 text-xs text-muted">Open a story first.</p>;
  if (!state?.injection) return !state || state.injectionLoading
    ? <p className="py-1 text-xs text-muted">Loading...</p>
    : <p className="text-xs text-danger">{state?.injectionError ?? "Injection could not be loaded."} <button className="underline" onClick={() => load(storyId)}>Retry</button></p>;

  const { injection, injectionSaving, noteDraft } = state;
  const toggle = (change: Partial<InjectionSettings>) => save(storyId, change).catch(() => undefined);
  return (
    <div className="flex flex-col gap-3 text-xs">
      <label className="flex flex-col gap-1 text-muted">Entities
        <select value={injection.entities} disabled={injectionSaving}
          onChange={(event) => toggle({ entities: event.target.value as InjectionSettings["entities"] })}
          className="w-full rounded border border-border bg-bg px-2 py-1.5 text-text focus:outline-none focus:border-accent">
          <option value="none">Off</option>
          <option value="all">All</option>
          <option value="scoped">Only recent</option>
        </select>
      </label>
      <label className="flex items-center gap-2 text-text">
        <input type="checkbox" checked={injection.author_note_enabled} disabled={injectionSaving}
          onChange={(event) => toggle({ author_note_enabled: event.target.checked })} className="accent-accent" />
        Author's note enabled
      </label>
      <label className="flex flex-col gap-1 text-muted">Author's note
        <textarea value={noteDraft} onChange={(event) => setNoteDraft(storyId, event.target.value)} rows={5}
          placeholder="Tone, ongoing constraints, things to keep in mind."
          className="w-full resize-y rounded border border-border bg-bg px-2 py-1.5 text-sm text-text placeholder:text-muted focus:outline-none focus:border-accent" />
      </label>
      <button onClick={() => saveNote(storyId).catch(() => undefined)} disabled={injectionSaving || noteDraft === injection.author_note}
        className="rounded bg-accent px-2 py-1.5 font-medium text-bg hover:bg-accent-hover disabled:opacity-40 transition-colors">
        {injectionSaving ? "Saving..." : "Save"}
      </button>
      <label className="flex items-center gap-2 text-text">
        <input type="checkbox" checked={injection.tool_instructions} disabled={injectionSaving}
          onChange={(event) => toggle({ tool_instructions: event.target.checked })} className="accent-accent" />
        Tool instructions
      </label>
      {state.injectionError && <p role="alert" className="text-danger">{state.injectionError}</p>}
    </div>
  );
}
