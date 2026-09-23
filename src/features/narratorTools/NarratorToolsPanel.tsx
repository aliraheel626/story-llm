import { useEffect } from "react";
import { DEFAULT_NARRATOR_TOOLS, type NarratorToolSettings } from "../../shared/types";
import { useStoryStore } from "../story/store";

const GROUPS: { title: string; items: { key: keyof NarratorToolSettings; label: string; description: string }[] }[] = [
  {
    title: "Entity",
    items: [
      { key: "get_entities", label: "Look up entities", description: "The narrator can read characters and other world state." },
      { key: "create_entity", label: "Create entities", description: "The narrator can add persistent characters, places, and objects." },
      { key: "update_entity", label: "Update entities", description: "The narrator can change persistent world details." },
      { key: "adjust_entity_attribute", label: "Adjust attributes", description: "The narrator can change entity stats. Manual edits remain available when off." },
    ],
  },
  {
    title: "Dice",
    items: [
      { key: "roll_check", label: "Roll uncertain outcomes", description: "The narrator may give a chance or reference up to two stored attributes. Rolls read stats but never change them." },
    ],
  },
  {
    title: "Image",
    items: [
      { key: "illustrate_scene", label: "Illustrate scenes", description: "Enables See and narrator-initiated images when the global image service and key are configured. Images may incur provider charges." },
    ],
  },
];

export function NarratorToolsPanel() {
  const storyId = useStoryStore((s) => s.activeStoryId);
  const draft = useStoryStore((s) => s.draft);
  const creating = useStoryStore((s) => s.creatingStory);
  const loaded = useStoryStore((s) => storyId ? s.bundles[storyId]?.narratorTools : null);
  const draftTools = useStoryStore((s) => s.draftNarratorTools);
  const loading = useStoryStore((s) => storyId ? s.bundles[storyId]?.narratorToolsLoading : false);
  const error = useStoryStore((s) => storyId ? s.bundles[storyId]?.narratorToolsError : null);
  const load = useStoryStore((s) => s.loadNarratorTools);
  const save = useStoryStore((s) => s.saveNarratorTools);

  useEffect(() => {
    if (storyId) load(storyId);
  }, [storyId, load]);

  if (!storyId && !draft) return <p className="text-xs text-muted">Open or start a story to choose its narrator tools.</p>;
  if (storyId && !loaded) {
    return loading
      ? <p className="text-xs text-muted">Loading narrator tools...</p>
      : <div className="text-xs text-danger">{error ?? "Narrator tools could not be loaded."} <button className="underline" onClick={() => load(storyId)}>Retry</button></div>;
  }

  const tools = storyId ? loaded! : draftTools ?? DEFAULT_NARRATOR_TOOLS;

  return (
    <div className="flex flex-col gap-3 text-xs">
      <p className="rounded border border-border bg-bg px-2 py-2 leading-5 text-muted">
        All six tools start on for every story. Entity tools can change persistent world state; dice and image calls can add cost. Turn off any tool independently before writing.
      </p>
      {GROUPS.map((group) => (
        <fieldset key={group.title} className="flex flex-col gap-1.5 border-t border-border pt-2">
          <legend className="px-1 font-medium uppercase tracking-wider text-muted">{group.title}</legend>
          {group.items.map((item) => (
            <label key={item.key} className="flex cursor-pointer items-start gap-2 rounded border border-border bg-bg px-2 py-2 has-[:checked]:border-accent">
              <input
                type="checkbox"
                checked={tools[item.key]}
                disabled={creating}
                onChange={(event) => {
                  save(storyId, { [item.key]: event.target.checked }).catch((cause) => console.error("failed to save narrator tools", cause));
                }}
                className="mt-0.5 accent-accent"
              />
              <span><span className="text-text">{item.label}</span><span className="mt-0.5 block leading-4 text-muted">{item.description}</span></span>
            </label>
          ))}
        </fieldset>
      ))}
      {error && <p role="alert" className="text-danger">Unable to save narrator tools: {error}</p>}
    </div>
  );
}
