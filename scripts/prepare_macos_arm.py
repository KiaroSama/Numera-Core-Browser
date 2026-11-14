#!/usr/bin/env python3
"""Prepare only the reviewed disposable public macOS ARM builder."""
import argparse
import datetime
import hashlib
import json
import os
from pathlib import Path
import platform
import plistlib
import re
import shutil
import subprocess
import sys
import time


APPLICATIONS = Path('/Applications')
ANDROID_SDK = Path('/Users/runner/Library/Android/sdk')
UNUSED_XCODES = {'Xcode_16.app': '16.0', 'Xcode_16.1.app': '16.1',
    'Xcode_16.2.app': '16.2', 'Xcode_16.3.app': '16.3', 'Xcode_16.4.app': '16.4',
    'Xcode_26.0.1.app': '26.0.1', 'Xcode_26.1.1.app': '26.1.1', 'Xcode_26.2.app': '26.2'}
ALARM_EXEC = ('import math,os,signal,sys,time; '
    'remaining=float(sys.argv[1])-time.monotonic(); '
    'sys.exit(124) if remaining<=0 else None; '
    'signal.signal(signal.SIGALRM,signal.SIG_DFL); signal.alarm(math.ceil(remaining)); '
    'os.execv(sys.argv[2],sys.argv[2:])')


def version(value):
    if not isinstance(value, str) or not re.fullmatch(r'\d+\.\d+(?:\.\d+)?', value):
        raise RuntimeError('Unexpected Xcode bundle version')
    parts = tuple(map(int, value.split('.')))
    return parts + (0,) * (3 - len(parts))


def directory(path, device):
    if (not path.is_dir() or path.is_symlink() or path.resolve() != path
            or os.path.ismount(path) or path.stat().st_dev != device):
        raise RuntimeError('Reviewed asset directory identity changed')


def xcode(path, expected, device):
    directory(path, device)
    info = path / 'Contents/Info.plist'
    if not info.is_file() or info.is_symlink() or info.resolve() != info:
        raise RuntimeError('Xcode bundle metadata identity changed')
    raw = info.read_bytes()
    metadata = plistlib.loads(raw)
    if (metadata.get('CFBundleIdentifier') != 'com.apple.dt.Xcode'
            or version(metadata.get('CFBundleShortVersionString')) != version(expected)):
        raise RuntimeError('Reviewed Xcode bundle identity/version changed')
    return hashlib.sha256(raw).hexdigest()


