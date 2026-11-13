# Bounded checkpoint research fixture

This experiment compiles a tiny authored Linux C program. It contains no Chromium source or engine objects and cannot pass engine acceptance. The actual cold Windows browser build is a separate workflow.

The manual `Tiny Siso checkpoint research` workflow uses two fresh standard public Ubuntu runners. The seed compiles one object from source. It sends a bounded checkpoint, matching complete first-party source and the unchanged project license to the second job. The second job creates fresh inputs, verifies the seed job's checkpoint digest, exact source/script/tool/compiler/image/path context, all source hashes and all output hashes before restoring timestamps. The download action also requires the GitHub artifact digest to match. Missing or changed provenance blocks resumption.

Native acceptance for this fixture requires:

- Seed: one actual compile action.
- Resume: exactly two actions, compiling the other object and linking; the program exits with its expected value 7.
- Unchanged invocation: zero actions and the same program result.
- Changed depfile header: exactly two actions and program result 8.
- Corrupted source or object with the same size and restored timestamp: rejected by the content guard.

The pinned Siso revision is `b521b230cb7c56c821235e498285cf8e548a73ee`, as selected by Chromium's current DEPS. The official package is `build/siso/linux-amd64`, not the obsolete guessed `infra/build/siso` path. Both transport ZIP and executable digests are checked before execution. The script records its exact source hash. No remote compiler or object cache is used.

Local primitive experiments with this version found that fresh source timestamps rebuild the unchanged tiny graph, and moving its absolute workspace also rebuilt all three actions. Copying the checkpoint back to its original path with matching timestamps reused completed work. Timestamp equality alone can miss same-size source/object corruption; the content guard is therefore mandatory. These are local observations, not cross-runner or full-engine evidence. The authored probe then passed partial resumption, no-op, header invalidation and corruption controls locally; hosted results must be retained separately before claiming the two-runner result.

Even a passing hosted fixture leaves full Chromium continuation unproven. Its transitive toolchain/SDK fingerprints, complete output/state inventory, Windows timestamp/path behavior, object storage size and free allowance, dependency invalidation across a real graph, cold bootstrap and final link/package fit still need measured evidence. The toy compiler-executable fingerprint does not cover the complete system toolchain. No Chromium checkpoint is uploaded and no paid storage limit is enabled by this experiment. GitHub's included per-repository cache allowance is 10 GB; a proposal cannot assume unlimited cache or paid overage.

Sources:

- https://chromium.googlesource.com/chromium/src/+/6dc9bc3bdaea5c35ed6d41fceab2f7a3a2227c4f/DEPS
- https://chromium.googlesource.com/chromium/src/+/6dc9bc3bdaea5c35ed6d41fceab2f7a3a2227c4f/docs/siso_tips.md
- https://chromium.googlesource.com/build/+/b521b230cb7c56c821235e498285cf8e548a73ee/siso/build/mtimecheck.go
- https://chromium.googlesource.com/build/+/b521b230cb7c56c821235e498285cf8e548a73ee/siso/hashfs/state.go
- https://github.com/actions/download-artifact/tree/9000827ccba6bdab643e8b6fd33ac0654aef8333
- https://docs.github.com/en/actions/using-workflows/caching-dependencies-to-speed-up-workflows#usage-limits-and-eviction-policy
