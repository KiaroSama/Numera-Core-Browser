import importlib.util
import io
import os
from pathlib import Path
from types import SimpleNamespace
import sys
import unittest
from unittest.mock import patch
import urllib.request


SCRIPTS = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(SCRIPTS))
spec = importlib.util.spec_from_file_location('upstream_feeds', SCRIPTS / 'upstream.py')
module = importlib.util.module_from_spec(spec)
spec.loader.exec_module(module)


class OfficialFeedAuthentication(unittest.TestCase):
    def request(self, url, environment):
        requests = []

        def receive(request, timeout):
            self.assertEqual(timeout, 30)
            requests.append(request)
            return io.BytesIO(b'{"observed":true}')

        with patch.dict(os.environ, environment, clear=True), \
                patch.object(module.urllib.request, 'urlopen', side_effect=receive), \
                patch.object(module.urllib.request, 'build_opener', return_value=SimpleNamespace(open=receive)):
            self.assertEqual(module.get(url), {'observed': True})
        self.assertEqual(len(requests), 1)
        return requests[0]

    def test_github_metadata_uses_the_workflow_credential(self):
        request = self.request('https://api.github.com/repos/WebKit/WebKit/commits/main', {'GH_TOKEN': 'fixture'})
        self.assertEqual(request.get_header('Authorization'), 'Bearer fixture')

    def test_standard_github_token_binding_is_also_supported(self):
        request = self.request('https://api.github.com/repos/microsoft/playwright/releases/latest', {'GITHUB_TOKEN': 'fixture'})
        self.assertEqual(request.get_header('Authorization'), 'Bearer fixture')

    def test_foreign_plaintext_and_lookalike_origins_never_receive_the_credential(self):
        for url in ['https://versionhistory.googleapis.com/v1/chrome/platforms',
                'https://product-details.mozilla.org/1.0/firefox_versions.json',
                'https://api.github.com.example.invalid/repos/test',
                'http://api.github.com/repos/test']:
            with self.subTest(url=url):
                request = self.request(url, {'GH_TOKEN': 'fixture'})
                self.assertIsNone(request.get_header('Authorization'))

    def test_public_reads_remain_possible_without_a_credential(self):
        request = self.request('https://api.github.com/repos/WebKit/WebKit/commits/main', {})
        self.assertIsNone(request.get_header('Authorization'))

    def test_authenticated_redirect_cannot_leave_the_https_github_origin(self):
        request = urllib.request.Request('https://api.github.com/repos/WebKit/WebKit/commits/main',
            headers={'Authorization': 'Bearer fixture'})
        for url in ['https://example.invalid/redirect', 'http://api.github.com/redirect',
                'https://api.github.com.example.invalid/redirect']:
            with self.subTest(url=url), self.assertRaises(RuntimeError):
                module.GitHubMetadataRedirect().redirect_request(request, None, 302, 'Found', {}, url)

    def test_same_origin_canonical_redirect_preserves_authentication(self):
        request = urllib.request.Request('https://api.github.com/repos/WebKit/WebKit/commits/main',
            headers={'Authorization': 'Bearer fixture'})
        redirected = module.GitHubMetadataRedirect().redirect_request(request, None, 302, 'Found', {},
            'https://api.github.com/repositories/123/commits/main')
        self.assertEqual(redirected.get_header('Authorization'), 'Bearer fixture')


if __name__ == '__main__':
    unittest.main()
