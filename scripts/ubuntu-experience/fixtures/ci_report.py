"""Retrieve the last local CI report; success means retrieval succeeded.

The report contains an intentional failed test. Its test outcome is preserved;
this command does not execute tests or convert a failed test run into success.
"""

from pathlib import Path

print(Path(__file__).with_name("ci-report.txt").read_text(), end="")
