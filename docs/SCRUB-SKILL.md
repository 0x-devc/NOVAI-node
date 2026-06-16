# The scrub skill: pre-publish security audit

`/scrub` is the deterministic audit we run before anything leaves the repo. Where
the git hooks (see `docs/CONTRIBUTING-pii.md`) prevent new PII at commit time,
scrub reviews a whole scope (working tree, history, or specific files) against the
full category list. This document covers what it checks, how to read its
findings, and a one-line config change we should apply so it stops flagging
strings that are public on purpose.

The skill lives at `~/.claude/skills/scrub/`. It is a Python scanner (`scrub.py`)
fronted by a `scrub` shell wrapper, with a skill definition that auto-loads in a
session.

## What scrub checks

Scrub scans line by line and reports findings in these categories:

| Category | What it looks for |
|---|---|
| credential | AWS, GitHub, Anthropic, OpenAI, and PyPI tokens; private key blocks; and generic 32 to 64 character hex that sits near a secret keyword |
| personal | configured personal names; personal-provider emails (gmail, yahoo, outlook, icloud, hotmail, protonmail, me, mac); a university name; and age markers |
| infra | the specific known production IP and a hostname pattern; any other public IPv4 (private, loopback, and documentation ranges excluded); IPv6; and provider-plus-city pairings |
| retired-project | the configured retired sibling-project names (the `banned_strings_retired` list) |
| collaborator | the configured collaborator names (the `banned_strings_collaborators` list) |
| ai-assist | an AI co-author trailer or inline assistant-credit phrasing (allowed by policy, so suppressed under the recommended config below) |

The exact strings and patterns live in the config (see below). The scanner skips
its own ignore paths (lockfiles, build output, binaries, and the
`tests/golden_vectors`, fixtures, and test-data trees) so determinism-critical
fixtures are never flagged.

## Severity and exit codes

Every finding has a severity. The process exit code is the highest severity seen,
which is what lets a pre-push hook gate on it:

| Exit | Verdict | Meaning |
|---:|---|---|
| 0 | CLEAN | no findings, or LOW only |
| 1 | BLOCKED | at least one HIGH or CRITICAL |
| 2 | NEEDS_ACK | at least one MEDIUM, and no HIGH or CRITICAL |
| 3 | error | not a git repo, malformed config, and similar |

Default severities: real credentials, personal emails, and a combined personal
name are CRITICAL; a lone first name or surname, real public IPs, hostnames, and
the university name are HIGH; retired project names and collaborator names are
MEDIUM (AI-assist markers would also be MEDIUM, but are suppressed by the allowed
policy described below); a bare city name or unkeyworded hex is LOW.

## Config: where the rules live and how to override them

Three JSON layers merge, in order, with list values unioned and scalar values
overridden:

1. Defaults: `~/.claude/skills/scrub/config.default.json`.
2. User-global: `~/.claude/scrub-config.json` (optional; absent by default).
3. Per-project: `<repo>/.claude/scrub-config.json` (optional).

Never edit `config.default.json` to silence a real finding. Use a project or user
overlay, or an inline annotation:

- `# scrub:ignore` on its own line suppresses the next line.
- `# scrub:ignore-line` on the same line suppresses that line.
- Comment variants work too: `// scrub:ignore`, `-- scrub:ignore`,
  `/* scrub:ignore */`.

## How to run it

Scrub takes a scope. The three we care about:

**(a) The working tree, plus unpushed commits: the default.**

```
bash ~/.claude/skills/scrub/scrub
```

This is what `/scrub` runs with no flags. It scans modified and untracked files
plus any commits not yet pushed to upstream.

**(b) All history.**

```
bash ~/.claude/skills/scrub/scrub --all
```

