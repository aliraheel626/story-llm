import type { RollPayload } from "./generated/Roll";

export interface Story {
  id: string; title: string; created_at: string; updated_at: string;
  settings_json: string;
}
export const DEFAULT_STORY_TITLE = "New story";
export interface StoryTitleUpdatedPayload { story_id: string; title: string }

export type ActionMode = "do" | "say" | "story" | "guide" | "see" | "continue";
export type InputMode = ActionMode | "generated";
export type TranscriptVisibility = "visible" | "hidden";
export type TranscriptEntryKind =
  | "player_message" | "narration" | "content_edited"
  | "diceroll" | "tool_call" | "entity_created" | "entity_updated" | "entity_deleted"
  | "entity_attribute_changed" | "entity_attribute_removed" | "image_generated" | "image_captioned"
  | "context_summary";

interface TranscriptPayloadBase extends Record<string, unknown> {
  input_mode?: InputMode;
  entity_id?: string;
  attribute_id?: string;
  source?: "user" | "mechanics" | "inferred" | string;
  through_entry_id?: string;
}
export interface NarrativePayload extends TranscriptPayloadBase { input_mode: InputMode; thoughts?: string }
export interface ContentEditedPayload extends TranscriptPayloadBase { reason: "user_edit" | string }
export interface EntityEventPayload extends TranscriptPayloadBase {
  entity_id: string; name?: string | null; kind?: EntityKind; appearance_anchor?: string | null;
  before?: Record<string, unknown> | null; after?: Record<string, unknown> | null;
}
export interface EntityAttributeEventPayload extends TranscriptPayloadBase {
  entity_id: string; attribute_id: string; attribute_name?: string;
  before?: number | null; after?: number | null; source: "user" | "mechanics" | "inferred" | string;
}
export interface ImageGeneratedPayload extends TranscriptPayloadBase { asset_id: string; prompt: string }
export interface ImageCaptionedPayload extends TranscriptPayloadBase { asset_id: string; model: string; caption: string }
export interface ContextSummaryPayload extends TranscriptPayloadBase {
  through_entry_id: string; facts?: string[]; entity_notes?: string[];
  open_threads?: string[]; unresolved_mechanics?: string[];
}

interface TranscriptEntryBase {
  id: string; story_id: string; seq: number; visibility: TranscriptVisibility;
  content: string | null; target_entry_id: string | null; turn_id: string | null; created_at: string;
}
export interface ToolCallPayload {
  tool: string; args: unknown; result: unknown; ok: boolean;
}
export type TranscriptEntry =
  | (TranscriptEntryBase & { kind: "player_message" | "narration"; payload: NarrativePayload })
  | (TranscriptEntryBase & { kind: "content_edited"; payload: ContentEditedPayload })
  | (TranscriptEntryBase & { kind: "entity_created" | "entity_updated" | "entity_deleted"; payload: EntityEventPayload })
  | (TranscriptEntryBase & { kind: "entity_attribute_changed" | "entity_attribute_removed"; payload: EntityAttributeEventPayload })
  | (TranscriptEntryBase & { kind: "image_generated"; payload: ImageGeneratedPayload })
  | (TranscriptEntryBase & { kind: "image_captioned"; payload: ImageCaptionedPayload })
  | (TranscriptEntryBase & { kind: "context_summary"; payload: ContextSummaryPayload })
  | (TranscriptEntryBase & { kind: "tool_call"; payload: ToolCallPayload })
  | (TranscriptEntryBase & { kind: "diceroll"; payload: TranscriptPayloadBase });
export interface TurnSummary { id: string; status: "pending" | "complete" | "failed" }
export interface TranscriptSnapshot { visible: TranscriptEntry[]; hidden: TranscriptEntry[]; turns: TurnSummary[] }

export interface SubmitTurnResult { entry: TranscriptEntry; stream_id: string }
export interface RetryResult { entry_id: string; stream_id: string }
export interface NarrationDeltaPayload { stream_id: string; text: string }
export interface NarrationDonePayload { stream_id: string; entry: TranscriptEntry }
export interface NarrationTextCompletePayload { stream_id: string }
export interface NarrationErrorPayload { stream_id: string; story_id: string; message: string }
export interface NarrationToolActivityPayload { stream_id: string; call_id: string; label: string; phase: "started" | "finished"; ok: boolean | null }

export interface TextModelSettings { provider: string; model: string; has_api_key: boolean; context_window: number; supports_images: boolean }
export interface ImageModelSettings { model: string; enabled: boolean; style: string; has_api_key: boolean; captions_enabled: boolean; caption_model: string }
export interface TranscriptItem { key: string; group: string; label: string; enabled: boolean }
export interface ContextSettings {
  entity_kinds: { character: boolean; relationship: boolean };
  author_note_enabled: boolean;
  author_note: string;
  tool_instructions: boolean;
}
export interface ContextPreview {
  system: string;
  messages: { role: string; text: string; image_count: number; has_reasoning: boolean }[];
  injected: string;
  images_unsupported: boolean;
}
export interface StoryImage { id: string; entry_id: string; prompt: string; created_at: string; caption: string | null }

