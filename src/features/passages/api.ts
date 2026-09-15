import { invoke } from "@tauri-apps/api/core";
import type {
  Passage,
  PassageVariant,
  RetryResult,
  RollDetail,
  StoryImage,
  SubmitTurnResult,
} from "../../shared/types";

export const passagesApi = {
  list: (branchId: string) => invoke<Passage[]>("list_passages", { branchId }),
  submitStory: (branchId: string, content: string) =>
    invoke<SubmitTurnResult>("submit_story", { branchId, content }),
  submitTurn: (branchId: string, inputMode: "do" | "say", content: string) =>
    invoke<SubmitTurnResult>("submit_turn", { branchId, inputMode, content }),
  submitGuide: (branchId: string, note: string) => invoke<string>("submit_guide", { branchId, note }),
  continueScene: (branchId: string) => invoke<string>("continue_scene", { branchId }),
  retry: (branchId: string, passageId: string) =>
    invoke<RetryResult>("retry_passage", { branchId, passageId }),
  swipe: (branchId: string, passageId: string) =>
    invoke<string>("swipe_passage", { branchId, passageId }),
  listVariants: (passageId: string) => invoke<PassageVariant[]>("list_variants", { passageId }),
  switchVariant: (passageId: string, variantId: string) =>
    invoke<Passage>("switch_variant", { passageId, variantId }),
  edit: (passageId: string, content: string) => invoke<Passage>("edit_passage", { passageId, content }),
  eraseLastExchange: (branchId: string) => invoke<string[]>("erase_last_exchange", { branchId }),
  generateImage: (passageId: string, promptHint?: string) =>
    invoke<StoryImage>("generate_scene_image", { passageId, promptHint: promptHint ?? null }),
  listImages: (branchId: string) => invoke<StoryImage[]>("list_images_for_branch", { branchId }),
  listRolls: (branchId: string) => invoke<RollDetail[]>("list_rolls_for_branch", { branchId }),
  getRollDetail: (passageId: string) => invoke<RollDetail | null>("get_roll_detail", { passageId }),
};
