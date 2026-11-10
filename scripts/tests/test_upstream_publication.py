"""Exercise candidate checkout/update against real local Git histories."""
import json
from pathlib import Path
import os
import subprocess
import tempfile
import textwrap
import unittest

REPOSITORY = Path(__file__).resolve().parents[2]


class CandidatePublication(unittest.TestCase):
    def test_existing_branch_update_and_repeat_are_safe(self):
        workflow = (REPOSITORY / '.github/workflows/upstream-watch.yml').read_text()
        body = workflow.split('      - name: Maintain one review-only candidate PR\n', 1)[1].split('        run: |\n', 1)[1]
        body = textwrap.dedent(body)
        with tempfile.TemporaryDirectory(prefix='numera-watcher-') as temporary:
            root = Path(temporary)
            origin = root / 'origin.git'
            seed = root / 'seed'

            def git(*args, cwd=None):
                return subprocess.run(['git', *args], cwd=cwd, text=True, capture_output=True, check=True).stdout.strip()

            git('init', '--bare', '--initial-branch=main', str(origin))
            git('clone', str(origin), str(seed))
            git('config', 'user.name', 'Fixture Author', cwd=seed)
            git('config', 'user.email', 'fixture@example.invalid', cwd=seed)
            file = seed / 'upstream/candidates.json'
            file.parent.mkdir()
            file.write_text(json.dumps({'version': 1}))
            git('add', '.', cwd=seed)
            git('commit', '-m', 'baseline', cwd=seed)
            git('push', 'origin', 'main', cwd=seed)
            git('checkout', '-b', 'automation/upstream-candidate', cwd=seed)
            file.write_text(json.dumps({'version': 2}))
            git('commit', '-am', 'previous candidate', cwd=seed)
            git('push', 'origin', 'HEAD', cwd=seed)
            # Stub only the external PR lookup. Checkout, restore, ancestry,
            # commits, pushes and repeat behavior all use actual Git.
            tools = root / 'tools'
            tools.mkdir()
            gh = tools / 'gh'
            gh.write_text('#!/bin/sh\nif [ "$1 $2" = "pr list" ]; then echo 1; else exit 97; fi\n')
            gh.chmod(0o755)
            env = os.environ.copy()
            env.update(PATH=str(tools) + os.pathsep + env['PATH'], RUNNER_TEMP=str(root))
            heads = []
            for attempt in range(2):
                checkout = root / f'run-{attempt}'
                git('clone', str(origin), str(checkout))
                (checkout / 'upstream/candidates.json').write_text(json.dumps({'version': 3}))
                subprocess.run(['bash', '-e', '-c', body], cwd=checkout, env=env, check=True, capture_output=True)
                heads.append(git('--git-dir=' + str(origin), 'rev-parse', 'refs/heads/automation/upstream-candidate'))
                value = git('--git-dir=' + str(origin), 'show', 'refs/heads/automation/upstream-candidate:upstream/candidates.json')
                self.assertEqual(json.loads(value)['version'], 3)
            self.assertEqual(heads[0], heads[1], 'Repeated observation must not create a new commit')
            baseline = git('--git-dir=' + str(origin), 'show', 'main:upstream/candidates.json')
            self.assertEqual(json.loads(baseline)['version'], 1, 'Observation must not promote main')


if __name__ == '__main__':
    unittest.main()
