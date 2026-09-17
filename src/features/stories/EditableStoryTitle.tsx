import { useEffect, useRef, useState } from "react";
import { useStoryStore } from "../story/store";
import { DEFAULT_STORY_TITLE } from "../../shared/types";

/** Click-to-edit story title. The model fills it in after the first passage
 *  (see the `story-title-updated` event); until then the placeholder shows
 *  in muted italics, ChatGPT-style. */
export function EditableStoryTitle({ storyId }: { storyId: string }) {
  const title = useStoryStore((s) => s.stories.find((st) => st.id === storyId)?.title ?? DEFAULT_STORY_TITLE);
  const renameStory = useStoryStore((s) => s.renameStory);

  const [editing, setEditing] = useState(false);
  const [draft, setDraft] = useState(title);
  const cancelledRef = useRef(false);
  const inputRef = useRef<HTMLInputElement>(null);

  useEffect(() => {
    if (editing) {
      inputRef.current?.focus();
      inputRef.current?.select();
    }
  }, [editing]);

  const isPlaceholder = title === DEFAULT_STORY_TITLE;

  const startEditing = () => {
    setDraft(title);
    cancelledRef.current = false;
    setEditing(true);
  };

  const commit = () => {
    setEditing(false);
    if (cancelledRef.current) return;
    const trimmed = draft.trim();
    if (!trimmed || trimmed === title) return;
    renameStory(storyId, trimmed).catch((e) => console.error("failed to rename story", e));
  };

  if (editing) {
    return (
      <input
        ref={inputRef}
        value={draft}
        onChange={(e) => setDraft(e.target.value)}
        onKeyDown={(e) => {
          if (e.key === "Enter") commit();
          if (e.key === "Escape") {
            cancelledRef.current = true;
            setEditing(false);
          }
        }}
        onBlur={commit}
        aria-label="Story title"
        className={`w-full rounded border border-border bg-transparent px-1 py-0.5 font-prose text-2xl outline-none focus:border-accent ${
          isPlaceholder ? "italic text-muted" : "text-text"
        }`}
      />
    );
  }

  return (
    <h1
      onClick={startEditing}
      title="Click to rename"
      className={`cursor-text font-prose text-2xl ${isPlaceholder ? "italic text-muted" : "text-text"}`}
    >
      {title}
    </h1>
  );
}
