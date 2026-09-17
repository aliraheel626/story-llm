import { invoke } from "@tauri-apps/api/core";
import type { NarrationVariant, RetryResult, RollDetail, StoryImage, SubmitTurnResult, TimelineEntry, TimelineSnapshot } from "../../shared/types";

export const timelineApi = {
  list: (storyId: string) => invoke<TimelineSnapshot>("list_timeline_entries", { storyId }),
  submitStory: (storyId: string, content: string) => invoke<SubmitTurnResult>("submit_story", { storyId, content }),
  submitTurn: (storyId: string, inputMode: "do" | "say", content: string) => invoke<SubmitTurnResult>("submit_turn", { storyId, inputMode, content }),
  submitGuide: (storyId: string, note: string) => invoke<string>("submit_guide", { storyId, note }),
  continueScene: (storyId: string) => invoke<string>("continue_scene", { storyId }),
  retry: (storyId: string, entryId: string) => invoke<RetryResult>("retry_narration", { storyId, entryId }),
  generateVariant: (storyId: string, entryId: string) => invoke<string>("generate_narration_variant", { storyId, entryId }),
  listVariants: (entryId: string) => invoke<NarrationVariant[]>("list_narration_variants", { entryId }),
  selectVariant: (entryId: string, variantEntryId: string) => invoke<TimelineEntry>("select_narration_variant", { entryId, variantEntryId }),
  edit: (entryId: string, content: string) => invoke<TimelineEntry>("edit_timeline_entry", { entryId, content }),
  eraseLastExchange: (storyId: string) => invoke<string[]>("erase_last_exchange", { storyId }),
  generateImage: (entryId: string, promptHint?: string) => invoke<StoryImage>("generate_scene_image", { entryId, promptHint: promptHint ?? null }),
  listImages: (storyId: string) => invoke<StoryImage[]>("list_images_for_story", { storyId }),
  listRolls: (storyId: string) => invoke<RollDetail[]>("list_rolls_for_story", { storyId }),
};
