#!/usr/bin/env python3
"""Every GitHub Action must be pinned to a reviewed commit, not a movable tag."""
from pathlib import Path
import re
import sys
sys.dont_write_bytecode = True
import unittest

WORKFLOWS = Path(__file__).resolve().parent.parent / ".github" / "workflows"
PINNED = re.compile(r"[\w.-]+/[\w./-]+@[0-9a-f]{40} # v\d+\.\d+\.\d+")


class WorkflowPins(unittest.TestCase):
    def test_actions_use_full_commit_hashes(self):
        uses = [(path.name, line.split("uses:", 1)[1].strip())
                for path in sorted(WORKFLOWS.glob("*.yml"))
                for line in path.read_text().splitlines() if "uses:" in line]
        self.assertTrue(uses)
        for workflow, action in uses:
            with self.subTest(workflow=workflow, action=action):
                self.assertRegex(action, PINNED)


if __name__ == "__main__":
    unittest.main()
