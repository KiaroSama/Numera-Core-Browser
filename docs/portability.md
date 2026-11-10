# Paths and packaging status

The requested default Windows project location is `G:\Program Files\Portable\Scripts\Numera-Core-Browser`, and the requested installed-program location is `G:\Program Files\Numera Core Browser`. These are user preferences, not assumptions about another machine or GitHub runner. There is no installer yet and no global PATH or PowerShell execution-policy mutation is performed.

For future portable runtime operation, explicit writable profile/data/cache grants are part of the contract. Current planning checks lexical absolute host paths without traversal and does not create them. Actual symlink/reparse-point authorization, read-only installation behavior, relocation, disk-full recovery, extraction safety and native database storage are unimplemented release requirements.

Keep engine source workspaces outside the portable/runtime directory. Chromium source builds require paths without spaces; `Build-Chromium.ps1` therefore requires an explicit absolute scratch source root. It never silently moves the user's project. GitHub jobs use their own runner scratch storage and do not assume a `G:` drive.

The CLI builds independently and has no display, engine-download, Node or Python runtime requirement. A source-built real browser GUI, per-engine packages, offline verification, install/repair/removal and actual platform smoke tests remain pending. Current source delivery has no commercial code signing, Authenticode, notarization, artifact attestation or trusted client-update signature claim.
