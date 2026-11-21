#!/usr/bin/env python3
"""Exercise the harness against the unmodified browser supplied by the hosted image."""
import json
import os
from pathlib import Path
import platform
import shutil

from chromedriver_fixture import smoke_clients, smoke_driver
from engine_build import now, sha256, smoke, stop_tree, write_json


def main():
    host = platform.system()
    event = json.loads(Path(os.environ['GITHUB_EVENT_PATH']).read_text(encoding='utf-8'))
    if (os.environ.get('GITHUB_ACTIONS') != 'true' or os.environ.get('RUNNER_ENVIRONMENT') != 'github-hosted'
            or os.environ.get('GITHUB_REPOSITORY') != 'KiaroSama/Numera-Core-Browser'
            or event['repository']['private'] is not False or os.environ.get('RUNNER_ARCH') != 'X64'
            or host not in {'Windows', 'Linux'}):
        raise RuntimeError('The comparison baseline requires an authorized hosted Windows or Linux job')
    output = Path('build/automation-baseline').resolve()
    output.mkdir(parents=True, exist_ok=False)
    profile_root = Path(os.environ['RUNNER_TEMP']).resolve() / 'Numera fixture baseline'
    profile_root.mkdir(exist_ok=False)
    environment = {key: value for key, value in os.environ.items() if key.upper() not in
                   {'GH_TOKEN', 'GITHUB_TOKEN', 'ACTIONS_RUNTIME_TOKEN', 'ACTIONS_ID_TOKEN_REQUEST_TOKEN'}}
    report = {'schema_revision': 1, 'source_commit': os.environ['GITHUB_SHA'],
              'runner_image': os.environ['ImageVersion'], 'host': host, 'baseline': 'runner-installed Google Chrome',
              'owned_engine_used': False, 'source_build': 'NOT_RUN', 'result': 'BLOCKED'}
    try:
        if host == 'Windows':
            applications = [Path(os.environ[key]) / 'Google/Chrome/Application/chrome.exe'
                            for key in ('ProgramFiles', 'ProgramFiles(x86)') if os.environ.get(key)]
            browser = next((path for path in applications if path.is_file()), None)
        else:
            browser = shutil.which('google-chrome')
        if browser is None:
            raise RuntimeError('The hosted image did not supply Google Chrome')
        driver = Path(os.environ['CHROMEWEBDRIVER']) / ('chromedriver.exe' if host == 'Windows' else 'chromedriver')
        browser, driver = Path(browser).resolve(strict=True), driver.resolve(strict=True)
        report.update(browser_sha256=sha256(browser), chromedriver_sha256=sha256(driver))
        smoke(browser, profile_root, output, environment)
        report['renderer_fixture'] = 'PASS'
        report['webdriver'] = smoke_driver(browser, driver, profile_root, output, environment, stop_tree)
        report['automation_clients'] = smoke_clients(browser, driver, profile_root, output, environment, stop_tree)
        report['result'] = 'PASS'
    finally:
        report['timestamp'] = now()
        write_json(output / 'report.json', report)


if __name__ == '__main__':
    main()
