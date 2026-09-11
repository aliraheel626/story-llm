export interface Story {
  id: string;
  title: string;
  created_at: string;
  updated_at: string;
  settings_json: string;
  default_branch_id: string | null;
}

/** Placeholder until the model (or the user) supplies a real title —
 *  mirrors `DEFAULT_STORY_TITLE` in `src-tauri/src/commands/stories.rs`. */
export const DEFAULT_STORY_TITLE = "New story";

export interface StoryTitleUpdatedPayload {
  story_id: string;
  title: string;
}

export type PassageRole = "player" | "narrator";
export type InputMode = "do" | "say" | "story" | "generated" | "generated_guide" | "generated_continue" | "generated_story";

export interface Passage {
  id: string;
  branch_id: string;
  seq: number;
  role: PassageRole;
  input_mode: InputMode;
  content: string;
  thoughts: string | null;
  created_at: string;
  edited_at: string | null;
}

export interface PassageVariant {
  id: string;
  passage_id: string;
  content: string;
  is_selected: boolean;
  created_at: string;
}

export interface SubmitTurnResult {
  passage: Passage;
  stream_id: string;
}

export interface RetryResult {
  removed_passage_id: string;
  stream_id: string;
}

export interface TextModelSettings {
  provider: string;
  model: string;
  has_api_key: boolean;
}

export interface NarrationDeltaPayload {
  stream_id: string;
  text: string;
}

export interface NarrationDonePayload {
  stream_id: string;
  passage: Passage;
}

export interface NarrationErrorPayload {
  stream_id: string;
  message: string;
}

export interface SwipeDonePayload {
  stream_id: string;
  passage: Passage;
  variants: PassageVariant[];
}

export interface ImageModelSettings {
  model: string;
  enabled: boolean;
  style: string;
  has_api_key: boolean;
}

export interface StoryImage {
  id: string;
  passage_id: string;
  path: string;
  prompt: string;
  seed: number | null;
  provider: string;
  created_at: string;
}

export type EntityKind = "character" | "object" | "location" | "relationship" | "campaign";

export interface Entity {
  id: string;
  story_id: string;
  kind: EntityKind;
  name: string;
  card_json: string;
  appearance_anchor: string | null;
  created_at: string;
}

export type DiceMode = "always" | "classifier" | "never";

export interface MechanicsSettings {
  dice_mode: DiceMode;
  attributes_enabled: boolean;
}

export interface Roll {
  id: string;
  passage_id: string;
  actor_entity_id: string;
  target_entity_id: string | null;
  actor_attribute_id: string | null;
  target_attribute_id: string | null;
  actor_value: number | null;
  target_value: number | null;
  p_success: number;
  seed: number;
  roll: number;
  outcome: string;
  degree: string;
  modifiers_json: string;
  created_at: string;
}

export interface EntityAttributeValue {
  entity_id: string;
  attribute_id: string;
  canonical_name: string;
  value: number;
  min: number;
  max: number;
  updated_at: string;
}

export interface RollDetail {
  roll: Roll;
  actor_name: string;
  actor_attribute_name: string | null;
  target_name: string | null;
  target_attribute_name: string | null;
  actor_attributes: EntityAttributeValue[];
  target_attributes: EntityAttributeValue[];
}