export interface StoryUsage {
  text_cost_usd: number;
  image_cost_usd: number;
  total_cost_usd: number;
  input_tokens: number;
  output_tokens: number;
  cached_input_tokens: number;
  cache_write_tokens: number;
  image_count: number;
  unpriced_calls: number;
  since: string | null;
}

export interface TurnCost {
  turn_id: string;
  text_cost_usd: number;
  image_cost_usd: number;
  total_cost_usd: number;
  earlier_attempts_cost_usd: number;
  unpriced_calls: number;
}
export interface ImageCost {
  asset_id: string;
  turn_id: string | null;
  cost_usd: number | null;
  duration_ms: number | null;
}
export interface StoryCostBreakdown { turns: TurnCost[]; images: ImageCost[] }

export type EntityKind = "character" | "relationship";
export interface CharacterFields {
  known_as: string | null; appearance_anchor: string | null;
  gender: string | null; age: string | null; role: string | null;
  location: string | null; outfit: string | null;
}
export type CharacterPatch = Partial<CharacterFields>;
export const CHARACTER_FIELD_LABELS: Record<keyof CharacterFields, string> = {
  known_as: "Known as", appearance_anchor: "Appearance", gender: "Gender",
  age: "Age", role: "Role", location: "Location", outfit: "Outfit",
};
export interface EntityLink {
  from_id: string; to_id: string; label: string;
  direction: "one_way" | "both"; description: string | null;
}
export interface Entity extends CharacterFields {
  id: string; story_id: string; kind: EntityKind; name: string;
  created_at: string; link: EntityLink | null;
}
export interface NarratorToolSettings {
  save_relationship: boolean;
  save_character: boolean;
  roll_check: boolean;
  illustrate_scene: boolean;
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
export type RollFactor = RollPayload["factors"][number];
export type Roll = RollPayload & { id: string; entry_id: string; created_at: string };

export function transcriptInputMode(entry: TranscriptEntry): InputMode {
  return ("input_mode" in entry.payload ? entry.payload.input_mode as InputMode | undefined : undefined) ?? "generated";
}
export function rollFromEntry(entry: TranscriptEntry): Roll | null {
  if (entry.payload === null || typeof entry.payload !== "object" || Array.isArray(entry.payload)) return null;
  const p = entry.payload as unknown as Record<string, unknown>;
  const { chance_percent: chance, roll, needed, outcome, seed } = p;
  if (
    !Number.isInteger(chance) || typeof chance !== "number" ||
    !Number.isInteger(roll) || typeof roll !== "number" ||
    !Number.isInteger(needed) || typeof needed !== "number" ||
    !Number.isInteger(seed) || typeof seed !== "number" ||
    typeof outcome !== "string" || outcome.length === 0 ||
    typeof entry.target_entry_id !== "string"
  ) return null;

  const factors: unknown = p.factors === undefined ? [] : p.factors;
  if (!Array.isArray(factors) || factors.length > 2 || !factors.every((factor: unknown): factor is RollFactor => {
    if (factor === null || typeof factor !== "object") return false;
    const value = factor as Partial<RollFactor>;
    return typeof value.entity_id === "string" && typeof value.entity_name === "string" &&
      typeof value.attribute_id === "string" && typeof value.attribute_name === "string" &&
      Number.isFinite(value.value) && Number.isFinite(value.min) && Number.isFinite(value.max) &&
      value.min! < value.max! && value.value! >= value.min! && value.value! <= value.max!;
  })) return null;

  const chanceSource = p.chance_source ?? null;
  if (chanceSource !== null && chanceSource !== "default" && chanceSource !== "narrator" && chanceSource !== "attributes") return null;

  return {
    id: entry.id, entry_id: entry.target_entry_id, created_at: entry.created_at,
    chance_percent: chance, roll, needed, outcome, seed,
    reason: typeof p.reason === "string" ? p.reason : null,
    chance_source: chanceSource, factors,
  };
}

export function groupRollsByEntry(hidden: readonly TranscriptEntry[]): Record<string, Roll[]> {
  const grouped: Record<string, Roll[]> = Object.create(null);
  for (const entry of hidden) {
    if (entry.kind !== "diceroll") continue;
    const roll = rollFromEntry(entry);
    if (roll) (grouped[roll.entry_id] ??= []).push(roll);
  }
  return grouped;
}
export function isPlayerEntry(entry: TranscriptEntry): boolean { return entry.kind === "player_message" }
