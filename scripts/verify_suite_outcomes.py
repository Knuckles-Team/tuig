"""Check a pytest junit report against the recorded skip explanations.

Usage: verify_suite_outcomes.py JUNIT_XML

Fails on any failure or error, and on any skip whose reason is not listed in
EXPLAINED_SKIPS. Prints the collection and outcome counts for the job log.
"""

import re
import sys
import xml.etree.ElementTree as ET

# Each entry is a skip that the full suite cannot avoid on the hosted runner.
EXPLAINED_SKIPS = (
    re.compile(r"isolated installed-provider qualification job"),
    re.compile(r"status\.json is generated"),
    re.compile(r"live disposable Arbora fixture"),
)


def main(path: str) -> int:
    cases = ET.parse(path).findall(".//testcase")
    counts = {"passed": 0, "skipped": 0, "failed": 0}
    problems: list[str] = []
    for case in cases:
        name = f"{case.get('classname')}::{case.get('name')}"
        skipped = case.find("skipped")
        if case.find("failure") is not None or case.find("error") is not None:
            counts["failed"] += 1
            problems.append(f"{name}: failed")
        elif skipped is not None:
            counts["skipped"] += 1
            reason = skipped.get("message") or ""
            if not any(known.search(reason) for known in EXPLAINED_SKIPS):
                problems.append(f"{name}: unexplained skip: {reason!r}")
        else:
            counts["passed"] += 1
    print(f"collected={len(cases)} {counts}")
    for problem in problems:
        print(problem, file=sys.stderr)
    return 1 if problems or not cases else 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1]))
