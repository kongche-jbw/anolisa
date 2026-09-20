#!/usr/bin/env python3
"""A portable, content-free observer for the experimental AW callback protocol."""

import json
import sys

event = json.load(sys.stdin)
if event.get("format") != 1 or event.get("event") != "tool.result_observed":
    raise SystemExit("unsupported AW observation")
print(json.dumps({"format": 1, "observed": True}))
