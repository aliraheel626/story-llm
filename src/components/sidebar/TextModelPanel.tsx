import { useEffect, useState } from "react";
import { useTextModelStore } from "../../store/textModelStore";

export function TextModelPanel() {
  const settings = useTextModelStore((s) => s.settings);
  const loading = useTextModelStore((s) => s.loading);
  const saving = useTextModelStore((s) => s.saving);
  const load = useTextModelStore((s) => s.load);
  const save = useTextModelStore((s) => s.save);

  const [model, setModel] = useState("");
  const [apiKey, setApiKey] = useState("");
  const [savedNotice, setSavedNotice] = useState(false);

  useEffect(() => {
    load();
  }, [load]);

  useEffect(() => {
    if (settings) setModel(settings.model);
  }, [settings]);

  const onSave = async () => {
    try {
      await save("openrouter", model.trim(), apiKey.trim() || undefined);
      setApiKey("");
      setSavedNotice(true);
      setTimeout(() => setSavedNotice(false), 2000);
    } catch (e) {
      console.error(e);
    }
  };

  if (loading && !settings) {
    return <div className="text-xs text-muted py-1">Loading...</div>;
  }

  return (
    <div className="flex flex-col gap-2 text-sm">
      <div>
        <label className="block text-xs text-muted mb-1">Provider</label>
        <select
          disabled
          value="openrouter"
          className="w-full rounded bg-bg border border-border px-2 py-1.5 text-sm text-text disabled:opacity-60"
        >
          <option value="openrouter">OpenRouter</option>
        </select>
      </div>

      <div>
        <label className="block text-xs text-muted mb-1">Model</label>
        <input
          value={model}
          onChange={(e) => setModel(e.target.value)}
          placeholder="e.g. anthropic/claude-sonnet-4.5"
          className="w-full rounded bg-bg border border-border px-2 py-1.5 text-sm text-text placeholder:text-muted focus:outline-none focus:border-accent"
        />
      </div>

      <div>
        <label className="block text-xs text-muted mb-1">
          API key {settings?.has_api_key && <span className="text-success">(set)</span>}
        </label>
        <input
          type="password"
          value={apiKey}
          onChange={(e) => setApiKey(e.target.value)}
          placeholder={settings?.has_api_key ? "•••••••• (leave blank to keep)" : "sk-or-..."}
          className="w-full rounded bg-bg border border-border px-2 py-1.5 text-sm text-text placeholder:text-muted focus:outline-none focus:border-accent"
        />
      </div>

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
