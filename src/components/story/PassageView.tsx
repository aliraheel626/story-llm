import { useEffect, useRef, useState } from "react";
import { convertFileSrc } from "@tauri-apps/api/core";
import type { Passage, PassageVariant, RollDetail, StoryImage } from "../../lib/types";
import { useStoryStore } from "../../store/storyStore";
import { RollDisclosure } from "./RollDisclosure";

interface PassageViewProps {
  passage: Passage;
  branchId: string;
  isLast: boolean;
  images?: StoryImage[];
  variants?: PassageVariant[];
  rollSummary?: RollDetail;
}

export function PassageView({ passage, branchId, isLast, images, variants, rollSummary }: PassageViewProps) {
  const streaming = useStoryStore((s) => s.streaming);
  const retryPassage = useStoryStore((s) => s.retryPassage);
  const eraseLastExchange = useStoryStore((s) => s.eraseLastExchange);
  const swipePassage = useStoryStore((s) => s.swipePassage);
  const editPassage = useStoryStore((s) => s.editPassage);
  const switchVariant = useStoryStore((s) => s.switchVariant);
  const loadVariantsForPassage = useStoryStore((s) => s.loadVariantsForPassage);
  const imagePending = useStoryStore((s) => s.imagePendingFor.includes(passage.id));

  const [editing, setEditing] = useState(false);
  const [draft, setDraft] = useState(passage.content);
  const [actionBusy, setActionBusy] = useState(false);
  const textareaRef = useRef<HTMLTextAreaElement>(null);

  const isNarrator = passage.role === "narrator";
  const isBeingReplaced = streaming?.mode === "replace" && streaming.targetPassageId === passage.id;
  const anyStreamBusy = !!streaming;

  useEffect(() => {
    if (isLast && isNarrator && !variants) {
      loadVariantsForPassage(passage.id);
    }
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [isLast, isNarrator, passage.id]);

  useEffect(() => {
    if (editing) {
      textareaRef.current?.focus();
      textareaRef.current?.setSelectionRange(draft.length, draft.length);
    }
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [editing]);

  const startEdit = () => {
    setDraft(passage.content);
    setEditing(true);
  };

  const saveEdit = async () => {
    const trimmed = draft.trim();
    if (!trimmed) return;
    setActionBusy(true);
    try {
      await editPassage(branchId, passage.id, trimmed);
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

  const selectedIndex = variants?.findIndex((v) => v.is_selected) ?? -1;
  const showVariantPager = isLast && isNarrator && !!variants && variants.length > 1;

  const onPrevVariant = () => {
    if (!variants || selectedIndex <= 0) return;
    runAction(() => switchVariant(branchId, passage.id, variants[selectedIndex - 1].id));
  };
  const onNextVariant = () => {
    if (!variants || selectedIndex === -1 || selectedIndex >= variants.length - 1) return;
    runAction(() => switchVariant(branchId, passage.id, variants[selectedIndex + 1].id));
  };

  const displayContent = isBeingReplaced ? streaming!.text : passage.content;

  const editControls = !editing && !anyStreamBusy && (
    <button
      onClick={startEdit}
      className="rounded border border-border bg-bg px-2 py-0.5 text-[11px] text-muted opacity-0 transition-opacity hover:text-text group-hover:opacity-100"
    >
      Edit
    </button>
  );

  if (passage.role === "player") {
    const isSay = passage.input_mode === "say";
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
            {isSay ? `"${passage.content}"` : passage.content}
          </p>
        )}
        <div className="mt-1 flex justify-end">{editControls}</div>
      </div>
    );
  }

  // A Story-mode draft is player input, not story prose: the model turns it
  // into the following passage, so it reads like a note rather than narration.
  if (passage.input_mode === "story") {
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
            className="border-l-2 border-dashed border-border pl-4 italic"
          />
        ) : (
          <p className="border-l-2 border-dashed border-border pl-4 font-prose text-sm italic leading-7 text-muted">
            <span className="select-none pr-2 text-[10px] uppercase tracking-wider">draft</span>
            {passage.content}
          </p>
        )}
        <div className="mt-1 flex justify-end gap-1.5">
          {editControls}
          {isLast && !anyStreamBusy && (
            <button
              onClick={() => runAction(() => eraseLastExchange(branchId))}
              disabled={actionBusy}
              className="rounded border border-border bg-bg px-2 py-0.5 text-[11px] text-danger hover:opacity-80 disabled:opacity-40"
            >
              Erase
            </button>
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
          {isBeingReplaced && (
            <span className="ml-0.5 inline-block h-4 w-1.5 translate-y-0.5 animate-pulse bg-muted motion-reduce:animate-none" />
          )}
        </p>
      )}

      {rollSummary && <RollDisclosure passageId={passage.id} summary={rollSummary} />}

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
        <div className="flex items-center justify-between">
          <div>
            {showVariantPager && (
              <div className="flex items-center gap-2 text-[11px] text-muted">
                <button onClick={onPrevVariant} disabled={selectedIndex <= 0 || actionBusy || anyStreamBusy} className="hover:text-text disabled:opacity-30">
                  ‹
                </button>
                <span>
                  {selectedIndex + 1}/{variants!.length}
                </span>
                <button
                  onClick={onNextVariant}
                  disabled={selectedIndex === -1 || selectedIndex >= variants!.length - 1 || actionBusy || anyStreamBusy}
                  className="hover:text-text disabled:opacity-30"
                >
                  ›
                </button>
              </div>
            )}
          </div>

          <div className="flex gap-1.5 opacity-0 transition-opacity group-hover:opacity-100">
            {editControls}
            {isLast && !anyStreamBusy && (
              <>
                <button
                  onClick={() => runAction(() => swipePassage(branchId, passage.id))}
                  disabled={actionBusy}
                  className="rounded border border-border bg-bg px-2 py-0.5 text-[11px] text-muted hover:text-text disabled:opacity-40"
                >
                  Swipe
                </button>
                <button
                  onClick={() => runAction(() => retryPassage(branchId, passage.id))}
                  disabled={actionBusy}
                  className="rounded border border-border bg-bg px-2 py-0.5 text-[11px] text-muted hover:text-text disabled:opacity-40"
                >
                  Retry
                </button>
                <button
                  onClick={() => runAction(() => eraseLastExchange(branchId))}
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
