#!/usr/bin/env python3
"""Online OSV audit; retrieval failures fail closed rather than becoming green."""
import argparse
import datetime
import json
from pathlib import Path
import sys
import tomllib
import urllib.request

parser = argparse.ArgumentParser()
parser.add_argument('--lock-file', default='Cargo.lock')
parser.add_argument('--output', default='build/security-audit.json')
args = parser.parse_args()
packages = [p for p in tomllib.loads(Path(args.lock_file).read_text())["package"] if "source" in p]
queries = [{"package": {"name": p["name"], "ecosystem": "crates.io"}, "version": p["version"]} for p in packages]
report = {"timestamp": datetime.datetime.now(datetime.timezone.utc).isoformat(), "source": "https://api.osv.dev/v1/querybatch", "lock_file": args.lock_file,
          "result": "BLOCKED", "packages_checked": len(packages), "findings": []}
try:
    request = urllib.request.Request(report["source"], data=json.dumps({"queries": queries}).encode(),
                                     headers={"Content-Type": "application/json", "User-Agent": "Numera-Core-Browser"})
    with urllib.request.urlopen(request, timeout=45) as response:
        results = json.load(response)["results"]
    assert len(results) == len(packages)
    for package, result in zip(packages, results):
        if result.get("vulns"):
            report["findings"].append({"package": package["name"], "version": package["version"],
                                       "advisories": [v["id"] for v in result["vulns"]]})
    report["result"] = "FAIL" if report["findings"] else "PASS"
except Exception as error:
    report["error_type"] = type(error).__name__
out = Path(args.output)
out.parent.mkdir(parents=True, exist_ok=True)
out.write_text(json.dumps(report, indent=2) + "\n", encoding="utf-8")
print(json.dumps({"timestamp": report["timestamp"], "severity": "INFO" if report["result"] == "PASS" else "ERROR",
                  "component": "dependency-audit", "message": f"OSV audit {report['result']}; {len(report['findings'])} affected packages"}))
sys.exit(0 if report["result"] == "PASS" else 1)
