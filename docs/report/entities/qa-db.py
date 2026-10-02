"""Read-only SQLite evidence capture; never accesses the secrets store."""
import argparse
import json
import os
import sqlite3
import sys
from pathlib import Path

sys.stdout.reconfigure(encoding="utf-8", errors="replace")

parser = argparse.ArgumentParser()
parser.add_argument("--query", required=True)
parser.add_argument("--output")
args = parser.parse_args()
database = Path(os.environ["APPDATA"]) / "com.story-llm.app" / "story-llm.sqlite3"
with sqlite3.connect(database.as_uri() + "?mode=ro", uri=True) as connection:
    connection.row_factory = sqlite3.Row
    rows = [dict(row) for row in connection.execute(args.query)]
result = {"database": str(database), "mode": "ro", "query": args.query, "rows": rows}
text = json.dumps(result, indent=2, ensure_ascii=False)
if args.output:
    output = Path(args.output)
    assert output.parent.is_dir()
    output.write_text(text + "\n", encoding="utf-8")
print(text)
