import { invoke } from "@tauri-apps/api/core";

export const worldApi = {
  getAuthorNote: (storyId: string) => invoke<string>("get_author_note", { storyId }),
  saveAuthorNote: (storyId: string, branchId: string, note: string) => invoke<void>("save_author_note", { storyId, branchId, note }),
};
