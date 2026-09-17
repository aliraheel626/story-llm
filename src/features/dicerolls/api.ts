import { invoke } from "@tauri-apps/api/core";
import type { DiceMode, DicerollSettings, ReasoningEffort, RollDetail } from "../../shared/types";

export const dicerollApi = {
  getSettings: (storyId: string) => invoke<DicerollSettings>("get_story_diceroll_settings", { storyId }),
  saveSettings: (storyId: string, branchId: string, diceMode: DiceMode, attributesEnabled: boolean, reasoningEffort: ReasoningEffort | null) =>
    invoke<void>("save_story_diceroll_settings", { storyId, branchId, diceMode, attributesEnabled, reasoningEffort }),
  listRollDetailsForEntry: (branchId: string, entryId: string) => invoke<RollDetail[]>("list_roll_details_for_entry", { branchId, entryId }),
};
