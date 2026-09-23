import { useEffect, useRef, useState } from "react";
import { convertFileSrc } from "@tauri-apps/api/core";
import { isPlayerEntry, ledgerInputMode, type ActionMode, type LedgerEntry, type Roll, type StoryImage } from "../../shared/types";
import { useStoryStore } from "../story/store";
import { RollDisclosure } from "./RollDisclosure";
import { modeDefinition } from "./Composer";

interface LedgerEntryViewProps {
  entry: LedgerEntry;
  storyId: string;
  isLast: boolean;
  retryEntryId: string;
  canRetry?: boolean;
  images?: StoryImage[];
  rolls?: Roll[];
}

export function LedgerEntryView({ entry, storyId, isLast, retryEntryId, canRetry = true, images, rolls }: LedgerEntryViewProps) {
  const streaming = useStoryStore((s) => s.bundles[storyId]?.streaming);
  const requestPending = useStoryStore((s) => s.bundles[storyId]?.requestPending ?? false);
  const retryNarration = useStoryStore((s) => s.retryNarration);
  const eraseLastExchange = useStoryStore((s) => s.eraseLastExchange);
  const editEntry = useStoryStore((s) => s.editEntry);
  const imagePending = useStoryStore((s) => s.bundles[storyId]?.imagePendingFor.includes(entry.id) ?? false);

  const [editing, setEditing] = useState(false);
  const [draft, setDraft] = useState(entry.content ?? "");
  const [actionBusy, setActionBusy] = useState(false);
  const textareaRef = useRef<HTMLTextAreaElement>(null);

  const inputMode = ledgerInputMode(entry);
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

  const displayContent = entry.content ?? "";

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
          {displayContent}
        </p>
      )}

      {isBeingReplaced && <p className="text-xs italic text-muted" role="status">Retrying this reply. The original stays until the new reply succeeds.</p>}

      {rolls?.map((roll) => (
        <RollDisclosure key={roll.id} roll={roll} />
      ))}

      {images?.map((image) => (
        <div key={image.id} className="flex flex-col gap-1">
          <img src={convertFileSrc(image.path)} alt={image.prompt} className="w-full rounded border border-border object-cover" />
          <ImageCaption prompt={image.prompt} />
        </div>
      ))}

      {imagePending && (
        <div className="relative h-56 w-full overflow-hidden rounded border border-border bg-surface" aria-label="Generating scene image">
          <div className="absolute inset-0 animate-shimmer bg-gradient-to-r from-transparent via-surface-hover to-transparent motion-reduce:animate-none" />
          <span className="absolute inset-x-0 bottom-2 text-center text-[11px] text-muted">Illustrating this scene...</span>
        </div>
      )}

      {!editing && (
        <div className="flex justify-end">
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
