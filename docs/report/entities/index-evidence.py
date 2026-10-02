"""Print full-path Markdown links for the report's saved evidence artifacts."""
from pathlib import Path
import sys

sys.stdout.reconfigure(encoding="utf-8")
root = Path(__file__).resolve().parent
for path in sorted(root.iterdir()):
    if path.is_file() and path.suffix in (".txt", ".json", ".patch", ".log", ".png"):
        print(f"- [{path}]({path.as_posix()})")
