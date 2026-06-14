# Version-controlled git hooks

This directory holds tracked hooks so every clone can share the same commit
guards. Git does **not** clone hook configuration, so each clone must opt in
once after cloning:

```
git config core.hooksPath hooks
```

That points git at this directory instead of `.git/hooks`. Verify with:

```
git config --get core.hooksPath      # should print: hooks
```

## pre-commit

Runs two guards and aborts the commit (non-zero exit) on any failure.

**1. Identity guard.** The commit author and committer must equal the canonical
project identity. When `hook.userIdentity` is unset the hook falls back to that
identity, so a fresh clone is protected by default. To set your local identity:

```
git config --local user.name  "NOVAInetwork"
git config --local user.email "NOVAInetwork@protonmail.com"
```

You may override the expected value locally with
`git config hook.userIdentity "NOVAInetwork <NOVAInetwork@protonmail.com>"`.

**2. Content guard.** Scans only the **added** lines of the staged diff and
refuses any commit that introduces, in tracked content:

- local home directory paths
- a personal username or surname
- the production host IP address
- the two external service domains
- AI-assist attribution phrasing: author / by / co-author / prepared / frozen
  tags, tool names, the assistant service URL, generation phrasing, and the
  local-only instructions filename

It deliberately does **not** flag the `ai_service` product code: the API client
type, the API key/URL environment names, and the model id are real product
references, not attribution markers.

## Notes

- The hook assembles its match patterns from fragments, so this directory never
  stores the verbatim identifiers it blocks. That keeps the detector clean for
  later history-verification scans, and it excludes its own files from the scan
  for the same reason.
- It scans added lines only, so it stops new introductions without penalizing
  edits made near pre-existing content.
- To bypass the hook for a single, vetted commit: `git commit --no-verify`.
