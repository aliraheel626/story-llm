import { useEffect, useRef } from "react";
import { useAppStore } from "../../app/store";
import { DEFAULT_STORY_TITLE, timelineInputMode } from "../../shared/types";
import { EditableStoryTitle } from "../stories/EditableStoryTitle";
import { TimelineEntryView } from "./TimelineEntryView";
import { Composer } from "./Composer";
import { useStoryStore } from "./store";

function usePrefersReducedMotion() {
  return typeof window !== "undefined" && window.matchMedia("(prefers-reduced-motion: reduce)").matches;
}

export function StoryView() {
  const activeStoryId = useAppStore((s) => s.activeStoryId);
  const stories = useAppStore((s) => s.stories);
  const draft = useAppStore((s) => s.draft);
  const activeStory = stories.find((s) => s.id === activeStoryId);
  const branchId = activeStory?.default_branch_id ?? null;

  const entriesByBranch = useStoryStore((s) => s.entriesByBranch);
  const loadTimeline = useStoryStore((s) => s.loadTimeline);
  const loadImagesForBranch = useStoryStore((s) => s.loadImagesForBranch);
  const loadRollsForBranch = useStoryStore((s) => s.loadRollsForBranch);
  const imagesByEntry = useStoryStore((s) => s.imagesByEntry);
  const variantsByEntry = useStoryStore((s) => s.variantsByEntry);
  const rollByEntry = useStoryStore((s) => s.rollByEntry);
  const streaming = useStoryStore((s) => (branchId ? s.streamingByBranch[branchId] : undefined));

  const entries = branchId ? entriesByBranch[branchId] ?? [] : [];
  const scrollRef = useRef<HTMLDivElement>(null);
  const pinRef = useRef<HTMLDivElement>(null);
  const reducedMotion = usePrefersReducedMotion();

  useEffect(() => {
    if (branchId) {
      loadTimeline(branchId);
      loadImagesForBranch(branchId);
      loadRollsForBranch(branchId);
    }
  }, [branchId, loadTimeline, loadImagesForBranch, loadRollsForBranch]);

  const isStreamingAppend = streaming?.mode === "append";
  const isStreamingReplace = streaming?.mode === "replace";

  const pinKey = isStreamingAppend
    ? `streaming:${streaming!.streamId}`
    : isStreamingReplace
      ? `replace:${streaming!.targetEntryId}`
      : (entries[entries.length - 1]?.id ?? null);

  useEffect(() => {
    pinRef.current?.scrollIntoView({ block: "start", behavior: reducedMotion ? "auto" : "smooth" });
  }, [pinKey, reducedMotion]);

  // A draft is a story that doesn't exist yet: composed client-side, created
  // on first submit (see Composer's ensureBranch).
  const drafting = draft && !(activeStory && branchId);
  if ((!activeStory || !branchId) && !drafting) {
    return (
      <div className="flex h-full flex-1 items-center justify-center">
        <div className="text-center">
          <p className="font-prose text-lg text-muted">No story open</p>
          <p className="mt-1 text-sm text-muted">Create or select a story from the sidebar.</p>
        </div>
      </div>
    );
  }

  return (
    <div className="flex h-full flex-1 flex-col overflow-hidden">
      <div ref={scrollRef} className="flex-1 overflow-y-auto">
        <div className="mx-auto flex w-full max-w-measure flex-col gap-5 px-6 py-10">
          {drafting ? (
            <h1 className="font-prose text-2xl italic text-muted">{DEFAULT_STORY_TITLE}</h1>
          ) : (
            <EditableStoryTitle storyId={activeStory!.id} />
          )}

          {entries.length === 0 && !isStreamingAppend && (
            <p className="font-prose text-base italic leading-8 text-muted">
              The page is blank. Use Do, Say, or Story below to begin.
            </p>
          )}

          {entries.map((entry, i) => {
            // A completed Story-mode draft is hidden: its generated_story
            // narration restates the same beat as prose, so rendering both would
            // read it twice. Stranded drafts (a failed generation left them
            // last) stay visible so they can still be edited or erased. Checked
            // against any later entry, not just the next one, to match the
            // backend's own history-folding rule (narration/history.rs) — an
            // intervening hidden event must not make a completed draft look
            // stranded here while the model already treats it as superseded.
            if (timelineInputMode(entry) === "story" && entries.slice(i + 1).some((e) => timelineInputMode(e) === "generated_story")) {
              return null;
            }
            const isLastEntry = i === entries.length - 1;
            const pinHere = isStreamingReplace ? entry.id === streaming!.targetEntryId : isLastEntry && !isStreamingAppend;
            return (
              <div key={entry.id} ref={pinHere ? pinRef : undefined}>
                <TimelineEntryView
                  entry={entry}
                  branchId={branchId!}
                  isLast={isLastEntry}
                  images={imagesByEntry[entry.id]}
                  variants={variantsByEntry[entry.id]}
                  rollSummary={rollByEntry[entry.id]}
                />
              </div>
            );
          })}

          {isStreamingAppend && (
            <div ref={pinRef} className="animate-fade-in">
              {streaming!.toolActivity?.phase === "started" && (
                <p className="mb-1 text-[11px] italic text-muted">{streaming!.toolActivity.label}</p>
              )}
              <p className="whitespace-pre-wrap font-prose text-base leading-8 text-text">
                {streaming!.text}
                <span className="ml-0.5 inline-block h-4 w-1.5 translate-y-0.5 animate-pulse bg-muted motion-reduce:animate-none" />
              </p>
            </div>
          )}
        </div>
      </div>

      <Composer branchId={branchId} />
    </div>
  );
}
