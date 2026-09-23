import { useEffect, useRef, useState } from "react";
import { REASONING_EFFORT_OPTIONS, type ActionMode, type ReasoningEffort } from "../../shared/types";
import { useImageModelStore } from "../settings/imageModelStore";
import { DEFAULT_DICEROLL_SETTINGS, useStoryStore } from "../story/store";

export type ModeDisplay = "bubble" | "chip" | "hidden";
type DisabledWhen = "image-unavailable" | "no-entry" | null;
export interface ModeDefinition {
  id: ActionMode;
  label: string;
  placeholder: string;
  textRequired: boolean;
  display: ModeDisplay;
  composerTab: boolean;
  disabledWhen: DisabledWhen;
}

export const MODES: readonly ModeDefinition[] = [
  { id: "do", label: "Do", placeholder: "What do you do?", textRequired: true, display: "bubble", composerTab: true, disabledWhen: null },
  { id: "say", label: "Say", placeholder: "What do you say?", textRequired: true, display: "bubble", composerTab: true, disabledWhen: null },
  { id: "story", label: "Story", placeholder: "Write the next passage yourself...", textRequired: true, display: "bubble", composerTab: true, disabledWhen: null },
  { id: "guide", label: "Guide", placeholder: "Steer the story out of character...", textRequired: true, display: "chip", composerTab: true, disabledWhen: null },
  { id: "see", label: "See", placeholder: 'Optional: what to show, such as "the brass orrery"...', textRequired: false, display: "hidden", composerTab: true, disabledWhen: "image-unavailable" },
  { id: "continue", label: "Continue", placeholder: "", textRequired: false, display: "hidden", composerTab: false, disabledWhen: "no-entry" },
];

export const modeDefinition = (mode: ActionMode) => MODES.find((candidate) => candidate.id === mode)!;

/** `storyId` is null while composing a not-yet-persisted draft story; the
 *  first submit creates the story (see `ensureStory`), so an
 *  abandoned draft never leaves an empty story behind. */
export function Composer({ storyId }: { storyId: string | null }) {
  const [mode, setMode] = useState<ActionMode>("do");
  const [text, setText] = useState("");
  const [submitting, setSubmitting] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const textareaRef = useRef<HTMLTextAreaElement>(null);

  const createStory = useStoryStore((s) => s.createStory);
  const submitTurn = useStoryStore((s) => s.submitTurn);
  const streaming = useStoryStore((s) => storyId ? s.bundles[storyId]?.streaming : undefined);
  const ledgerLoading = useStoryStore((s) => storyId ? (s.bundles[storyId]?.ledgerLoading ?? false) : false);
  const turnError = useStoryStore((s) => storyId ? s.bundles[storyId]?.turnError : null);
  const entries = useStoryStore((s) => storyId ? s.bundles[storyId]?.entries : undefined);
  const imagePendingFor = useStoryStore((s) => storyId ? s.bundles[storyId]?.imagePendingFor : undefined);
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

  const busy = submitting || !!streaming || ledgerLoading;
  const lastEntry = entries && entries.length > 0 ? entries[entries.length - 1] : undefined;
  const lastNarration = entries?.slice().reverse().find((entry) => entry.kind === "narration");
  const imageBusy = !!lastNarration && (imagePendingFor?.includes(lastNarration.id) ?? false);
  const imagesDisabled = imageSettings ? !imageSettings.enabled || !imageSettings.has_api_key : false;

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

    const trimmed = text.trim();
    const definition = modeDefinition(mode);
    if (definition.textRequired && !trimmed) return;
    if (mode === "see" && (!storyId || !lastNarration || imageBusy || imagesDisabled)) return;
    setSubmitting(true);
    try {
      const persistedStoryId = mode === "see" ? storyId! : await ensureStory();
      await submitTurn(persistedStoryId, mode, trimmed);
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
      await submitTurn(storyId, "continue", "");
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

  const activeMode = modeDefinition(mode);
  const displayedError = error ?? turnError;

  const submitDisabled = busy
    || (activeMode.textRequired && !text.trim())
    || (activeMode.disabledWhen === "image-unavailable" && (!lastNarration || imageBusy || imagesDisabled));

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
            {MODES.filter((candidate) => candidate.composerTab).map((m) => (
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
