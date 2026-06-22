from __future__ import annotations

import json
import sqlite3
import threading
import time
from dataclasses import asdict, dataclass, field
from pathlib import Path
from typing import Any


JSON = dict[str, Any]


@dataclass
class StoredResponse:
    response_id: str
    model_alias: str
    model_upstream: str
    messages: list[JSON]
    pending_call_ids: list[str] = field(default_factory=list)
    output: list[JSON] = field(default_factory=list)
    created_at: int = field(default_factory=lambda: int(time.time()))
    previous_response_id: str = ""


class StateStore:
    def __init__(self, path: str, ttl_seconds: int = 21600):
        self.path = path
        self.ttl_seconds = ttl_seconds
        self._lock = threading.Lock()
        Path(path).parent.mkdir(parents=True, exist_ok=True)
        with self._connect() as db:
            db.execute(
                "CREATE TABLE IF NOT EXISTS responses "
                "(response_id TEXT PRIMARY KEY, created_at INTEGER NOT NULL, payload TEXT NOT NULL)"
            )

    def _connect(self) -> sqlite3.Connection:
        return sqlite3.connect(self.path, timeout=10)

    def put(self, item: StoredResponse) -> None:
        payload = json.dumps(asdict(item), ensure_ascii=False, separators=(",", ":"))
        with self._lock, self._connect() as db:
            db.execute(
                "INSERT OR REPLACE INTO responses(response_id, created_at, payload) VALUES(?,?,?)",
                (item.response_id, item.created_at, payload),
            )

    def get(self, response_id: str) -> StoredResponse | None:
        cutoff = int(time.time()) - self.ttl_seconds
        with self._lock, self._connect() as db:
            row = db.execute(
                "SELECT payload FROM responses WHERE response_id=? AND created_at>=?",
                (response_id, cutoff),
            ).fetchone()
        return StoredResponse(**json.loads(row[0])) if row else None

    def find_by_call_ids(self, call_ids: list[str]) -> StoredResponse | None:
        wanted = {str(value) for value in call_ids if value}
        if not wanted:
            return None
        cutoff = int(time.time()) - self.ttl_seconds
        with self._lock, self._connect() as db:
            rows = db.execute(
                "SELECT payload FROM responses WHERE created_at>=? ORDER BY created_at DESC",
                (cutoff,),
            ).fetchall()
        for row in rows:
            item = StoredResponse(**json.loads(row[0]))
            if wanted.issubset(set(item.pending_call_ids)):
                return item
        return None

    def cleanup(self) -> int:
        cutoff = int(time.time()) - self.ttl_seconds
        with self._lock, self._connect() as db:
            cursor = db.execute("DELETE FROM responses WHERE created_at<?", (cutoff,))
            return cursor.rowcount

