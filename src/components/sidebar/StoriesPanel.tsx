import { useEffect, useState } from "react";
import { useAppStore } from "../../store/appStore";

export function StoriesPanel() {
  const stories = useAppStore((s) => s.stories);
  const storiesLoading = useAppStore((s) => s.storiesLoading);
  const activeStoryId = useAppStore((s) => s.activeStoryId);
  const loadStories = useAppStore((s) => s.loadStories);
  const createStory = useAppStore((s) => s.createStory);
  const setActiveStory = useAppStore((s) => s.setActiveStory);

  const [creating, setCreating] = useState(false);
  const [title, setTitle] = useState("");
  const [submitting, setSubmitting] = useState(false);

  useEffect(() => {
    loadStories();
  }, [loadStories]);

  const submitNewStory = async () => {
    const trimmed = title.trim();
    if (!trimmed || submitting) return;
    setSubmitting(true);
    try {
      await createStory(trimmed);
      setTitle("");
      setCreating(false);
    } catch (e) {
      console.error(e);
    } finally {
      setSubmitting(false);
    }
  };

  return (
    <div className="flex flex-col gap-2">
      {creating ? (
        <div className="flex flex-col gap-1.5">
          <input
            autoFocus
            value={title}
            onChange={(e) => setTitle(e.target.value)}
            onKeyDown={(e) => {
              if (e.key === "Enter") submitNewStory();
              if (e.key === "Escape") setCreating(false);
            }}
            placeholder="Story title..."
            className="w-full rounded bg-bg border border-border px-2 py-1.5 text-sm text-text placeholder:text-muted focus:outline-none focus:border-accent"
          />
          <div className="flex gap-1.5">
            <button
              onClick={submitNewStory}
              disabled={submitting || !title.trim()}
              className="flex-1 rounded bg-accent px-2 py-1 text-xs font-medium text-bg hover:bg-accent-hover disabled:opacity-40 transition-colors"
            >
              Create
            </button>
            <button
              onClick={() => setCreating(false)}
              className="rounded border border-border px-2 py-1 text-xs text-muted hover:text-text transition-colors"
            >
              Cancel
            </button>
          </div>
        </div>
      ) : (
        <button
          onClick={() => setCreating(true)}
          className="w-full rounded border border-dashed border-border px-2 py-1.5 text-xs text-muted hover:text-text hover:border-accent transition-colors"
        >
          + New story
        </button>
      )}

      <div className="flex flex-col gap-0.5 mt-1">
        {storiesLoading && <div className="text-xs text-muted px-1 py-1">Loading...</div>}
        {!storiesLoading && stories.length === 0 && (
          <div className="text-xs text-muted px-1 py-1">No stories yet.</div>
        )}
        {stories.map((story) => (
          <button
            key={story.id}
            onClick={() => setActiveStory(story.id)}
            className={`w-full truncate rounded px-2 py-1.5 text-left text-sm transition-colors ${
              activeStoryId === story.id
                ? "bg-surface-hover text-text"
                : "text-muted hover:bg-surface-hover hover:text-text"
            }`}
            title={story.title}
          >
            {story.title}
          </button>
        ))}
      </div>
    </div>
  );
}
