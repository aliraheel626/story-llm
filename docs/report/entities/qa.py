"""User-driven tauri-pilot evidence capture, with read-only database access."""
import argparse
import json
import os
import sqlite3
import subprocess
import sys
import time
from pathlib import Path

sys.stdout.reconfigure(encoding="utf-8", errors="replace")

root = Path(__file__).resolve().parent
database = Path(os.environ["APPDATA"]) / "com.story-llm.app" / "story-llm.sqlite3"
pipe = r"\\.\pipe\tauri-pilot-com.story-llm.app"


def pilot(*arguments):
    completed = subprocess.run(["tauri-pilot", "--socket", pipe, "--json", *arguments], capture_output=True, text=True, encoding="utf-8")
    if completed.returncode:
        raise RuntimeError(completed.stdout + completed.stderr)
    return json.loads(completed.stdout)


def query(sql, parameters=()):
    with sqlite3.connect(database.as_uri() + "?mode=ro", uri=True) as connection:
        connection.row_factory = sqlite3.Row
        return [dict(row) for row in connection.execute(sql, parameters)]


def save(filename, data):
    (root / filename).write_text(json.dumps(data, indent=2, ensure_ascii=False) + "\n", encoding="utf-8")
    print(str(root / filename))


def capture(story_id):
    return {
        "captured_at": time.time(),
        "database": str(database),
        "database_mode": "ro",
        "story_id": story_id,
        "tool_settings": pilot("ipc", "get_story_narrator_tools", "--args", json.dumps({"storyId": story_id})),
        "context_settings": pilot("ipc", "get_story_context_settings", "--args", json.dumps({"storyId": story_id})),
        "transcript_settings": pilot("ipc", "get_story_transcript_settings", "--args", json.dumps({"storyId": story_id})),
        "snapshot": pilot("snapshot"),
        "dom": pilot("eval", "({text:document.body.innerText,controls:Array.from(document.querySelectorAll('input,textarea,select,button')).map(e=>({tag:e.tagName,text:e.textContent,type:e.type,value:e.type==='password'?'[redacted]':e.value,checked:e.checked,disabled:e.disabled,label:e.getAttribute('aria-label'),expanded:e.getAttribute('aria-expanded'),pressed:e.getAttribute('aria-pressed')}))})"),
        "schema": query("SELECT type,name,sql FROM sqlite_master WHERE type IN ('table','index') ORDER BY type,name"),
        "registry": query("SELECT canonical_name,entity_kinds_json,min,max FROM attribute_registry ORDER BY canonical_name"),
        "story": query("SELECT id,title,settings_json FROM stories WHERE id=?", (story_id,)),
        "entities": query("SELECT e.*,c.known_as,c.appearance_anchor,c.gender,c.age,c.role,c.location,c.outfit FROM entities e LEFT JOIN characters c ON c.entity_id=e.id WHERE e.story_id=? ORDER BY e.created_at", (story_id,)),
        "relationships": query("SELECT l.*,e.is_present FROM relationships l JOIN entities e ON e.id=l.entity_id WHERE e.story_id=? ORDER BY l.entity_id", (story_id,)),
        "attributes": query("SELECT a.*,r.canonical_name FROM entity_attributes a JOIN entities e ON e.id=a.entity_id JOIN attribute_registry r ON r.id=a.attribute_id WHERE e.story_id=? ORDER BY a.entity_id,r.canonical_name", (story_id,)),
        "transcript": query("SELECT * FROM transcript_entries WHERE story_id=? ORDER BY seq", (story_id,)),
        "usage": query("SELECT * FROM usage_records ORDER BY created_at"),
        "logs": pilot("logs"),
    }


parser = argparse.ArgumentParser()
parser.add_argument("action", choices=["capture", "preview", "poll", "budget", "clicktext"])
parser.add_argument("filename")
parser.add_argument("--story")
parser.add_argument("--condition")
args = parser.parse_args()
if args.action in ("capture", "preview"):
    data = capture(args.story)
    if args.action == "preview":
        data["read_only_preview"] = pilot("ipc", "preview_story_context", "--args", json.dumps({"storyId": args.story}))
    save(args.filename, data)
elif args.action == "budget":
    data = query("SELECT kind,COUNT(*) AS calls,SUM(COALESCE(cost_usd,0)) AS cost_usd FROM usage_records GROUP BY kind ORDER BY kind")
    totals = {row["kind"]: row["calls"] for row in data}
    text_calls = sum(totals.get(kind, 0) for kind in ("narration", "summary", "title"))
    save(args.filename, {"mode": "ro", "usage_totals": data, "text_calls": text_calls, "text_calls_remaining": 15 - text_calls, "image_calls": totals.get("image", 0), "caption_calls": totals.get("caption", 0)})
    print(json.dumps(data))
    assert text_calls <= 15 and totals.get("image", 0) == 0 and totals.get("caption", 0) == 0, "QA spending cap exceeded"
elif args.action == "clicktext":
    matches = [element for element in pilot("snapshot")["elements"] if element.get("role") == "button" and element.get("name") == args.filename]
    assert len(matches) == 1, matches
    print(json.dumps(pilot("click", "@" + matches[0]["ref"])))
elif args.action == "poll":
    frames = []
    deadline = time.monotonic() + 240
    condition = args.condition or "Array.from(document.querySelectorAll('button')).some(b=>b.textContent.trim().startsWith('Continue')&&!b.disabled)"
    while time.monotonic() < deadline:
        snapshot = pilot("snapshot")
        value = pilot("eval", condition)
        frames.append({"time": time.time(), "condition": value, "snapshot": snapshot})
        if value is True or value == "true":
            save(args.filename, frames)
            break
        time.sleep(0.4)
    else:
        save(args.filename, frames)
        raise RuntimeError("DOM polling timed out; evidence saved")
