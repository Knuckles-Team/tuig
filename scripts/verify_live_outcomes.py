"""Require every live-sandbox case to pass without a skip.

Usage: verify_live_outcomes.py JUNIT_XML
"""

import sys
import xml.etree.ElementTree as ET


def main(path: str) -> int:
    cases = ET.parse(path).findall(".//testcase")
    bad = [
        f"{case.get('classname')}::{case.get('name')}"
        for case in cases
        if any(case.find(tag) is not None for tag in ("skipped", "failure", "error"))
    ]
    print(f"collected={len(cases)} not-passed={len(bad)}")
    for name in bad:
        print(f"{name}: did not pass", file=sys.stderr)
    return 1 if bad or not cases else 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1]))
