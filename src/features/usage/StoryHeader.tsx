import { useEffect } from "react";
import { useStoryStore } from "../story/store";
import { formatTokens, formatUsd, useUsageStore } from "./store";

export function StoryHeader({ storyId }: { storyId: string }) {
  const busy = useStoryStore((state) => !!state.bundles[storyId]?.streaming);
  const usage = useUsageStore((state) => state.byStory[storyId]);
  const load = useUsageStore((state) => state.load);

  useEffect(() => { if (!busy) load(storyId); }, [storyId, busy, load]);

  const cachedPercent = usage?.input_tokens ? Math.round(usage.cached_input_tokens / usage.input_tokens * 100) : 0;
  const totalTooltip = usage?.since
    ? `Text + images since ${new Date(usage.since).toLocaleDateString()}`
    : "No usage recorded yet";
  const unpriced = usage?.unpriced_calls ?? 0;
  const totalTitle = unpriced > 0
    ? `${totalTooltip}. ${unpriced} calls reported no cost (Ollama, Nous Portal, or a request that failed or timed out before reporting its cost) and aren't included`
    : totalTooltip;

  return (
    <div className="flex h-12 shrink-0 items-center justify-end gap-2 overflow-x-auto whitespace-nowrap border-b border-border px-3 text-xs text-muted tabular-nums sm:gap-4 sm:px-6">
      <span title="Narration, context summaries and titles">Text {usage ? formatUsd(usage.text_cost_usd) : "—"}</span>
      <span title={usage ? `${usage.image_count} scene images` : "Scene images"}>
        Images {usage ? `${formatUsd(usage.image_cost_usd)} (${usage.image_count})` : "—"}
      </span>
      <span title={usage
        ? `${usage.cached_input_tokens.toLocaleString()} of ${usage.input_tokens.toLocaleString()} input tokens (${cachedPercent}%) were read from the provider's cache`
        : "Cached input tokens"}>
        Cached input {usage ? formatTokens(usage.cached_input_tokens) : "—"}
      </span>
      <span title="Input tokens written to the provider's cache">
        Cache write {usage ? formatTokens(usage.cache_write_tokens) : "—"}
      </span>
      <span className="font-medium text-text" title={totalTitle}>
        Total {usage ? formatUsd(usage.total_cost_usd) : "—"}{unpriced > 0 ? "*" : ""}
      </span>
    </div>
  );
}
