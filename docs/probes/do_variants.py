"""Measure tool-text leakage on normal Do turns; keep the API key in memory."""
import json
import os
import re
import urllib.request
from pathlib import Path

APP = Path(os.environ["APPDATA"]) / "com.story-llm.app"
LOG = Path(os.environ["LOCALAPPDATA"]) / "com.story-llm.app" / "logs" / "story-llm.log"
OUT = Path(os.environ["TEMP"]) / "codex" / "turn-costs-finish" / "f1-do-variants.jsonl"


def unrust(text):
    return re.sub(r'\\(["\\n])', lambda match: "\n" if match.group(1) == "n" else match.group(1), text)


with LOG.open(encoding="utf-8") as log:
    line = next(line for line in log if "<see>the pebble</see>" in line)
system = unrust(re.search(r'system="((?:[^"\\]|\\.)*)"', line).group(1))
last = unrust(re.search(r'last_turn="((?:[^"\\]|\\.)*)"\s*$', line).group(1))
history_text = re.search(r"history=\[(.*)\] last_turn=", line).group(1)
history = [{"role": "user" if role == "player" else "assistant", "content": unrust(content)}
           for role, content in re.findall(r'\("(player|narrator)", "((?:[^"\\]|\\.)*)"\)', history_text)]
assert last.endswith("<see>the pebble</see>") and "<additional_instructions>" in last
last = last[:-len("<see>the pebble</see>")] + "<do>Pick up the pebble and turn it toward the window light.</do>"

source = (Path("src-tauri") / "src" / "prompts.rs").read_text(encoding="utf-8")
description = re.sub(r"\\\n\s*", "", re.search(r'ILLUSTRATE_SCENE_DESCRIPTION: &str =\s*"(.*?)";', source, re.S).group(1))
see_warning = "Make it a real tool call through the tool-calling interface; never write the call out as text, and write no other prose."
desc_warning = "Invoke it as a real tool call, at most once per turn; never write the call out as text."
assert see_warning in system and desc_warning in description

params = {"type": "object", "properties": {
    "description": {"type": "string", "description": "A vivid, concrete visual description of the scene's subject, setting, composition, and lighting."},
    "character_ids": {"type": "array", "items": {"type": "string"}, "description": "Ids of characters visible in the scene, from get_entities."}},
    "required": ["description"]}
variants = {
    "current": (system, description),
    "reworded": (system.replace(" " + see_warning, ""), description.replace(desc_warning, "Call it at most once per turn.")),
}
leak_pattern = re.compile(r'\{"description"|"character_ids"|<tool_call>|illustrate_scene\(')


def run(name, prompt, tool_description, number):
    body = {"model": "x-ai/grok-4.7", "stream": True, "tool_choice": "auto", "usage": {"include": True},
            "messages": [{"role": "system", "content": prompt}, *history, {"role": "user", "content": last}],
            "tools": [{"type": "function", "function": {"name": "illustrate_scene", "description": tool_description, "parameters": params}}]}
    with (APP / "secrets.json").open(encoding="utf-8") as secret:
        key = json.load(secret)["text_model.openrouter.api_key"]
    request = urllib.request.Request("https://openrouter.ai/api/v1/chat/completions", json.dumps(body).encode(),
                                     {"Authorization": "Bearer " + key, "Content-Type": "application/json"})
    del key
    content, arguments, cost = [], [], 0.0
    with urllib.request.urlopen(request, timeout=180) as response:
        for raw in response:
            text = raw.decode("utf-8").strip()
            if not text.startswith("data: ") or text == "data: [DONE]":
                continue
            chunk = json.loads(text[6:])
            cost = (chunk.get("usage") or {}).get("cost", cost)
            for choice in chunk.get("choices", []):
                delta = choice.get("delta", {})
                content.append(delta.get("content") or "")
                arguments.extend((call.get("function") or {}).get("arguments") or "" for call in delta.get("tool_calls") or [])
    visible = "".join(content).strip()
    prose = visible[:leak_pattern.search(visible).start()] if leak_pattern.search(visible) else visible
    return {"variant": name, "run": number, "called_tool": bool("".join(arguments)),
            "leaked_tool_text": bool(leak_pattern.search(visible)), "has_prose": len(prose.strip()) >= 80,
            "cost": cost}


with OUT.open("w", encoding="utf-8") as output:
    for name, (prompt, tool_description) in variants.items():
        for number in range(1, 6):
            result = run(name, prompt, tool_description, number)
            output.write(json.dumps(result) + "\n")
            output.flush()
            print(json.dumps(result), flush=True)
