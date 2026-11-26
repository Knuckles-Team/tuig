"""Check an llvm-cov JSON summary against the line and branch requirements.

Usage: verify_native_coverage.py SUMMARY_JSON [--branches-advisory]

Both measures need at least REQUIRED percent. With --branches-advisory a
shortfall in branch coverage is reported as a warning annotation and the
requirement stays open instead of failing the job.
"""

import json
import sys

REQUIRED = 80.0


def main(path: str, branches_advisory: bool) -> int:
    with open(path, encoding="utf-8") as handle:
        totals = json.load(handle)["data"][0]["totals"]
    failed = False
    for kind in ("lines", "branches"):
        entry = totals.get(kind) or {}
        percent = entry.get("percent")
        print(f"{kind}: {entry.get('covered')}/{entry.get('count')} = {percent}% (required {REQUIRED}%)")
        if percent is not None and entry.get("count") and percent >= REQUIRED:
            continue
        if kind == "branches" and branches_advisory:
            print(f"::warning::branch coverage is below {REQUIRED}%; TUIG-RUNTIME-R015.58 stays open")
        else:
            failed = True
    return 1 if failed else 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1], "--branches-advisory" in sys.argv[2:]))
