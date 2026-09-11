import { useEffect, useRef, useState } from "react";
import { useStoryStore } from "../../store/storyStore";
import { useImageModelStore } from "../../store/imageModelStore";

type Mode = "do" | "say" | "story" | "guide" | "see";

const MODES: { id: Mode; label: string; placeholder: string }[] = [
  { id: "do", label: "Do", placeholder: "What do you do?" },
  { id: "say", label: "Say", placeholder: "What do you say?" },
  { id: "story", label: "Story", placeholder: "Write the next passage yourself..." },
  { id: "guide", label: "Guide", placeholder: "Steer the story out of character (won't appear as an action)..." },
  { id: "see", label: "See", placeholder: 'Optional: what to show — "the bucket", "the girl you are seeing"...' },
];

export function Composer({ branchId }: { branchId: string }) {
  const [mode, setMode] = useState<Mode>("do");
  const [text, setText] = useState("");
  const [submitting, setSubmitting] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const textareaRef = useRef<HTMLTextAreaElement>(null);

  const submitTurn = useStoryStore((s) => s.submitTurn);
  const submitStoryText = useStoryStore((s) => s.submitStoryText);
  const submitGuide = useStoryStore((s) => s.submitGuide);
  const continueScene = useStoryStore((s) => s.continueScene);
  const streaming = useStoryStore((s) => s.streaming);
  const turnError = useStoryStore((s) => s.turnError);
  const passages = useStoryStore((s) => s.passagesByBranch[branchId]);
  const generateImageForPassage = useStoryStore((s) => s.generateImageForPassage);
  const generatingImageFor = useStoryStore((s) => s.generatingImageFor);
  const imageError = useStoryStore((s) => s.imageError);
  const imageSettings = useImageModelStore((s) => s.settings);
  const loadImageSettings = useImageModelStore((s) => s.load);

  const busy = submitting || (!!streaming && streaming.branchId === branchId);
  const lastPassage = passages && passages.length > 0 ? passages[passages.length - 1] : undefined;
  const imageBusy = generatingImageFor === lastPassage?.id;
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

  const onSubmit = async () => {
    if (busy) return;
    setError(null);

    if (mode === "see") {
      if (!lastPassage || imageBusy || imagesDisabled) return;
      setSubmitting(true);
      try {
        await generateImageForPassage(lastPassage.id, text.trim() || undefined);
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
      if (mode === "story") {
        await submitStoryText(branchId, trimmed);
      } else if (mode === "guide") {
        await submitGuide(branchId, trimmed);
      } else {
        await submitTurn(branchId, mode, trimmed);
      }
      setText("");
    } catch (e) {
      setError(String(e));
    } finally {
      setSubmitting(false);
    }
  };

  const onContinue = async () => {
    if (busy || !lastPassage) return;
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
    if (e.key === "Enter" && (e.metaKey || e.ctrlKey)) {
      e.preventDefault();
      onSubmit();
    }
  };

  const activeMode = MODES.find((m) => m.id === mode)!;
  const displayedError = mode === "see" ? (error ?? imageError) : (error ?? turnError);

  const submitDisabled = mode === "see" ? busy || !lastPassage || imageBusy || imagesDisabled : busy || !text.trim();

  const submitLabel = mode === "see" ? (imageBusy ? "Generating..." : "Generate") : busy ? "Writing..." : "Send";

  const statusHint =
    mode === "see"
      ? imagesDisabled
        ? "Image generation is disabled in the Image Model panel."
        : !lastPassage
          ? "Write a passage first, then generate an image."
          : (displayedError ?? "⌘/Ctrl + Enter to generate")
      : (displayedError ?? "⌘/Ctrl + Enter to send");

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
            disabled={busy || !lastPassage}
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
        <div className="mt-2 flex items-center justify-between">
          <span className="text-xs text-muted">
            {displayedError ? <span className="text-danger">{displayedError}</span> : statusHint}
          </span>
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
  );
}
