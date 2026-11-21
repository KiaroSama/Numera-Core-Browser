#!/usr/bin/env python3
"""Stage a source-built runtime and verify it from a relocated path."""
import json
import os
from pathlib import PurePosixPath
import shutil
import stat
import subprocess
import tarfile

from chromedriver_fixture import smoke_driver
from engine_build import install_linux_sandbox, sha256, smoke, stop_tree, write_json


def runtime_dependency_path(line):
    relative = PurePosixPath(line)
    if (relative.is_absolute() or not relative.parts or '..' in relative.parts
            or '\\' in line or ':' in line or any(ord(char) < 32 for char in line)):
        raise RuntimeError('GN runtime dependency is outside the owned build directory')
    return relative


def resolve_runtime_files(build, dependencies):
    files = {}
    for line in dependencies:
        relative = runtime_dependency_path(line)
        path = build.joinpath(*relative.parts)
        if path.resolve(strict=True) != path:
            raise RuntimeError('GN runtime dependency follows an unexpected link')
        candidates = path.rglob('*') if path.is_dir() else (path,)
        for candidate in candidates:
            if candidate.is_symlink() or candidate.resolve(strict=True) != candidate:
                raise RuntimeError('Runtime directory contains an unexpected link')
            if candidate.is_dir():
                continue
            if not candidate.is_file():
                raise RuntimeError('Runtime dependency is not a regular file')
            name = candidate.relative_to(build).as_posix()
            previous = files.setdefault(name.casefold(), name)
            if previous != name:
                raise RuntimeError('Runtime dependency names conflict on a case-insensitive filesystem')
    return sorted(files.values())


def describe_portable_runtime(source, host, environment):
    gn = source / ('buildtools/win/gn.exe' if host == 'Windows' else 'buildtools/linux64/gn')
    targets = ['//chrome:chrome_initial', '//chrome/test/chromedriver:chromedriver_server']
    binary = 'chrome.exe' if host == 'Windows' else 'chrome'
    driver = 'chromedriver.exe' if host == 'Windows' else 'chromedriver'
    required = {binary, driver}
    if host == 'Linux':
        # The strip action has no runtime dependency list. The linked target
        # supplies the dependencies, while the portable runtime uses the stripped executable.
        targets[1] += '.unstripped'
        required.add('chrome_sandbox')
        intermediates = {'chromedriver.unstripped'}
    else:
        # Windows reorders imports into the final top-level chrome.exe.
        intermediates = {'initialexe/chrome.exe', 'initialexe/chrome.exe.pdb'}
    dependencies = set(required)
    for target in targets:
        inventory = subprocess.check_output([str(gn), 'desc', 'out/Numera', target, 'runtime_deps'],
                                            cwd=source, env=environment, timeout=90)
        if not 0 < len(inventory) <= 1024 * 1024:
            raise RuntimeError('Unexpected GN runtime dependency list size')
        dependencies.update(line for line in inventory.decode('utf-8').splitlines() if line)
    dependencies = {runtime_dependency_path(name).as_posix() for name in dependencies} - intermediates
    return {'binary': binary, 'driver': driver, 'required': sorted(required), 'dependencies': sorted(dependencies),
            'gn_sha256': sha256(gn), 'gn_targets': targets}


def measure_portable_runtime(source, output, host, environment):
    build = source / 'out/Numera'
    if build.resolve(strict=True) != build:
        raise RuntimeError('Chromium build output follows an unexpected link')
    layout = describe_portable_runtime(source, host, environment)
    files = resolve_runtime_files(build, layout['dependencies'])
    if not set(layout['required']) <= set(files):
        raise RuntimeError('Runtime dependency list is missing required executable files')
    manifest = {name: {'sha256': sha256(build / name), 'bytes': (build / name).stat().st_size} for name in files}
    bytes_required = sum(item['bytes'] for item in manifest.values())
    write_json(output / 'portable-runtime-inventory.json', {'files': manifest, 'bytes': bytes_required,
               'gn_sha256': layout['gn_sha256'], 'gn_targets': layout['gn_targets'],
               'publisher_signing': 'NOT_CONFIGURED'})
    return build, layout['binary'], layout['driver']


