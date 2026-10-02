"""Replays the logged `<see>the pebble</see>` request against OpenRouter with
streaming on, and records which SSE field every piece arrives in. The API key is
read from secrets.json into memory only; it is never printed or written."""
import json, os, re, sys, urllib.request

APP = os.environ["APPDATA"] + r"\com.story-llm.app"
LOG = os.environ["LOCALAPPDATA"] + r"\com.story-llm.app\logs\story-llm.log"
OUT = os.path.dirname(os.path.abspath(__file__))
run = sys.argv[1] if len(sys.argv) > 1 else "1"


def unrust(s):  # Rust Debug string body -> text
    return re.sub(r'\\(["\\n])', lambda m: "\n" if m.group(1) == "n" else m.group(1), s)


line = next(l for l in open(LOG, encoding="utf-8") if "<see>the pebble</see>" in l)
system = unrust(re.search(r'system="((?:[^"\\]|\\.)*)"', line).group(1))
last = unrust(re.search(r'last_turn="((?:[^"\\]|\\.)*)"\s*$', line).group(1))
hist = re.search(r"history=\[(.*)\] last_turn=", line).group(1)
pairs = re.findall(r'\("(player|narrator)", "((?:[^"\\]|\\.)*)"\)', hist)
messages = [{"role": "system", "content": system}]
messages += [{"role": "user" if r == "player" else "assistant", "content": unrust(c)} for r, c in pairs]
messages.append({"role": "user", "content": last})

src = open(r"src-tauri\src\prompts.rs", encoding="utf-8").read()
desc = re.search(r'ILLUSTRATE_SCENE_DESCRIPTION: &str =\s*"(.*?)";', src, re.S).group(1)
desc = re.sub(r"\\\n\s*", "", desc)
tool = {"type": "function", "function": {"name": "illustrate_scene", "description": desc, "parameters": {
    "type": "object",
    "properties": {
        "description": {"type": "string", "description": "A vivid, concrete visual description of the scene's subject, setting, composition, and lighting."},
        "character_ids": {"type": "array", "items": {"type": "string"}, "description": "Ids of characters visible in the scene, from get_entities."}},
    "required": ["description"]}}}

body = {"model": "x-ai/grok-4.7", "stream": True, "messages": messages, "tools": [tool],
        "tool_choice": "required", "usage": {"include": True}}
key = json.load(open(APP + r"\secrets.json"))["text_model.openrouter.api_key"]
req = urllib.request.Request("https://openrouter.ai/api/v1/chat/completions", json.dumps(body).encode(),
                             {"Authorization": "Bearer " + key, "Content-Type": "application/json"})
del key

content, tool_args, reasoning_chars, usage, finish = [], [], 0, None, None
with urllib.request.urlopen(req, timeout=180) as resp, open(f"{OUT}\\see_raw_{run}.sse", "w", encoding="utf-8") as raw:
    for rawline in resp:
        text = rawline.decode("utf-8").strip()
        raw.write(text + "\n")
        if not text.startswith("data: ") or text == "data: [DONE]":
            continue
        chunk = json.loads(text[6:])
        usage = chunk.get("usage") or usage
        for choice in chunk.get("choices", []):
            delta = choice.get("delta", {})
            finish = choice.get("finish_reason") or finish
            if delta.get("content"):
                content.append(delta["content"])
            reasoning_chars += len(delta.get("reasoning") or "")
            for call in delta.get("tool_calls") or []:
                tool_args.append((call.get("function") or {}).get("arguments") or "")

print(json.dumps({
    "run": run,
    "delta.content (visible text)": "".join(content),
    "delta.tool_calls arguments": "".join(tool_args),
    "reasoning chars": reasoning_chars,
    "finish_reason": finish,
    "usage": usage,
}, indent=2, ensure_ascii=False))
