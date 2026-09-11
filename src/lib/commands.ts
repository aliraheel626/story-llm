import { invoke } from "@tauri-apps/api/core";
import type {
  DiceMode,
  Entity,
  EntityKind,
  ImageModelSettings,
  MechanicsSettings,
  Passage,
  PassageVariant,
  RetryResult,
  RollDetail,
  Story,
  StoryImage,
  SubmitTurnResult,
  TextModelSettings,
} from "./types";

export const commands = {
  listStories: () => invoke<Story[]>("list_stories"),
  createStory: (title?: string, settings?: MechanicsSettings | null) =>
    invoke<Story>("create_story", { title: title ?? null, settings: settings ?? null }),
  renameStory: (storyId: string, title: string) => invoke<void>("rename_story", { storyId, title }),

  listPassages: (branchId: string) => invoke<Passage[]>("list_passages", { branchId }),
  submitStory: (branchId: string, content: string) =>
    invoke<Passage>("submit_story", { branchId, content }),
  submitTurn: (branchId: string, inputMode: "do" | "say", content: string) =>
    invoke<SubmitTurnResult>("submit_turn", { branchId, inputMode, content }),
  submitGuide: (branchId: string, note: string) =>
    invoke<string>("submit_guide", { branchId, note }),
  continueScene: (branchId: string) => invoke<string>("continue_scene", { branchId }),
  retryPassage: (branchId: string, passageId: string) =>
    invoke<RetryResult>("retry_passage", { branchId, passageId }),
  swipePassage: (branchId: string, passageId: string) =>
    invoke<string>("swipe_passage", { branchId, passageId }),
  listVariants: (passageId: string) => invoke<PassageVariant[]>("list_variants", { passageId }),
  switchVariant: (passageId: string, variantId: string) =>
    invoke<Passage>("switch_variant", { passageId, variantId }),
  editPassage: (passageId: string, content: string) =>
    invoke<Passage>("edit_passage", { passageId, content }),
  eraseLastExchange: (branchId: string) => invoke<string[]>("erase_last_exchange", { branchId }),

  getTextModelSettings: () => invoke<TextModelSettings>("get_text_model_settings"),
  saveTextModelSettings: (provider: string, model: string, apiKey?: string) =>
    invoke<void>("save_text_model_settings", { provider, model, apiKey: apiKey ?? null }),

  getImageModelSettings: () => invoke<ImageModelSettings>("get_image_model_settings"),
  saveImageModelSettings: (model: string, enabled: boolean, style: string) =>
    invoke<void>("save_image_model_settings", { model, enabled, style }),
  generateSceneImage: (passageId: string, promptHint?: string) =>
    invoke<StoryImage>("generate_scene_image", { passageId, promptHint: promptHint ?? null }),
  listImagesForPassage: (passageId: string) =>
    invoke<StoryImage[]>("list_images_for_passage", { passageId }),
  listImagesForBranch: (branchId: string) =>
    invoke<StoryImage[]>("list_images_for_branch", { branchId }),

  listEntities: (storyId: string, kind?: EntityKind) =>
    invoke<Entity[]>("list_entities", { storyId, kind: kind ?? null }),
  createEntity: (storyId: string, kind: EntityKind, name: string, appearanceAnchor?: string) =>
    invoke<Entity>("create_entity", { storyId, kind, name, appearanceAnchor: appearanceAnchor ?? null }),
  updateEntity: (entityId: string, name: string, appearanceAnchor?: string) =>
    invoke<Entity>("update_entity", { entityId, name, appearanceAnchor: appearanceAnchor ?? null }),
  deleteEntity: (entityId: string) => invoke<void>("delete_entity", { entityId }),

  getStoryMechanicsSettings: (storyId: string) =>
    invoke<MechanicsSettings>("get_story_mechanics_settings", { storyId }),
  saveStoryMechanicsSettings: (storyId: string, diceMode: DiceMode, attributesEnabled: boolean) =>
    invoke<void>("save_story_mechanics_settings", { storyId, diceMode, attributesEnabled }),
  listRollsForBranch: (branchId: string) => invoke<RollDetail[]>("list_rolls_for_branch", { branchId }),
  getRollDetail: (passageId: string) => invoke<RollDetail | null>("get_roll_detail", { passageId }),

  getAuthorNote: (storyId: string) => invoke<string>("get_author_note", { storyId }),
  saveAuthorNote: (storyId: string, note: string) => invoke<void>("save_author_note", { storyId, note }),
};
