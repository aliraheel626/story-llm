"""Retain the plan's source-search proofs verbatim."""
import argparse
import json
import subprocess
from pathlib import Path

parser = argparse.ArgumentParser()
parser.add_argument("step")
args = parser.parse_args()
root = Path(__file__).resolve().parents[3]
commands = [
    ["rg", "-n", "story_entity_state|last_event_id", "src-tauri/src"],
    ["rg", "-n", "get_entities|GET_ENTITIES", "src-tauri/src", "src"],
    ["rg", "-n", "entity_queried|ENTITY_QUERIED", "src-tauri/src", "src"],
    ["rg", "-n", "-i", "campaign|dice_mode|attributes_enabled", "src-tauri/src", "src"],
    ["rg", "-n", '"(object|location|campaign)"', "src-tauri/src", "src"],
    ["rg", "-n", "EntityContext|entities_touched_since|touched_entity_ids|detailed_entity_ids", "src-tauri/src", "src"],
]
results = []
for command in commands:
    completed = subprocess.run(command, cwd=root, capture_output=True, text=True, encoding="utf-8", errors="replace")
    results.append({"command": command, "exit": completed.returncode, "stdout": completed.stdout, "stderr": completed.stderr})
path = Path(__file__).resolve().parent / f"{args.step}-source-scans.json"
path.write_text(json.dumps(results, indent=2) + "\n", encoding="utf-8")
print(json.dumps(results[:4], indent=2))
print(str(path))
