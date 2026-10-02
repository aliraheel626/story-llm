"""Retain development command output without modifying application sources."""
import subprocess
import sys
from pathlib import Path

evidence = Path(__file__).resolve().parent
root = evidence.parents[2]
output, command = sys.argv[1], sys.argv[2:]
completed = subprocess.run(command, cwd=root, stdout=subprocess.PIPE, stderr=subprocess.STDOUT)
text = completed.stdout.decode("utf-8", errors="replace")
(evidence / output).write_text(
    f"Command: {' '.join(command)}\nExit: {completed.returncode}\n\n{text}", encoding="utf-8"
)
print(text, end="", flush=True)
raise SystemExit(completed.returncode)
