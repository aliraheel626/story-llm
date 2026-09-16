export interface Story {
  id: string; title: string; created_at: string; updated_at: string;
  settings_json: string; default_branch_id: string | null;
}
export const DEFAULT_STORY_TITLE = "New story";
export interface StoryTitleUpdatedPayload { story_id: string; title: string }

export type InputMode = "do" | "say" | "story" | "generated" | "generated_guide" | "generated_continue" | "generated_story";
export type TimelineVisibility = "visible" | "hidden";
export type TimelineEntryKind =
  | "player_message" | "narration" | "narration_variant" | "narration_selected" | "content_edited"
  | "mechanical_result" | "entity_created" | "entity_updated" | "entity_deleted"
  | "entity_attribute_changed" | "entity_attribute_removed" | "image_generated"
  | "context_note_updated" | "mechanics_settings_changed" | "world_event" | "context_summary";

interface TimelinePayloadBase extends Record<string, unknown> {
  input_mode?: InputMode;
  selected_entry_id?: string;
  entity_id?: string;
  attribute_id?: string;
  source?: "user" | "mechanics" | "inferred" | string;
  through_entry_id?: string;
}
export interface NarrativePayload extends TimelinePayloadBase { input_mode: InputMode; thoughts?: string }
export interface NarrationVariantPayload extends TimelinePayloadBase { reason: "retry" | "swipe" | string; input_mode: InputMode; thoughts?: string }
export interface NarrationSelectedPayload extends TimelinePayloadBase { selected_entry_id: string; reason?: string }
export interface ContentEditedPayload extends TimelinePayloadBase { reason: "user_edit" | string; applies_to?: string }
export interface EntityEventPayload extends TimelinePayloadBase {
  entity_id: string; name?: string; kind?: EntityKind; appearance_anchor?: string | null;
  before?: Record<string, unknown> | null; after?: Record<string, unknown> | null;
}
export interface EntityAttributeEventPayload extends TimelinePayloadBase {
  entity_id: string; attribute_id: string; attribute_name?: string;
  before?: number | null; after?: number | null; source: "user" | "mechanics" | "inferred" | string;
}
export interface ImageGeneratedPayload extends TimelinePayloadBase { asset_id: string; prompt: string; provider: string }
export interface ContextSummaryPayload extends TimelinePayloadBase {
  through_entry_id: string; facts?: string[]; entity_notes?: string[];
  open_threads?: string[]; unresolved_mechanics?: string[];
}

interface TimelineEntryBase {
  id: string; branch_id: string; seq: number; visibility: TimelineVisibility;
  content: string | null; target_entry_id: string | null; created_at: string;
}
export type TimelineEntry =
  | (TimelineEntryBase & { kind: "player_message" | "narration"; payload: NarrativePayload })
  | (TimelineEntryBase & { kind: "narration_variant"; payload: NarrationVariantPayload })
  | (TimelineEntryBase & { kind: "narration_selected"; payload: NarrationSelectedPayload })
  | (TimelineEntryBase & { kind: "content_edited"; payload: ContentEditedPayload })
  | (TimelineEntryBase & { kind: "entity_created" | "entity_updated" | "entity_deleted"; payload: EntityEventPayload })
  | (TimelineEntryBase & { kind: "entity_attribute_changed" | "entity_attribute_removed"; payload: EntityAttributeEventPayload })
  | (TimelineEntryBase & { kind: "image_generated"; payload: ImageGeneratedPayload })
  | (TimelineEntryBase & { kind: "context_summary"; payload: ContextSummaryPayload })
  | (TimelineEntryBase & { kind: "mechanical_result" | "context_note_updated" | "mechanics_settings_changed" | "world_event"; payload: TimelinePayloadBase });

export interface NarrationVariant {
  id: string; entry_id: string; content: string; is_selected: boolean; created_at: string; thoughts?: string | null;
}
export interface SubmitTurnResult { entry: TimelineEntry; stream_id: string }
export interface RetryResult { entry_id: string; stream_id: string }
export interface NarrationDeltaPayload { stream_id: string; text: string }
export interface NarrationDonePayload { stream_id: string; entry: TimelineEntry }
export interface NarrationErrorPayload { stream_id: string; message: string }
export interface NarrationToolActivityPayload { stream_id: string; label: string; phase: "started" | "finished"; ok: boolean | null }
export interface SwipeDonePayload { stream_id: string; entry: TimelineEntry; variants: NarrationVariant[] }

export interface TextModelSettings { provider: string; model: string; has_api_key: boolean; context_window: number }
export interface ImageModelSettings { model: string; enabled: boolean; style: string; has_api_key: boolean; narrator_images: boolean }
export interface StoryImage { id: string; entry_id: string; path: string; prompt: string; seed: number | null; provider: string; created_at: string }

export type EntityKind = "character" | "object" | "location" | "relationship" | "campaign";
export interface Entity {
  id: string; story_id: string; branch_id: string; kind: EntityKind; name: string;
  appearance_anchor: string | null; created_at: string;
}
export type DiceMode = "always" | "classifier" | "never";
export interface MechanicsSettings {
  dice_mode: DiceMode;
  attributes_enabled: boolean;
  /** How much the model reasons before writing. `null` = the model's own
   *  default. See `normalize_reasoning_effort` in the backend. */
  reasoning_effort: ReasoningEffort | null;
}
export type ReasoningEffort = "none" | "minimal" | "low" | "medium" | "high" | "xhigh" | "max";
export interface AttributeRegistryEntry {
  id: string; canonical_name: string; aliases_json: string; entity_kinds_json: string;
  min: number; max: number; category: string; is_user_created: boolean;
  created_in_story_id: string | null; created_at: string;
}
export interface EntityAttributeValue {
  branch_id: string; entity_id: string; attribute_id: string; canonical_name: string;
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

export function timelineInputMode(entry: TimelineEntry): InputMode {
  return (entry.payload.input_mode as InputMode | undefined) ?? "generated";
}
export function isPlayerEntry(entry: TimelineEntry): boolean { return entry.kind === "player_message" }

// Mirrors src-tauri/src/features/timeline/reducer.rs::active_visible_entries.
// An edit is recorded against whichever variant was active when it was made
// (`applies_to`), so it stays attached to that variant across reselection
// instead of being wiped out by an unrelated later selection.
export function foldVisibleTimeline(entries: TimelineEntry[]): TimelineEntry[] {
  const byId = new Map(entries.map((entry) => [entry.id, entry]));
  const selectedVariant = new Map<string, string>();
  const edits = new Map<string, string>(); // key: `${target}|${appliesTo}`
  for (const entry of entries) {
    if (entry.kind === "narration_selected" && entry.target_entry_id) {
      const selectedId = entry.payload.selected_entry_id;
      if (typeof selectedId === "string") selectedVariant.set(entry.target_entry_id, selectedId);
    }
    if (entry.kind === "content_edited" && entry.target_entry_id && entry.content != null) {
      const appliesTo = entry.payload.applies_to ?? entry.target_entry_id;
      edits.set(`${entry.target_entry_id}|${appliesTo}`, entry.content);
    }
  }
  return entries.filter((entry) => entry.kind === "player_message" || entry.kind === "narration")
    .map((entry) => {
      const variantId = selectedVariant.get(entry.id) ?? entry.id;
      const edited = edits.get(`${entry.id}|${variantId}`);
      if (edited != null) return { ...entry, content: edited };
      if (variantId !== entry.id) {
        const variantContent = byId.get(variantId)?.content;
        if (variantContent != null) return { ...entry, content: variantContent };
      }
      return entry;
    });
}
