import { useEffect, useRef } from "react";
import { DEFAULT_STORY_TITLE, timelineInputMode } from "../../shared/types";
import type { NarrativePayload, TimelineEntry } from "../../shared/types";
import { useStoryStore } from "../story/store";
import { EditableStoryTitle } from "../stories/EditableStoryTitle";
import { TimelineEntryView } from "./TimelineEntryView";
import { TurnActivity, toolCallsFromEvents, type TurnActivityData } from "./TurnActivity";
import { Composer } from "./Composer";

const EMPTY_ENTRIES: TimelineEntry[] = [];

function usePrefersReducedMotion() {
  return typeof window !== "undefined" && window.matchMedia("(prefers-reduced-motion: reduce)").matches;
}

export function StoryView() {
  const activeStoryId = useStoryStore((s) => s.activeStoryId);
  const stories = useStoryStore((s) => s.stories);
  const draft = useStoryStore((s) => s.draft);
  const activeStory = stories.find((s) => s.id === activeStoryId);

  const entries = useStoryStore((s) => activeStoryId ? (s.bundles[activeStoryId]?.entries ?? EMPTY_ENTRIES) : EMPTY_ENTRIES);
  const loadTimeline = useStoryStore((s) => s.loadTimeline);
  const loadImagesForStory = useStoryStore((s) => s.loadImagesForStory);
  const loadRollsForStory = useStoryStore((s) => s.loadRollsForStory);
  const imagesByEntry = useStoryStore((s) => activeStoryId ? s.bundles[activeStoryId]?.imagesByEntry : undefined);
  const variantsByEntry = useStoryStore((s) => activeStoryId ? s.bundles[activeStoryId]?.variantsByEntry : undefined);
  const rollsByEntry = useStoryStore((s) => activeStoryId ? s.bundles[activeStoryId]?.rollsByEntry : undefined);
  const streaming = useStoryStore((s) => activeStoryId ? s.bundles[activeStoryId]?.streaming : undefined);
  const hidden = useStoryStore((s) => activeStoryId ? s.bundles[activeStoryId]?.hidden : undefined);
  const lastTurnActivity = useStoryStore((s) => activeStoryId ? s.bundles[activeStoryId]?.turnActivity : undefined);
  const scrollRef = useRef<HTMLDivElement>(null);
  const pinRef = useRef<HTMLDivElement>(null);
  const reducedMotion = usePrefersReducedMotion();

  /**
   * Reasoning and tool calls for one narration entry. Committed turns read
   * from the entry's own payload plus the hidden events it targeted (so the
   * panel survives a reload); the newest turn also falls back to the
   * in-session snapshot for anything the fetched events don't cover yet.
   */
  const activityFor = (entry: TimelineEntry, isLastEntry: boolean): TurnActivityData | undefined => {
    if (entry.kind !== "narration") return undefined;
    const selected = variantsByEntry?.[entry.id]?.find((variant) => variant.is_selected);
    const thoughts = selected ? selected.thoughts : (entry.payload as NarrativePayload).thoughts;
    const tools = toolCallsFromEvents(entry.id, hidden);
    // Hidden events for the turn that just finished aren't in the store until
    // a reload, so fill whichever half is missing from the session snapshot.
    const snapshot = isLastEntry ? lastTurnActivity : undefined;
    const activity: TurnActivityData = {
      thoughts: thoughts ?? snapshot?.thoughts,
      tools: tools.length > 0
        ? tools
        : (snapshot?.tools.map((tool) => ({ key: tool.callId, label: tool.label, done: tool.phase === "finished", ok: tool.ok })) ?? []),
    };
    return activity.thoughts || activity.tools.length > 0 ? activity : undefined;
  };

  useEffect(() => {
    if (activeStoryId) {
      loadTimeline(activeStoryId);
      loadImagesForStory(activeStoryId);
      loadRollsForStory(activeStoryId);
    }
  }, [activeStoryId, loadTimeline, loadImagesForStory, loadRollsForStory]);

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

  // A draft is composed client-side and created on first submit.
  const drafting = draft && !activeStory;
  if (!activeStory && !drafting) {
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
            const activity = activityFor(entry, isLastEntry);
            return (
              <div key={entry.id} ref={pinHere ? pinRef : undefined}>
                {activity && <TurnActivity activity={activity} />}
                <TimelineEntryView
                  entry={entry}
                  storyId={activeStoryId!}
                  isLast={isLastEntry}
                  images={imagesByEntry?.[entry.id]}
                  variants={variantsByEntry?.[entry.id]}
                  rollSummaries={rollsByEntry?.[entry.id]}
                />
              </div>
            );
          })}

          {isStreamingAppend && (
            <div ref={pinRef} className="animate-fade-in">
              <TurnActivity
                live
                activity={{
                  thoughts: streaming!.thoughts,
                  tools: streaming!.toolLog.map((tool) => ({ key: tool.callId, label: tool.label, done: tool.phase === "finished", ok: tool.ok })),
                }}
              />
              <p className="whitespace-pre-wrap font-prose text-base leading-8 text-text">
                {streaming!.text}
                <span className="ml-0.5 inline-block h-4 w-1.5 translate-y-0.5 animate-pulse bg-muted motion-reduce:animate-none" />
              </p>
            </div>
          )}
        </div>
      </div>

      <Composer storyId={activeStoryId} />
    </div>
  );
}
