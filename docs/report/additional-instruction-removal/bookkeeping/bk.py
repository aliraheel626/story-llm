"""Bookkeeping profile: the same 10 normal turns, N fresh stories, against whichever build is running.

Usage: python bk.py <arm> <runs> <out_dir>
Database access is read-only; all writes go through the app's own commands.
"""
import json
import os
import sqlite3
import subprocess
import sys
import time
from pathlib import Path

sys.stdout.reconfigure(encoding="utf-8", errors="replace")
DATABASE = Path(os.environ["APPDATA"]) / "com.story-llm.app" / "story-llm.sqlite3"
PIPE = r"\\.\pipe\tauri-pilot-com.story-llm.app"

# (mode, content, expected) — expected names the record this turn should produce.
TURNS = [
    ("story", "I step out of the rain into the Gilded Lantern inn. Behind the bar, a broad-shouldered woman named "
              "Hedda Grane polishes a tankard; she wears a leather apron and a brass key on a chain.", "character: Hedda created"),
    ("say", "\"Hedda, who's the old man dozing by the fire?\"", "character: narrator-named old man created"),
    ("story", "A hooded figure in the far corner hasn't touched her drink. She's been watching me since I came in.",
              "character: true name + known_as"),
    ("do", "While Hedda's back is turned, I try to pick the lock of the cellar door with a bent hairpin.", "roll_check"),
    ("do", "I walk over to the bar and sit down on a stool.", "nothing"),
    ("say", "\"Hedda, I'm Aldric's son. Your brother's boy. He died last winter.\"", "relationship created"),
    ("do", "I hand Hedda the letter my father wrote her before he died, and watch her read it.", "relationship stats"),
    ("story", "A drunk dockhand lunges at me with a knife; Hedda steps between us and takes a deep cut across her "
              "forearm before she throws him out.", "character stats (injury) on Hedda"),
    ("story", "Hedda binds her arm, swaps her bloodied apron for a black oilskin cloak, and we leave the inn together "
              "for the docks.", "character: Hedda location + outfit"),
    ("story", "On the way, Hedda spots her silver candlestick from the cellar sticking out of my pack, and her face goes cold.",
              "relationship label or stats change"),
]


def pilot(*arguments):
    done = subprocess.run(["tauri-pilot", "--socket", PIPE, "--json", *arguments],
                          capture_output=True, text=True, encoding="utf-8")
    if done.returncode:
        raise RuntimeError(done.stdout + done.stderr)
    return json.loads(done.stdout) if done.stdout.strip() else None


def ipc(command, **args):
    return pilot("ipc", command, "--args", json.dumps(args))


def query(sql, parameters=()):
    with sqlite3.connect(DATABASE.as_uri() + "?mode=ro", uri=True) as connection:
        connection.row_factory = sqlite3.Row
        return [dict(row) for row in connection.execute(sql, parameters)]


def latest_turn(story_id):
    rows = query("SELECT id, status FROM turns WHERE story_id=? ORDER BY seq DESC LIMIT 1", (story_id,))
    return rows[0] if rows else None


def run_turn(story_id, mode, content):
    before = latest_turn(story_id)
    error = None
    try:
        ipc("submit_turn", storyId=story_id, mode=mode, content=content)
    except RuntimeError as failure:
        error = str(failure)[:500]
    started = time.time()
    turn = latest_turn(story_id)
    while error is None and time.time() - started < 300:
        turn = latest_turn(story_id)
        if turn and turn != before and turn["status"] != "pending":
            break
        time.sleep(2)
    fresh = turn if turn and turn != before else None
    entries = query("SELECT kind, content, payload_json FROM transcript_entries WHERE story_id=? AND turn_id=? ORDER BY seq",
                    (story_id, fresh["id"])) if fresh else []
    calls = [json.loads(entry["payload_json"]) for entry in entries if entry["kind"] == "tool_call"]
    return {
        "submit_error": error,
        "turn_status": fresh["status"] if fresh else None,
        "tools": [{"tool": call.get("tool"), "ok": call.get("ok"), "args": call.get("args"), "result": call.get("result")} for call in calls],
        "tool_names": [call.get("tool") for call in calls],
        "narration": " ".join(entry["content"] or "" for entry in entries if entry["kind"] == "narration"),
        "seconds": round(time.time() - started, 1),
    }


def one_run(arm, run, out_dir):
    story_id = ipc("new_story")["id"]
    ipc("rename_story", storyId=story_id, title=f"Bookkeeping {arm} run {run}")
    ipc("save_story_narrator_tools", storyId=story_id,
        tools={"roll_check": True, "save_character": True, "save_relationship": True, "illustrate_scene": False})
    result = {"arm": arm, "run": run, "story_id": story_id, "database_mode": "ro", "turns": []}
    for index, (mode, content, expected) in enumerate(TURNS, 1):
        turn = {"index": index, "mode": mode, "content": content, "expected": expected, **run_turn(story_id, mode, content)}
        result["turns"].append(turn)
        print(f"[{arm} r{run}] {index:2} {mode:5} {turn['turn_status']} {turn['tool_names']} (expected: {expected})", flush=True)
    result["entities"] = query("SELECT e.id, e.kind, e.name, e.is_present, c.known_as, c.role, c.location, c.outfit "
                               "FROM entities e LEFT JOIN characters c ON c.entity_id=e.id WHERE e.story_id=? ORDER BY e.created_at", (story_id,))
    result["relationships"] = query("SELECT r.entity_id, r.from_id, r.to_id, r.label, r.direction, r.description FROM relationships r "
                                    "JOIN entities e ON e.id=r.entity_id WHERE e.story_id=?", (story_id,))
    result["attributes"] = query("SELECT a.entity_id, g.canonical_name, a.value FROM entity_attributes a JOIN entities e ON e.id=a.entity_id "
                                 "JOIN attribute_registry g ON g.id=a.attribute_id WHERE e.story_id=?", (story_id,))
    result["usage"] = query("SELECT kind, model, COUNT(*) AS calls, ROUND(SUM(COALESCE(cost_usd,0)),6) AS cost_usd "
                            "FROM usage_records WHERE story_id=? GROUP BY kind, model", (story_id,))
    out = Path(out_dir) / f"bk-{arm}-run{run}.json"
    out.write_text(json.dumps(result, indent=2, ensure_ascii=False) + "\n", encoding="utf-8")
    print(out, flush=True)


if __name__ == "__main__":
    arm, runs, out_dir = sys.argv[1], int(sys.argv[2]), sys.argv[3]
    Path(out_dir).mkdir(parents=True, exist_ok=True)
    for run in range(1, runs + 1):
        one_run(arm, run, out_dir)
