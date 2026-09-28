import { invoke } from "@tauri-apps/api/core";
import type { TranscriptItem, ContextPreview, ContextSettings } from "../../shared/types";

export const contextApi = {
  getTranscript: (storyId: string) => invoke<TranscriptItem[]>("get_story_transcript_settings", { storyId }),
  saveTranscript: (storyId: string, include: Record<string, boolean>) =>
    invoke<TranscriptItem[]>("save_story_transcript_settings", { storyId, include }),
  get: (storyId: string) => invoke<ContextSettings>("get_story_context_settings", { storyId }),
  save: (storyId: string, settings: ContextSettings) =>
    invoke<void>("save_story_context_settings", { storyId, settings }),
  preview: (storyId: string) => invoke<ContextPreview>("preview_story_context", { storyId }),
};
