# Bounded checkpoint research fixture

This experiment compiles a tiny authored Linux C program. It contains no Chromium source or engine objects and cannot pass engine acceptance. The actual cold Windows browser build is a separate workflow.

The manual `Tiny Siso checkpoint research` workflow uses two fresh standard public Ubuntu runners and an exact immutable official Python toolchain container. The seed compiles one object from source. It sends a bounded checkpoint, matching complete first-party source and the unchanged project license to the second job. The second job creates fresh inputs, verifies the seed job's checkpoint digest, exact source/script/tool/compiler/container/path context, all source hashes and all output hashes before restoring timestamps. Host image fields inside container steps may be null; actual job headers retain the host versions separately from the pinned container filesystem. The download action also requires the GitHub artifact digest to match. Missing or changed provenance blocks resumption.

Native acceptance for this fixture requires:

- Seed: one actual compile action.
- Resume: exactly two actions, compiling the other object and linking; the program exits with its expected value 7.
- Unchanged invocation: zero actions and the same program result.
- Changed depfile header: exactly two actions and program result 8.
- Corrupted source or object with the same size and restored timestamp: rejected by the content guard.

The pinned Siso revision is `b521b230cb7c56c821235e498285cf8e548a73ee`, as selected by Chromium's current DEPS. The official package is `build/siso/linux-amd64`, not the obsolete guessed `infra/build/siso` path. Both transport ZIP and executable digests are checked before execution. The script records its exact source hash. No remote compiler or object cache is used.

Local primitive experiments with this version found that fresh source timestamps rebuild the unchanged tiny graph, and moving its absolute workspace also rebuilt all three actions. Copying the checkpoint back to its original path with matching timestamps reused completed work. Timestamp equality alone can miss same-size source/object corruption; the content guard is therefore mandatory. These are local observations, not cross-runner or full-engine evidence. The authored probe then passed partial resumption, no-op, header invalidation and corruption controls locally; hosted results must be retained separately before claiming the two-runner result.

The first hosted attempt, [43514165006](https://github.com/KiaroSama/Numera-Core-Browser/actions/runs/43514165006), passed its cold one-object seed and artifact digest verification but correctly blocked resumption on changed context. Job headers show Ubuntu images `20261111.370.1` and `20261111.127.1` despite using the same `ubuntu-24.04` label. The label alone cannot pin a build toolchain. The diagnosed follow-up pins the official Linux/amd64 `python:3.15.0-bookworm` image by manifest digest `934f4289019d6c3dc94d5e6341ff3a70c82f19bb118756eb03bdd700fe4b5c98`. Official OCI metadata binds it to docker-library/python revision `ce73ea7cebf01824125c30c20cd1acbde9b364ec`. This change applies only to the tiny Linux research fixture, not the Windows browser recipe.

The first container attempt, [43515163276](https://github.com/KiaroSama/Numera-Core-Browser/actions/runs/43515163276), pulled the pinned image but stopped before compilation because Git rejected the runner-owned bind mount under the container UID. The recipe scopes `safe.directory` to the exact authorized checkout on its two Git commands; it changes no global Git setting. That failure is a container orchestration defect, not evidence against checkpoint resumption or browser feasibility.

[Hosted run 43515347693](https://github.com/KiaroSama/Numera-Core-Browser/actions/runs/43515347693), at `9167829bc14220ae228141b99e8200d139eaedd3`, then passed both jobs and all native action-count, executable-result, dependency-invalidation and corruption controls. Its checkpoint was 440,378 bytes including complete matching source and license. All three downloaded artifact ZIP hashes, the inner checkpoint hash, restored member hashes and complete-source/license correspondence were independently verified. [Retained evidence](../evidence/siso-checkpoint-43515347693.json) records the exact context and limits. Both host job headers happened to report `20261111.370.1` on this passing run; no result across differing host versions is claimed.

Even a passing hosted fixture leaves full Chromium continuation unproven. Its transitive toolchain/SDK fingerprints, complete output/state inventory, Windows timestamp/path behavior, object storage size and free allowance, dependency invalidation across a real graph, cold bootstrap and final link/package fit still need measured evidence. The toy compiler-executable fingerprint does not cover the complete system toolchain. No Chromium checkpoint is uploaded and no paid storage limit is enabled by this experiment. GitHub's included per-repository cache allowance is 10 GB; a proposal cannot assume unlimited cache or paid overage.

Sources:

- https://chromium.googlesource.com/chromium/src/+/6dc9bc3bdaea5c35ed6d41fceab2f7a3a2227c4f/DEPS
- https://chromium.googlesource.com/chromium/src/+/6dc9bc3bdaea5c35ed6d41fceab2f7a3a2227c4f/docs/siso_tips.md
- https://chromium.googlesource.com/build/+/b521b230cb7c56c821235e498285cf8e548a73ee/siso/build/mtimecheck.go
- https://chromium.googlesource.com/build/+/b521b230cb7c56c821235e498285cf8e548a73ee/siso/hashfs/state.go
- https://github.com/actions/download-artifact/tree/9000827ccba6bdab643e8b6fd33ac0654aef8333
- https://github.com/docker-library/python/tree/ce73ea7cebf01824125c30c20cd1acbde9b364ec/3.15/bookworm
- https://docs.github.com/en/actions/using-workflows/caching-dependencies-to-speed-up-workflows#usage-limits-and-eviction-policy
