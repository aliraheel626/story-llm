import { useState } from "react";
import type { LedgerEntry } from "../../shared/types";

export interface ToolCall { key: string; label: string; done: boolean; ok: boolean | null }
export interface TurnActivityData { thoughts?: string | null; tools: ToolCall[] }

/** Legacy effect-event kinds used only for turns created before tool_call rows. */
const TOOL_EVENT_KINDS = new Set([
  "diceroll",
  "entity_created",
  "entity_queried",
  "entity_updated",
  "entity_deleted",
  "entity_attribute_changed",
  "entity_attribute_removed",
  "image_generated",
]);

const eventLabel = (entry: LedgerEntry): string => {
  const text = (entry.content ?? "").replace(/^Dice-roll outcome:\s*/i, "").replace(/\.$/, "");
  return text || entry.kind.replace(/_/g, " ");
};

/** The tool calls committed against one narration revision, in call order. */
export function toolCallsFromEvents(entryId: string, hidden: LedgerEntry[] | undefined): ToolCall[] {
  if (!hidden) return [];
  const targeted = hidden
    .filter((event) => event.target_entry_id === entryId)
    .sort((a, b) => a.seq - b.seq);
  const captured = targeted.filter(
    (event): event is Extract<LedgerEntry, { kind: "tool_call" }> => event.kind === "tool_call",
  );
  if (captured.length > 0) {
    return captured.map((event) => ({
      key: event.id,
      label: event.content ?? event.payload.tool,
      done: true,
      ok: typeof event.payload.ok === "boolean" ? event.payload.ok : null,
    }));
  }
  return targeted
    .filter((event) => TOOL_EVENT_KINDS.has(event.kind))
    .map((event) => ({ key: event.id, label: eventLabel(event), done: true, ok: true }));
}

/**
 * The narrator's reasoning and tool calls for a single turn, collapsed behind
 * a one-line summary and shown between the player's action and the narration
 * it produced. While the turn is still streaming it opens itself so the
 * thinking is visible as it arrives; afterwards it stays closed unless the
 * reader opens it.
 */
export function TurnActivity({ activity, live }: { activity: TurnActivityData; live?: boolean }) {
  const [chosen, setChosen] = useState<boolean | null>(null);
  const thoughts = (activity.thoughts ?? "").trim();
  const tools = activity.tools;
  if (!thoughts && tools.length === 0) return null;

  const open = chosen ?? !!live;
  const running = live && tools.some((tool) => !tool.done);
  const summary = thoughts.split("\n")[0]?.slice(0, 90) ?? "";
  const callCount = tools.length > 0 ? `${tools.length} tool call${tools.length === 1 ? "" : "s"}` : "Thinking";

  return (
    <div className="mb-2 overflow-hidden rounded border border-border/60">
      <button
        onClick={() => setChosen(!open)}
        aria-expanded={open}
        title="The narrator's reasoning and tool calls for this turn"
        className="flex w-full items-center gap-2 px-2 py-1 text-left text-[11px] text-muted transition-colors hover:text-text"
      >
        <span aria-hidden>{open ? "▾" : "▸"}</span>
        <span>{tools.length > 0 && thoughts ? `Thinking & ${callCount}` : callCount}</span>
        {!open && summary && <span className="truncate opacity-60">{summary}</span>}
        {(running || (live && thoughts)) && (
          <span className="ml-auto inline-block h-1.5 w-1.5 shrink-0 animate-pulse rounded-full bg-muted motion-reduce:animate-none" />
        )}
      </button>

      {open && (
        <div className="max-h-64 animate-fade-in overflow-y-auto border-t border-border/60 px-2 py-1.5">
          {tools.length > 0 && (
            <ul className="mb-1.5 flex flex-col gap-0.5">
              {tools.map((tool) => (
                <li key={tool.key} className={`flex items-start gap-1.5 text-[11px] ${tool.ok === false ? "text-danger" : "text-muted"}`}>
                  <span aria-hidden>{tool.ok === false ? "×" : tool.done ? "✓" : "◌"}</span>
                  <span className={tool.ok === false ? "text-danger" : "text-text"}>{tool.label}</span>
                </li>
              ))}
            </ul>
          )}
          {thoughts ? (
            <p className="whitespace-pre-wrap font-prose text-[12px] leading-5 text-muted">{thoughts}</p>
          ) : (
            <p className="text-[11px] italic text-muted">No reasoning for this turn.</p>
          )}
        </div>
      )}
    </div>
  );
}
