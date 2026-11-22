"""Verify a retained Linux development bundle and run its extracted browser."""
import io
import json
import os
from pathlib import Path
import re
import subprocess
import tarfile
from tempfile import TemporaryDirectory
import zipfile

from chromium_headed_fixture import smoke_headed_browser
from engine_build import install_linux_sandbox, now, sha256, smoke, write_json

REPOSITORY = 'KiaroSama/Numera-Core-Browser'
SIGNER = REPOSITORY + '/.github/workflows/chromium-development-provenance.yml'
RUNTIME = 'numera-portable-runtime.tar.gz'
SOURCE = 'numera-source.zip'
MANIFEST = 'development-runtime.json'


def verify_distribution(package, provenance, revision, run):
    bundle, = (path for path in provenance.iterdir()
               if path.suffix in {'.json', '.jsonl'} and path.is_file() and not path.is_symlink())
    manifest = json.loads((package / MANIFEST).read_text(encoding='utf-8'))
    if (manifest['target'] != 'x86_64-unknown-linux-gnu' or manifest['numera_commit'] != revision
            or manifest['workflow_run'] != run or manifest['trusted_release'] is not False
            or manifest['build_mode'] != 'development'):
        raise RuntimeError('The downloaded manifest does not describe the selected development run')

    for subject in (MANIFEST, RUNTIME, SOURCE):
        subprocess.run(['gh', 'attestation', 'verify', str(package / subject), '--repo', REPOSITORY,
                        '--bundle', str(bundle), '--source-ref', 'refs/heads/main', '--source-digest', revision,
                        '--signer-workflow', SIGNER, '--deny-self-hosted-runners'], check=True, timeout=180)

    expected = manifest['attachments_sha256'].copy()
    if (expected['runtime-inventory.json'] != manifest['runtime_inventory_sha256']
            or manifest['runtime']['file'] != RUNTIME or manifest['numera_source']['file'] != SOURCE):
        raise RuntimeError('The authenticated manifest contains inconsistent attachments')
    expected.update({RUNTIME: manifest['runtime']['sha256'], SOURCE: manifest['numera_source']['sha256']})
    paths = list(package.rglob('*'))
    files = {path.relative_to(package).as_posix() for path in paths if path.is_file()}
    if files != set(expected) | {MANIFEST} or any(path.is_symlink() for path in paths):
        raise RuntimeError('The downloaded file set does not match its authenticated manifest')
    for name, digest in expected.items():
        if sha256(package / name) != digest:
            raise RuntimeError(f'Downloaded artifact digest mismatch for {name}')

    reference = subprocess.check_output(['git', 'archive', '--format=zip', revision], timeout=60)
    with zipfile.ZipFile(package / SOURCE) as source, zipfile.ZipFile(io.BytesIO(reference)) as expected_source:
        if (len(set(source.namelist())) != len(source.namelist())
                or set(source.namelist()) != set(expected_source.namelist())
                or any(source.read(name) != expected_source.read(name) for name in expected_source.namelist())):
            raise RuntimeError('The downloaded first-party source differs from the exact build commit')
    return manifest


