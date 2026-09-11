import { useEffect } from "react";
import { listen } from "@tauri-apps/api/event";
import { useStoryStore } from "../store/storyStore";
import type { NarrationDonePayload, NarrationDeltaPayload, NarrationErrorPayload, SwipeDonePayload } from "./types";

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
    ];

    return () => {
      unlistenPromises.forEach((p) => p.then((unlisten) => unlisten()));
    };
  }, []);
}
