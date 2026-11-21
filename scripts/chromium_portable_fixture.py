#!/usr/bin/env python3
"""Stage a source-built runtime and verify it from a relocated path."""
from pathlib import PurePosixPath
import shutil
import stat
import subprocess

from chromedriver_fixture import smoke_driver
from engine_build import sha256, smoke, stop_tree, write_json


def resolve_runtime_files(build, dependencies):
    files = {}
    for line in dependencies:
        relative = PurePosixPath(line)
        if (relative.is_absolute() or not relative.parts or '..' in relative.parts
                or '\\' in line or ':' in line or any(ord(char) < 32 for char in line)):
            raise RuntimeError('GN runtime dependency is outside the owned build directory')
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


def stage_portable_runtime(source, scratch, output, host, environment):
    build = source / 'out/Numera'
    if build.resolve(strict=True) != build:
        raise RuntimeError('Chromium build output follows an unexpected link')
    gn = source / ('buildtools/win/gn.exe' if host == 'Windows' else 'buildtools/linux64/gn')
    targets = ['//chrome:chrome_initial', '//chrome/test/chromedriver:chromedriver_server']
    binary = 'chrome.exe' if host == 'Windows' else 'chrome'
    driver = 'chromedriver.exe' if host == 'Windows' else 'chromedriver'
    required = {binary, driver}
    if host == 'Linux':
        required.add('chrome_sandbox')
    dependencies = set(required)
    for target in targets:
        inventory = subprocess.check_output([str(gn), 'desc', 'out/Numera', target, 'runtime_deps'],
                                            cwd=source, env=environment, timeout=90)
        if not 0 < len(inventory) <= 1024 * 1024:
            raise RuntimeError('Unexpected GN runtime dependency list size')
        dependencies.update(line for line in inventory.decode('utf-8').splitlines() if line)
    if host == 'Windows':
        # The Windows build reorders imports into the final top-level chrome.exe.
        dependencies = {name for name in dependencies if PurePosixPath(name).as_posix()
                        not in {'initialexe/chrome.exe', 'initialexe/chrome.exe.pdb'}}
    files = resolve_runtime_files(build, dependencies)
    if not required <= set(files):
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
               'gn_sha256': sha256(gn), 'gn_targets': targets, 'publisher_signing': 'NOT_CONFIGURED'})
    return runtime, binary, driver


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
    # Unavailable original output exposes stale absolute runtime dependencies.
    build.rename(original)
    try:
        smoke(runtime / binary, profile, evidence, environment)
        result = smoke_driver(runtime / binary, runtime / driver, profile, evidence, environment, stop_tree)
    finally:
        original.rename(build)
    return {'result': 'PASS', 'scope': 'Relocated runtime and profile, controlled renderer and WebDriver tests',
            'runtime_inventory_sha256': sha256(output / 'portable-runtime-inventory.json'), 'webdriver': result}
