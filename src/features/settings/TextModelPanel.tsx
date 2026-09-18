import { useState } from "react";
import { useTextModelStore } from "./textModelStore";
import { useSettingsForm } from "./useSettingsForm";

const PROVIDERS: { value: string; label: string }[] = [
  { value: "openrouter", label: "OpenRouter" },
  { value: "nous_portal", label: "Nous Portal" },
];

const MODELS_BY_PROVIDER: Record<string, { slug: string; label: string }[]> = {
  openrouter: [
    { slug: "x-ai/grok-4.3", label: "Grok 4.3" },
    { slug: "anthropic/claude-sonnet-4.5", label: "Claude Sonnet 4.5" },
    { slug: "nousresearch/hermes-4-70b", label: "Hermes 4 70B" },
    { slug: "nousresearch/hermes-4-405b", label: "Hermes 4 405B" },
  ],
  // Confirmed against Nous Portal's own API docs (portal.nousresearch.com) —
  // its entire available-model list, all with 128k context. Hermes on
  // Nous Portal's chat-completions endpoint specifically turned out to be
  // broken server-side as of this session (confirmed via their own
  // playground) — kept here since that's likely to get fixed upstream;
  // OpenRouter's Hermes route above is the working alternative meanwhile.
  nous_portal: [
    { slug: "Hermes-4.3-36B", label: "Hermes 4.3 36B" },
    { slug: "Hermes-4-70B", label: "Hermes 4 70B" },
    { slug: "Hermes-4-405B", label: "Hermes 4 405B" },
  ],
};

const API_KEY_PLACEHOLDER: Record<string, string> = {
  openrouter: "sk-or-...",
  nous_portal: "your Nous Portal API key",
};

export function TextModelPanel() {
  const [provider, setProvider] = useState("openrouter");
  const [model, setModel] = useState("");
  const [customModel, setCustomModel] = useState(false);
  const [apiKey, setApiKey] = useState("");
  const { settings, loading, saving, savedNotice, save } = useSettingsForm(
    useTextModelStore,
    (next) => {
      setProvider(next.provider);
      setModel(next.model);
      setCustomModel(
        !(MODELS_BY_PROVIDER[next.provider] ?? []).some((m) => m.slug === next.model),
      );
    },
  );

  // Switching provider clears the model field rather than leaving whatever
  // was typed for the other provider (their model namespaces don't overlap —
  // an OpenRouter slug like "x-ai/grok-4.3" means nothing to Nous Portal).
  const onProviderChange = (nextProvider: string) => {
    setProvider(nextProvider);
    const presets = MODELS_BY_PROVIDER[nextProvider] ?? [];
    setModel(presets[0]?.slug ?? "");
    setCustomModel(presets.length === 0);
  };

  const onModelSelect = (value: string) => {
    if (value === "__custom__") {
      setCustomModel(true);
      return;
    }
    setCustomModel(false);
    setModel(value);
  };

  const onSave = async () => {
    if (await save(provider, model.trim(), apiKey.trim() || undefined)) {
      setApiKey("");
    }
  };

  if (loading && !settings) {
    return <div className="text-xs text-muted py-1">Loading...</div>;
  }

  // `has_api_key` is fetched for whatever provider is currently saved, so it
  // only reflects the selected provider's own key when they match — picking
  // a different provider from the dropdown without saving yet shows neither
  // "set" nor "not set" until save confirms which key was checked.
  const apiKeyMatchesSelectedProvider = settings?.provider === provider;

  return (
    <div className="flex flex-col gap-2 text-sm">
      <div>
        <label className="block text-xs text-muted mb-1">Provider</label>
        <select
          value={provider}
          onChange={(e) => onProviderChange(e.target.value)}
          className="w-full rounded bg-bg border border-border px-2 py-1.5 text-sm text-text focus:outline-none focus:border-accent"
        >
          {PROVIDERS.map((p) => (
            <option key={p.value} value={p.value}>
              {p.label}
            </option>
          ))}
        </select>
      </div>

      <div>
        <label className="block text-xs text-muted mb-1">Model</label>
        <select
          value={customModel ? "__custom__" : model}
          onChange={(e) => onModelSelect(e.target.value)}
          className="w-full rounded bg-bg border border-border px-2 py-1.5 text-sm text-text focus:outline-none focus:border-accent"
        >
          {(MODELS_BY_PROVIDER[provider] ?? []).map((m) => (
            <option key={m.slug} value={m.slug}>
              {m.label}
            </option>
          ))}
          <option value="__custom__">Custom…</option>
        </select>
        {customModel && (
          <input
            value={model}
            onChange={(e) => setModel(e.target.value)}
            placeholder={
              provider === "nous_portal"
                ? "exact model id from Nous Portal's listing"
                : "e.g. anthropic/claude-opus-5"
            }
            className="mt-1.5 w-full rounded bg-bg border border-border px-2 py-1.5 text-sm text-text placeholder:text-muted focus:outline-none focus:border-accent"
          />
        )}
      </div>

      <div>
        <label className="block text-xs text-muted mb-1">
          API key {apiKeyMatchesSelectedProvider && settings?.has_api_key && <span className="text-success">(set)</span>}
        </label>
        <input
          type="password"
          value={apiKey}
          onChange={(e) => setApiKey(e.target.value)}
          placeholder={
            apiKeyMatchesSelectedProvider && settings?.has_api_key
              ? "•••••••• (leave blank to keep)"
              : API_KEY_PLACEHOLDER[provider] ?? API_KEY_PLACEHOLDER.openrouter
          }
          className="w-full rounded bg-bg border border-border px-2 py-1.5 text-sm text-text placeholder:text-muted focus:outline-none focus:border-accent"
        />
      </div>

      {provider === "nous_portal" && (
        <p className="text-[11px] text-muted">
          Image generation and attribute matching always use OpenRouter separately — keep an
          OpenRouter key set too, even with Nous Portal as your narration provider.
        </p>
      )}

      <button
        onClick={onSave}
        disabled={saving || !model.trim()}
        className="mt-1 rounded bg-accent px-2 py-1.5 text-xs font-medium text-bg hover:bg-accent-hover disabled:opacity-40 transition-colors"
      >
        {saving ? "Saving..." : savedNotice ? "Saved" : "Save"}
      </button>
    </div>
  );
}
