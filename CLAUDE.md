# NoteCraft

## Git workflow (GitHub Flow)

- `master` is protected and always releasable. Never commit or push to it directly.
- Start every task on a new branch from an up-to-date `master`:
  `git switch master && git pull && git switch -c <type>/<short-name>`
  where `<type>` is `feat`, `fix`, `chore`, `docs` or `refactor`.
- One task per branch. Keep commits focused; history is squashed at merge.
- When the work is done and verified, push the branch and open a pull request with `gh pr create`
  (title = the squash commit message, body = what changed and how it was verified).
- Merge with `gh pr merge --squash --delete-branch` only when the user asks; then
  `git switch master && git pull` and remove the local branch.
- For parallel work, use a separate git worktree per branch.