def extract_portable_archive(staged, scratch, inventory):
    files = inventory['files']
    minimum_free = inventory['bytes'] + 64 * 1024**2
    if shutil.disk_usage(scratch).free < minimum_free:
        raise RuntimeError('Insufficient space to write the portable archive')
    package = scratch / 'numera-portable-runtime.tar.gz'
    with tarfile.open(package, 'x:gz', compresslevel=3) as archive:
        for name, entry in files.items():
            with (staged / name).open('rb') as source:
                metadata = os.fstat(source.fileno())
                if not stat.S_ISREG(metadata.st_mode) or metadata.st_size != entry['bytes']:
                    raise RuntimeError('Runtime file identity changed during packaging')
                member = tarfile.TarInfo(f'runtime/{name}')
                member.size = metadata.st_size
                member.mode = metadata.st_mode & 0o777
                archive.addfile(member, source)
    if shutil.disk_usage(scratch).free < minimum_free:
        raise RuntimeError('Insufficient space to extract the portable archive')
    destination = scratch / 'Numera extracted مسیر'
    destination.mkdir()
    expected = {f'runtime/{name}': entry for name, entry in files.items()}
    with tarfile.open(package, 'r:gz') as archive:
        members = archive.getmembers()
        if (len(members) != len(expected) or {member.name for member in members} != set(expected)
                or any(not member.isfile() or member.mode & 0o7000 or member.size != expected[member.name]['bytes']
                       for member in members)):
            raise RuntimeError('Portable archive members differ from the staged file inventory')
        archive.extractall(destination, members, filter='data')
    if any(sha256(destination / name) != entry['sha256'] for name, entry in expected.items()):
        raise RuntimeError('Extracted runtime is not byte-identical to the staged files')
    return destination / 'runtime', {'format': 'tar.gz', 'files': len(files),
                                    'bytes': package.stat().st_size, 'sha256': sha256(package)}


def smoke_portable_runtime(source, scratch, output, host, environment):
    staged, binary, driver = measure_portable_runtime(source, output, host, environment)
    inventory_path = output / 'portable-runtime-inventory.json'
    inventory = json.loads(inventory_path.read_text(encoding='utf-8'))
    runtime, archive = extract_portable_archive(staged, scratch, inventory)
    build = source / 'out/Numera'
    original = build.with_name('Numera-fixture-held')
    if original.exists() or original.is_symlink():
        raise RuntimeError('Reserved source build path already exists')
    evidence = output / 'portable-runtime'
    evidence.mkdir()
    profile = runtime.parent / 'داده‌های مرورگر'
    profile.mkdir()
    relocated_environment = dict(environment)
    sandbox_helper = None
    if host == 'Linux':
        # Chromium's SUID launcher splits wrapper paths on spaces. Install the
        # staged helper bytes into a separate, space-free fixture directory.
        helper_directory = scratch / 'portable-sandbox'
        helper_directory.mkdir()
        sandbox_helper = install_linux_sandbox(runtime, helper_directory, relocated_environment)
    # Unavailable original output exposes stale absolute runtime dependencies.
    build.rename(original)
    try:
        smoke(runtime / binary, profile, evidence, relocated_environment)
        result = smoke_driver(runtime / binary, runtime / driver, profile, evidence, relocated_environment, stop_tree)
    finally:
        original.rename(build)
    report = {'result': 'PASS', 'scope': 'Extracted relocated runtime and profile, controlled renderer and WebDriver tests',
              'archive': archive, 'runtime_inventory_sha256': sha256(inventory_path), 'webdriver': result}
    if sandbox_helper is not None:
        report['sandbox_helper'] = {**sandbox_helper, 'staged_source': 'chrome_sandbox',
                                    'fixture_location': 'Separate space-free privileged fixture directory'}
    return report
