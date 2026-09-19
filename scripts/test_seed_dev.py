"""Regression checks for the development seed, with no server or SDK needed.

Run with: python3 -m unittest discover -s scripts -p 'test_seed_dev.py'
"""

from __future__ import annotations

import importlib.util
import json
import sys
import tempfile
import unittest
from pathlib import Path
from unittest.mock import patch

_spec = importlib.util.spec_from_file_location("seed_dev", Path(__file__).with_name("seed-dev.py"))
assert _spec is not None and _spec.loader is not None
seed_dev = importlib.util.module_from_spec(_spec)
sys.modules[_spec.name] = seed_dev
_spec.loader.exec_module(seed_dev)


class RegistrySeedTests(unittest.TestCase):
    def test_a_failed_step_with_external_data_directory_does_not_stop_later_steps(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            data = Path(directory)
            record = data / "dev-seed.json"
            log = data / "dev-seed.log"
            with (
                patch.object(seed_dev, "DATA", data),
                patch.object(seed_dev, "RECORD", record),
                patch.object(seed_dev, "LOG", log),
                patch.object(seed_dev, "seed_prompts", side_effect=RuntimeError("store unavailable")),
                patch.object(seed_dev, "seed_training", return_value="training seeded"),
                patch.object(seed_dev, "script", return_value=None) as script,
                patch.object(seed_dev, "say") as say,
            ):
                seed_dev.registries(seed_dev.Api("http://unused.invalid"), force=True)

            completed = json.loads(record.read_text())
            self.assertNotIn("prompts", completed)
            self.assertEqual(set(completed), {"annotations", "training", "imports", "conversations"})
            self.assertEqual(script.call_count, 3)
            messages = [call.args[0] for call in say.call_args_list]
            self.assertTrue(any(str(log) in message and "store unavailable" in message for message in messages))
