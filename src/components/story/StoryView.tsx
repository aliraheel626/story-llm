import { useEffect, useRef } from "react";
import { useAppStore } from "../../store/appStore";
import { useStoryStore } from "../../store/storyStore";
import { PassageView } from "./PassageView";
import { Composer } from "./Composer";
import { EditableStoryTitle } from "./EditableStoryTitle";
import { DEFAULT_STORY_TITLE } from "../../lib/types";

function usePrefersReducedMotion() {
  return typeof window !== "undefined" && window.matchMedia("(prefers-reduced-motion: reduce)").matches;
}

export function StoryView() {
  const activeStoryId = useAppStore((s) => s.activeStoryId);
  const stories = useAppStore((s) => s.stories);
  const draft = useAppStore((s) => s.draft);
  const activeStory = stories.find((s) => s.id === activeStoryId);
  const branchId = activeStory?.default_branch_id ?? null;

  const passagesByBranch = useStoryStore((s) => s.passagesByBranch);
  const loadPassages = useStoryStore((s) => s.loadPassages);
  const loadImagesForBranch = useStoryStore((s) => s.loadImagesForBranch);
  const loadRollsForBranch = useStoryStore((s) => s.loadRollsForBranch);
  const imagesByPassage = useStoryStore((s) => s.imagesByPassage);
  const variantsByPassage = useStoryStore((s) => s.variantsByPassage);
  const rollByPassage = useStoryStore((s) => s.rollByPassage);
  const streaming = useStoryStore((s) => s.streaming);

  const passages = branchId ? passagesByBranch[branchId] ?? [] : [];
  const scrollRef = useRef<HTMLDivElement>(null);
  const pinRef = useRef<HTMLDivElement>(null);
  const reducedMotion = usePrefersReducedMotion();

  useEffect(() => {
    if (branchId) {
      loadPassages(branchId);
      loadImagesForBranch(branchId);
      loadRollsForBranch(branchId);
    }
  }, [branchId, loadPassages, loadImagesForBranch, loadRollsForBranch]);

  const isStreamingAppend = !!streaming && streaming.branchId === branchId && streaming.mode === "append";
  const isStreamingReplace = !!streaming && streaming.branchId === branchId && streaming.mode === "replace";

  const pinKey = isStreamingAppend
    ? `streaming:${streaming!.streamId}`
    : isStreamingReplace
      ? `replace:${streaming!.targetPassageId}`
      : (passages[passages.length - 1]?.id ?? null);

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

          {passages.length === 0 && !isStreamingAppend && (
            <p className="font-prose text-base italic leading-8 text-muted">
              The page is blank. Use Do, Say, or Story below to begin.
            </p>
          )}

          {passages.map((passage, i) => {
            // A completed Story-mode draft is hidden: its generated_story
            // passage restates the same beat as prose, so rendering both would
            // read it twice. Stranded drafts (a failed generation left them
            // last) stay visible so they can still be edited or erased.
            if (passage.input_mode === "story" && passages[i + 1]?.input_mode === "generated_story") {
              return null;
            }
            const isLastPassage = i === passages.length - 1;
            const pinHere = isStreamingReplace ? passage.id === streaming!.targetPassageId : isLastPassage && !isStreamingAppend;
            return (
              <div key={passage.id} ref={pinHere ? pinRef : undefined}>
                <PassageView
                  passage={passage}
                  branchId={branchId!}
                  isLast={isLastPassage}
                  images={imagesByPassage[passage.id]}
                  variants={variantsByPassage[passage.id]}
                  rollSummary={rollByPassage[passage.id]}
                />
              </div>
            );
          })}

          {isStreamingAppend && (
            <div ref={pinRef} className="animate-fade-in">
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
