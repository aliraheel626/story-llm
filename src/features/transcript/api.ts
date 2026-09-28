import { invoke } from "@tauri-apps/api/core";
import type { ActionMode, TranscriptEntry, TranscriptSnapshot, RetryResult, StoryImage, SubmitTurnResult } from "../../shared/types";

export const transcriptApi = {
  list: (storyId: string) => invoke<TranscriptSnapshot>("list_transcript_entries", { storyId }),
  submitTurn: (storyId: string, mode: ActionMode, content: string) => invoke<SubmitTurnResult>("submit_turn", { storyId, mode, content }),
  retry: (storyId: string, entryId: string) => invoke<RetryResult>("retry_narration", { storyId, entryId }),
  edit: (entryId: string, content: string) => invoke<TranscriptEntry>("edit_transcript_entry", { entryId, content }),
  eraseLastExchange: (storyId: string) => invoke<string[]>("erase_last_exchange", { storyId }),
  listImages: (storyId: string) => invoke<StoryImage[]>("list_images_for_story", { storyId }),
};
