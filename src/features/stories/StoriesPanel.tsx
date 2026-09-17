import { useEffect, useState } from "react";
import { useStoryStore } from "../story/store";
import { DEFAULT_STORY_TITLE } from "../../shared/types";

export function StoriesPanel() {
  const stories = useStoryStore((s) => s.stories);
  const storiesLoading = useStoryStore((s) => s.storiesLoading);
  const activeStoryId = useStoryStore((s) => s.activeStoryId);
  const draft = useStoryStore((s) => s.draft);
  const loadStories = useStoryStore((s) => s.loadStories);
  const startDraft = useStoryStore((s) => s.startDraft);
  const setActiveStory = useStoryStore((s) => s.setActiveStory);
  const deleteStory = useStoryStore((s) => s.deleteStory);

  const [deletingId, setDeletingId] = useState<string | null>(null);

  useEffect(() => {
    loadStories();
  }, [loadStories]);

  const onDelete = async (e: React.MouseEvent, storyId: string) => {
    e.stopPropagation();
    if (deletingId) return;
    setDeletingId(storyId);
    try {
      await deleteStory(storyId);
    } catch (err) {
      console.error("failed to delete story", err);
    } finally {
      setDeletingId(null);
    }
  };

  return (
    <div className="flex flex-col gap-2">
      <button
        onClick={startDraft}
        className={`w-full rounded border border-dashed px-2 py-1.5 text-xs transition-colors ${
          draft
            ? "border-accent text-text bg-surface-hover"
            : "border-border text-muted hover:text-text hover:border-accent"
        }`}
      >
        + New story
      </button>

      <div className="flex flex-col gap-0.5 mt-1">
        {storiesLoading && <div className="text-xs text-muted px-1 py-1">Loading...</div>}
        {!storiesLoading && stories.length === 0 && !draft && (
          <div className="text-xs text-muted px-1 py-1">No stories yet.</div>
        )}
        {stories.map((story) => (
          <div key={story.id} className="group relative">
            <button
              onClick={() => setActiveStory(story.id)}
              className={`w-full truncate rounded py-1.5 pl-2 pr-7 text-left text-sm transition-colors ${
                activeStoryId === story.id
                  ? "bg-surface-hover text-text"
                  : "text-muted hover:bg-surface-hover hover:text-text"
              } ${story.title === DEFAULT_STORY_TITLE ? "italic" : ""}`}
              title={story.title}
            >
              {story.title}
            </button>
            <button
              onClick={(e) => onDelete(e, story.id)}
              disabled={deletingId === story.id}
              title="Delete story"
              className="absolute right-1 top-1/2 -translate-y-1/2 rounded px-1.5 py-0.5 text-[11px] text-danger opacity-0 transition-opacity hover:opacity-100 group-hover:opacity-100 disabled:opacity-40"
            >
              ✕
            </button>
          </div>
        ))}
      </div>
    </div>
  );
}
