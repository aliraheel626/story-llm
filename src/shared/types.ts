export interface Story {
  id: string; title: string; created_at: string; updated_at: string;
  settings_json: string;
}
export const DEFAULT_STORY_TITLE = "New story";
export interface StoryTitleUpdatedPayload { story_id: string; title: string }

export type ActionMode = "do" | "say" | "story" | "guide" | "see" | "continue";
export type InputMode = ActionMode | "generated";
export type LedgerVisibility = "visible" | "hidden";
export type LedgerEntryKind =
  | "player_message" | "narration" | "narration_variant" | "narration_selected" | "content_edited"
  | "diceroll" | "entity_created" | "entity_queried" | "entity_updated" | "entity_deleted"
  | "entity_attribute_changed" | "entity_attribute_removed" | "image_generated"
  | "diceroll_settings_changed" | "context_summary";

interface LedgerPayloadBase extends Record<string, unknown> {
  input_mode?: InputMode;
  selected_entry_id?: string;
  entity_id?: string;
  attribute_id?: string;
  source?: "user" | "mechanics" | "inferred" | string;
  through_entry_id?: string;
}
export interface NarrativePayload extends LedgerPayloadBase { input_mode: InputMode; thoughts?: string }
export interface NarrationVariantPayload extends LedgerPayloadBase { reason: "retry" | "swipe" | string; input_mode: InputMode; thoughts?: string }
export interface NarrationSelectedPayload extends LedgerPayloadBase { selected_entry_id: string; reason?: string }
export interface ContentEditedPayload extends LedgerPayloadBase { reason: "user_edit" | string; applies_to?: string }
export interface EntityEventPayload extends LedgerPayloadBase {
  entity_id: string; name?: string; kind?: EntityKind; appearance_anchor?: string | null;
  before?: Record<string, unknown> | null; after?: Record<string, unknown> | null;
}
export interface EntityAttributeEventPayload extends LedgerPayloadBase {
  entity_id: string; attribute_id: string; attribute_name?: string;
  before?: number | null; after?: number | null; source: "user" | "mechanics" | "inferred" | string;
}
export interface ImageGeneratedPayload extends LedgerPayloadBase { asset_id: string; prompt: string }
export interface ContextSummaryPayload extends LedgerPayloadBase {
  through_entry_id: string; facts?: string[]; entity_notes?: string[];
  open_threads?: string[]; unresolved_mechanics?: string[];
}

interface LedgerEntryBase {
  id: string; story_id: string; seq: number; visibility: LedgerVisibility;
  content: string | null; target_entry_id: string | null; created_at: string;
}
export type LedgerEntry =
  | (LedgerEntryBase & { kind: "player_message" | "narration"; payload: NarrativePayload })
  | (LedgerEntryBase & { kind: "narration_variant"; payload: NarrationVariantPayload })
  | (LedgerEntryBase & { kind: "narration_selected"; payload: NarrationSelectedPayload })
  | (LedgerEntryBase & { kind: "content_edited"; payload: ContentEditedPayload })
  | (LedgerEntryBase & { kind: "entity_created" | "entity_updated" | "entity_deleted"; payload: EntityEventPayload })
  | (LedgerEntryBase & { kind: "entity_attribute_changed" | "entity_attribute_removed"; payload: EntityAttributeEventPayload })
  | (LedgerEntryBase & { kind: "image_generated"; payload: ImageGeneratedPayload })
  | (LedgerEntryBase & { kind: "context_summary"; payload: ContextSummaryPayload })
  | (LedgerEntryBase & { kind: "diceroll" | "entity_queried" | "diceroll_settings_changed"; payload: LedgerPayloadBase });
export interface LedgerSnapshot { visible: LedgerEntry[]; hidden: LedgerEntry[] }

export interface NarrationVariant {
  id: string; entry_id: string; content: string; is_selected: boolean; created_at: string; thoughts?: string | null;
}
export interface SubmitTurnResult { entry: LedgerEntry; stream_id: string }
export interface RetryResult { entry_id: string; stream_id: string }
export interface NarrationDeltaPayload { stream_id: string; text: string }
export interface NarrationDonePayload { stream_id: string; entry: LedgerEntry }
export interface NarrationErrorPayload { stream_id: string; message: string }
export interface NarrationToolActivityPayload { stream_id: string; call_id: string; label: string; phase: "started" | "finished"; ok: boolean | null }
export interface SwipeDonePayload { stream_id: string; entry: LedgerEntry; variants: NarrationVariant[] }

export interface TextModelSettings { provider: string; model: string; has_api_key: boolean; context_window: number }
export interface ImageModelSettings { model: string; enabled: boolean; style: string; has_api_key: boolean; narrator_images: boolean }
export type EntityContextMode = "all" | "scoped" | "none";
export interface ContextInjectionSettings { entity_context_mode: EntityContextMode; dice_rolls_in_context: boolean }
export interface LedgerRetentionSettings { tool_call_persistence: boolean }
export interface StoryImage { id: string; entry_id: string; path: string; prompt: string; created_at: string }

export type EntityKind = "character" | "object" | "location" | "relationship" | "campaign";
export interface Entity {
  id: string; story_id: string; kind: EntityKind; name: string;
  appearance_anchor: string | null; created_at: string;
}
export type DiceMode = "always" | "classifier" | "never";
export interface DicerollSettings {
  dice_mode: DiceMode;
  attributes_enabled: boolean;
  /** How much the model reasons before writing. `null` = the model's own
   *  default. See `normalize_reasoning_effort` in the backend. */
  reasoning_effort: ReasoningEffort | null;
}
export type ReasoningEffort = "none" | "minimal" | "low" | "medium" | "high" | "xhigh" | "max";
export const REASONING_EFFORT_OPTIONS: ReadonlyArray<{ value: ReasoningEffort | ""; label: string }> = [
  { value: "", label: "Default" },
  { value: "none", label: "None" },
  { value: "minimal", label: "Minimal" },
  { value: "low", label: "Low" },
  { value: "medium", label: "Medium" },
  { value: "high", label: "High" },
  { value: "xhigh", label: "Extra high" },
  { value: "max", label: "Max" },
];
export interface AttributeRegistryEntry {
  id: string; canonical_name: string; aliases_json: string; entity_kinds_json: string;
  min: number; max: number; category: string; is_user_created: boolean;
  created_in_story_id: string | null; created_at: string;
}
export interface EntityAttributeValue {
  story_id: string; entity_id: string; attribute_id: string; canonical_name: string;
  value: number; min: number; max: number; updated_at: string; source: string;
}
export interface Roll {
  id: string; entry_id: string; actor_entity_id: string; target_entity_id: string | null;
  actor_attribute_id: string | null; target_attribute_id: string | null;
  actor_value: number | null; target_value: number | null; p_success: number;
  seed: number; roll: number; outcome: string; degree: string; modifiers_json: string; created_at: string;
}
export interface RollDetail {
  roll: Roll; actor_name: string; actor_attribute_name: string | null; target_name: string | null;
  target_attribute_name: string | null; actor_attributes: EntityAttributeValue[]; target_attributes: EntityAttributeValue[];
}

export function ledgerInputMode(entry: LedgerEntry): InputMode {
  return (entry.payload.input_mode as InputMode | undefined) ?? "generated";
}
export function isPlayerEntry(entry: LedgerEntry): boolean { return entry.kind === "player_message" }
