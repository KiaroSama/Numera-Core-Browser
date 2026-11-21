"""Preserve a tested development runtime with its exact source inputs."""
import json
from pathlib import Path
import re
import shutil
import subprocess

from engine_build import sha256, write_json


def read_source_export(filename, host, source_lock_sha256):
    if not re.fullmatch(r'chromium-source-export-[0-9]+-(linux|windows)\.json', filename):
        raise RuntimeError('Select a verified Chromium source export from docs/evidence')
    path = Path('docs/evidence') / filename
    proof = json.loads(path.read_text(encoding='utf-8'))
    export = proof['source_archive']
    checks = proof['content_checks']
    version = proof['chromium_version']
    generation = export['generation']
    archive = f'https://commondatastorage.googleapis.com/chromium-browser-official/chromium-{version}.tar.xz'
    if (proof['host'] != host or proof['arch'] != 'x86-64' or proof['source_lock_sha256'] != source_lock_sha256
            or proof['branding_patch_check'] != 'PASS' or not checks['deps_identical_to_pinned_git']
            or not checks['branding_matches_pinned_git'] or not checks['version_matches']
            or checks['absent_checkout_directories'] or checks['ffmpeg_changed_files']
            or checks['ffmpeg_omitted_regular_files']):
        raise RuntimeError('Verified source export does not match this native Chromium build')
    if (not re.fullmatch(r'[0-9]+(?:\.[0-9]+){3}', version) or not re.fullmatch(r'[0-9]+', generation)
            or not re.fullmatch(r'[0-9a-f]{64}', export['sha256']) or export['bytes'] < 1
            or export['url'] != f'{archive}?generation={generation}' or export['hashes_url'] != f'{archive}.hashes'):
        raise RuntimeError('Verified source export contains inconsistent upstream content metadata')
    return path, proof


def retain_development_bundle(root, evidence, report, source_proof, environment):
    proof_path, proof = source_proof
    materials = report['build_materials']
    portable = report['portable_runtime']
    archive = portable['archive']
    if (materials['chromium_revision'] != proof['chromium_revision']
            or materials['chromium_version'] != proof['chromium_version']
            or materials['source_inventory']['sha256'] != proof['native_inventory']['git_checkouts_sha256']
            or materials['source_inventory']['deps_sha256'] != proof['content_checks']['verified_upstream_files_sha256']['DEPS']
            or portable['archive_verification']['archive_sha256'] != archive['sha256']):
        raise RuntimeError('The tested Chromium runtime and its retained source inputs differ')
    inventory = json.loads((evidence / 'portable-runtime-inventory.json').read_text(encoding='utf-8'))
    if any('widevine' in name.lower() for name in inventory['files']):
        raise RuntimeError('The development bundle may only contain reviewed Chromium components')

    output = evidence.with_name('chromium-development-runtime')
    output.mkdir()
    shutil.copytree(evidence / 'linked-build-materials', output / 'source-evidence',
                    ignore=shutil.ignore_patterns('*.log'))
    shutil.copyfile(proof_path, output / 'verified-upstream-source.json')
    shutil.copyfile(evidence / 'portable-runtime-inventory.json', output / 'runtime-inventory.json')
    source = output / 'numera-source.zip'
    subprocess.run(['git', 'archive', '--format=zip', '--output', str(source), report['commit']],
                   env=environment, check=True, timeout=90)
    runtime = output / 'numera-portable-runtime.tar.gz'
    shutil.move(root / runtime.name, runtime)
    if runtime.stat().st_size != archive['bytes'] or sha256(runtime) != archive['sha256']:
        raise RuntimeError('The staged download does not match the tested Chromium runtime')

    patch = Path('upstream/chromium/patches/0001-product-name.patch')
    manifest = {'schema_revision': 1, 'package': 'numera-chromium', 'build_mode': 'development',
                'target': 'x86_64-unknown-linux-gnu' if report['host'] == 'Linux' else 'x86_64-pc-windows-msvc',
                'chromium_version': materials['chromium_version'], 'chromium_revision': materials['chromium_revision'],
                'numera_commit': report['commit'], 'patch_sha256': sha256(patch),
                'workflow_run': int(report['context']['run_id']),
                'runtime': {'file': runtime.name, **archive},
                'numera_source': {'file': source.name, 'sha256': sha256(source)},
                'upstream_source': proof['source_archive'], 'source_evidence': 'source-evidence',
                'runtime_inventory_sha256': sha256(output / 'runtime-inventory.json'),
                'publisher_signature': 'NOT_CONFIGURED', 'trusted_release': False,
                'validation': {'portable_runtime': 'PASS', 'native_headed_browser': 'PASS',
                               'browser_clients': 'PASS', 'rust_process_ownership': 'PASS',
                               'profile_and_downloads': 'PASS'},
                'linux_sandbox': portable.get('sandbox_helper'),
                'signed_application_updates': 'NOT_RUN', 'clean_host_installation': 'NOT_RUN'}
    write_json(output / 'development-runtime.json', manifest)
    return {'subdirectory': output.name, 'manifest_sha256': sha256(output / 'development-runtime.json'),
            'runtime_sha256': archive['sha256'], 'trusted_release': False}
