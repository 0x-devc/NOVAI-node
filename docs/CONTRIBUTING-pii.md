# Keeping PII and infrastructure detail out of this repo

## The principle, in one line

PII and infrastructure detail never enter the repo. Prevention happens at commit
time, and a history rewrite is a last resort, not a habit.

We enforce this with two tracked git hooks (commit-time prevention) and a scrub
audit skill (pre-publish review). This document covers what counts as PII, what
is public on purpose, the path convention that prevents a whole class of leaks,
how to turn the guards on, and how to prove they are live.

## What is PII: what must never be committed

Never commit, in tracked content or in a commit message:

- Real names of people, yours or anyone else's.
- Absolute home or root paths. The content guard blocks the macOS per-user home
  prefix, Linux `/home/<user>/` paths, and `/root/<path>` paths. Use the path
  convention below instead of hardcoding any of them.
- Server IP addresses: the production host address, or any other real public
  address.
- Hostnames and datacenter or hosting-provider labels. This includes a provider
  name paired with a city, machine hostnames, and monitor labels that embed the
  provider.
- Domains that belong to other projects.
- Secrets of any kind: API keys, access tokens, private key blocks, passwords,
  seeds.
- The local-only instructions filename (the gitignored per-developer notes
  file). Referencing it in tracked content points at a file that is not in the
  repo, so the content guard still blocks it.

AI-vendor attribution (Claude or Anthropic author, co-author, or credit lines)
is allowed by project policy. It is not on the list above: the commit hooks do
not block it, and the scrub audit does not flag it under the recommended config.

## What is public on purpose: what must NOT be scrubbed

Some strings look like findings but are public by design. Do not remove or
rewrite these:

- **The published project identity email:** `NOVAInetwork@protonmail.com`. This
  is the canonical git identity on every commit and is published with the
  project. The scrub skill currently reports it as a personal email; see
  `docs/SCRUB-SKILL.md` for the one-line allowlist that clears that false
  positive.
- **The product strings in the AI service code:** the client type names
  `AiClient` and the `AnthropicClient` alias, the provider names `Anthropic` and
  `OpenAiCompatible`, the environment variable names `ANTHROPIC_API_KEY`,
  `ANTHROPIC_API_URL`, and `OPENAI_API_KEY`, the Anthropic model id string (the
  `claude-sonnet-...` form), and the names of local OpenAI-compatible runtimes
  referenced in docs and examples (Ollama, vLLM, LM Studio, llama.cpp). These
  are real product references in `crates/ai_service`, not attribution markers,
  and the code does not work without them.
- **Determinism-critical constants:** genesis public keys, golden-vector hex,
  and test or example IP addresses. Scrubbing any of these changes a hash or a
  fixture and breaks the tests. Concretely this covers the documentation address
  ranges reserved for examples (192.0.2.0/24, 198.51.100.0/24, 203.0.113.0/24),
  loopback (127.0.0.0/8), and the private container ranges (10.0.0.0/8,
  172.16.0.0/12, 192.168.0.0/16).

Both AI-vendor product strings and AI-vendor attribution are allowed. Product
strings like `AnthropicClient` always passed because they are real code. AI
attribution (author, co-author, and credit lines) is allowed by project policy,
so the hooks no longer block it and the scrub audit no longer flags it.

## The path convention

Never hardcode an absolute user or root path in a script, a config, or a test.
Use `$HOME`, a repo-relative path, or a documented environment variable.

### Why this matters: a worked example

`scripts/testnet-server.sh` once defaulted its data directory to an absolute root
path. The history-rewrite rules redact root paths to a placeholder, so that
hardcoded default would have been rewritten into a literal placeholder string
that is not a real directory. That is the worst kind of bug: it builds and runs
on the machine where the original path happens to exist, then breaks on a fresh
checkout of the rewritten history, or on a server with a different layout,
because the script is now pointing at a path that was never meant to be read as a
real location.

The fix was a one-line default that carries no PII and needs no rewrite:

```
DATA_DIR="${DATA_DIR:-$HOME/.novai/data}"
```

`$HOME` is resolved per machine at runtime. It never contains a hardcoded user or
root literal for a path scrub to mangle, so the script is correct before and
after any rewrite, and on every machine.

The general rule: if a value would be changed by a future PII rewrite, it does
not belong hardcoded in tracked code. Resolve it at runtime instead.

