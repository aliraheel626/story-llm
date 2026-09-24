import { useEffect, useMemo, useRef } from "react";
import { DEFAULT_STORY_TITLE, groupRollsByEntry, ledgerInputMode, type ActionMode } from "../../shared/types";
import type { LedgerEntry, NarrativePayload } from "../../shared/types";
import { useStoryStore } from "../story/store";
import { EditableStoryTitle } from "../stories/EditableStoryTitle";
import { LedgerEntryView } from "./LedgerEntryView";
import { TurnActivity, toolCallsFromEvents, type TurnActivityData } from "./TurnActivity";
import { Composer, modeDefinition } from "./Composer";

const EMPTY_ENTRIES: LedgerEntry[] = [];

function usePrefersReducedMotion() {
  return typeof window !== "undefined" && window.matchMedia("(prefers-reduced-motion: reduce)").matches;
}

export function StoryView() {
  const activeStoryId = useStoryStore((s) => s.activeStoryId);
  const stories = useStoryStore((s) => s.stories);
  const draft = useStoryStore((s) => s.draft);
  const activeStory = stories.find((s) => s.id === activeStoryId);

  const entries = useStoryStore((s) => activeStoryId ? (s.bundles[activeStoryId]?.entries ?? EMPTY_ENTRIES) : EMPTY_ENTRIES);
  const loadLedger = useStoryStore((s) => s.loadLedger);
  const loadImagesForStory = useStoryStore((s) => s.loadImagesForStory);
  const imagesByEntry = useStoryStore((s) => activeStoryId ? s.bundles[activeStoryId]?.imagesByEntry : undefined);
  const streaming = useStoryStore((s) => activeStoryId ? s.bundles[activeStoryId]?.streaming : undefined);
  const hidden = useStoryStore((s) => activeStoryId ? s.bundles[activeStoryId]?.hidden : undefined);
  const turns = useStoryStore((s) => activeStoryId ? s.bundles[activeStoryId]?.turns : undefined);
  const rollsByEntry = useMemo(() => groupRollsByEntry(hidden ?? EMPTY_ENTRIES), [hidden]);
  const lastTurnActivity = useStoryStore((s) => activeStoryId ? s.bundles[activeStoryId]?.turnActivity : undefined);
  const scrollRef = useRef<HTMLDivElement>(null);
  const pinRef = useRef<HTMLDivElement>(null);
  const reducedMotion = usePrefersReducedMotion();

  /**
   * Reasoning and tool calls for one narration entry. Committed turns read
   * from the entry's payload plus the hidden events it targeted (so the
   * panel survives a reload); the newest turn also falls back to the
   * in-session snapshot for anything the fetched events don't cover yet.
   */
  const activityFor = (entry: LedgerEntry, isLastEntry: boolean): TurnActivityData | undefined => {
    if (entry.kind !== "narration") return undefined;
    const thoughts = (entry.payload as NarrativePayload).thoughts;
    const tools = toolCallsFromEvents(entry.id, hidden);
    // Hidden events for the turn that just finished aren't in the store until
    // a reload, so fill whichever half is missing from the session snapshot.
    const snapshot = isLastEntry && lastTurnActivity?.entryId === entry.id ? lastTurnActivity : undefined;
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
      loadLedger(activeStoryId);
      loadImagesForStory(activeStoryId);
    }
  }, [activeStoryId, loadLedger, loadImagesForStory]);

  const isStreamingAppend = streaming?.mode === "append";
  const isStreamingReplace = streaming?.mode === "replace";
  const displayedEntries = entries.filter((entry) => {
    const inputMode = ledgerInputMode(entry);
    return inputMode === "generated" || modeDefinition(inputMode as ActionMode).display !== "hidden";
  });
  const actualLastEntry = entries[entries.length - 1];

  const pinKey = isStreamingAppend
    ? `streaming:${streaming!.streamId}`
    : isStreamingReplace
      ? `replace:${streaming!.targetEntryId}`
      : (displayedEntries[displayedEntries.length - 1]?.id ?? null);

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

          {displayedEntries.length === 0 && !isStreamingAppend && (
            <p className="font-prose text-base italic leading-8 text-muted">
              The page is blank. Use Do, Say, or Story below to begin.
            </p>
          )}

          {displayedEntries.map((entry, i) => {
            const isLastEntry = i === displayedEntries.length - 1;
            const pinHere = isStreamingReplace ? entry.id === streaming!.targetEntryId : isLastEntry && !isStreamingAppend;
            const activity = activityFor(entry, isLastEntry);
            const hiddenTrailingAction = isLastEntry
               && actualLastEntry?.kind === "player_message"
               && ledgerInputMode(actualLastEntry) !== "see"
              && modeDefinition(ledgerInputMode(actualLastEntry) as ActionMode).display === "hidden"
              ? actualLastEntry
              : undefined;
            const retryEntryId = hiddenTrailingAction?.id ?? entry.id;
            const retryTurnId = entries.find((candidate) => candidate.id === retryEntryId)?.turn_id;
            return (
              <div key={entry.id} ref={pinHere ? pinRef : undefined}>
                {activity && <TurnActivity activity={activity} />}
                <LedgerEntryView
                  entry={entry}
                  storyId={activeStoryId!}
                  isLast={isLastEntry}
                  retryEntryId={retryEntryId}
                  canRetry={!(isLastEntry && actualLastEntry?.kind === "player_message" && ledgerInputMode(actualLastEntry) === "see")}
                  turnFailed={isLastEntry && !!retryTurnId && turns?.some((turn) => turn.id === retryTurnId && turn.status === "failed")}
                  images={imagesByEntry?.[entry.id]}
                  rolls={rollsByEntry?.[entry.id]}
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
