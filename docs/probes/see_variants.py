"""Tries prompt / tool_choice variants of the logged See request against
OpenRouter (Grok 4.7) and counts how often the tool call leaks into visible
text. The API key stays in memory; it is never printed or written."""
import json, os, re, sys, urllib.request
from concurrent.futures import ThreadPoolExecutor

APP = os.environ["APPDATA"] + r"\com.story-llm.app"
LOG = os.environ["LOCALAPPDATA"] + r"\com.story-llm.app\logs\story-llm.log"
RUNS = int(sys.argv[2]) if len(sys.argv) > 2 else 3
ONLY = sys.argv[1].split(",") if len(sys.argv) > 1 and sys.argv[1] != "all" else None


def unrust(s):
    return re.sub(r'\\(["\\n])', lambda m: "\n" if m.group(1) == "n" else m.group(1), s)


line = next(l for l in open(LOG, encoding="utf-8") if "<see>the pebble</see>" in l)
SYSTEM = unrust(re.search(r'system="((?:[^"\\]|\\.)*)"', line).group(1))
LAST = unrust(re.search(r'last_turn="((?:[^"\\]|\\.)*)"\s*$', line).group(1))
hist = re.search(r"history=\[(.*)\] last_turn=", line).group(1)
HISTORY = [{"role": "user" if r == "player" else "assistant", "content": unrust(c)}
           for r, c in re.findall(r'\("(player|narrator)", "((?:[^"\\]|\\.)*)"\)', hist)]
src = open(r"src-tauri\src\prompts.rs", encoding="utf-8").read()
DESC = re.sub(r"\\\n\s*", "", re.search(r'ILLUSTRATE_SCENE_DESCRIPTION: &str =\s*"(.*?)";', src, re.S).group(1))
PARAMS = {"type": "object", "properties": {
    "description": {"type": "string", "description": "A vivid, concrete visual description of the scene's subject, setting, composition, and lighting."},
    "character_ids": {"type": "array", "items": {"type": "string"}, "description": "Ids of characters visible in the scene, from get_entities."}},
    "required": ["description"]}

SEE_OLD = "Make it a real tool call through the tool-calling interface; never write the call out as text, and write no other prose."
DESC_OLD = "Invoke it as a real tool call, at most once per turn; never write the call out as text."
assert SEE_OLD in SYSTEM and (DESC_OLD in DESC or "Call it at most once per turn." in DESC)


def variant(system=SYSTEM, desc=DESC, choice="required"):
    return {"system": system, "desc": desc, "choice": choice}


VARIANTS = {
    "baseline": variant(),
    "updated": variant(SYSTEM.replace(" " + SEE_OLD, "")),
    "no_mention": variant(SYSTEM.replace(" " + SEE_OLD, ""), DESC.replace(DESC_OLD, "Call it at most once per turn.")),
    "empty_reply": variant(
        SYSTEM.replace(SEE_OLD, "The tool call is your whole reply: leave the message text completely empty."),
        DESC.replace(DESC_OLD, "Call it at most once per turn. The call is the whole reply; send no message text with it.")),
    "named_choice": variant(choice={"type": "function", "function": {"name": "illustrate_scene"}}),
    "auto_choice": variant(choice="auto"),
}
NO_MENTION_SYSTEM = SYSTEM.replace(" " + SEE_OLD, "")
NO_MENTION_DESC = DESC.replace(DESC_OLD, "Call it at most once per turn.")
VARIANTS["no_mention_named"] = variant(NO_MENTION_SYSTEM, NO_MENTION_DESC,
                                       {"type": "function", "function": {"name": "illustrate_scene"}})
VARIANTS["no_mention_auto"] = variant(NO_MENTION_SYSTEM, NO_MENTION_DESC, "auto")


def run(name, v, i):
    body = {"model": "x-ai/grok-4.7", "stream": True, "tool_choice": v["choice"], "usage": {"include": True},
            "messages": [{"role": "system", "content": v["system"]}, *HISTORY, {"role": "user", "content": LAST}],
            "tools": [{"type": "function", "function": {"name": "illustrate_scene", "description": v["desc"], "parameters": PARAMS}}]}
    key = json.load(open(APP + r"\secrets.json"))["text_model.openrouter.api_key"]
    req = urllib.request.Request("https://openrouter.ai/api/v1/chat/completions", json.dumps(body).encode(),
                                 {"Authorization": "Bearer " + key, "Content-Type": "application/json"})
    del key
    content, args, cost, provider = [], [], 0.0, None
    with urllib.request.urlopen(req, timeout=180) as resp:
        for rawline in resp:
            text = rawline.decode("utf-8").strip()
            if not text.startswith("data: ") or text == "data: [DONE]":
                continue
            chunk = json.loads(text[6:])
            provider = chunk.get("provider") or provider
            cost = (chunk.get("usage") or {}).get("cost", cost)
            for choice in chunk.get("choices", []):
                delta = choice.get("delta", {})
                content.append(delta.get("content") or "")
                for call in delta.get("tool_calls") or []:
                    args.append((call.get("function") or {}).get("arguments") or "")
    return {"variant": name, "run": i, "provider": provider, "cost": cost, "called_tool": bool("".join(args)),
            "leaked_text": "".join(content).strip()[:90]}


jobs = [(n, v, i) for n, v in VARIANTS.items() if not ONLY or n in ONLY for i in range(1, RUNS + 1)]
with ThreadPoolExecutor(max_workers=5) as pool:
    results = list(pool.map(lambda j: run(*j), jobs))
if len(sys.argv) > 3:
    with open(sys.argv[3], "w", encoding="utf-8") as evidence:
        for result in results:
            evidence.write(json.dumps(result, ensure_ascii=False) + "\n")
for r in results:
    print(json.dumps(r, ensure_ascii=False))
print("total cost", round(sum(r["cost"] for r in results), 6))
