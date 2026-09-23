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
  | "player_message" | "narration" | "content_edited"
  | "diceroll" | "entity_created" | "entity_queried" | "entity_updated" | "entity_deleted"
  | "entity_attribute_changed" | "entity_attribute_removed" | "image_generated"
  | "context_summary";

interface LedgerPayloadBase extends Record<string, unknown> {
  input_mode?: InputMode;
  entity_id?: string;
  attribute_id?: string;
  source?: "user" | "mechanics" | "inferred" | string;
  through_entry_id?: string;
}
export interface NarrativePayload extends LedgerPayloadBase { input_mode: InputMode; thoughts?: string }
export interface ContentEditedPayload extends LedgerPayloadBase { reason: "user_edit" | string }
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
  | (LedgerEntryBase & { kind: "content_edited"; payload: ContentEditedPayload })
  | (LedgerEntryBase & { kind: "entity_created" | "entity_updated" | "entity_deleted"; payload: EntityEventPayload })
  | (LedgerEntryBase & { kind: "entity_attribute_changed" | "entity_attribute_removed"; payload: EntityAttributeEventPayload })
  | (LedgerEntryBase & { kind: "image_generated"; payload: ImageGeneratedPayload })
  | (LedgerEntryBase & { kind: "context_summary"; payload: ContextSummaryPayload })
  | (LedgerEntryBase & { kind: "diceroll" | "entity_queried"; payload: LedgerPayloadBase });
export interface LedgerSnapshot { visible: LedgerEntry[]; hidden: LedgerEntry[] }

export interface SubmitTurnResult { entry: LedgerEntry; stream_id: string }
export interface RetryResult { entry_id: string; stream_id: string }
export interface NarrationDeltaPayload { stream_id: string; text: string }
export interface NarrationDonePayload { stream_id: string; entry: LedgerEntry }
export interface NarrationErrorPayload { stream_id: string; message: string }
export interface NarrationToolActivityPayload { stream_id: string; call_id: string; label: string; phase: "started" | "finished"; ok: boolean | null }

export interface TextModelSettings { provider: string; model: string; has_api_key: boolean; context_window: number }
export interface ImageModelSettings { model: string; enabled: boolean; style: string; has_api_key: boolean }
export type EntityContextMode = "all" | "scoped" | "none";
export interface ContextInjectionSettings { entity_context_mode: EntityContextMode; dice_rolls_in_context: boolean }
export interface LedgerRetentionSettings { tool_call_persistence: boolean }
export interface StoryImage { id: string; entry_id: string; path: string; prompt: string; created_at: string }

export type EntityKind = "character" | "object" | "location" | "relationship" | "campaign";
export interface Entity {
  id: string; story_id: string; kind: EntityKind; name: string;
  appearance_anchor: string | null; created_at: string;
}
export interface NarratorToolSettings {
  get_entities: boolean;
  create_entity: boolean;
  update_entity: boolean;
  adjust_entity_attribute: boolean;
  roll_check: boolean;
  illustrate_scene: boolean;
}
export const DEFAULT_NARRATOR_TOOLS: NarratorToolSettings = {
  get_entities: true,
  create_entity: true,
  update_entity: true,
  adjust_entity_attribute: true,
  roll_check: true,
  illustrate_scene: true,
};
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
export interface RollFactor {
  entity_id: string; entity_name: string; attribute_id: string; attribute_name: string;
  value: number; min: number; max: number;
}
export interface Roll {
  id: string; entry_id: string; reason: string | null; chance_percent: number;
  chance_source?: "default" | "narrator" | "attributes"; factors?: RollFactor[];
  seed: number; roll: number; outcome: string; created_at: string;
}

export function ledgerInputMode(entry: LedgerEntry): InputMode {
  return (entry.payload.input_mode as InputMode | undefined) ?? "generated";
}
export function rollFromEntry(entry: LedgerEntry): Roll | null {
  if (entry.payload === null || typeof entry.payload !== "object" || Array.isArray(entry.payload)) return null;
  const p: Record<string, unknown> = entry.payload;
  const { chance_percent: chance, roll, outcome, seed } = p;
  if (
    !Number.isInteger(chance) || typeof chance !== "number" || chance < 0 || chance > 100 ||
    !Number.isInteger(roll) || typeof roll !== "number" || roll < 0 || roll >= 100 ||
    !Number.isInteger(seed) || typeof seed !== "number" || seed < -(2 ** 63) || seed >= 2 ** 63 ||
    (outcome !== "success" && outcome !== "failure") ||
    (outcome === "success") !== (roll >= 100 - chance) ||
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

  let chanceSource: Roll["chance_source"];
  if (typeof p.chance_source === "string") {
    if (p.chance_source === "default" && factors.length === 0 && chance === 50) chanceSource = "default";
    else if (p.chance_source === "narrator" && factors.length === 0) chanceSource = "narrator";
    else if (p.chance_source === "attributes" && factors.length > 0) chanceSource = "attributes";
    else return null;
  }

  return {
    id: entry.id, entry_id: entry.target_entry_id, created_at: entry.created_at,
    chance_percent: chance, roll, outcome, seed,
    reason: typeof p.reason === "string" ? p.reason : null,
    chance_source: chanceSource, factors,
  };
}

export function groupRollsByEntry(hidden: readonly LedgerEntry[]): Record<string, Roll[]> {
  const grouped: Record<string, Roll[]> = {};
  for (const entry of hidden) {
    if (entry.kind !== "diceroll") continue;
    const roll = rollFromEntry(entry);
    if (roll) (grouped[roll.entry_id] ??= []).push(roll);
  }
  return grouped;
}
export function isPlayerEntry(entry: LedgerEntry): boolean { return entry.kind === "player_message" }
