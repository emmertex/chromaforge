# Upstream PRs on watch

`WATCH` rows in [`ledger.md`](ledger.md) are the live list. This page keeps the rule.

**Rule:** when upstream approves a watched PR for merge (an approving review) or merges it, treat it as vetted by
upstream and port it without a fresh full review. It still goes through every gate in
[`docs/upstream-pr-intake.md`](../upstream-pr-intake.md) — clean-room, brand, never-crash, assets and regression
checks — and test on GitHub only. A PR that stays open and unapproved stays `WATCH`; one upstream closes without
merging becomes `SKIP` with the reason recorded.

Checked every 15 minutes while a release or merge round is running, otherwise in each intake round.

Set `UP` as in the runbook's preflight (`<owner>/<repo>` from the `upstream` remote).

```sh
gh pr view <n> --repo "$UP" --json state,isDraft,reviewDecision,mergedAt,headRefOid
```

Record the reviewed head SHA in the ledger row: a PR whose head moved after the review has to be read again.
