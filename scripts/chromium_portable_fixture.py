#!/usr/bin/env python3
"""Stage a source-built runtime and verify it from a relocated path."""
from pathlib import PurePosixPath
import shutil
import stat
import subprocess

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


def stage_portable_runtime(source, scratch, output, host, environment):
    build = source / 'out/Numera'
    if build.resolve(strict=True) != build:
        raise RuntimeError('Chromium build output follows an unexpected link')
    layout = describe_portable_runtime(source, host, environment)
    files = resolve_runtime_files(build, layout['dependencies'])
    if not set(layout['required']) <= set(files):
        raise RuntimeError('Runtime dependency list is missing required executable files')
    runtime = scratch / 'Numera portable مسیر' / 'runtime'
    bytes_required = sum((build / name).stat().st_size for name in files)
    if shutil.disk_usage(scratch).free - bytes_required < 64 * 1024**2:
        raise RuntimeError('Insufficient disk space for the relocated runtime and its test evidence')
    runtime.mkdir(parents=True, exist_ok=False)
    manifest = {}
    for name in files:
        original, staged = build / name, runtime / name
        staged.parent.mkdir(parents=True, exist_ok=True)
        shutil.copyfile(original, staged)
        staged.chmod(stat.S_IMODE(original.stat().st_mode) & 0o777)
        digest = sha256(staged)
        if digest != sha256(original):
            raise RuntimeError('Relocated runtime file does not match the source-built file')
        manifest[name] = {'sha256': digest, 'bytes': staged.stat().st_size}
    write_json(output / 'portable-runtime-inventory.json', {'files': manifest, 'bytes': bytes_required,
               'gn_sha256': layout['gn_sha256'], 'gn_targets': layout['gn_targets'],
               'publisher_signing': 'NOT_CONFIGURED'})
    return runtime, layout['binary'], layout['driver']


def smoke_portable_runtime(source, scratch, output, host, environment):
    runtime, binary, driver = stage_portable_runtime(source, scratch, output, host, environment)
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
    report = {'result': 'PASS', 'scope': 'Relocated runtime and profile, controlled renderer and WebDriver tests',
              'runtime_inventory_sha256': sha256(output / 'portable-runtime-inventory.json'), 'webdriver': result}
    if sandbox_helper is not None:
        report['sandbox_helper'] = {**sandbox_helper, 'staged_source': 'chrome_sandbox',
                                    'fixture_location': 'Separate space-free privileged fixture directory'}
    return report
