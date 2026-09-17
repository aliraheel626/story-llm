import { useEffect } from "react";
import { listen } from "@tauri-apps/api/event";
import { useStoryStore } from "../features/story/store";
import type {
  NarrationDonePayload,
  NarrationDeltaPayload,
  NarrationErrorPayload,
  NarrationToolActivityPayload,
  StoryImage,
  StoryTitleUpdatedPayload,
  SwipeDonePayload,
} from "../shared/types";

/** Wires the Rust-side narration-* events into the story store. Mount once near the app root. */
export function useNarrationEvents() {
  useEffect(() => {
    const unlistenPromises = [
      listen<NarrationDeltaPayload>("narration-delta", (event) => {
        useStoryStore.getState()._appendDelta(event.payload.stream_id, event.payload.text);
      }),
      listen<NarrationDeltaPayload>("narration-thoughts", (event) => {
        useStoryStore.getState()._appendThoughts(event.payload.stream_id, event.payload.text);
      }),
      listen<NarrationDonePayload>("narration-done", (event) => {
        useStoryStore.getState()._finalize(event.payload);
      }),
      listen<SwipeDonePayload>("swipe-done", (event) => {
        useStoryStore.getState()._swipeDone(event.payload);
      }),
      listen<NarrationErrorPayload>("narration-error", (event) => {
        useStoryStore.getState()._fail(event.payload.stream_id, event.payload.message);
      }),
      listen<NarrationToolActivityPayload>("narration-tool-activity", (event) => {
        useStoryStore.getState()._toolActivity(event.payload);
      }),
      listen<StoryTitleUpdatedPayload>("story-title-updated", (event) => {
        useStoryStore.getState().applyStoryTitle(event.payload.story_id, event.payload.title);
      }),
      listen<string>("scene-image-pending", (event) => {
        useStoryStore.getState()._imagePending(event.payload);
      }),
      listen<StoryImage>("scene-image-generated", (event) => {
        useStoryStore.getState()._imageGenerated(event.payload);
      }),
      listen<string>("scene-image-failed", (event) => {
        useStoryStore.getState()._imageFailed(event.payload);
      }),
    ];

    return () => {
      unlistenPromises.forEach((p) => p.then((unlisten) => unlisten()));
    };
  }, []);
}
