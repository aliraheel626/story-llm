"""Save the mandatory source scan and reject non-test upgrade code."""
import argparse
import subprocess
from pathlib import Path
from audit import test_lines

parser = argparse.ArgumentParser()
parser.add_argument("step")
args = parser.parse_args()
root = Path(__file__).resolve().parents[3]
command = ["rg", "-n", "ALTER TABLE|pragma_table_info|table_info|migration", "src-tauri/src"]
completed = subprocess.run(command, cwd=root, capture_output=True, text=True, encoding="utf-8", errors="replace")
failures = []
for match in completed.stdout.splitlines():
    path, number, _ = match.split(":", 2)
    text = (root / path).read_text(encoding="utf-8")
    if int(number) not in test_lines(path, text):
        failures.append(match)
text = "Command: " + " ".join(command) + "\n" + completed.stdout + "\nNon-test hits: " + str(len(failures)) + "\n"
target = Path(__file__).resolve().parent / f"{args.step}-precommit.txt"
target.write_text(text, encoding="utf-8")
print(text)
raise SystemExit(1 if failures or completed.returncode not in (0, 1) else 0)