def prepare(output, environment):
    output.mkdir(parents=True, exist_ok=True)
    report = {'schema_revision': 1, 'run_id': environment.get('GITHUB_RUN_ID'),
        'source_commit': environment.get('GITHUB_SHA'), 'result': 'BLOCKED',
        'stage': 'context-guard', 'free_before_bytes': None, 'free_after_bytes': None,
        'removed_paths': [], 'source_build': 'NOT_RUN', 'renderer_fixture': 'NOT_RUN',
        'release_ready': False}
    attempted = []
    try:
        event = json.loads(Path(environment['GITHUB_EVENT_PATH']).read_text(encoding='utf-8'))
        if (environment.get('GITHUB_ACTIONS') != 'true'
                or environment.get('RUNNER_ENVIRONMENT') != 'github-hosted'
                or environment.get('GITHUB_REPOSITORY') != 'KiaroSama/Numera-Core-Browser'
                or environment.get('GITHUB_REF') != 'refs/heads/main'
                or environment.get('GITHUB_EVENT_NAME') not in {'push', 'workflow_dispatch'}
                or environment.get('RUNNER_ARCH') != 'ARM64' or platform.system() != 'Darwin'
                or event['repository']['private'] is not False):
            raise RuntimeError('Preparation requires the authorized public hosted macOS ARM main job')
        report['free_before_bytes'] = shutil.disk_usage('.').free
        if report['free_before_bytes'] >= 100_000_000_000:
            report['result'] = 'PASS'
        else:
            report['stage'] = 'validate-installations'
            if environment.get('ImageVersion') != '20261104.0514.1':
                raise RuntimeError('Runner image differs from the measured SDK inventory')
            device = Path.cwd().stat().st_dev
            protected = APPLICATIONS / 'Xcode_26.3.app'
            report['protected_xcode_info_sha256'] = xcode(protected, '26.3', device)
            candidates = []
            for name, expected in UNUSED_XCODES.items():
                path = APPLICATIONS / name
                report.setdefault('unused_xcode_info_sha256', {})[str(path)] = xcode(path, expected, device)
                candidates.append(path)
            if environment.get('ANDROID_HOME') != str(ANDROID_SDK):
                raise RuntimeError('Reviewed Android SDK environment identity changed')
            directory(ANDROID_SDK, device)
            for marker in ['cmdline-tools', 'licenses']:
                directory(ANDROID_SDK / marker, device)
            candidates.append(ANDROID_SDK)
            interpreter = Path(sys.executable).resolve(strict=True)
            if not interpreter.is_file() or any(interpreter.is_relative_to(path) for path in candidates):
                raise RuntimeError('Cleanup deadline interpreter would be removed')
            report['stage'] = 'verify-protected-toolchain'
            child_environment = dict(environment)
            child_environment['DEVELOPER_DIR'] = str(protected / 'Contents/Developer')
            for key in ['GH_TOKEN', 'GITHUB_TOKEN', 'ACTIONS_RUNTIME_TOKEN', 'ACTIONS_ID_TOKEN_REQUEST_TOKEN']:
                child_environment.pop(key, None)
            selected = subprocess.run(['xcodebuild', '-version'], capture_output=True,
                text=True, check=True, timeout=30, env=child_environment).stdout.strip()
            sdk = subprocess.run(['xcrun', '--sdk', 'macosx', '--show-sdk-version'], capture_output=True,
                text=True, check=True, timeout=30, env=child_environment).stdout.strip()
            if not re.search(r'(?m)^Xcode 26\.3$', selected) or sdk != '26.2':
                raise RuntimeError('Protected Xcode/SDK probe differs from the reviewed development candidate')
            report['selected_toolchain'] = {'developer_dir': child_environment['DEVELOPER_DIR'],
                'xcode_version': selected, 'sdk_version': sdk, 'official_sdk_equivalence': False}
            report['stage'] = 'remove-verified-unused-assets'
            report['attempted_paths'] = list(map(str, candidates))
            report['deletion_deadline_seconds'] = 330
            report['deletion_deadline_monotonic_seconds'] = time.monotonic() + 330
            report['deadline_interpreter'] = str(interpreter)
            (output / 'mac-arm-preparation.json').write_text(json.dumps(report, indent=2) + '\n', encoding='utf-8')
            attempted = candidates
            # The kernel alarm survives exec and directly bounds the single
            # rm process, even if sudo supervises it through another process.
            # Isolated Python skips site hooks; BSD rm -x stays on each
            # operand's filesystem. No user-supplied operand is accepted.
            subprocess.run(['sudo', '-n', '--', str(interpreter), '-I', '-S', '-c', ALARM_EXEC,
                str(report['deletion_deadline_monotonic_seconds']), '/bin/rm', '-rf', '-x', '--', *map(str, candidates)],
                check=True, timeout=360, env=child_environment)
            if any(path.exists() or path.is_symlink() for path in candidates):
                raise RuntimeError('Reviewed unused assets were not completely removed')
            report['stage'] = 'capacity-check'
            if shutil.disk_usage('.').free < 100_000_000_000:
                raise RuntimeError('100 GB source preflight remains unmet after bounded preparation')
            report['result'] = 'PASS'
    except Exception as error:
        report['error_type'] = type(error).__name__
        report['reason'] = str(error) if isinstance(error, RuntimeError) else 'Preparation failed; inspect the retained stage and exception type'
    finally:
        report['removed_paths'] = [str(path) for path in attempted if not path.exists() and not path.is_symlink()]
        report['free_after_bytes'] = shutil.disk_usage('.').free
        report['timestamp'] = datetime.datetime.now(datetime.timezone.utc).isoformat()
        report['note'] = 'Disposable hosted builder preparation only; actual before/after capacity is measured, not inferred by adding APFS directory sizes. No global xcode-select change.'
        (output / 'mac-arm-preparation.json').write_text(json.dumps(report, indent=2) + '\n', encoding='utf-8')
    return report


if __name__ == '__main__':
    parser = argparse.ArgumentParser()
    parser.add_argument('--output', type=Path, default=Path('build/engine-evidence'))
    args = parser.parse_args()
    result = prepare(args.output, os.environ)
    print(json.dumps(result, indent=2))
    raise SystemExit(0 if result['result'] == 'PASS' else 1)
