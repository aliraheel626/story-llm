"""Run the requested checks and retain complete generated evidence logs."""
import argparse
import json
import subprocess
import sys
from pathlib import Path

sys.stdout.reconfigure(encoding="utf-8", errors="replace")

parser = argparse.ArgumentParser()
parser.add_argument("step")
parser.add_argument("--start", type=int, default=1)
parser.add_argument("--end", type=int, default=4)
args = parser.parse_args()
root = Path(__file__).resolve().parents[3]
evidence = Path(__file__).resolve().parent
commands = [
    ("cargo-test", ["cargo", "test", "--manifest-path", "src-tauri/Cargo.toml"]),
    ("cargo-clippy", ["cargo", "clippy", "--manifest-path", "src-tauri/Cargo.toml", "--all-targets"]),
    ("node-test", ["node", "--test", "src/features/story/store.test.mjs", "src/features/transcript/replacement.test.mjs", "src/features/usage/store.test.mjs"]),
    ("tsc", ["cmd", "/c", "npx", "tsc", "--noEmit"]),
]
results = []
for index, (name, command) in enumerate(commands, 1):
    if index < args.start or index > args.end:
        continue
    completed = subprocess.run(command, cwd=root, stdout=subprocess.PIPE, stderr=subprocess.STDOUT)
    text = completed.stdout.decode("utf-8", errors="replace")
    path = evidence / f"{args.step}-{name}.txt"
    path.write_text(f"Command: {' '.join(command)}\nExit: {completed.returncode}\n\n{text}", encoding="utf-8")
    passed = completed.returncode == 0 and not (name == "cargo-clippy" and "warning:" in text)
    result = {"check": name, "exit": completed.returncode, "status": "PASS" if passed else "FAIL", "evidence": str(path)}
    results.append(result)
    print(json.dumps(result), flush=True)
    if not passed:
        print(text, flush=True)
        break
(evidence / f"{args.step}-checks.json").write_text(json.dumps(results, indent=2) + "\n", encoding="utf-8")
raise SystemExit(0 if len(results) == args.end - args.start + 1 and all(result["status"] == "PASS" for result in results) else 1)
