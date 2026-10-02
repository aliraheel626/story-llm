import os
import shutil
import sqlite3
from pathlib import Path

root = Path(__file__).resolve().parent
backup = root / 'db-backup'
backup.mkdir(exist_ok=True)
db = Path(os.environ['APPDATA']) / 'com.story-llm.app' / 'story-llm.sqlite3'
assert db.is_file()
for suffix in ('', '-wal', '-shm'):
    source = db.with_name(db.name + suffix)
    if source.is_file():
        destination = backup / source.name
        assert not destination.exists(), 'Never overwrite the original backup'
        shutil.copy2(source, destination)
with sqlite3.connect(db) as conn:
    before = conn.execute('SELECT COUNT(*) FROM usage_records').fetchone()[0]
    sql = conn.execute("SELECT sql FROM sqlite_master WHERE name='usage_records'").fetchone()[0]
    assert "'caption'" not in sql
    widened = sql.replace("'title', 'image'", "'title', 'image', 'caption'")
    assert widened != sql
    conn.executescript('PRAGMA foreign_keys=OFF; BEGIN; ALTER TABLE usage_records RENAME TO usage_records_old;'
                       + widened + '; INSERT INTO usage_records SELECT * FROM usage_records_old;'
                       + 'DROP TABLE usage_records_old; CREATE INDEX idx_usage_story ON usage_records(story_id);'
                       + 'CREATE INDEX idx_usage_turn ON usage_records(turn_id); COMMIT; PRAGMA foreign_keys=ON;')
with sqlite3.connect(db.as_uri() + '?mode=ro', uri=True) as conn:
    after = conn.execute('SELECT COUNT(*) FROM usage_records').fetchone()[0]
    sql = conn.execute("SELECT sql FROM sqlite_master WHERE name='usage_records'").fetchone()[0]
    integrity = conn.execute('PRAGMA integrity_check').fetchall()
    evidence = f'before={before}\nafter={after}\nequal={before == after}\n{sql}\nintegrity_check={integrity}\n'
    (root / 'c0-schema.txt').write_text(evidence, encoding='utf-8')
    print(evidence)
    assert before == after and "'caption'" in sql and integrity == [('ok',)]
