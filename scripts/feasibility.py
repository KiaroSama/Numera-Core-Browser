#!/usr/bin/env python3
"""Bounded, cold source-build preflight. This never claims a source build passed."""
import argparse
import datetime
import json
import os
from pathlib import Path
import platform
import shutil
import signal
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
        "execution_context": "github-hosted" if os.environ.get("GITHUB_ACTIONS") == "true" else "local-observation-only",
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
    if after.free >= report["required_free_bytes"] and os.environ.get("GITHUB_ACTIONS") == "true":
        report["stage"] = "source-acquisition"
        build_start = time.monotonic()
        command = ["pwsh", "-NoProfile", "-File", "scripts/Build-Chromium.ps1", "-SourceRoot", str(scratch), "-Phase", "Build"]
        process = subprocess.Popen(command, start_new_session=os.name != "nt")
        min_free = after.free
        stop_reason = None
        while process.poll() is None:
            free = shutil.disk_usage(root).free
            min_free = min(min_free, free)
            if time.monotonic() - build_start > 300:
                stop_reason = "Five-minute cold spike budget exhausted"
            elif free < 20_000_000_000:
                stop_reason = "Disk guard reached: preserve 20 GB free"
            if stop_reason:
                if os.name == "nt":
                    subprocess.run(["taskkill", "/PID", str(process.pid), "/T", "/F"], check=False)
                else:
                    os.killpg(process.pid, signal.SIGTERM)
                try:
                    process.wait(timeout=10)
                except subprocess.TimeoutExpired:
                    if os.name != "nt":
                        os.killpg(process.pid, signal.SIGKILL)
                    process.wait(timeout=10)
                break
            time.sleep(1)
        progress = scratch / "phase.txt"
        report["stage"] = progress.read_text(encoding="utf-8").strip() if progress.exists() else "source-acquisition"
        report["source_and_build_attempt_seconds"] = round(time.monotonic() - build_start, 3)
        report["source_attempt_exit_code"] = process.returncode
        report["source_acquisition_bytes"] = sum(f.stat().st_size for f in scratch.rglob("*") if f.is_file() and not f.is_symlink())
        report["peak_volume_disk_delta_bytes"] = max(0, after.free - min_free)
        report["disk_measurement_note"] = "Sampled volume delta, not an exact process-tree working set"
        report["reason"] = stop_reason or "Native recipe returned; compile/link/package provenance and real-engine acceptance remain unproven"
        if process.returncode == 0:
            report["stage"] = "native-acceptance-and-packaging"
    report["duration_seconds"] = round(time.monotonic() - started, 3)
    output = Path(args.output)
    output.parent.mkdir(parents=True, exist_ok=True)
    output.write_text(json.dumps(report, indent=2) + "\n", encoding="utf-8")
    log(report["reason"], "WARNING")
    summary = os.environ.get("GITHUB_STEP_SUMMARY")
    if summary:
        with open(summary, "a", encoding="utf-8") as f:
            f.write(f"### Chromium source-build gate: BLOCKED\n\nStage: `{report['stage']}`. "
                    f"Free: {after.free / 1e9:.2f} GB; required safety budget: 100 GB.\n\n"
                    f"{report['reason']} Consult JSON for attempted stages; missing measurements are not a PASS.\n")


if __name__ == "__main__":
    main()