Scans the working tree plus every commit reachable from all refs. Use this before
making a repo public, and pair it with the operator history greps (see "Reading
the findings" below) for the path and marker classes scrub does not cover.

**(c) A specific draft file before publishing.**

```
bash ~/.claude/skills/scrub/scrub --scan-files docs/blog-535004-consensus-deep-dive-DRAFT.md
```

`--scan-files` bypasses git scoping and scans the loose file directly, which is
exactly what you want for a blog draft or a release note before it goes out. Pass
more than one path to scan several at once.

Add `--json` to any of these for machine-readable output (the skill renders the
human report from it). `--last N` scans the working tree plus the last N commits.
`--print-hook` prints a ready-to-install pre-push hook template.

Scrub never edits files on its own. For working-tree findings the skill walks you
through fixes interactively. For findings already in committed history it prints a
`git filter-repo` plan and stops; it never rewrites history for you.

## Reading the findings: by-design hooks findings vs real findings

One class of finding is expected and must NOT be "fixed": anything whose path is
under `hooks/`.

The tracked hooks necessarily contain the canonical project email (the identity
guard falls back to it) and reference the markers they block. Scrub reports the
project email in `hooks/pre-commit` and `hooks/README.md` as a personal-email
finding. That is by design. Editing those files to silence scrub breaks the
identity guard.

A subtle point worth stating: the hooks store their marker patterns as fragments,
so a fixed-string scan does not actually match them. The only live hooks finding
is the project email, which the allowlist below clears. Measured counts confirm
the fragmented patterns contribute zero to a marker scan.

The precise, repo-wide check for the real leak classes is the set of operator
greps in the gitignored `docs/gate-rewrite-runbook.md`, which exclude `hooks/` so
the guards can never inflate a count. Treat scrub as the broad sweep and those
greps as the exact check. (That runbook is operator-only and gitignored because it
embeds the literals it maps; the canonical rule set it drives currently has 22
replacement mappings plus an explicit keep-list.)

## Recommended config change: apply this yourself

Two intended-public things surface as findings by default: the project email,
and (now that policy allows it) AI attribution. Both are handled by a small
user-global overlay. Create `~/.claude/scrub-config.json` with:

```json
{
  "allowed_emails": [
    "NOVAInetwork@protonmail.com"
  ],
  "ai_assistance_disclosure": "allow"
}
```

`allowed_emails` is a list key, so it unions with the existing allowlist
(`noreply@github.com` and `noreply@anthropic.com`) rather than replacing it. After
this, the canonical project email stops showing as a CRITICAL personal-email
finding. `ai_assistance_disclosure: "allow"` is a scalar that makes scrub skip
the AI co-author and inline-attribution patterns entirely, matching the project
policy that attribution may appear in the repo.

### On AI-vendor references

Both kinds of Anthropic reference are allowed in the repo:

- **Product strings** (`AnthropicClient`, `ANTHROPIC_API_KEY`,
  `ANTHROPIC_API_URL`, the `claude-sonnet-...` model id) were never flagged by
  scrub. They are real product code.
- **Attribution markers** (an AI co-author trailer, inline assistant-credit
  phrasing) are allowed by project policy. The commit hooks no longer block
  them, and with `ai_assistance_disclosure: "allow"` in the config above, scrub
  skips them too, so they no longer show up as MEDIUM findings.

This is a deliberate policy: attribution may appear in commits, commit messages,
and tracked files. The hooks and the scrub config were changed together so the
two layers agree. To reverse it later, set `ai_assistance_disclosure` back to
`"strip"` and restore the attribution needles in both hooks.

## Optional: the pre-push hook

`scrub --print-hook` prints a hook that scans the last 10 commits before each push
and blocks on HIGH or CRITICAL, prompting for an explicit acknowledgement on
MEDIUM. Install it with:

```
bash ~/.claude/skills/scrub/scrub --print-hook > .git/hooks/pre-push
chmod +x .git/hooks/pre-push
```

Bypass one push with `git push --no-verify`. This is separate from the tracked
`hooks/` guards: a `.git/hooks/pre-push` file is local and not tracked, so it is
one more thing a fresh clone does not inherit and you have to install per machine.
