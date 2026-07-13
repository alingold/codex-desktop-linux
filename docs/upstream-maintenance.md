# Maintaining A Published Fork

This fork follows two independent update streams:

1. The official Codex application in `Codex.dmg`. The update manager handles
   this locally, while `update-codex-hash.yml` and `upstream-build-app.yml`
   validate it in GitHub Actions.
2. Linux wrapper changes from `ilysenko/codex-desktop-linux`. The
   `upstream-sync.yml` workflow checks these every Monday and opens a review
   pull request when the fork is behind.

The weekly workflow deliberately does not merge its pull request. It merges
upstream into the stable `automation/upstream-sync` branch, reports conflicts
in the failed workflow summary without publishing a partial merge, and
dispatches both the full CI matrix and the build against the current upstream
DMG. A maintainer reviews and merges the pull request only after those checks
pass.

Scheduled workflows run only from the repository's default branch. After
publishing a fork, merge the initial Computer Use branch into `main`, enable
GitHub Actions for the fork, and allow workflows to create pull requests under
**Settings → Actions → General → Workflow permissions**. Keep branch protection
or a ruleset on `main` so automation cannot bypass review.

The same update can be requested manually from **Actions → Weekly upstream
sync → Run workflow**. If its summary reports a conflict, use a trusted
checkout:

```bash
git fetch origin
git fetch upstream
git switch main
git merge upstream/main
```

Resolve and test the merge locally, then push it through a normal reviewed pull
request. Do not enable auto-merge for generated upstream-sync pull requests.
