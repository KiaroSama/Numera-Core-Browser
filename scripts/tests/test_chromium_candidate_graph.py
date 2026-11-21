"""Verify reviewed stable metadata can select an isolated native graph."""
import json
from pathlib import Path
import sys
import unittest

ROOT = Path(__file__).resolve().parents[2]
sys.path.insert(0, str(ROOT / 'scripts'))
from chromium_license_inventory import reviewed_stable_lock


class CandidateGraph(unittest.TestCase):
    def test_reads_official_versions_without_modifying_the_source_lock(self):
        lock = json.loads((ROOT / 'upstream/chromium/lock.json').read_text(encoding='utf-8'))
        evidence = json.loads((ROOT / 'docs/evidence/upstream-review-2026-11-21.json').read_text(encoding='utf-8'))
        original = json.dumps(lock)
        candidate = reviewed_stable_lock(lock, evidence)
        self.assertEqual(json.dumps(lock), original)
        self.assertFalse(candidate['approved'])
        self.assertEqual(candidate['patches'], lock['patches'])
        self.assertEqual(candidate['platform_pins']['linux']['version'], '158.0.8127.36')
        self.assertEqual(candidate['platform_pins']['windows']['revision'], 'b8c2be851f62f6a394ec57955d7f31335d7cb6d4')

        evidence['chromium_active_target_tag_checks'][0]['original_patch_applies'] = False
        with self.assertRaisesRegex(RuntimeError, 'version, tag and branding evidence'):
            reviewed_stable_lock(lock, evidence)
