import { useEffect } from "react";
import { listen } from "@tauri-apps/api/event";
import { useTextModelStore } from "../settings/textModelStore";
import { useStoryStore } from "../story/store";
import { useContextStore } from "./store";

export function TranscriptPanel() {
  const storyId = useStoryStore((state) => state.activeStoryId);
  const state = useContextStore((state) => storyId ? state.stories[storyId] : undefined);
  const load = useContextStore((state) => state.loadTranscriptSettings);
  const toggle = useContextStore((state) => state.toggleTranscriptItem);
  const loadPreview = useContextStore((state) => state.loadPreview);
  const model = useTextModelStore((state) => state.settings);
  const loadModel = useTextModelStore((state) => state.load);

  useEffect(() => {
    if (storyId) load(storyId);
  }, [storyId, load]);
  useEffect(() => {
    if (!storyId) return;
    let active = true;
    let unlisten: (() => void) | undefined;
    listen("text-model-capabilities-refreshed", () => { if (active) void loadModel(); })
      .then((dispose) => {
        if (active) { unlisten = dispose; void loadModel(); }
        else dispose();
      })
      .catch(() => { if (active) void loadModel(); });
    return () => { active = false; unlisten?.(); };
  }, [storyId, loadModel]);

  if (!storyId) return <p className="py-1 text-xs text-muted">Open a story first.</p>;
  if (!state?.items) return !state || state.transcriptLoading
    ? <p className="py-1 text-xs text-muted">Loading...</p>
    : <p className="text-xs text-danger">{state?.transcriptError ?? "Transcript could not be loaded."} <button className="underline" onClick={() => load(storyId)}>Retry</button></p>;

  const groups = new Map<string, typeof state.items>();
  for (const item of state.items) {
    if (!groups.has(item.group)) groups.set(item.group, []);
    groups.get(item.group)!.push(item);
  }

  return (
    <div className="flex flex-col gap-3 text-xs">
      {[...groups].map(([group, items]) => (
        <fieldset key={group} className="flex flex-col gap-1.5 border-t border-border pt-2">
          <legend className="px-1 font-medium uppercase tracking-wider text-muted">{group}</legend>
          {items.map((item) => (
            <label key={item.key} className="flex cursor-pointer items-start gap-2 rounded border border-border bg-bg px-2 py-2 has-[:checked]:border-accent">
              <input type="checkbox" checked={item.enabled} disabled={state.transcriptSaving}
                onChange={(event) => toggle(storyId, item.key, event.target.checked).catch(() => undefined)}
                className="mt-0.5 accent-accent" />
              <span className="text-text">{item.label}</span>
            </label>
          ))}
        </fieldset>
      ))}
      {state.items.some((item) => item.key === "images" && item.enabled) && model?.supports_images === false &&
        <p className="text-muted">Your text model doesn't accept images, so this has no effect.</p>}
      {state.transcriptError && <p role="alert" className="text-danger">{state.transcriptError}</p>}
      <button onClick={() => loadPreview(storyId)} disabled={state.transcriptSaving || state.contextSaving || state.previewLoading}
        className="rounded bg-accent px-2 py-1.5 font-medium text-bg hover:bg-accent-hover disabled:opacity-40 transition-colors">
        {state.previewLoading ? "Loading..." : "Preview next request"}
      </button>
      {state.previewError && <p role="alert" className="text-danger">{state.previewError}</p>}
      {state.preview && <div className="flex flex-col gap-2 rounded border border-border bg-bg p-2 text-muted">
        <p className="font-medium text-text">System: {state.preview.system.slice(0, 200)}</p>
        {state.preview.messages.map((message, index) => (
          <p key={index} className="break-words">
            <span className="font-medium text-text">{message.role}</span>: {message.text.slice(0, 200)}
            <span className="block">Images: {message.image_count} | Reasoning: {message.has_reasoning ? "Yes" : "No"}</span>
          </p>
        ))}
        <p className="break-words">Injected: {state.preview.injected.slice(0, 200)}</p>
        {state.preview.images_unsupported && <p>Images are not supported by this text model.</p>}
      </div>}
    </div>
  );
}
