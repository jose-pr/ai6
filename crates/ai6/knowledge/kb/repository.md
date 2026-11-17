---
title: What a repository is to AI6
kind: tradecraft
topics: [repository, operation, safe house, git, checkout]
summary: How AI6 treats a repository, as an Operation's checkout and the safe house a Mission works in.
---

# What a repository is to AI6

AI6 does not manage repositories. It runs work on one.

- An **Operation** is a registered project: its `path` is the checkout, and its policy says who
  may work on it. Operations are global; any Station that has the checkout may run its Missions.
- A **safe house** is where a Mission's Agent works: with the `git` provider, a worktree on a new
  branch cut from the Operation's checkout. Your edits and commits are in that worktree, not in
  the checkout.
- **Exfiltration** is how work leaves the safe house: the kept branch and its commits. An
  untouched safe house is removed when the Mission ends; one with changes is retained and
  recorded in the Mission's report.
- A push to a protected branch, a merge or a tag needs green ink: `request_approval` first.
- A Source (GitHub, GitLab, Jira) is where issues come from; it is separate from the checkout.

Work in your safe house, commit there, and say in your closing summary what changed and how it
was checked. Do not touch the Operation's checkout or another Mission's safe house.