def verify_and_smoke(package, manifest, scratch, output, report):
    archive = package / RUNTIME
    digest = manifest['runtime']['sha256']
    binary = Path('target/release/numera-core').resolve(strict=True)
    info = json.loads(subprocess.check_output([str(binary), '--json', 'engines', 'verify-archive', str(archive), digest],
                                              env={'PATH': ''}, timeout=300))
    inventory = json.loads((package / 'runtime-inventory.json').read_text(encoding='utf-8'))
    if info != {'archive_verified': True, 'archive_sha256': digest, 'archive_bytes': manifest['runtime']['bytes'],
                'files': manifest['runtime']['files'], 'expanded_file_bytes': inventory['bytes'],
                'approved_package': False}:
        raise RuntimeError('The native archive verifier did not confirm the tested download')
    report['archive_verification'] = info

    extracted = scratch / 'Numera clean runtime مسیر'
    extracted.mkdir()
    with tarfile.open(archive, 'r:gz') as tar:
        tar.extractall(extracted, filter='data')
    runtime = extracted / 'runtime'
    files = {path.relative_to(runtime).as_posix() for path in runtime.rglob('*') if path.is_file()}
    if files != set(inventory['files']) or any(sha256(runtime / name) != data['sha256']
                                              for name, data in inventory['files'].items()):
        raise RuntimeError('The extracted runtime differs from the signed file inventory')
    report['extracted_file_hashes'] = 'PASS'

    environment = {key: value for key, value in os.environ.items() if key in {'PATH', 'HOME', 'LANG', 'LC_ALL', 'TMPDIR'}}
    no_admin = output / 'no-admin'
    no_admin.mkdir()
    profile = scratch / 'no-admin-profile'
    profile.mkdir()
    try:
        smoke(runtime / 'chrome', profile, no_admin, environment)
        report['no_admin_renderer'] = 'PASS'
    except RuntimeError:
        diagnostics_path = no_admin / 'smoke-renderer.stderr'
        if not diagnostics_path.is_file():
            raise
        diagnostics = diagnostics_path.read_bytes()
        if not any(message in diagnostics for message in (
                b'No usable sandbox', b'SUID sandbox helper binary was found, but is not configured correctly')):
            raise
        report['no_admin_renderer'] = 'BLOCKED_BY_HOST_SANDBOX'
        helper = scratch / 'verified-sandbox-helper'
        helper.mkdir()
        report['sandbox_helper'] = install_linux_sandbox(runtime, helper, environment)
    native = output / 'extracted-browser'
    native.mkdir()
    other_profile = scratch / 'پروفایل مستقل'
    other_profile.mkdir()
    smoke(runtime / 'chrome', other_profile, native, environment)
    report['extracted_renderer'] = 'PASS'
    report['headful_browser'] = smoke_headed_browser(runtime / 'chrome', runtime / 'chromedriver',
                                                    other_profile, native, environment)


def main():
    revision = os.environ['NUMERA_BUNDLE_SOURCE_SHA']
    run = int(os.environ['NUMERA_BUNDLE_RUN'])
    if (os.environ.get('GITHUB_ACTIONS') != 'true' or os.environ.get('RUNNER_ENVIRONMENT') != 'github-hosted'
            or os.environ.get('GITHUB_REPOSITORY') != REPOSITORY or os.environ.get('RUNNER_OS') != 'Linux'
            or os.environ.get('RUNNER_ARCH') != 'X64' or os.environ.get('GITHUB_REF') != 'refs/heads/main'
            or not re.fullmatch(r'[0-9a-f]{40}', revision) or run < 1):
        raise RuntimeError('The retained browser test requires the authorized public Linux runner')
    package = Path('build/downloaded-chromium/package').resolve(strict=True)
    provenance = Path('build/downloaded-chromium/provenance').resolve(strict=True)
    output = Path('build/chromium-download-smoke').resolve()
    output.mkdir(exist_ok=False, parents=True)
    report = {'schema_revision': 1, 'result': 'BLOCKED', 'artifact_source_commit': revision,
              'artifact_run_id': run, 'verification_commit': os.environ['GITHUB_SHA'],
              'runner_image': os.environ['ImageVersion'], 'compiled_chromium': False,
              'installation_scope': 'Fresh standard Ubuntu GitHub-hosted runner', 'trusted_release': False}
    try:
        manifest = verify_distribution(package, provenance, revision, run)
        report.update(manifest_sha256=sha256(package / MANIFEST), source_provenance='PASS')
        with TemporaryDirectory(prefix='numera-retained-browser-', dir=os.environ['RUNNER_TEMP']) as directory:
            verify_and_smoke(package, manifest, Path(directory), output, report)
        report['result'] = 'PASS'
    finally:
        report['timestamp'] = now()
        write_json(output / 'report.json', report)


if __name__ == '__main__':
    main()
