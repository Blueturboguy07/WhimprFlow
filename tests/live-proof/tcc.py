#!/usr/bin/env python3
"""TCC rows for the live update proof (tests/live-proof/proof-macos.sh). Runner only.

Writes and reads rows of the `access` table of a TCC database the way System Settings stores a
grant, with the columns that this macOS version's schema has (read with PRAGMA table_info, as
`.schema access` shows them).

  tcc.py schema <db>
  tcc.py grant  <db> <service> <bundle id> <csreq file> <auth_reason>
  tcc.py rows   <db> <bundle id>                     one JSON line per row of the app
  tcc.py csreq  <db> <service> <bundle id> <out file>  the stored csreq blob, byte for byte
"""
import json
import sqlite3
import sys
import time


def connect(db):
    return sqlite3.connect(db, timeout=10)


def columns(con):
    return [(r[1], r[3], r[4]) for r in con.execute("PRAGMA table_info(access)")]


def schema(db):
    con = connect(db)
    row = con.execute("SELECT sql FROM sqlite_master WHERE type = 'table' AND name = 'access'").fetchone()
    print(row[0] if row else "(no access table)")


def grant(db, service, client, csreq_file, reason):
    with open(csreq_file, "rb") as f:
        csreq = f.read()
    # A grant as System Settings writes it: client is a bundle id (client_type 0), allowed
    # (auth_value 2), with the code requirement the app must satisfy (csreq).
    wanted = {
        "service": service,
        "client": client,
        "client_type": 0,
        "auth_value": 2,
        "auth_reason": int(reason),
        "auth_version": 1,
        "csreq": csreq,
        "policy_id": None,
        "indirect_object_identifier_type": 0,
        "indirect_object_identifier": "UNUSED",
        "indirect_object_code_identity": None,
        "flags": 0,
        "last_modified": int(time.time()),
        "pid": None,
        "pid_version": None,
        "boot_uuid": "UNUSED",
        "last_reminded": 0,
    }
    con = connect(db)
    names, values = [], []
    for name, notnull, default in columns(con):
        if name in wanted:
            names.append(name)
            values.append(wanted[name])
        elif notnull and default is None:
            # A column this script does not know, NOT NULL without a default: 0 is TCC's usual "none".
            names.append(name)
            values.append(0)
    sql = "INSERT OR REPLACE INTO access ({}) VALUES ({})".format(", ".join(names), ", ".join("?" * len(names)))
    con.execute(sql, values)
    con.commit()
    print("granted {} to {} in {} ({} columns)".format(service, client, db, len(names)))


def rows(db, client):
    con = connect(db)
    con.row_factory = sqlite3.Row
    for r in con.execute("SELECT * FROM access WHERE client = ?", (client,)):
        d = {k: r[k] for k in r.keys()}
        for k, v in d.items():
            if isinstance(v, bytes):
                d[k] = v.hex()
        print(json.dumps(d, sort_keys=True))


def csreq(db, service, client, out):
    con = connect(db)
    row = con.execute("SELECT csreq FROM access WHERE service = ? AND client = ?", (service, client)).fetchone()
    if not row or row[0] is None:
        sys.exit("no {} row with a csreq for {}".format(service, client))
    with open(out, "wb") as f:
        f.write(row[0])


if __name__ == "__main__":
    cmd, args = sys.argv[1], sys.argv[2:]
    {"schema": schema, "grant": grant, "rows": rows, "csreq": csreq}[cmd](*args)
