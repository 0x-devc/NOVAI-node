# Releasing novai-sdk to PyPI

Releases are triggered by pushing a git tag matching `python-sdk-v*` to `main`. The tag push fires `.github/workflows/python-sdk-release.yml`, which re-runs the test/lint/type gates, verifies the tag version matches `pyproject.toml`, builds wheel + sdist, and uploads to PyPI using the `PYPI_API_TOKEN` repository secret.

## Prerequisites (one-time)

1. PyPI account at https://pypi.org/account/register/ with 2FA enabled.
2. PyPI API token scoped to the `novai-sdk` project (or "Entire account" for the first publish), generated at https://pypi.org/manage/account/token/.
3. Add the token to repo settings → Secrets and variables → Actions → `PYPI_API_TOKEN`.
4. (Recommended) Create a `pypi-release` environment at repo settings → Environments. Add a required reviewer so the publish step pauses for manual approval before uploading.

For TestPyPI dry-runs, repeat for https://test.pypi.org/. TestPyPI uploads are easiest done locally — see "Dry-run via TestPyPI" below.

## Release procedure

1. Confirm `main` is green:
   ```bash
   git checkout main && git pull
   gh run list --workflow="Python SDK" --limit 3
   ```
2. Bump `version` in `sdk/novai-python-sdk/pyproject.toml` and `__version__` in `sdk/novai-python-sdk/novai_sdk/__init__.py`. Add a new section to `CHANGELOG.md`. Commit and push.
3. (Recommended) Dry-run on TestPyPI — see below.
4. Tag and push:
   ```bash
   git tag python-sdk-vX.Y.Z
   git push origin python-sdk-vX.Y.Z
   ```
5. Monitor: `gh run watch`.
6. Verify the package at https://pypi.org/project/novai-sdk/X.Y.Z/ — README renders, classifiers display, deps correct.
7. End-to-end install test:
   ```bash
   python -m venv /tmp/novai-install-test && \
     /tmp/novai-install-test/bin/pip install novai-sdk==X.Y.Z && \
     /tmp/novai-install-test/bin/python -c "from novai_sdk import NOVAIClient, Keypair; print('OK')"
   ```

## Dry-run via TestPyPI

From inside `sdk/novai-python-sdk/`:
```bash
rm -rf dist/ build/
.venv/bin/python -m build
.venv/bin/python -m twine check dist/*
.venv/bin/python -m twine upload --repository testpypi dist/*
# username: __token__
# password: <TestPyPI token, including pypi- prefix>
```
Then verify rendering at https://test.pypi.org/project/novai-sdk/.

## Irreversibility notes

PyPI **never** lets you re-upload the same version number, even if you yank it. If the upload fails partway through, bump to the next patch version and tag again. Never reuse a tagged version number.

If a published version has a critical bug:
- Yank at https://pypi.org/manage/project/novai-sdk/release/X.Y.Z/ (hides from pip's default resolver; pinned installs still work).
- Cut a fix release X.Y.(Z+1).

## Tag naming convention

`python-sdk-v<MAJOR>.<MINOR>.<PATCH>` — e.g. `python-sdk-v0.1.0`. The `python-sdk-` prefix namespaces this from other release tags in the monorepo (Rust crates, TS SDK).
