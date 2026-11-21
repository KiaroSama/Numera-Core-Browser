#!/usr/bin/env python3
"""Attempt the full native Chromium graph in bounded jobs with verified local cache handoffs."""
import argparse
from contextlib import nullcontext
import json
import os
from pathlib import Path
import platform
import re
import shutil
import subprocess
import time

import psutil
from chromedriver_fixture import smoke_clients, smoke_driver
from chromium_cache_probe import (REPOSITORY, RELEASES, STAGED_CACHE_CONFIG, compile_graph,
                                  install_tool, pack_cache, restore_cache, sccache_server)
from chromium_portable_fixture import smoke_portable_runtime
from engine_build import CheckRun, install_linux_sandbox, now, publish_check, read_progress, sha256, smoke, stop_tree, write_json
from logging_util import log

SEGMENTS = 4
CACHE_LIMIT = 2_650_000_000
CACHE_ACCOUNTING_GUARD = 2_500_000_000
MAX_ENTRIES = 100_000
CHECKPOINT_RUNNERS = {'Linux': 'ubuntu-24.04', 'Windows': 'windows-2025-vs2026'}
PRODUCER_IDENTITY = ('head', 'run_id', 'run_attempt')


def reviewed_checkpoint(directory, run_id, segment, host):
    if not re.fullmatch(r'[1-9][0-9]{0,17}', run_id) or segment not in {'1', '2', '3'}:
        raise RuntimeError('Select a reviewed run ID and checkpoint segment from 1 to 3')
    path = directory / f'chromium-staged-{run_id}-segment-{segment}.json'
    try:
        evidence = json.loads(path.read_text(encoding='utf-8'))
        report = evidence['report']
        context = report['context']
        digest = report['cache_archive_sha256']
        key = evidence['cache']['exact_key']
        job_id = evidence['job_id']
        runner = CHECKPOINT_RUNNERS[host]
        producer_head = context['head']
        producer_attempt = context['run_attempt']
        matches_run = (evidence['run_id'] == int(run_id) and report['segment'] == int(segment)
                       and report['host'] == host and context['host'] == host and context['run_id'] == run_id)
        matches_result = report['result'] == report['source_build'] == 'PARTIAL' and producer_head == report['commit']
        valid_ids = (re.fullmatch(r'[0-9a-f]{40}', producer_head) and re.fullmatch(r'[0-9a-f]{64}', digest)
                     and re.fullmatch(r'[1-9][0-9]*', producer_attempt) and type(job_id) is int and job_id > 0)
    except (KeyError, OSError, TypeError, ValueError) as error:
        raise RuntimeError('No complete reviewed checkpoint evidence exists for this runner') from error
    if not (matches_run and matches_result and valid_ids):
        raise RuntimeError('Reviewed checkpoint metadata does not match the requested native graph')
    if key != f'chromium-staged-{run_id}-{producer_attempt}-{segment}-{runner}':
        raise RuntimeError('Reviewed checkpoint has an unexpected cache key')
    return {'context': context, 'sha256': digest, 'key': key, 'job_id': job_id}


def restore_reviewed_context(context, checkpoint):
    # The producing job supplies its identity only. Build inputs must still
    # match the fresh graph, toolchain, configuration and platform.
    return dict(context, **{key: checkpoint['context'][key] for key in PRODUCER_IDENTITY})


def cache_command(tool, environment, *args):
    return subprocess.check_output([str(tool), *args], env=environment, timeout=120)


def statistics(tool, environment):
    if tool.stem == 'ccache':
        return json.loads(cache_command(tool, environment, '--print-stats', '--format=json'))
    result = json.loads(cache_command(tool, environment, '--show-stats', '--stats-format=json'))
    disk = result['cache_location']
    if not disk.startswith('Local disk: ') or result['max_cache_size'] != 4 * 1024**3:
        raise RuntimeError('The staged sccache server did not confirm the expected local storage policy')
    details = result['stats']
    return {'preprocessed_cache_hit': sum(details['cache_hits']['counts'].values()),
            'local_storage_write': details['cache_writes'], 'cache_size_kibibyte': (result['cache_size'] or 0) // 1024,
            'backend_statistics': details}


