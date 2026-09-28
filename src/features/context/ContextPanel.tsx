import { useEffect } from "react";
import type { ContextSettings } from "../../shared/types";
import { useStoryStore } from "../story/store";
import { useContextStore } from "./store";

export function ContextPanel() {
  const storyId = useStoryStore((state) => state.activeStoryId);
  const state = useContextStore((state) => storyId ? state.stories[storyId] : undefined);
  const load = useContextStore((state) => state.loadContextSettings);
  const save = useContextStore((state) => state.saveContextSettings);
  const saveNote = useContextStore((state) => state.saveNote);
  const setNoteDraft = useContextStore((state) => state.setNoteDraft);

  useEffect(() => { if (storyId) load(storyId); }, [storyId, load]);

  if (!storyId) return <p className="py-1 text-xs text-muted">Open a story first.</p>;
  if (!state?.context) return !state || state.contextLoading
    ? <p className="py-1 text-xs text-muted">Loading...</p>
    : <p className="text-xs text-danger">{state?.contextError ?? "Context could not be loaded."} <button className="underline" onClick={() => load(storyId)}>Retry</button></p>;

  const { context, contextSaving, noteDraft } = state;
  const toggle = (change: Partial<ContextSettings>) => save(storyId, change).catch(() => undefined);
  return (
    <div className="flex flex-col gap-3 text-xs">
      <label className="flex flex-col gap-1 text-muted">Entities
        <select value={context.entities} disabled={contextSaving}
          onChange={(event) => toggle({ entities: event.target.value as ContextSettings["entities"] })}
          className="w-full rounded border border-border bg-bg px-2 py-1.5 text-text focus:outline-none focus:border-accent">
          <option value="none">Off</option>
          <option value="all">All</option>
          <option value="scoped">Only recent</option>
        </select>
      </label>
      <label className="flex items-center gap-2 text-text">
        <input type="checkbox" checked={context.author_note_enabled} disabled={contextSaving}
          onChange={(event) => toggle({ author_note_enabled: event.target.checked })} className="accent-accent" />
        Author's note enabled
      </label>
      <label className="flex flex-col gap-1 text-muted">Author's note
        <textarea value={noteDraft} onChange={(event) => setNoteDraft(storyId, event.target.value)} rows={5}
          placeholder="Tone, ongoing constraints, things to keep in mind."
          className="w-full resize-y rounded border border-border bg-bg px-2 py-1.5 text-sm text-text placeholder:text-muted focus:outline-none focus:border-accent" />
      </label>
      <button onClick={() => saveNote(storyId).catch(() => undefined)} disabled={contextSaving || noteDraft === context.author_note}
        className="rounded bg-accent px-2 py-1.5 font-medium text-bg hover:bg-accent-hover disabled:opacity-40 transition-colors">
        {contextSaving ? "Saving..." : "Save"}
      </button>
      <label className="flex items-center gap-2 text-text">
        <input type="checkbox" checked={context.tool_instructions} disabled={contextSaving}
          onChange={(event) => toggle({ tool_instructions: event.target.checked })} className="accent-accent" />
        Tool instructions
      </label>
      {state.contextError && <p role="alert" className="text-danger">{state.contextError}</p>}
    </div>
  );
}
