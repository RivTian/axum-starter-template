#!/usr/bin/env python3
"""Called only by template.py for its owned scratch migration-rebuild fixture."""
from __future__ import annotations

import argparse
from pathlib import Path
import sqlite3
import sys


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--project", required=True, type=Path)
    parser.add_argument("--binary", required=True, type=Path)
    parser.add_argument("--directory", required=True, type=Path)
    parser.add_argument("--expect", required=True, choices=["applied", "rejected", "empty"])
    args = parser.parse_args()
    # The generated helpers are byte-for-byte copies of the checked template.
    sys.path.insert(0, str(args.project / "app/tests"))
    from check_service import Service, has_field, require
    with Service(args.binary, args.directory) as service:
        if args.expect == "rejected":
            text = service.wait_exit(1)
            require("service_started" not in text and "storage migrate failed" in text
                    and has_field(text, "storage", "closed"), "migration error lost fail-fast/cleanup evidence")
        else:
            service.started()
            service.stop()
    if args.expect != "rejected":
        with sqlite3.connect(args.directory / "data/service.sqlite3") as database:
            exists = database.execute("SELECT COUNT(*) FROM sqlite_master WHERE name='embedded_migration_probe'").fetchone()[0]
            require(exists == int(args.expect == "applied"), "binary embedded the wrong migration set")
            versions = database.execute("SELECT COUNT(*) FROM _sqlx_migrations").fetchone()[0]
            require(versions == exists, "unexpected migration version set")
            if exists:
                require(database.execute("SELECT COUNT(*) FROM embedded_migration_probe").fetchone()[0] == 1,
                        "a successful version was applied more than once")
    print(f"embedded migration process: {args.expect}")


if __name__ == "__main__":
    main()
