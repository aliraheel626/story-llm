import { invoke } from "@tauri-apps/api/core";
import type { NarrationVariant, RetryResult, RollDetail, StoryImage, SubmitTurnResult, TimelineEntry } from "../../shared/types";

export const timelineApi = {
  list: (branchId: string) => invoke<TimelineEntry[]>("list_timeline_entries", { branchId }),
  submitStory: (branchId: string, content: string) => invoke<SubmitTurnResult>("submit_story", { branchId, content }),
  submitTurn: (branchId: string, inputMode: "do" | "say", content: string) => invoke<SubmitTurnResult>("submit_turn", { branchId, inputMode, content }),
  submitGuide: (branchId: string, note: string) => invoke<string>("submit_guide", { branchId, note }),
  continueScene: (branchId: string) => invoke<string>("continue_scene", { branchId }),
  retry: (branchId: string, entryId: string) => invoke<RetryResult>("retry_narration", { branchId, entryId }),
  generateVariant: (branchId: string, entryId: string) => invoke<string>("generate_narration_variant", { branchId, entryId }),
  listVariants: (entryId: string) => invoke<NarrationVariant[]>("list_narration_variants", { entryId }),
  selectVariant: (entryId: string, variantEntryId: string) => invoke<TimelineEntry>("select_narration_variant", { entryId, variantEntryId }),
  edit: (entryId: string, content: string) => invoke<TimelineEntry>("edit_timeline_entry", { entryId, content }),
  eraseLastExchange: (branchId: string) => invoke<string[]>("erase_last_exchange", { branchId }),
  generateImage: (entryId: string, promptHint?: string) => invoke<StoryImage>("generate_scene_image", { entryId, promptHint: promptHint ?? null }),
  listImages: (branchId: string) => invoke<StoryImage[]>("list_images_for_branch", { branchId }),
  listRolls: (branchId: string) => invoke<RollDetail[]>("list_rolls_for_branch", { branchId }),
};
