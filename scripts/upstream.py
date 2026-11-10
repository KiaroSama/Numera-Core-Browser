#!/usr/bin/env python3
"""Observe official feeds; create review candidates, never trusted releases."""
import argparse
from logging_util import log
import datetime
import json
from pathlib import Path
import urllib.request


def get(url):
    request = urllib.request.Request(url, headers={"User-Agent": "Numera-Core-Browser-upstream-watcher"})
    with urllib.request.urlopen(request, timeout=30) as response:
        body = response.read(4 * 1024 * 1024)
    return json.loads(body)


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--output", default="upstream/candidates.json")
    args = parser.parse_args()
    chromium = {}
    for platform in ("win", "linux", "mac"):
        url = f"https://versionhistory.googleapis.com/v1/chrome/platforms/{platform}/channels/stable/versions/all/releases?filter=endtime=none"
        releases = get(url)["releases"]
        approved_rollouts = [r for r in releases if r.get("pinnable") is True and r.get("fraction", 0) >= 1]
        if not approved_rollouts:
            raise RuntimeError("Official feed has no fully rolled out pinnable stable release; manual review required")
        latest = max(approved_rollouts, key=lambda r: (r["serving"]["startTime"], tuple(map(int, r["version"].split(".")))))
        chromium[platform] = {"version": latest["version"], "serving_start": latest["serving"]["startTime"], "source": url}
    mozilla_url = "https://product-details.mozilla.org/1.0/firefox_versions.json"
    mozilla = get(mozilla_url)
    webkit_url = "https://api.github.com/repos/WebKit/WebKit/commits/main"
    webkit = get(webkit_url)
    clients = {}
    for name, repo in (("playwright", "microsoft/playwright"), ("puppeteer", "puppeteer/puppeteer"), ("selenium", "SeleniumHQ/selenium")):
        source = f"https://api.github.com/repos/{repo}/releases/latest"
        if name == "puppeteer":
            source = f"https://api.github.com/repos/{repo}/releases?per_page=50"
            # Puppeteer's monorepo also releases @puppeteer/browsers. That
            # package is not the automation client and must not be mislabeled.
            releases = get(source)
            release = next(r for r in releases if r["tag_name"].startswith("puppeteer-v") and not r["prerelease"] and not r["draft"])
        else:
            release = get(source)
        clients[name] = {"tag": release["tag_name"], "published_at": release["published_at"], "source": source,
                         "compatibility": "NOT_RUN; not an engine protocol version lock"}
    versions = {"chromium": chromium, "automation_clients": clients,
        "gecko": {"stable": mozilla["LATEST_FIREFOX_VERSION"], "esr": mozilla.get("FIREFOX_ESR"), "source": mozilla_url, "scope": "deferred"},
        "webkit": {"revision": webkit["sha"], "source": webkit_url, "scope": "deferred", "note": "Not a universal Safari release version"}}
    output = Path(args.output)
    old = json.loads(output.read_text(encoding="utf-8")) if output.exists() else {}
    if old.get("versions") == versions:
        log("Official versions unchanged; candidate retained")
        return
    output.parent.mkdir(parents=True, exist_ok=True)
    output.write_text(json.dumps({"schema_revision": 1, "observed_at": datetime.datetime.now(datetime.timezone.utc).isoformat(),
        "status": "unreviewed-candidate", "trusted_release": False, "versions": versions}, indent=2) + "\n", encoding="utf-8")
    log("Wrote unreviewed upstream candidate; no engine was built, installed or promoted")


if __name__ == "__main__":
    main()
