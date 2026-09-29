import { useEffect, useRef, useState } from "react";
import { convertFileSrc } from "@tauri-apps/api/core";
import { isPlayerEntry, transcriptInputMode, type ActionMode, type TranscriptEntry, type Roll, type StoryImage } from "../../shared/types";
import { useStoryStore } from "../story/store";
import { formatSeconds, formatUsd, useUsageStore } from "../usage/store";
import { ImagePlaceholder } from "./ImagePlaceholder";
import { RollDisclosure } from "./RollDisclosure";
import { modeDefinition } from "./Composer";
import { entryDisplay } from "./replacement";

interface TranscriptEntryViewProps {
  entry: TranscriptEntry;
  storyId: string;
  isLast: boolean;
  retryEntryId: string;
  canRetry?: boolean;
  turnFailed?: boolean;
  images?: StoryImage[];
  rolls?: Roll[];
}

export function TranscriptEntryView({ entry, storyId, isLast, retryEntryId, canRetry = true, turnFailed = false, images, rolls }: TranscriptEntryViewProps) {
  const streaming = useStoryStore((s) => s.bundles[storyId]?.streaming);
  const requestPending = useStoryStore((s) => s.bundles[storyId]?.requestPending ?? false);
  const retryNarration = useStoryStore((s) => s.retryNarration);
  const eraseLastExchange = useStoryStore((s) => s.eraseLastExchange);
  const editEntry = useStoryStore((s) => s.editEntry);
  const imagePending = useStoryStore((s) => s.bundles[storyId]?.imagePendingFor.includes(entry.id) ?? false);
  const breakdown = useUsageStore((state) => state.breakdownByStory[storyId]);

  const [editing, setEditing] = useState(false);
  const [draft, setDraft] = useState(entry.content ?? "");
  const [actionBusy, setActionBusy] = useState(false);
  const textareaRef = useRef<HTMLTextAreaElement>(null);

  const inputMode = transcriptInputMode(entry);
  const isBeingReplaced = streaming?.mode === "replace" && streaming.targetEntryId === entry.id;
  const anyStreamBusy = requestPending || !!streaming;

  useEffect(() => {
    if (editing) {
      textareaRef.current?.focus();
      textareaRef.current?.setSelectionRange(draft.length, draft.length);
    }
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [editing]);

  const startEdit = () => {
    setDraft(entry.content ?? "");
    setEditing(true);
  };

  const saveEdit = async () => {
    const trimmed = draft.trim();
    if (!trimmed) return;
    setActionBusy(true);
    try {
      await editEntry(storyId, entry.id, trimmed);
      setEditing(false);
    } catch (e) {
      console.error("failed to save edit", e);
    } finally {
      setActionBusy(false);
    }
  };

  const cancelEdit = () => setEditing(false);

  const runAction = async (action: () => Promise<void>) => {
    if (actionBusy || anyStreamBusy) return;
    setActionBusy(true);
    try {
      await action();
    } catch (e) {
      console.error(e);
    } finally {
      setActionBusy(false);
    }
  };

  const display = entryDisplay(entry, streaming);
  const turnCost = entry.turn_id ? breakdown?.turns[entry.turn_id] : undefined;
  const turnCostTitle = turnCost ? [
    "Text for this turn: narration, title and any summary.",
    ...(turnCost.image_cost_usd > 0
      ? [`Turn total with its images: ${formatUsd(turnCost.total_cost_usd)}.`] : []),
    ...(turnCost.earlier_attempts_cost_usd > 0
      ? [`Includes ${formatUsd(turnCost.earlier_attempts_cost_usd)} from earlier attempts (Retry).`] : []),
    ...(turnCost.unpriced_calls > 0 ? [`${turnCost.unpriced_calls} calls reported no cost.`] : []),
  ].join(" ") : "";

  const editControls = !editing && !anyStreamBusy && (
    <button
      onClick={startEdit}
      className="rounded border border-border bg-bg px-2 py-0.5 text-[11px] text-muted opacity-0 transition-opacity hover:text-text group-hover:opacity-100"
    >
      Edit
    </button>
  );

  if (isPlayerEntry(entry)) {
    const definition = modeDefinition(inputMode as ActionMode);
    if (definition.display === "hidden") return null;
    const isSay = inputMode === "say";
    const content = isSay ? `"${entry.content ?? ""}"` : entry.content;
    if (definition.display === "chip") {
      return (
        <div className="group flex items-center justify-between gap-3 rounded border border-border bg-surface px-3 py-2 text-xs text-muted">
          <span><strong className="mr-2 uppercase tracking-wider">{definition.label}</strong>{content}</span>
          <div className="flex gap-1.5">
            {editControls}
            {isLast && !anyStreamBusy && (
              <>
                {canRetry && <button onClick={() => runAction(() => retryNarration(storyId, retryEntryId))} disabled={actionBusy} className="rounded border border-border bg-bg px-2 py-0.5 text-[11px] text-muted hover:text-text disabled:opacity-40">Retry</button>}
                {turnFailed && <span className="px-1 py-0.5 text-[11px] text-danger">Failed</span>}
                <button onClick={() => runAction(() => eraseLastExchange(storyId))} disabled={actionBusy} className="rounded border border-border bg-bg px-2 py-0.5 text-[11px] text-danger hover:opacity-80 disabled:opacity-40">Erase</button>
              </>
            )}
          </div>
        </div>
      );
    }
    return (
      <div className="group relative">
        {editing ? (
          <EditBox
            textareaRef={textareaRef}
            draft={draft}
            setDraft={setDraft}
            onSave={saveEdit}
            onCancel={cancelEdit}
            busy={actionBusy}
            className="border-l-2 border-accent pl-4 italic"
          />
        ) : (
          <p className="border-l-2 border-accent pl-4 font-prose text-base italic leading-8 text-muted">
            {inputMode === "story" && <span className="select-none pr-2 text-[10px] uppercase tracking-wider">Story</span>}
            {content}
          </p>
        )}
        <div className="mt-1 flex justify-end gap-1.5">
          {editControls}
          {isLast && !anyStreamBusy && (
            <>
              {canRetry && <button onClick={() => runAction(() => retryNarration(storyId, retryEntryId))} disabled={actionBusy} className="rounded border border-border bg-bg px-2 py-0.5 text-[11px] text-muted hover:text-text disabled:opacity-40">Retry</button>}
              {turnFailed && <span className="px-1 py-0.5 text-[11px] text-danger">Failed</span>}
              <button onClick={() => runAction(() => eraseLastExchange(storyId))} disabled={actionBusy} className="rounded border border-border bg-bg px-2 py-0.5 text-[11px] text-danger hover:opacity-80 disabled:opacity-40">Erase</button>
            </>
          )}
        </div>
      </div>
    );
  }

  return (
    <div className="group relative flex flex-col gap-3">
      {editing ? (
        <EditBox textareaRef={textareaRef} draft={draft} setDraft={setDraft} onSave={saveEdit} onCancel={cancelEdit} busy={actionBusy} />
      ) : (
        <p className="whitespace-pre-wrap font-prose text-base leading-8 text-text">
          {display.text}
          {display.streaming && !streaming?.imagePending && <span className="ml-0.5 inline-block h-4 w-1.5 translate-y-0.5 animate-pulse bg-muted motion-reduce:animate-none" />}
        </p>
      )}

      {isBeingReplaced && <p className="text-xs italic text-muted" role="status">Retrying. The original comes back if this fails.</p>}

      {display.showOldExtras && rolls?.map((roll) => (
        <RollDisclosure key={roll.id} roll={roll} />
      ))}

      {display.showOldExtras && images?.map((image) => {
        const cost = breakdown?.images[image.id];
        const seeTurn = cost?.turn_id && cost.turn_id !== entry.turn_id ? breakdown?.turns[cost.turn_id] : undefined;
        const tooltip = `Image cost and generation time.${seeTurn
          ? ` See turn total ${formatUsd(seeTurn.total_cost_usd)}, including ${formatUsd(seeTurn.text_cost_usd)} to plan the image.` : ""}`;
        return <div key={image.id} className="flex flex-col gap-1">
          <img src={convertFileSrc(image.id, "storyimg")} alt={image.prompt} className="w-full rounded border border-border object-cover" />
          <div className="flex items-center justify-between gap-2">
            <ImageCaption prompt={image.prompt} />
            {cost && <span className="text-[11px] text-muted tabular-nums" title={tooltip}>
              {cost.cost_usd === null ? "cost unknown" : formatUsd(cost.cost_usd)}
              {cost.duration_ms !== null && ` · ${formatSeconds(cost.duration_ms)}`}
            </span>}
          </div>
        </div>;
      })}

      {(imagePending || (isBeingReplaced && streaming.imagePending)) && <ImagePlaceholder />}

      {!editing && (
        <div className="flex items-center justify-between gap-3">
          {entry.kind === "narration" && !isBeingReplaced && !display.streaming && turnCost && (
            <span className="text-[11px] text-muted tabular-nums" title={turnCostTitle}>
              Text {formatUsd(turnCost.text_cost_usd)}
              {turnCost.unpriced_calls > 0 && "*"}
            </span>
          )}
          <div className="flex gap-1.5 opacity-0 transition-opacity group-hover:opacity-100">
            {editControls}
            {isLast && !anyStreamBusy && (
              <>
                {canRetry && <button
                  onClick={() => runAction(() => retryNarration(storyId, retryEntryId))}
                  disabled={actionBusy}
                  className="rounded border border-border bg-bg px-2 py-0.5 text-[11px] text-muted hover:text-text disabled:opacity-40"
                >
                  Retry
                </button>}
                {turnFailed && <span className="px-1 py-0.5 text-[11px] text-danger">Failed</span>}
                <button
                  onClick={() => runAction(() => eraseLastExchange(storyId))}
                  disabled={actionBusy}
                  className="rounded border border-border bg-bg px-2 py-0.5 text-[11px] text-danger hover:opacity-80 disabled:opacity-40"
                >
                  Erase
                </button>
              </>
            )}
          </div>
        </div>
      )}
    </div>
  );
}

function ImageCaption({ prompt }: { prompt: string }) {
  const [open, setOpen] = useState(false);
  return (
    <div className="text-[11px] text-muted">
      <button onClick={() => setOpen((v) => !v)} className="flex items-center gap-1 hover:text-text">
        <span>🖼</span>
        <span>Image prompt</span>
        <span className="text-muted">{open ? "▲" : "▼"}</span>
      </button>
      {open && <p className="mt-1 whitespace-pre-wrap rounded border border-border bg-bg px-2.5 py-2 leading-5">{prompt}</p>}
    </div>
  );
}

function EditBox({
  textareaRef,
  draft,
  setDraft,
  onSave,
  onCancel,
  busy,
  className = "",
}: {
  textareaRef: React.RefObject<HTMLTextAreaElement>;
  draft: string;
  setDraft: (v: string) => void;
  onSave: () => void;
  onCancel: () => void;
  busy: boolean;
  className?: string;
}) {
  return (
    <div className="flex flex-col gap-2">
      <textarea
        ref={textareaRef}
        value={draft}
        onChange={(e) => setDraft(e.target.value)}
        onKeyDown={(e) => {
          if (e.key === "Escape") onCancel();
          if (e.key === "Enter" && (e.metaKey || e.ctrlKey)) onSave();
        }}
        rows={4}
        className={`w-full resize-y rounded border border-accent bg-bg px-3 py-2 font-prose text-base leading-7 text-text focus:outline-none ${className}`}
      />
      <div className="flex justify-end gap-2">
        <button onClick={onCancel} className="rounded border border-border px-3 py-1 text-xs text-muted hover:text-text">
          Cancel
        </button>
        <button
          onClick={onSave}
          disabled={busy || !draft.trim()}
          className="rounded bg-accent px-3 py-1 text-xs font-medium text-bg hover:bg-accent-hover disabled:opacity-40"
        >
          Save
        </button>
      </div>
    </div>
  );
}
