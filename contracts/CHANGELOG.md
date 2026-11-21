# NCB-1 contract changelog

## Wire major 1, schema revision 1

The initial development contract defines the `numera.core` namespace, version and capability negotiation, engine-manifest inspection, launch snapshot planning, and framed request and response schemas. It includes positive and negative fixtures, initial theme and compatibility data, and a Python consumer that exercises the Rust producer on Windows and Linux.

Verify `numera-core-contracts-v1.zip` against `numera-core-contracts-v1.sha256`, then extract the ZIP and open its `numera-core-contracts-v1` directory. Install the pinned Python dependencies from `requirements-dev.txt`. Run `python scripts/check-contracts.py --binary /absolute/path/to/numera-core` with the matching development CLI. On Linux this conformance test also requires `cc` for the file-open regression fixture.

Use the commit in `source-commit.txt` to pin this development snapshot. Native browser launch, manager pairing and production update trust require their own acceptance evidence.

The compatibility data records the owned Linux headless automation checks. Runtime automation capabilities remain unavailable in the CLI. [Native test evidence](https://github.com/KiaroSama/Numera-Core-Browser/blob/111d89e041d230e148977d2298e11202e9655f78/docs/evidence/chromium-replay-44910785784.json)
