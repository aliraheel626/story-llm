import { useEffect, useRef, useState } from "react";
import { useAppStore } from "../../app/store";
import { DEFAULT_MECHANICS_SETTINGS, useMechanicsStore } from "../mechanics/store";
import { useImageModelStore } from "../settings/imageModelStore";
import { useStoryStore } from "./store";
import type { ReasoningEffort } from "../../shared/types";

type Mode = "do" | "say" | "story" | "guide" | "see";

const MODES: { id: Mode; label: string; placeholder: string }[] = [
  { id: "do", label: "Do", placeholder: "What do you do?" },
  { id: "say", label: "Say", placeholder: "What do you say?" },
  { id: "story", label: "Story", placeholder: "Write the next passage yourself..." },
  { id: "guide", label: "Guide", placeholder: "Steer the story out of character (won't appear as an action)..." },
  { id: "see", label: "See", placeholder: 'Optional: what to show — "the bucket", "the girl you are seeing"...' },
];

const EFFORT_OPTIONS: { value: ReasoningEffort | ""; label: string }[] = [
  { value: "", label: "Model default" },
  { value: "none", label: "None" },
  { value: "minimal", label: "Minimal" },
  { value: "low", label: "Low" },
  { value: "medium", label: "Medium" },
  { value: "high", label: "High" },
  { value: "xhigh", label: "Extra high" },
  { value: "max", label: "Max" },
];

/** `branchId` is null while composing a not-yet-persisted draft story; the
 *  first submit creates the story and branch (see `ensureBranch`), so an
 *  abandoned draft never leaves an empty story behind. */
export function Composer({ branchId }: { branchId: string | null }) {
  const [mode, setMode] = useState<Mode>("do");
  const [text, setText] = useState("");
  const [submitting, setSubmitting] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const textareaRef = useRef<HTMLTextAreaElement>(null);

  const createStory = useAppStore((s) => s.createStory);
  const submitTurn = useStoryStore((s) => s.submitTurn);
  const submitStoryText = useStoryStore((s) => s.submitStoryText);
  const submitGuide = useStoryStore((s) => s.submitGuide);
  const continueScene = useStoryStore((s) => s.continueScene);
  const streaming = useStoryStore((s) => (branchId ? s.streamingByBranch[branchId] : undefined));
  const turnError = useStoryStore((s) => s.turnError);
  const entries = useStoryStore((s) => (branchId ? s.entriesByBranch[branchId] : undefined));
  const generateImageForEntry = useStoryStore((s) => s.generateImageForEntry);
  const imagePendingFor = useStoryStore((s) => s.imagePendingFor);
  const imageError = useStoryStore((s) => s.imageError);
  const imageSettings = useImageModelStore((s) => s.settings);
  const loadImageSettings = useImageModelStore((s) => s.load);
  const activeStoryId = useAppStore((s) => s.activeStoryId);
  const saveSettings = useMechanicsStore((s) => s.saveSettings);
  const reasoningEffort =
    (useMechanicsStore((s) => (activeStoryId ? s.settingsByStory[activeStoryId] : s.draftSettings)) ?? DEFAULT_MECHANICS_SETTINGS)
      .reasoning_effort;

  const changeReasoningEffort = async (value: string) => {
    try {
      await saveSettings(activeStoryId, branchId, { reasoning_effort: (value || null) as ReasoningEffort | null });
    } catch (e) {
      console.error("failed to save reasoning effort", e);
    }
  };

  const busy = submitting || !!streaming;
  const lastEntry = entries && entries.length > 0 ? entries[entries.length - 1] : undefined;
  const imageBusy = !!lastEntry && imagePendingFor.includes(lastEntry.id);
  const imagesDisabled = imageSettings ? !imageSettings.enabled : false;

  useEffect(() => {
    loadImageSettings();
  }, [loadImageSettings]);

  useEffect(() => {
    const el = textareaRef.current;
    if (!el) return;
    el.style.height = "auto";
    const maxHeight = window.innerHeight * 0.4;
    el.style.height = `${Math.min(el.scrollHeight, maxHeight)}px`;
  }, [text, mode]);

  /** Lazily persists the story on first submit. No-op once it exists. */
  const ensureBranch = async (): Promise<string> => {
    if (branchId) return branchId;
    const story = await createStory();
    if (!story.default_branch_id) throw new Error("new story has no branch");
    return story.default_branch_id;
  };

  const onSubmit = async () => {
    if (busy) return;
    setError(null);

    if (mode === "see") {
      if (!lastEntry || imageBusy || imagesDisabled) return;
      setSubmitting(true);
      try {
        await generateImageForEntry(lastEntry.id, text.trim() || undefined);
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
      const branch = await ensureBranch();
      if (mode === "story") {
        await submitStoryText(branch, trimmed);
      } else if (mode === "guide") {
        await submitGuide(branch, trimmed);
      } else {
        await submitTurn(branch, mode, trimmed);
      }
      setText("");
    } catch (e) {
      setError(String(e));
    } finally {
      setSubmitting(false);
    }
  };

  const onContinue = async () => {
    if (busy || !branchId || !lastEntry) return;
    setSubmitting(true);
    setError(null);
    try {
      await continueScene(branchId);
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
                {EFFORT_OPTIONS.map((option) => (
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
