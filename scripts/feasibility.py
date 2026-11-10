#!/usr/bin/env python3
"""Bounded, cold source-build preflight. This never claims a source build passed."""
import argparse
import datetime
import json
import os
from pathlib import Path
import platform
import shutil
import subprocess
import time


def log(message, level="INFO"):
    print(json.dumps({"timestamp": datetime.datetime.now(datetime.timezone.utc).isoformat(),
                      "severity": level, "component": "engine-feasibility", "message": message}), flush=True)


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--output", required=True)
    args = parser.parse_args()
    started = time.monotonic()
    root = Path(os.environ.get("RUNNER_TEMP", ".")).resolve()
    before = shutil.disk_usage(root)
    tools = {}
    for tool in ("git", "python3", "python", "clang", "cmake", "ninja", "pwsh", "cl"):
        tools[tool] = shutil.which(tool)
    # Only task-owned temporary content may be cleaned. Never delete runner SDKs,
    # toolchains, system directories, or unrelated work to inflate free storage.
    scratch = root / "numera-engine-feasibility"
    scratch.mkdir(exist_ok=True)
    after = shutil.disk_usage(root)
    report = {
        "schema_revision": 1,
        "timestamp": datetime.datetime.now(datetime.timezone.utc).isoformat(),
        "engine": "chromium", "result": "BLOCKED", "stage": "resource-preflight",
        "runner_os": os.environ.get("RUNNER_OS", platform.system()),
        "runner_arch": os.environ.get("RUNNER_ARCH", platform.machine()),
        "runner_image": os.environ.get("ImageOS"),
        "runner_image_version": os.environ.get("ImageVersion"),
        "os_description": platform.platform(), "cpu_count": os.cpu_count(),
        "commit": os.environ.get("GITHUB_SHA"), "run_id": os.environ.get("GITHUB_RUN_ID"),
        "run_url": f"https://github.com/{os.environ.get('GITHUB_REPOSITORY')}/actions/runs/{os.environ.get('GITHUB_RUN_ID')}",
        "tools": tools, "cache": "cold; no cache restored",
        "cleanup": "task-owned scratch only; no system/SDK cleanup",
        "disk_total_bytes": before.total, "disk_free_before_bytes": before.free,
        "disk_free_after_cleanup_bytes": after.free,
        "required_free_bytes": 100_000_000_000,
        "requirement_source": "https://chromium.googlesource.com/chromium/src/+/main/docs/windows_build_instructions.md",
        "source_acquisition_bytes": None, "compile_seconds": None,
        "link_seconds": None, "artifact_bytes": None, "peak_engine_rss_bytes": None,
        "peak_engine_disk_bytes": None, "cold_build": "NOT_RUN", "warm_build": "NOT_RUN",
        "reason": "Upstream minimum free disk is not available; source checkout and compilation were not attempted.",
    }
    if after.free >= report["required_free_bytes"]:
        report["stage"] = "recipe-validation"
        report["reason"] = "Disk preflight passed, but a reviewed pinned Numera patch/build recipe is required before compilation. No engine was built."
    report["duration_seconds"] = round(time.monotonic() - started, 3)
    output = Path(args.output)
    output.parent.mkdir(parents=True, exist_ok=True)
    output.write_text(json.dumps(report, indent=2) + "\n", encoding="utf-8")
    log(report["reason"], "WARNING")
    summary = os.environ.get("GITHUB_STEP_SUMMARY")
    if summary:
        with open(summary, "a", encoding="utf-8") as f:
            f.write(f"### Chromium source-build gate: BLOCKED\n\nStage: `{report['stage']}`. "
                    f"Free: {after.free / 1e9:.2f} GB; required: 100 GB.\n\n"
                    "Compile, link, package, peak engine memory, and cold/warm build metrics are NOT_RUN.\n")


if __name__ == "__main__":
    main()
