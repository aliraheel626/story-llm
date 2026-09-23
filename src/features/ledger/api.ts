import { invoke } from "@tauri-apps/api/core";
import type { ActionMode, LedgerEntry, LedgerSnapshot, NarrationVariant, RetryResult, RollDetail, StoryImage, SubmitTurnResult } from "../../shared/types";

export const ledgerApi = {
  list: (storyId: string) => invoke<LedgerSnapshot>("list_ledger_entries", { storyId }),
  submitTurn: (storyId: string, mode: ActionMode, content: string) => invoke<SubmitTurnResult>("submit_turn", { storyId, mode, content }),
  retry: (storyId: string, entryId: string) => invoke<RetryResult>("retry_narration", { storyId, entryId }),
  generateVariant: (storyId: string, entryId: string) => invoke<string>("generate_narration_variant", { storyId, entryId }),
  listVariants: (entryId: string) => invoke<NarrationVariant[]>("list_narration_variants", { entryId }),
  selectVariant: (entryId: string, variantEntryId: string) => invoke<LedgerEntry>("select_narration_variant", { entryId, variantEntryId }),
  edit: (entryId: string, content: string) => invoke<LedgerEntry>("edit_ledger_entry", { entryId, content }),
  eraseLastExchange: (storyId: string) => invoke<string[]>("erase_last_exchange", { storyId }),
  listImages: (storyId: string) => invoke<StoryImage[]>("list_images_for_story", { storyId }),
  listRolls: (storyId: string) => invoke<RollDetail[]>("list_rolls_for_story", { storyId }),
};
