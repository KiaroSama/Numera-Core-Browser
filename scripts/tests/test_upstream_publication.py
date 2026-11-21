"""Exercise candidate checkout/update against real local Git histories."""
import json
from pathlib import Path
import os
import shutil
import subprocess
import tempfile
import textwrap
import unittest

REPOSITORY = Path(__file__).resolve().parents[2]


class CandidatePublication(unittest.TestCase):
    def test_existing_branch_update_and_repeat_are_safe(self):
        workflow = (REPOSITORY / '.github/workflows/upstream-watch.yml').read_text(encoding='utf-8')
        body = workflow.split('      - name: Maintain one review-only candidate PR\n', 1)[1].split('        run: |\n', 1)[1]
        # Stub the external PR lookup inside Bash. Git Bash and native Linux
        # can execute the same fixture without a host-dependent script shim.
        body = textwrap.dedent('''\
            gh() {
              if [ "$1 $2" = "pr list" ]; then echo 1; else return 97; fi
            }
        ''') + textwrap.dedent(body)
        bash = shutil.which('bash')
        self.assertIsNotNone(bash, 'The candidate publication fixture requires Bash')
        with tempfile.TemporaryDirectory(prefix='numera-watcher-') as temporary:
            root = Path(temporary)
            origin = root / 'origin.git'
            seed = root / 'seed'

            def git(*args, cwd=None):
                return subprocess.run(['git', *args], cwd=cwd, encoding='utf-8',
                                      capture_output=True, check=True).stdout.strip()

            git('init', '--bare', '--initial-branch=main', str(origin))
            git('clone', str(origin), str(seed))
            git('config', 'user.name', 'Fixture Author', cwd=seed)
            git('config', 'user.email', 'fixture@example.invalid', cwd=seed)
            file = seed / 'upstream/candidates.json'
            file.parent.mkdir()
            file.write_text(json.dumps({'version': 1}), encoding='utf-8')
            git('add', '.', cwd=seed)
            git('commit', '-m', 'baseline', cwd=seed)
            git('push', 'origin', 'main', cwd=seed)
            git('checkout', '-b', 'automation/upstream-candidate', cwd=seed)
            file.write_text(json.dumps({'version': 2}), encoding='utf-8')
            git('commit', '-am', 'previous candidate', cwd=seed)
            git('push', 'origin', 'HEAD', cwd=seed)
            env = dict(os.environ, RUNNER_TEMP=root.as_posix())
            heads = []
            for attempt in range(2):
                checkout = root / f'run-{attempt}'
                git('clone', str(origin), str(checkout))
                (checkout / 'upstream/candidates.json').write_text(json.dumps({'version': 3}), encoding='utf-8')
                result = subprocess.run([bash, '-e', '-c', body], cwd=checkout, env=env,
                                        encoding='utf-8', capture_output=True)
                self.assertEqual(result.returncode, 0, result.stderr)
                heads.append(git('--git-dir=' + str(origin), 'rev-parse', 'refs/heads/automation/upstream-candidate'))
                value = git('--git-dir=' + str(origin), 'show', 'refs/heads/automation/upstream-candidate:upstream/candidates.json')
                self.assertEqual(json.loads(value)['version'], 3)
            self.assertEqual(heads[0], heads[1], 'Repeated observation must not create a new commit')
            baseline = git('--git-dir=' + str(origin), 'show', 'main:upstream/candidates.json')
            self.assertEqual(json.loads(baseline)['version'], 1, 'Observation must not promote main')


if __name__ == '__main__':
    unittest.main()
