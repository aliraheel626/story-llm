import type { TranscriptEntry } from "../../shared/types";

export interface EntryDisplay {
  text: string;
  streaming: boolean;
  showOldExtras: boolean;
}

/** The old entry stays in the store, so clearing a failed stream restores it. */
export function entryDisplay(
  entry: TranscriptEntry,
  streaming: { mode: string; targetEntryId?: string; text: string; textComplete?: boolean } | null | undefined,
): EntryDisplay {
  const replacing = streaming?.mode === "replace" && streaming.targetEntryId === entry.id;
  if (!replacing) return { text: entry.content ?? "", streaming: false, showOldExtras: true };
  return { text: streaming.text, streaming: !streaming.textComplete, showOldExtras: false };
}
