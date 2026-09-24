import { useEffect, useRef, useState } from "react";
import type { EntityContextMode } from "../../shared/types";
import { useStoryStore } from "../story/store";
import { writingStyleApi } from "../writingStyle/api";
import { useContextInjectionStore } from "./contextInjectionStore";
import { useSettingsForm } from "./useSettingsForm";

type EnabledEntityContextMode = Exclude<EntityContextMode, "none">;

export function ContextInjectionPanel() {
  const activeStoryId = useStoryStore((state) => state.activeStoryId);
  const [entityContextMode, setEntityContextMode] = useState<EntityContextMode>("all");
  const [lastEnabledMode, setLastEnabledMode] = useState<EnabledEntityContextMode>("all");
  const lastPersistedEntityMode = useRef<EntityContextMode | null>(null);
  const [authorNoteEnabled, setAuthorNoteEnabled] = useState(true);
  const [authorNoteLoading, setAuthorNoteLoading] = useState(false);
  const [authorNoteSaving, setAuthorNoteSaving] = useState(false);
  const authorNoteRequest = useRef(0);
  const { settings, loading, saving, savedNotice, save } = useSettingsForm(useContextInjectionStore, (next) => {
    if (lastPersistedEntityMode.current !== next.entity_context_mode) {
      setEntityContextMode(next.entity_context_mode);
      if (next.entity_context_mode !== "none") setLastEnabledMode(next.entity_context_mode);
      lastPersistedEntityMode.current = next.entity_context_mode;
    }
  }, true);

  useEffect(() => {
    const request = ++authorNoteRequest.current;
    let current = true;
    if (!activeStoryId) {
      setAuthorNoteEnabled(true);
      setAuthorNoteLoading(false);
      setAuthorNoteSaving(false);
      return () => { current = false; };
    }

    setAuthorNoteEnabled(true);
    setAuthorNoteLoading(true);
    setAuthorNoteSaving(false);
    writingStyleApi.getAuthorNoteEnabled(activeStoryId)
      .then((enabled) => {
        if (current && authorNoteRequest.current === request) setAuthorNoteEnabled(enabled);
      })
      .catch((error) => console.error("failed to load author's note setting", error))
      .finally(() => {
        if (current && authorNoteRequest.current === request) setAuthorNoteLoading(false);
      });
    return () => { current = false; };
  }, [activeStoryId]);

  const setEntityContextEnabled = (enabled: boolean) => {
    if (enabled) {
      setEntityContextMode(lastEnabledMode);
    } else {
      if (entityContextMode !== "none") setLastEnabledMode(entityContextMode);
      setEntityContextMode("none");
    }
  };

  const selectEntityContextMode = (mode: EnabledEntityContextMode) => {
    setLastEnabledMode(mode);
    setEntityContextMode(mode);
  };

  const setAuthorNoteMuted = async (muted: boolean) => {
    if (!activeStoryId) return;
    const storyId = activeStoryId;
    const request = ++authorNoteRequest.current;
    const previous = authorNoteEnabled;
    const enabled = !muted;
    setAuthorNoteEnabled(enabled);
    setAuthorNoteSaving(true);
    try {
      await writingStyleApi.setAuthorNoteEnabled(storyId, enabled);
    } catch (error) {
      console.error("failed to save author's note setting", error);
      if (useStoryStore.getState().activeStoryId === storyId && authorNoteRequest.current === request) {
        setAuthorNoteEnabled(previous);
      }
    } finally {
      if (useStoryStore.getState().activeStoryId === storyId && authorNoteRequest.current === request) {
        setAuthorNoteSaving(false);
      }
    }
  };

  if (loading && !settings) {
    return <div className="text-xs text-muted py-1">Loading...</div>;
  }

  return (
    <div className="flex flex-col gap-3 text-sm">
      <fieldset>
        <legend className="mb-1 text-xs text-muted">Entity context</legend>
        <div className="flex gap-3 text-xs text-text">
          <label className="flex items-center gap-1.5">
            <input
              type="radio"
              name="entity-context-enabled"
              checked={entityContextMode !== "none"}
              onChange={() => setEntityContextEnabled(true)}
              className="accent-accent"
            />
            On
          </label>
          <label className="flex items-center gap-1.5">
            <input
              type="radio"
              name="entity-context-enabled"
              checked={entityContextMode === "none"}
              onChange={() => setEntityContextEnabled(false)}
              className="accent-accent"
            />
            Off
          </label>
        </div>
      </fieldset>

      {entityContextMode !== "none" && (
        <div>
          <label className="mb-1 block text-xs text-muted" htmlFor="entity-context-mode">Context detail</label>
          <select
            id="entity-context-mode"
            value={entityContextMode}
            onChange={(event) => selectEntityContextMode(event.target.value as EnabledEntityContextMode)}
            className="w-full rounded bg-bg border border-border px-2 py-1.5 text-xs text-text focus:outline-none focus:border-accent"
          >
            <option value="all">Full entity dump every turn</option>
            <option value="scoped">Scoped to recently active entities</option>
          </select>
        </div>
      )}

      <label className="flex items-start gap-2 text-xs text-muted">
        <input
          type="checkbox"
          checked={!authorNoteEnabled}
          onChange={(event) => setAuthorNoteMuted(event.target.checked)}
          disabled={!activeStoryId || authorNoteLoading || authorNoteSaving}
          className="mt-0.5 accent-accent"
        />
        <span>
          Mute author's note for this story
          <span className="mt-0.5 block text-[11px]">
            {activeStoryId ? "Keeps the saved note without sending it to the narrator." : "Open a story to change this setting."}
          </span>
        </span>
      </label>

      <button
        onClick={() => save(entityContextMode)}
        disabled={saving}
        className="rounded bg-accent px-2 py-1.5 text-xs font-medium text-bg hover:bg-accent-hover disabled:opacity-40 transition-colors"
      >
        {saving ? "Saving..." : savedNotice ? "Saved" : "Save"}
      </button>
    </div>
  );
}
