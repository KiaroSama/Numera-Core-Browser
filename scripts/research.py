#!/usr/bin/env python3
"""Refresh bounded public reference metadata. Development tool, not runtime."""
import concurrent.futures
from logging_util import log
import datetime
import json
from pathlib import Path
import subprocess

REFERENCES = [
    ("R1", "microsoft/playwright"), ("R2", "Kaliiiiiiiiii-Vinyzu/patchright"),
    ("R3", "daijro/camoufox"), ("R4", "CloakHQ/cloakbrowser"),
    ("R5", "clearcotelabs/clearcote-browser"), ("R6", "mattsse/chromiumoxide"),
    ("R7", "vercel-labs/agent-browser"), ("R8a", "chromiumembedded/cef"),
    ("R8b", "tauri-apps/cef-rs"), ("R9", "ungoogled-software/ungoogled-chromium"),
    ("R10", "apify/fingerprint-suite"), ("R11", "daijro/browserforge"),
    ("R12", "brave/adblock-rust"), ("R13", "rebrowser/rebrowser-patches"),
    ("R14", "web-platform-tests/wpt"), ("R15", "awslabs/tough"), ("R16", "servo/servo"),
]


def api(path):
    result = subprocess.run(["gh", "api", path], capture_output=True, text=True, timeout=30)
    if result.returncode:
        return None
    return json.loads(result.stdout)


def inspect(item):
    ref, repo = item
    metadata = api(f"repos/{repo}")
    if metadata is None:
        return {"id": ref, "repository": repo, "decision": "reference-only", "status": "unavailable", "copied_code": False}
    commit = api(f"repos/{repo}/commits/{metadata['default_branch']}")
    return {"id": ref, "repository": metadata["full_name"], "url": metadata["html_url"],
        "revision": commit["sha"] if commit else None, "archived": metadata["archived"],
        "last_pushed_at": metadata["pushed_at"], "declared_license": (metadata.get("license") or {}).get("spdx_id"),
        "decision": "reference-only", "copied_code": False,
        "rationale": "Public architecture/protocol/build reference; no code or patches adopted. Repository license metadata is not a redistribution audit."}


def main():
    with concurrent.futures.ThreadPoolExecutor(max_workers=4) as pool:
        entries = list(pool.map(inspect, REFERENCES))
    out = Path("docs/research/references.json")
    out.parent.mkdir(parents=True, exist_ok=True)
    out.write_text(json.dumps({"observed_at": datetime.datetime.now(datetime.timezone.utc).isoformat(),
        "references": entries}, indent=2) + "\n", encoding="utf-8")
    log(f"Recorded {len(entries)} public references; none adopted")


if __name__ == "__main__":
    main()
