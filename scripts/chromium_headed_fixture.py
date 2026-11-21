#!/usr/bin/env python3
"""Verify a native Chromium window during a controlled ChromeDriver session."""
import json
import os
from pathlib import Path
import platform
import subprocess
import sys

from chromedriver_fixture import smoke_driver
from engine_build import stop_tree, write_json


def smoke_headed_browser(binary, driver, scratch, output, environment):
    profile_root = scratch / 'headed-browser'
    profile_root.mkdir()
    evidence = output / 'headed-browser'
    evidence.mkdir()
    if platform.system() == 'Windows':
        return smoke_driver(binary, driver, profile_root, evidence, environment, stop_tree, headed=True)

    # xvfb-run isolates the X11 session with an automatically selected display and authority file.
    command = ['xvfb-run', '--auto-servernum', '--server-args=-screen 0 1280x900x24 -nolisten tcp',
               sys.executable, str(Path(__file__).resolve()), str(binary), str(driver), str(profile_root), str(evidence)]
    with (evidence / 'display.log').open('wb') as log:
        child = subprocess.Popen(command, env=environment, stdout=log, stderr=subprocess.STDOUT)
        try:
            if child.wait(timeout=100) != 0:
                raise RuntimeError('The Linux headed browser fixture failed; inspect headed-browser/display.log')
        finally:
            if child.poll() is None:
                stop_tree(child)
                child.wait(timeout=5)
    report = json.loads((evidence / 'report.json').read_text(encoding='utf-8'))
    if report.get('result') != 'PASS' or report.get('native_window') != 'x11-window-with-browser-pid':
        raise RuntimeError('The Linux headed browser fixture did not verify a native window')
    return report


if __name__ == '__main__':
    chromium, chromedriver, scratch_path, output_path = map(Path, sys.argv[1:])
    result = smoke_driver(chromium, chromedriver, scratch_path, output_path, os.environ, stop_tree, headed=True)
    write_json(output_path / 'report.json', result)
