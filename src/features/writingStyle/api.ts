import { invoke } from "@tauri-apps/api/core";

export const writingStyleApi = {
  getAuthorNote: (storyId: string) => invoke<string>("get_author_note", { storyId }),
  saveAuthorNote: (storyId: string, note: string) => invoke<void>("save_author_note", { storyId, note }),
  getAuthorNoteEnabled: (storyId: string) => invoke<boolean>("get_author_note_enabled", { storyId }),
  setAuthorNoteEnabled: (storyId: string, enabled: boolean) =>
    invoke<void>("set_author_note_enabled", { storyId, enabled }),
};