This convention is load-bearing even though the tooling now backs it up. The
commit-time content guard blocks the macOS home prefix and now also blocks
`/root/<path>` and `/home/<user>/` paths (that gap is what let the data dir
above slip through before). The scrub audit still does not scan for filesystem
home paths at all, so it would not catch a stray root path on its own. Either
way, resolving paths at runtime with `$HOME` is the real fix: it leaves nothing
for a guard to catch or a rewrite to mangle.

## The enforcement layer: two git hooks

The repo ships two tracked hooks in `hooks/`. They are the commit-time prevention
layer.

### pre-commit

Runs two guards and aborts the commit on any failure:

1. **Identity guard.** The commit author and committer must equal the canonical
   project identity (`NOVAInetwork <NOVAInetwork@protonmail.com>`). When the
   local override is unset the hook falls back to that identity, so once the hook
   path is set, identity is protected by default.
2. **Content guard.** Scans only the ADDED lines of the staged diff and refuses
   any commit that introduces an absolute home or root path (the macOS prefix,
   `/home/<user>/`, or `/root/<path>`), a personal name, the production host IP,
   or an external project domain. It excludes its own files from the scan. AI
   attribution is allowed by policy and is not blocked, and the AI service
   product strings (client type, env var names, model id) pass as they always
   have.

### commit-msg

A pre-commit hook cannot see the commit message, so this companion hook scans the
proposed message and aborts on an em dash, en dash, figure dash, horizontal bar,
or minus sign (use the ASCII hyphen instead). AI attribution in a commit message
is allowed by policy and is not scanned.

Both hooks assemble their remaining match patterns from fragments, so the tracked
files never store the verbatim identifiers they block. Do not "tidy" those
fragments: joining them back together is exactly what re-introduces the leak and
breaks the guard.

### Turning the guards on, required after every clone

Git does NOT clone hook configuration. A fresh clone has these guards switched OFF
until you run, once, from the repo root:

```
git config core.hooksPath hooks
```

Verify it took:

```
git config --get core.hooksPath      # must print: hooks
```

This is a real failure mode, and it just bit us: if you skip this step, every
guard is silently off and a bad commit sails through with no warning. Make it the
first thing you do after cloning.

To bypass the guards for a single, vetted commit: `git commit --no-verify`.

## Contributor onboarding checklist

Run these once on every machine and every fresh clone:

1. Point git at the tracked hooks:

   ```
   git config core.hooksPath hooks
   ```

2. Confirm it took (must print `hooks`):

   ```
   git config --get core.hooksPath
   ```

3. Set your local identity to the project identity:

   ```
   git config --local user.name  "NOVAInetwork"
   git config --local user.email "NOVAInetwork@protonmail.com"
   ```

4. Prove the guards are live with deliberate commits that MUST be blocked:

   - **Identity guard.** Attempt an empty commit under a wrong identity. It must
     abort with an identity mismatch.

     ```
     git -c user.name="Wrong Name" -c user.email="wrong@example.com" \
         commit --allow-empty -m "identity probe"
     ```

   - **Content guard.** Create a scratch file whose single line contains an
     absolute root or home path: `/root/` followed by a real directory name, or
     `/home/` followed by a real username and a slash (use an actual name, not a
     `<placeholder>`). Stage it and attempt a commit. The content guard must
     abort and name the category. Delete the scratch file afterward.

   - **Message guard.** Attempt a commit whose message body contains an em dash
     (U+2014) instead of a hyphen. The commit-msg guard must abort. Redo with an
     ASCII hyphen.

   If any of these three is NOT blocked, your hooks are not active. Go back to
   step 1.

## When the hooks are not enough: the pre-publish audit

The hooks are commit-time prevention. They stop NEW introductions in the diff you
are about to commit. They are not a full-repository audit: they only see the
current staged diff, not the existing tree or history.

Before any outward-facing step, run the scrub skill as a deterministic audit of
the whole scope:

- before making the repo or a branch public,
- before publishing a blog post or any document drawn from the repo,
- before a release or a force-push.

Run it with `/scrub` in a session, or as a shell command. The full skill
reference, including how to scan all history and how to scan a single draft file
before you publish it, is in `docs/SCRUB-SKILL.md`.

Division of labor:

- **Hooks are commit-time prevention.** Per commit, on the added lines. Always on
  once the hook path is set. Cheap.
- **Scrub is the pre-publish audit.** On demand, across the working tree, all
  history, or specific files. Broader category coverage. Run it before you
  publish, not on every commit.

A history rewrite (the `git filter-repo` runbook the operator keeps) is the last
resort for leaks that already reached shared history. It is destructive and
outward-facing. Prevention at commit time is always cheaper than a rewrite, which
is the entire point of this document.