def compile_segment(ninja, wrapper, source, targets, environment, output, report, minutes, checks=None):
    """Only stop the owned native tree, preserving completed cache writes."""
    available = psutil.virtual_memory().available
    workers = max(1, min(os.cpu_count() or 1, 4, available // (3 * 1024**3)))
    report['parallelism'] = workers
    cache_command(wrapper, environment, '--zero-stats')
    command = [str(ninja), '-C', 'out/Numera', '-j', str(workers), *targets]
    native_log = output / 'native-build.log'
    started = time.monotonic()
    next_report = 0
    with native_log.open('wb') as stream:
        child = subprocess.Popen(command, cwd=source, env=environment, stdout=stream, stderr=subprocess.STDOUT)
        try:
            while child.poll() is None:
                elapsed = time.monotonic() - started
                free = shutil.disk_usage(source).free
                if free < 25_000_000_000:
                    raise RuntimeError('Staged build reached its 25 GB free disk floor')
                if elapsed >= minutes * 60:
                    return 'time-budget'
                if elapsed >= next_report:
                    values = statistics(wrapper, environment)
                    report.update(elapsed_seconds=round(elapsed, 3), disk_free_bytes=free,
                                  native_action_progress=read_progress(native_log), compiler_cache=values)
                    try:
                        parent = psutil.Process(child.pid)
                        rss = 0
                        for process in [parent, *parent.children(recursive=True)]:
                            try:
                                rss += process.memory_info().rss
                            except (psutil.NoSuchProcess, psutil.AccessDenied):
                                pass
                        report['peak_tree_rss_bytes'] = max(report.get('peak_tree_rss_bytes', 0), rss)
                    except (psutil.NoSuchProcess, psutil.AccessDenied):
                        pass
                    write_json(output / 'report.json', report)
                    if checks is not None:
                        publish_check(report, checks)
                    log(f"Full graph: {elapsed:.0f}s, {values['preprocessed_cache_hit']} cache hits, "
                        f"{values['local_storage_write']} writes, {free / 1e9:.2f} GB free", component='staged-chromium')
                    next_report = elapsed + 30
                time.sleep(2)
        finally:
            if child.poll() is None:
                stop_tree(child)
            report.update(elapsed_seconds=round(time.monotonic() - started, 3),
                          native_action_progress=read_progress(native_log))
    if child.returncode != 0:
        raise RuntimeError('Native Chromium build failed; inspect native-build.log')
    return 'built'


def write_outputs(result, checkpoint_sha=''):
    with Path(os.environ['GITHUB_OUTPUT']).open('a', encoding='utf-8') as stream:
        stream.write(f'result={result}\ncheckpoint_sha={checkpoint_sha}\n')


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument('--segment', required=True, type=int, choices=range(1, SEGMENTS + 1))
    parser.add_argument('--minutes', type=int, choices=range(10, 276), default=275)
    parser.add_argument('--checkpoint-key', action='store_true')
    args = parser.parse_args()
    host = platform.system()
    event = json.loads(Path(os.environ['GITHUB_EVENT_PATH']).read_text(encoding='utf-8'))
    if (os.environ.get('GITHUB_ACTIONS') != 'true' or os.environ.get('RUNNER_ENVIRONMENT') != 'github-hosted'
            or os.environ.get('GITHUB_REPOSITORY') != REPOSITORY or event['repository']['private'] is not False
            or os.environ.get('RUNNER_ARCH') != 'X64' or host not in RELEASES):
        raise RuntimeError('The staged build requires the authorized public Windows or Linux x86-64 job')
    reviewed = None
    reviewed_run = os.environ.get('REVIEWED_CHECKPOINT_RUN', '')
    if reviewed_run:
        if args.segment != 1:
            raise RuntimeError('Only the first segment can start from a reviewed earlier run')
        reviewed = reviewed_checkpoint(Path('docs/evidence'), reviewed_run,
                                       os.environ.get('REVIEWED_CHECKPOINT_SEGMENT', ''), host)
    read_only_replay = os.environ.get('READ_ONLY_REPLAY') == 'true'
    if read_only_replay and reviewed is None:
        raise RuntimeError('A read-only replay requires a reviewed earlier checkpoint')
    if args.checkpoint_key:
        if reviewed is None:
            raise RuntimeError('A reviewed run is required to resolve a cache key')
        print(reviewed['key'])
        return
    output = Path('build/chromium-staged-evidence').resolve()
    output.mkdir(parents=True, exist_ok=False)
    transfer = Path('build/chromium-staged-cache').resolve()
    transfer.mkdir(parents=True, exist_ok=True)
    root = Path(os.environ['RUNNER_TEMP']).resolve() / 'numera-chromium-staged'
    tools = root.with_name(root.name + '-tools')
    tools.mkdir(exist_ok=False)
    entries = tools / 'entries'
    entries.mkdir()
    report = {'schema_revision': 1, 'result': 'BLOCKED', 'stage': 'prepare', 'segment': args.segment,
              'commit': os.environ['GITHUB_SHA'], 'host': host, 'release_ready': False, 'source_build': 'NOT_RUN',
              'read_only_replay': read_only_replay,
              'renderer_fixture': 'NOT_RUN', 'driver_fixture': 'NOT_RUN', 'automation_client_smokes': 'NOT_RUN',
              'automation_suites': 'NOT_RUN', 'portable_runtime_fixture': 'NOT_RUN',
              'compile_budget_minutes': args.minutes,
              'debug_build': True, 'elapsed_seconds': 0, 'disk_free_bytes': shutil.disk_usage(root.parent).free,
              'peak_tree_rss_bytes': 0}
    checks = CheckRun(job_name=('first', 'second', 'third', 'fourth')[args.segment - 1] + ' / native') if os.environ.get('GH_TOKEN') else None
    try:
        wrapper, ninja = [install_tool(tools, spec) for spec in RELEASES[host]]
        cache_tool = wrapper.stem
        config = tools / 'local-cache.conf'
        environment = {k: v for k, v in os.environ.items() if not k.upper().startswith(('CCACHE_', 'SCCACHE_')) and k not in
                       {'GH_TOKEN', 'GITHUB_TOKEN', 'ACTIONS_RUNTIME_TOKEN', 'ACTIONS_ID_TOKEN_REQUEST_TOKEN'}}
        environment.update(DEPOT_TOOLS_UPDATE='0', DEPOT_TOOLS_WIN_TOOLCHAIN='0', NINJA_STATUS='[%f/%t] ')
        if cache_tool == 'sccache':
            config.write_bytes(b'')
            environment.update(SCCACHE_CONF=str(config), SCCACHE_DIR=str(entries), SCCACHE_CACHE_SIZE='4G',
                               SCCACHE_SERVER_PORT='4234', SCCACHE_DIRECT='false')
        else:
            config.write_text(STAGED_CACHE_CONFIG, encoding='utf-8')
            temp_cache = tools / 'temp'
            temp_cache.mkdir()
            environment.update(CCACHE_DIR=str(entries), CCACHE_CONFIGPATH=str(config), CCACHE_TEMPDIR=str(temp_cache))
        environment['PATH'] = os.pathsep.join((str(tools), str(root / 'depot_tools'), environment['PATH']))
        compile_graph(root, environment, output, cache_tool)
        source = root / 'checkout/src'
        native_output = source / 'out/Numera'
        targets = ['chrome', 'chromedriver'] + (['chrome_sandbox'] if host == 'Linux' else [])
        graph_commands = tools / 'ninja-commands.txt'
        with graph_commands.open('wb') as stream:
            subprocess.run([str(ninja), '-C', 'out/Numera', '-t', 'commands', *targets], cwd=source,
                           env=environment, timeout=120, stdout=stream, check=True)
        if not 0 < graph_commands.stat().st_size <= 1024 * 1024 * 1024:
            raise RuntimeError('Unexpected full Ninja command inventory size')
        clang = source / ('third_party/llvm-build/Release+Asserts/bin/clang-cl.exe' if host == 'Windows'
                          else 'third_party/llvm-build/Release+Asserts/bin/clang')
        context = {'head': os.environ['GITHUB_SHA'], 'run_id': os.environ['GITHUB_RUN_ID'],
                   'run_attempt': os.environ['GITHUB_RUN_ATTEMPT'], 'runner_image': os.environ['ImageVersion'],
                   'host': host, 'scratch': str(root), 'targets': targets,
                   'source_lock_sha256': sha256(Path('upstream/chromium/lock.json')),
                   'gn_args_sha256': sha256(native_output / 'args.gn'), 'commands_sha256': sha256(graph_commands),
                   'cache_tool': cache_tool, 'cache_tool_sha256': sha256(wrapper), 'cache_config_sha256': sha256(config),
                   'ninja_sha256': sha256(ninja), 'clang_sha256': sha256(clang)}
        if host == 'Windows':
            sdk = json.loads((root / 'windows-sdk.json').read_text(encoding='utf-8-sig'))
            context['sdk'] = {k: sdk[k] for k in ('directory_version', 'debugging_tools_version', 'requested_release')}
            context['sccache_policy'] = {key: environment[key] for key in ('SCCACHE_CACHE_SIZE', 'SCCACHE_DIRECT')}
        report['context'] = context
        graph_commands.unlink()
        previous_inventory = {} if cache_tool == 'sccache' else None
        if args.segment > 1 or reviewed is not None:
            report['stage'] = 'restore'
            previous = transfer / 'checkpoint.tar'
            report['runner_images'] = {}
            digest, expected_context = os.environ.get('EXPECTED_CHECKPOINT_SHA'), context
            if reviewed is not None:
                digest = reviewed['sha256']
                expected_context = restore_reviewed_context(context, reviewed)
                report['reviewed_checkpoint'] = {key: reviewed[key] for key in ('sha256', 'key', 'job_id')}
                report['reviewed_checkpoint']['source_commit'] = reviewed['context']['head']
            restore_cache(entries, previous, digest, expected_context,
                          limit=CACHE_LIMIT, max_entries=MAX_ENTRIES, linux_runner_images=report['runner_images'],
                          restored_entries=previous_inventory)
            previous.unlink()
            if cache_tool == 'ccache':
                # Rebuild occupancy accounting after transport excluded ccache stats.
                cache_command(wrapper, environment, '--cleanup')
        report.update(stage='compile-link', source_build='RUNNING', result='RUNNING')
        with sccache_server(wrapper, environment) if cache_tool == 'sccache' else nullcontext():
            reason = compile_segment(ninja, wrapper, source, targets, environment, output, report, args.minutes, checks)
            values = statistics(wrapper, environment)
        report.update(compiler_cache=values, segment_stop=reason)
        if (args.segment > 1 or reviewed is not None) and values['preprocessed_cache_hit'] < 1:
            raise RuntimeError('Fresh runner did not reuse any transferred Chromium compile outputs')
        if reason == 'time-budget':
            if read_only_replay:
                raise RuntimeError('The read-only replay exhausted its compile budget')
            if args.segment == SEGMENTS:
                raise RuntimeError('The full graph did not finish within the four configured job segments')
            if values['local_storage_write'] < 1:
                raise RuntimeError('The segment did not add transferable compiler outputs')
            if values.get('cleanups_performed', 0) or values['cache_size_kibibyte'] * 1024 >= CACHE_ACCOUNTING_GUARD:
                raise RuntimeError('The compiler cache did not fit the bounded checkpoint allowance')
            report['stage'] = 'checkpoint'
            archive = transfer / 'checkpoint.tar'
            pack_cache(entries, archive, context, None, limit=CACHE_LIMIT, max_entries=MAX_ENTRIES,
                       required_entries=previous_inventory)
            report.update(result='PARTIAL', source_build='PARTIAL', cache_archive_bytes=archive.stat().st_size,
                          cache_archive_sha256=sha256(archive))
            write_outputs('PARTIAL', report['cache_archive_sha256'])
            return
        binary = (native_output / ('chrome.exe' if host == 'Windows' else 'chrome')).resolve(strict=True)
        driver = (native_output / ('chromedriver.exe' if host == 'Windows' else 'chromedriver')).resolve(strict=True)
        report.update(stage='renderer-fixture', source_build='PASS', renderer_fixture='RUNNING',
                      browser_sha256=sha256(binary), browser_bytes=binary.stat().st_size,
                      chromedriver_sha256=sha256(driver), chromedriver_bytes=driver.stat().st_size)
        if host == 'Linux':
            report['linux_sandbox_helper'] = install_linux_sandbox(native_output, root, environment)
        smoke(binary, root, output, environment)
        report.update(stage='webdriver-fixture', renderer_fixture='PASS', driver_fixture='RUNNING')
        report['webdriver'] = smoke_driver(binary, driver, root, output, environment, stop_tree)
        report.update(stage='automation-client-fixtures', driver_fixture='PASS', automation_client_smokes='RUNNING')
        report['automation_clients'] = smoke_clients(binary, driver, root, output, environment, stop_tree)
        report.update(stage='portable-runtime-fixture', automation_client_smokes='PASS', portable_runtime_fixture='RUNNING')
        report['portable_runtime'] = smoke_portable_runtime(source, root, output, host, environment)
        report.update(result='PASS', stage='source-and-native-fixtures-complete', portable_runtime_fixture='PASS')
        write_outputs('PASS')
    except Exception as error:
        report.update(result='BLOCKED', error_type=type(error).__name__)
        if isinstance(error, RuntimeError):
            report['diagnostic'] = str(error)
        for key in ('source_build', 'renderer_fixture', 'driver_fixture', 'automation_client_smokes', 'portable_runtime_fixture'):
            if report[key] == 'RUNNING':
                report[key] = 'BLOCKED'
        raise
    finally:
        report['timestamp'] = now()
        write_json(output / 'report.json', report)
        if checks is not None:
            publish_check(report, checks)


if __name__ == '__main__':
    main()
