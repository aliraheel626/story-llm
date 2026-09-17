import { useEffect, useRef, useState } from "react";
import { REASONING_EFFORT_OPTIONS, type ReasoningEffort } from "../../shared/types";
import { useImageModelStore } from "../settings/imageModelStore";
import { DEFAULT_DICEROLL_SETTINGS, useStoryStore } from "../story/store";

type Mode = "do" | "say" | "story" | "guide" | "see";

const MODES: { id: Mode; label: string; placeholder: string }[] = [
  { id: "do", label: "Do", placeholder: "What do you do?" },
  { id: "say", label: "Say", placeholder: "What do you say?" },
  { id: "story", label: "Story", placeholder: "Write the next passage yourself..." },
  { id: "guide", label: "Guide", placeholder: "Steer the story out of character (won't appear as an action)..." },
  { id: "see", label: "See", placeholder: 'Optional: what to show — "the bucket", "the girl you are seeing"...' },
];

/** `storyId` is null while composing a not-yet-persisted draft story; the
 *  first submit creates the story (see `ensureStory`), so an
 *  abandoned draft never leaves an empty story behind. */
export function Composer({ storyId }: { storyId: string | null }) {
  const [mode, setMode] = useState<Mode>("do");
  const [text, setText] = useState("");
  const [submitting, setSubmitting] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const textareaRef = useRef<HTMLTextAreaElement>(null);

  const createStory = useStoryStore((s) => s.createStory);
  const submitTurn = useStoryStore((s) => s.submitTurn);
  const submitStoryText = useStoryStore((s) => s.submitStoryText);
  const submitGuide = useStoryStore((s) => s.submitGuide);
  const continueScene = useStoryStore((s) => s.continueScene);
  const streaming = useStoryStore((s) => storyId ? s.bundles[storyId]?.streaming : undefined);
  const timelineLoading = useStoryStore((s) => storyId ? (s.bundles[storyId]?.timelineLoading ?? false) : false);
  const turnError = useStoryStore((s) => storyId ? s.bundles[storyId]?.turnError : null);
  const entries = useStoryStore((s) => storyId ? s.bundles[storyId]?.entries : undefined);
  const generateImageForEntry = useStoryStore((s) => s.generateImageForEntry);
  const imagePendingFor = useStoryStore((s) => storyId ? s.bundles[storyId]?.imagePendingFor : undefined);
  const imageError = useStoryStore((s) => storyId ? s.bundles[storyId]?.imageError : null);
  const imageSettings = useImageModelStore((s) => s.settings);
  const loadImageSettings = useImageModelStore((s) => s.load);
  const saveSettings = useStoryStore((s) => s.saveDiceSettings);
  const loadDiceSettings = useStoryStore((s) => s.loadDiceSettings);
  const reasoningEffort =
    (useStoryStore((s) => (storyId ? s.bundles[storyId]?.diceSettings : s.draftDiceSettings)) ?? DEFAULT_DICEROLL_SETTINGS)
      .reasoning_effort;

  const changeReasoningEffort = async (value: string) => {
    try {
      await saveSettings(storyId, { reasoning_effort: (value || null) as ReasoningEffort | null });
    } catch (e) {
      console.error("failed to save reasoning effort", e);
    }
  };

  const busy = submitting || !!streaming || timelineLoading;
  const lastEntry = entries && entries.length > 0 ? entries[entries.length - 1] : undefined;
  const imageBusy = !!lastEntry && (imagePendingFor?.includes(lastEntry.id) ?? false);
  const imagesDisabled = imageSettings ? !imageSettings.enabled : false;

  useEffect(() => {
    loadImageSettings();
  }, [loadImageSettings]);

  useEffect(() => {
    if (storyId) loadDiceSettings(storyId);
  }, [storyId, loadDiceSettings]);

  useEffect(() => {
    const el = textareaRef.current;
    if (!el) return;
    el.style.height = "auto";
    const maxHeight = window.innerHeight * 0.4;
    el.style.height = `${Math.min(el.scrollHeight, maxHeight)}px`;
  }, [text, mode]);

  /** Lazily persists the story on first submit. No-op once it exists. */
  const ensureStory = async (): Promise<string> => {
    if (storyId) return storyId;
    const story = await createStory();
    return story.id;
  };

  const onSubmit = async () => {
    if (busy) return;
    setError(null);

    if (mode === "see") {
      if (!lastEntry || imageBusy || imagesDisabled) return;
      setSubmitting(true);
      try {
        await generateImageForEntry(storyId!, lastEntry.id, text.trim() || undefined);
        setText("");
      } catch (e) {
        setError(String(e));
      } finally {
        setSubmitting(false);
      }
      return;
    }

    const trimmed = text.trim();
    if (!trimmed) return;
    setSubmitting(true);
    try {
      const persistedStoryId = await ensureStory();
      if (mode === "story") {
        await submitStoryText(persistedStoryId, trimmed);
      } else if (mode === "guide") {
        await submitGuide(persistedStoryId, trimmed);
      } else {
        await submitTurn(persistedStoryId, mode, trimmed);
      }
      setText("");
    } catch (e) {
      setError(String(e));
    } finally {
      setSubmitting(false);
    }
  };

  const onContinue = async () => {
    if (busy || !storyId || !lastEntry) return;
    setSubmitting(true);
    setError(null);
    try {
      await continueScene(storyId);
    } catch (e) {
      setError(String(e));
    } finally {
      setSubmitting(false);
    }
  };

  const onKeyDown = (e: React.KeyboardEvent<HTMLTextAreaElement>) => {
    if (e.key === "Enter" && !e.shiftKey && !e.nativeEvent.isComposing) {
      e.preventDefault();
      onSubmit();
    }
  };

  const activeMode = MODES.find((m) => m.id === mode)!;
  const displayedError = mode === "see" ? (error ?? imageError) : (error ?? turnError);

  const submitDisabled = mode === "see" ? busy || !lastEntry || imageBusy || imagesDisabled : busy || !text.trim();

  const submitLabel = mode === "see" ? (imageBusy ? "Generating..." : "Generate") : busy ? "Writing..." : "Send";

  const statusHint =
    mode === "see"
      ? imagesDisabled
        ? "Image generation is disabled in the Image Model panel."
        : !lastEntry
          ? "Write a passage first, then generate an image."
          : (displayedError ?? "Enter to generate · Shift + Enter for a new line")
      : (displayedError ?? "Enter to send · Shift + Enter for a new line");

  return (
    <div className="shrink-0 border-t border-border bg-surface px-6 py-3">
      <div className="mx-auto w-full max-w-measure">
        <div className="mb-2 flex items-center justify-between gap-1.5">
          <div className="flex gap-1.5">
            {MODES.map((m) => (
              <button
                key={m.id}
                onClick={() => setMode(m.id)}
                className={`rounded px-3 py-1 text-xs font-medium transition-colors ${
                  mode === m.id ? "bg-accent text-bg" : "border border-border bg-bg text-muted hover:text-text"
                }`}
              >
                {m.label}
              </button>
            ))}
          </div>
          <button
            onClick={onContinue}
            disabled={busy || !lastEntry}
            title="Advance the scene with no player input"
            className="rounded border border-border bg-bg px-3 py-1 text-xs font-medium text-muted transition-colors hover:text-text disabled:opacity-40"
          >
            Continue ⏵
          </button>
        </div>

        <textarea
          ref={textareaRef}
          value={text}
          onChange={(e) => setText(e.target.value)}
          onKeyDown={onKeyDown}
          placeholder={activeMode.placeholder}
          rows={2}
          disabled={busy || (mode === "see" && imagesDisabled)}
          className="w-full resize-none overflow-y-auto rounded border border-border bg-bg px-3 py-2 font-prose text-sm leading-6 text-text placeholder:text-muted focus:outline-none focus:border-accent disabled:opacity-60"
        />
        <div className="mt-2 flex items-center justify-between gap-2">
          <span className="text-xs text-muted">
            {displayedError ? <span className="text-danger">{displayedError}</span> : statusHint}
          </span>
          <div className="flex shrink-0 items-center gap-2">
            <label
              className="flex items-center gap-1.5 text-xs text-muted"
              title="How much the model reasons before writing. Changing it mid-story changes the request prefix, so the provider's prompt cache is invalidated once."
            >
              <span>Effort</span>
              <select
                value={reasoningEffort ?? ""}
                onChange={(e) => changeReasoningEffort(e.target.value)}
                disabled={busy}
                className="rounded border border-border bg-bg px-1.5 py-0.5 text-xs text-text focus:border-accent focus:outline-none disabled:opacity-60"
              >
                {REASONING_EFFORT_OPTIONS.map((option) => (
                  <option key={option.value} value={option.value}>
                    {option.label}
                  </option>
                ))}
              </select>
            </label>
            <button
              onClick={onSubmit}
              disabled={submitDisabled}
              className="rounded bg-accent px-4 py-1.5 text-xs font-medium text-bg hover:bg-accent-hover disabled:opacity-40 transition-colors"
            >
              {submitLabel}
            </button>
          </div>
        </div>
      </div>
    </div>
  );
}
