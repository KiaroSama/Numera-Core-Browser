#!/usr/bin/env python3
"""Verify actual GITHUB_TOKEN draft-PR creation, then remove only probe refs.

Development administration check, not part of the installed Core runtime.
"""
import datetime
import json
import os
from pathlib import Path
import urllib.error
import urllib.request

from logging_util import log


def main():
    repository = os.environ.get('GITHUB_REPOSITORY')
    if repository != 'KiaroSama/Numera-Core-Browser' or os.environ.get('GITHUB_ACTIONS') != 'true':
        raise SystemExit('Permission proof requires the authorized repository workflow')
    token = os.environ['GH_TOKEN']
    run = os.environ['GITHUB_RUN_ID']
    attempt = os.environ['GITHUB_RUN_ATTEMPT']
    if not run.isdigit() or not attempt.isdigit():
        raise SystemExit('Invalid workflow identity')
    branch = f'automation/permission-probe-{run}-{attempt}'
    root = f'https://api.github.com/repos/{repository}'
    output = Path('build/permission-check.json')
    output.parent.mkdir(exist_ok=True)
    report = {'timestamp': datetime.datetime.now(datetime.timezone.utc).isoformat(),
        'commit': os.environ['GITHUB_SHA'], 'run_id': run, 'result': 'RUNNING',
        'branch': branch, 'pull_request': None, 'probe_pr_closed': False,
        'probe_branch_removed': False, 'engine_promoted': False}

    def api(path, data=None, method=None):
        request = urllib.request.Request(root + path,
            data=json.dumps(data).encode() if data is not None else None,
            method=method or ('POST' if data is not None else 'GET'),
            headers={'Authorization': 'Bearer ' + token, 'Accept': 'application/vnd.github+json',
                'X-GitHub-Api-Version': '2026-03-10', 'User-Agent': 'Numera-Core-Browser'})
        with urllib.request.urlopen(request, timeout=20) as response:
            return json.load(response) if response.status != 204 else None

    created = False
    commit = None
    number = None
    try:
        base = api('/git/ref/heads/main')['object']['sha']
        base_tree = api('/git/commits/' + base)['tree']['sha']
        tree = api('/git/trees', {'base_tree': base_tree, 'tree': [{
            'path': 'docs/evidence/temporary-permission-probe.json', 'mode': '100644',
            'type': 'blob', 'content': json.dumps({'purpose': 'Temporary Actions PR permission probe', 'run_id': run})}]})
        commit = api('/git/commits', {'message': 'test: temporary Actions permission probe',
            'tree': tree['sha'], 'parents': [base]})['sha']
        api('/git/refs', {'ref': 'refs/heads/' + branch, 'sha': commit})
        created = True
        pr = api('/pulls', {'title': 'test: verify Actions PR creation ' + run,
            'head': branch, 'base': 'main', 'draft': True,
            'body': 'Temporary permission proof requested by the repository owner. This probe is closed without merging; its task-owned branch is removed after verification. No engine or source lock is promoted.'})
        number = pr['number']
        report['pull_request'] = {'number': number, 'url': pr['html_url'], 'author': pr['user']['login']}
        if pr['user']['login'] != 'github-actions[bot]' or not pr['draft'] or pr['head']['ref'] != branch:
            raise RuntimeError('Unexpected draft-PR identity')
        report['result'] = 'PASS'
    except Exception as error:
        report['result'] = 'BLOCKED'
        report['error_type'] = type(error).__name__
        if isinstance(error, urllib.error.HTTPError):
            report['http_status'] = error.code
        log('Actions permission proof failed; retained status is authoritative', severity='ERROR', component='permission-probe')
    finally:
        try:
            if number is not None:
                api('/pulls/' + str(number), {'state': 'closed'}, method='PATCH')
                report['probe_pr_closed'] = True
            if created:
                # Never remove a ref if another writer has changed its target.
                current = api('/git/ref/heads/' + branch)['object']['sha']
                if current != commit:
                    raise RuntimeError('Probe branch changed; preserve it')
                api('/git/refs/heads/' + branch, method='DELETE')
                report['probe_branch_removed'] = True
        except Exception as error:
            report['cleanup_error_type'] = type(error).__name__
            report['result'] = 'BLOCKED'
        output.write_text(json.dumps(report, indent=2) + '\n', encoding='utf-8')
    log('Actions permission proof: ' + report['result'], component='permission-probe')
    return 0 if report['result'] == 'PASS' else 1


if __name__ == '__main__':
    raise SystemExit(main())
