# FileSolutions release workflow

Complete source, version, documentation and workflow changes before the final main CI run. The existing Windows workflow is the required release gate. No repository branch protection or rulesets were configured when inspected; this change does not alter repository settings.

1. Update all version manifests, Cargo.lock, the installer fallback and release notes. Check the working tree and preserve unrelated changes. Do not tag an unfinished source/version state.
2. Push the final commit to main through the project's authorized review process. Wait for the Windows build and release run to finish successfully. PR validation tests the merge commit; it is not interchangeable with a main commit.
3. Only when publication is explicitly authorized, tag that exact successful main commit with its matching version. Do not push main and its tag simultaneously. Tags run promotion only and never compile. The gate rejects PR/manual/fork producers, failed or incomplete runs, expired/missing/ambiguous artifacts, wrong attempts and mismatched provenance or bytes.
4. The release job validates the fixed owner repository ID and workflow ID/path against GitHub, downloads the artifact by immutable ID, verifies the archive SHA-256 before extraction, then rechecks producer success/attempt and manifest fields. Version, target, pinned Rust version, build feature/profile, CRT and signing policy must match. Installer and checksum sizes/hashes must match. Artifact names include commit and attempt.
5. Only an API-confirmed absent release may be created. Existing releases, permission failures and service failures stop publication. Never replace assets. The job downloads published installer/checksum assets and compares their sizes/hashes to the trusted manifest. A post-publication failure must be investigated; rerunning cannot overwrite the release.
6. Report publication separately from installation. Installation, real backups/restores and application restarts need their own authorization. The workflow never performs them.

The current CI package is unsigned. Existing optional local signing hooks remain unchanged. A future signing policy must sign before final hashes/provenance, update the declared policy and verification together, and retain publisher approval/key protections. Do not relabel unsigned bytes as signed.

## Avoid duplicate work

`scripts/build.ps1` owns npm installation, version checks, guide generation, frontend compilation, Rust tests, production compilation, shell-DLL verification and packaging. Run it once locally if a local package is needed; then run the source audit, Edge tests and promotion regression tests. Do not run npm ci/frontend build/Rust tests again just before this script. CI remains the release authority; a local build is not a substitute.

Debug Rust tests and release/custom-protocol builds have incompatible profiles/features. Keep both. Edge tests are short and reuse the installed npm dependencies in the same job; splitting them into another Windows runner would add setup/install work. There are no deploy workflows or extra push triggers to remove. Main push and PR validation cover different commits and remain enabled. Manual dispatch is diagnostic and cannot produce promotable artifacts.

Rust cache keys include compiler/environment, Cargo manifests/lockfiles, static CRT/remap flags established before cache restore, the build script and npm lockfile. Workspace crates are excluded to rebuild embedded frontend/resources. Only successful main pushes save Rust caches; no failure saves. npm uses setup-node's lockfile cache and still runs npm ci. Cache hits only accelerate compilation/downloads; all gates still execute. Superseded nonrelease runs cancel within their ref; tags cannot be cancelled by this concurrency rule.

## Recovery and verification

Artifacts expire after 30 days. If no complete valid producer exists, stop and run/rerun main CI on the exact commit, with all gates enabled. Do not fall back to an older commit, a manual build, a PR artifact, or an installer found locally. Attempt-specific names prevent a partial rerun from silently promoting a prior attempt's package; rerun all producer jobs when needed. A tag pushed too early can be rerun after its exact main producer succeeds, provided no release already exists.

Use `node --test scripts/release-artifact.test.mjs` for positive and negative provenance/hash/API-path checks, actionlint for workflow syntax, and the actual PR CI once for changed workflow behavior. Do not repeat a green full suite without changed inputs or a specific unresolved failure. Compare GitHub job/step timings and cache-hit evidence before claiming a speed improvement.

The inspected baseline built commit 80f823b196ccdc8ea1942754548a9a70f1051008 twice: main run 42448133987 took 586 seconds; tag run 42448135003 took 763 seconds. Their package steps took 510 and 668 seconds respectively. Both npm restores missed; there was no Rust cache. Eliminating the tag package step removes that repeated work by design; a new end-to-end release time and warm-cache saving remain unmeasured until an authorized future release.
