import { useEffect, useState } from "react";
import { useAppStore } from "../../app/store";
import { useAuthorNoteStore } from "./store";

export function WorldPanel() {
  const activeStoryId = useAppStore((s) => s.activeStoryId);
  const noteByStory = useAuthorNoteStore((s) => s.noteByStory);
  const loading = useAuthorNoteStore((s) => s.loading);
  const saving = useAuthorNoteStore((s) => s.saving);
  const load = useAuthorNoteStore((s) => s.load);
  const save = useAuthorNoteStore((s) => s.save);

  const [draft, setDraft] = useState("");
  const [savedNotice, setSavedNotice] = useState(false);

  useEffect(() => {
    if (activeStoryId) load(activeStoryId);
  }, [activeStoryId, load]);

  useEffect(() => {
    if (activeStoryId) setDraft(noteByStory[activeStoryId] ?? "");
  }, [activeStoryId, noteByStory]);

  if (!activeStoryId) {
    return <div className="text-xs text-muted py-1">Open a story first.</div>;
  }
  if (loading && noteByStory[activeStoryId] === undefined) {
    return <div className="text-xs text-muted py-1">Loading...</div>;
  }

  const onSave = async () => {
    try {
      await save(activeStoryId, draft);
      setSavedNotice(true);
      setTimeout(() => setSavedNotice(false), 2000);
    } catch (e) {
      console.error(e);
    }
  };

  return (
    <div className="flex flex-col gap-2 text-sm">
      <div>
        <label className="block text-xs text-muted mb-1">Author's note</label>
        <p className="mb-1.5 text-[11px] text-muted">
          A persistent instruction sent with every turn — tone, ongoing constraints, things to keep in mind.
        </p>
        <textarea
          value={draft}
          onChange={(e) => setDraft(e.target.value)}
          placeholder="e.g. Keep the tone grim and understated. Never let the player die outright — injure instead."
          rows={5}
          className="w-full resize-y rounded bg-bg border border-border px-2 py-1.5 text-sm text-text placeholder:text-muted focus:outline-none focus:border-accent"
        />
      </div>
      <button
        onClick={onSave}
        disabled={saving}
        className="rounded bg-accent px-2 py-1.5 text-xs font-medium text-bg hover:bg-accent-hover disabled:opacity-40 transition-colors"
      >
        {saving ? "Saving..." : savedNotice ? "Saved" : "Save"}
      </button>
    </div>
  );
}
