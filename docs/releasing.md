# Releasing

1. Bump the version in `Cargo.toml` (and `Cargo.lock`, with `cargo check`), in the README line
   "This tree is version X.Y.Z", and add a `debian/changelog` entry `hushphone (X.Y.Z)`.
2. Open a pull request "Release X.Y.Z" and merge it.
3. On main: `git tag -a vX.Y.Z -m vX.Y.Z && git push origin vX.Y.Z`
4. The tag runs `.github/workflows/release.yml`: it tests, builds the Linux binary, the APKs
   and the debs twice each, checks the builds match, and publishes them with `SHA256SUMS` as
   a GitHub release. Check the release page.

Tag only after step 2: the workflow checks that the top `debian/changelog` entry names the
tag's version, and publishes nothing if it does not.
