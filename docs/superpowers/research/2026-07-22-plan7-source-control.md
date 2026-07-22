# Plan 7 Research: Source-Control / PR UI Integration

**Date:** 2026-07-22
**Author:** research subagent (source-control inventory)
**Orca reference:** clone pinned at `v1.4.150-rc.0` (commit `b25c298`), paths below are relative to that clone's `src/`.
**Status:** research only — no Rust written. Scopes what a faithful Rust port of Orca's SCM/issue-tracker integrations needs, and names THE architecture decision the plan hinges on.

---

## §0 THE architecture decision — embed API clients vs shell out to vendor CLIs

**This is the fork the whole plan turns on, analogous to Plan 6's launch-model fork.**

### What Orca actually does: a deliberate MIX, split by service class

Orca does **not** pick one transport. It uses three, and the split is not accidental — it is even reflected in the settings UI, which groups integrations into "CLI" cards vs "token" cards.

| Class | Services | Transport | Evidence |
|---|---|---|---|
| **Vendor CLI** | GitHub, GitLab | shell out to `gh` / `glab` | `main/github/client.ts:232` `execFileAsync('gh', …)`; `main/gitlab/client.ts:74` `glabExecFileAsync(['api','user'])`, `:98` `glabExecFileAsync(['auth','status'])` |
| **Embedded HTTP (forge)** | Gitea, Bitbucket, Azure DevOps | `fetch()` REST client, Orca's own auth | `main/gitea/client.ts:91` `fetch(`; `main/bitbucket/client.ts:102` `fetch(`; `main/azure-devops/azure-devops-api-request.ts:84` `fetch(` |
| **Embedded HTTP (issue tracker)** | Jira, Linear | `fetch()` / GraphQL SDK | `main/jira/client.ts:369` `fetch(`; `main/linear/linear-sdk.ts:1` `import type { LinearClient } from '@linear/sdk'` (GraphQL) |

The settings UI encodes this split explicitly:
- `renderer/src/components/settings/cli-source-control-integration-cards.tsx` — GitHub card links to `cli.github.com` and shows `gh auth login` (`:107`, `:135`); GitLab card keys off `glab` preflight status (`:168`, `:186`).
- `renderer/src/components/settings/token-source-control-integration-cards.tsx` — Bitbucket "Pull requests and build statuses via Bitbucket Cloud API tokens" (`:30`), Azure DevOps "via Azure DevOps REST API tokens" (`:152`).

The single unifying abstraction is the **`ForgeProvider`** interface (`main/source-control/forge-provider.ts:60-72`): every forge (github/gitlab/bitbucket/azure-devops/gitea) implements the same `resolveRepository / getReviewForBranch / getReviewByNumber / createReview?` shape, and callers never see whether the body shells out or `fetch`es. `FORGE_PROVIDERS` (`:265-271`) is the registry; `getForgeProviderForRepository` probes each provider's `resolveRepository` to auto-detect which forge owns a remote (`:277-286`). **This provider-trait abstraction is the single most portable idea in the whole subsystem** and suaegi should copy it regardless of which transports it implements.

### Why the split exists (the reasoning to inherit)

- **`gh`/`glab` exist and are ubiquitous, and they solve auth for you.** GitHub is where the volume is; `gh` is already installed by most target users and already holds their credentials/enterprise hosts. Orca stores **zero** GitHub/GitLab secrets — auth is entirely delegated to `gh auth login` / `glab auth login` (see §1 auth column). That is a huge scope reduction.
- **Bitbucket/Azure/Gitea have no comparable ubiquitous CLI** (Bitbucket's is deprecated; `az`/`tea` are niche), so Orca embeds thin REST clients for them — but keeps them **read-mostly** and gated behind bring-your-own-token env vars (see §1). Bitbucket doesn't even support review creation (`forge-provider.ts:171` `supportsReviewCreation: false`).
- **Jira/Linear are not forges at all** — they're issue trackers with rich write APIs (create/update/comment/transition), so they get real embedded clients with proper credential storage (safeStorage keychain).

### The recommendation for suaegi (framed as the user's call)

**Recommend: shell out to `gh` for the GitHub milestone, mirroring suaegi's existing `git` shell-out, and defer every other forge.** Rationale:

- suaegi already shells out to `git` via `tokio::process` with a clean `GitRunner` (`crates/suaegi-git/src/runner.rs:60` `pub struct GitRunner`, `:182` `Command::new("git")`). A `GhRunner` is the same pattern with `Command::new("gh")` — near-zero new architecture, and it is exactly what Orca does for its #1 service.
- Shelling out to `gh` means **suaegi builds no HTTP client, no OAuth flow, and no secrets storage** for GitHub. `gh` owns the token, enterprise host routing, and re-auth. suaegi's persistence is a single plaintext JSON file with **no secrets story** (`crates/suaegi-core/src/persistence.rs` writes `serde_json::to_string_pretty`, `:189`) — embedding an API client would force suaegi to build keychain integration first (see §3), which is a large, platform-specific side quest.
- Embedding gives more control (no `gh` dependency, structured errors, works where `gh` isn't installed) but costs: an HTTP+JSON client, GitHub's REST **and** GraphQL surfaces (Orca uses both — `github/client.ts:3466` REST checks fallback, `:3607` GraphQL rollup), token acquisition/refresh, and a secrets vault. That is the bulk of a whole plan on its own, for a capability `gh` already provides.

**The tradeoff to put in front of the user:** shell-out inherits `gh`'s install/auth as a hard dependency and its human-oriented output/versioning as a parsing risk (Orca pins to `--json` and has fallbacks for older `gh` — `github/client.ts:1727` "older gh versions without --json support"). Embedding removes that dependency but is 3–5× the work and unblocks nothing users can't already do by typing `gh` in the terminal today. Given suaegi's whole thesis is "shell out, don't reimplement," **CLI shell-out is the coherent choice**; embedding should only win if suaegi later needs a forge with no usable CLI (Bitbucket/Azure), and even then only after a secrets-storage plan lands.

---

## §1 Per-service table

Every non-obvious claim cited `file:line` in the Orca clone.

### Source-control forges

| Service | Operations offered | Transport | Auth (where creds live) | UI surface | Worktree coupling |
|---|---|---|---|---|---|
| **GitHub** | The richest by far: create PR (`github/client.ts:1811` `createGitHubPullRequest`), PR-for-branch lookup (`:2883` `getPRForBranch`, `:2908` `getPRForBranchOutcome`), checks/CI status (`:3552` `getPRChecks`, `:3827` `getPRCheckDetails`, `:3929` `rerunPRChecks`), comments/reviews (`:4075` `getPRComments`, `:4438` `addPRReviewCommentReply`, `:4488` `addPRReviewComment`, `:4365` `resolveReviewThread`, `:4314` `setPRFileViewed`), **merge** (`:4552` `mergePR`, `:4602` `setPRAutoMerge`), state/metadata (`:4812` `updatePRState`, `:4935` `updatePRTitle`, `:4970` `updatePRDetails`), reviewers (`:4850` `requestPRReviewers`, `:4891` `removePRReviewers`), issues/work-items (`:1324` `listWorkItems`, `:1949` `getWorkItem`), auth probe (`:409` `getAuthenticatedViewer`) | **`gh` CLI** via `execFileAsync('gh', …)` (`:232`, `:412`, `:394`). Uses `gh pr …`, `gh api` (REST), `gh issue`, plus `gh api graphql` for rollups (`:3607`). Falls back across REST/GraphQL/`gh pr checks` (`:3466`, `:3540`, `:3640`) | Delegated to **`gh`'s own auth** — Orca stores nothing. On failure it tells the user "run gh auth login in this environment" (`:1682`). Enterprise (GHES) hosts resolved by whichever host `gh` is authenticated to (`forge-provider.ts:128-129`) | `renderer/src/components/PullRequestPage.tsx`; PR filter UI (`github/PRFilterDropdowns.tsx`, `PRFilterPickers.tsx`); markdown composer for comments (`github/GitHubMarkdownComposer.tsx`); merge-state helper (`github-pr-merge-state.ts`); reviewer display (`github-pr-reviewer-display.test.ts`); rate-limit pill (`github/github-rate-limit-display.tsx`); settings card (`settings/cli-source-control-integration-cards.tsx:135`) | Create targets the origin owning the head branch (`client.ts:1832` `getOriginGitHubApiRepository`); input carries `worktreePath`, `base`, `head`, `title`, `body`, `useTemplate` (`shared/hosted-review.ts:60-67`). Each worktree persists a `linkedGitHubPR`/`fallbackGitHubPR` number used to re-resolve the review (`shared/hosted-review.ts:45-46`, resolution in `forge-provider.ts:133-150`) |
| **GitLab** | MR lookup by branch/number (`gitlab/client.ts` `getMergeRequestForBranchOrThrow`, `getMergeRequest`), project slug resolve (`getProjectSlug`), create MR (`gitlab/merge-request-creation.ts` `createGitLabMergeRequest`), work-items/issues (`gitlab/issues.ts`), auth probe (`client.ts:74`) | **`glab` CLI** via `glabExecFileAsync([...])` (`:74`, `:98`); addresses REST through `glab api` (`:50` comment, `glabApiWithHeaders`) | Delegated to **`glab`'s own auth** (`glab auth status`, `client.ts:98`); Orca stores nothing | `renderer/src/components/gitlab/gitlab-rate-limit-display.tsx`; settings CLI card (`cli-source-control-integration-cards.tsx:168`, `:186`) | Same `ForgeProvider` shape (`forge-provider.ts:82-110`); worktree persists `linkedGitLabMR` (`shared/hosted-review.ts:47`) |
| **Gitea** | PR lookup by branch/number, repo slug resolve, **create PR** (`gitea/pull-request-creation.ts` `createGiteaPullRequest`); `supportsReviewCreation: true` (`forge-provider.ts:236`) | **Embedded HTTP** `fetch()` (`gitea/client.ts:91`) | **Env-var PAT only** — `ORCA_GITEA_TOKEN`, `ORCA_GITEA_API_BASE_URL` (`gitea/client.ts:55-58`). No keychain, no persisted credential file | Token-style settings card (integration cards); no dedicated PR page beyond the shared `ForgeProvider` path | `linkedGiteaPR` per worktree (`shared/hosted-review.ts:50`) |
| **Bitbucket** | PR lookup by branch/number only — **no creation** (`forge-provider.ts:171` `supportsReviewCreation: false`) | **Embedded HTTP** `fetch()` (`bitbucket/client.ts:102`) | **Env-var only** — `ORCA_BITBUCKET_ACCESS_TOKEN` (Bearer) or `ORCA_BITBUCKET_EMAIL`+`ORCA_BITBUCKET_API_TOKEN` (Basic) (`bitbucket/client.ts:49-62`). No keychain | Token settings card ("via Bitbucket Cloud API tokens", `token-source-control-integration-cards.tsx:30`) | `linkedBitbucketPR` per worktree (`shared/hosted-review.ts:48`) |
| **Azure DevOps** | PR lookup by branch/number, **create PR** (`azure-devops/pull-request-creation.ts` `createAzureDevOpsPullRequest`); `supportsReviewCreation: true` (`forge-provider.ts:203`) | **Embedded HTTP** `fetch()` (`azure-devops/azure-devops-api-request.ts:84`) | **Env-var PAT only** — `ORCA_AZURE_DEVOPS_TOKEN`/`_PAT` (Basic) or `ORCA_AZURE_DEVOPS_ACCESS_TOKEN` (Bearer) (`azure-devops-api-request.ts:29-46`). No keychain | Token settings card ("via Azure DevOps REST API tokens", `token-source-control-integration-cards.tsx:152`) | `linkedAzureDevOpsPR` per worktree (`shared/hosted-review.ts:49`) |

### Issue trackers (not forges — separate abstraction)

| Service | Operations offered | Transport | Auth (where creds live) | UI surface | Worktree/agent coupling |
|---|---|---|---|---|---|
| **Jira** | Full CRUD: `listIssues`/`searchIssues` (JQL) (`jira/issues.ts:380`,`:388`), `getIssue` (`:434`), `createIssue` (`:465`), `updateIssue` (`:511`), `addIssueComment` (`:569`), `getIssueComments` (`:610`), `listProjects`/`listIssueTypes`/`listCreateFields`/`listPriorities`/`listAssignableUsers`/`listTransitions` (`:641`–`:849`); multi-site `connect`/`disconnect`/`selectSite` (`jira/client.ts:504`,`:573`,`:587`) | **Embedded HTTP** `fetch()` (`jira/client.ts:369`) | **Persisted, encrypted** — PAT/OAuth token in a **safeStorage-encrypted credential file** per site (`jira/client.ts:132` `credentialFileHasContent(getTokenPath(siteId))`, `:520` "personal access token sent as Bearer"; `main/integration-credential-file.ts` uses `electron.safeStorage`) | Connect dialog (`jira-connect-dialog.tsx`), issue workspace (`JiraIssueWorkspace.tsx`), project picker (`jira-project-picker-filter`), issue list + sort (`task-page-jira-issue-list.tsx`, `task-page-jira-sort-controls.tsx`), ADF/markdown compose (`jira-create-adf.ts`), settings card (`settings/jira-integration-card.tsx`) | Issues surface as "task sources" a worktree/agent can be launched against (`task-project-source-combobox.tsx`, `task-source-provider-availability.test.ts`) |
| **Linear** | CRUD + agent-oriented variants: `searchIssues`/`listIssues` (`linear/issues.ts:812`,`:1059`), `getIssue` (`:710`), `createIssue`/`createIssueForAgent` (`:1091`,`:1158`), `updateIssue`/`…ForAgent` (`:1227`,`:1294`), `addIssueComment`/`…ForAgent` (`:1347`,`:1391`), `createIssueAttachment` (`:1429`), relations (`issue-relation-mutation.ts`), rich issue-context fan-out (`issue-context*.ts`); multi-workspace connect (`linear/client.ts:577`,`:612`) | **Embedded GraphQL** via `@linear/sdk` `LinearClient` (`linear/linear-sdk.ts:1`, `client.ts:511` `getClient`) | **Persisted, encrypted** — API key via `saveToken`/`loadToken` into safeStorage credential file per workspace (`linear/client.ts:346`,`:350`,`:403`; `hasStoredToken`→`getWorkspaceTokenPath`) | API-key dialog (`linear-api-key-dialog-state.ts`, `LinearIssueWorkspace.tsx`), rich issue editor (`LinearIssueMarkdownDescriptionEditor.tsx`, `LinearIssueTextEditor.tsx`), attribute filters (`linear-issue-attribute-filter-*.tsx`), scope/project surfaces (`linear-scope-selector.tsx`, `linear-project-view-surfaces.tsx`), state pills, settings card (`settings/task-tracker-integration-cards.tsx`), agent-skill install pane (`LinearAgentSkillPane.tsx`) | Same task-source model; `…ForAgent` operations exist specifically so a launched agent can create/update/comment on the issue it's working (`issues.ts:1158`,`:1294`,`:1391`) |

### Cross-cutting infrastructure worth noting

- **`ForgeProvider` trait + registry** — the portable core (`source-control/forge-provider.ts:60-292`).
- **Review mappers** normalize each forge's PR/MR shape into one `HostedReviewInfo` (`source-control/forge-review-mappers.ts`; `shared/hosted-review.ts:18-38`).
- **Creation eligibility** — a substantial gating layer decides whether "Create PR" should even be offered for a worktree (`source-control/hosted-review-creation-eligibility.test.ts`, 21k; `hosted-review-creation.ts`, 21k).
- **PR body templates** — reads repo `.github` PR template when `useTemplate` set (`source-control/pull-request-template.ts`; `github/client.ts:1867` `readPullRequestTemplate`).
- **Refresh coordinator / caching** — errors are surfaced (not swallowed to "no review") so a transient `gh`/`git` failure doesn't poison the sidebar cache (`forge-provider.ts:112-123`; `github/pr-refresh-coordinator.ts`).
- **Default-branch handling** — hides non-open reviews on the default branch (`source-control/repo-default-branch.ts`).

---

## §2 Prioritization & milestone breakdown

**Priority order (by user value / frequency / cost):**

1. **GitHub via `gh`** — #1 by a wide margin. It's the only forge with rich write ops in Orca (merge, reviews, auto-merge), the only one with a dedicated PR page, and the one suaegi users already reach for by typing `gh` in the terminal today (`docs/superpowers/specs/2026-07-20-suaegi-mvp-design.md:57`). Lowest cost too — mirrors the existing `git` shell-out.
2. **GitLab via `glab`** — same transport model as GitHub (vendor CLI, delegated auth), so it's cheap *once the `gh` pattern exists*, but far lower user frequency.
3. **Linear / Jira (issue trackers)** — high value for the "launch an agent against a ticket" workflow, but these are a **different subsystem** (embedded HTTP + **mandatory secrets storage**), not a forge. They should be their own plan, not folded into 7.
4. **Gitea / Bitbucket / Azure DevOps** — lowest frequency, embedded-HTTP transport, and Bitbucket can't even create PRs in Orca. Defer indefinitely; only revisit if a specific user needs one.

**Recommended milestones:**

- **7a — GitHub PR create + status via `gh`** (the MVP of this plan). A `GhRunner` (mirror of `GitRunner`), `gh auth status` preflight, "Create PR from this worktree's branch" (title/body/base/draft, optional repo PR template), and read-back of PR state + CI checks for the worktree's branch (`gh pr view --json`, `gh pr checks`). Add a `linked_github_pr: Option<u64>` field to the `Worktree` domain (see §3). This alone retires the "type `gh` in the terminal" workaround.
- **7b — GitHub PR interaction** (fast-follow, same runner): comments/reviews read, mark-file-viewed, merge / enable auto-merge, request reviewers. Higher UI cost (a PR panel), so it's worth splitting from 7a.
- **7c — GitLab via `glab`** (optional): reuse the 7a provider abstraction for MR create/status. Only if there's demand.
- **Separate plan — issue trackers (Linear/Jira)**: gated on suaegi having a secrets-storage story (§3). Do not couple to 7.
- **Not planned — Gitea/Bitbucket/Azure**: embedded HTTP, defer until a concrete need.

**Design 7a's provider layer as a trait from day one** (Rust equivalent of `ForgeProvider`) even though GitHub is the only impl — it's the one abstraction from Orca that pays off, and it keeps 7c/embedded-forge doors open without rework.

---

## §3 What suaegi lacks (concrete gaps)

1. **A `gh` runner.** suaegi has `GitRunner` (`crates/suaegi-git/src/runner.rs:60`, `:182` `Command::new("git")`) but nothing for `gh`. 7a needs a sibling `GhRunner` — same `tokio::process` shell-out, plus `gh` install/auth **preflight** (Orca gates on `gh auth status`; suaegi must detect "gh missing" and "gh not authenticated" and surface the "run `gh auth login`" message rather than failing opaquely — mirror `github/client.ts:1682`).
2. **`gh` output-parsing discipline.** Every read must use `gh … --json <fields>` and parse structured output; never scrape human text. Orca even carries fallbacks for `gh` versions predating `--json` (`github/client.ts:1727`). suaegi should pin a minimum `gh` version in preflight instead of carrying fallbacks.
3. **Worktree↔PR link in the domain + persistence.** `Worktree` (`crates/suaegi-core/src/domain.rs:41`) has `branch` but no PR linkage. Orca persists `linkedGitHubPR` per worktree and re-resolves the review from it (`shared/hosted-review.ts:45`). suaegi needs to add e.g. `linked_github_pr: Option<u64>` to `Worktree`, which flows through the single-JSON persistence (schema is versioned and forward/back-compatible — `crates/suaegi-core/src/persistence.rs:96-105`, and the domain already tolerates missing fields via `#[serde(default)]`, `domain.rs` tests at `:249-274`), so this is additive and low-risk.
4. **No secrets storage — the blocker for embedded clients / issue trackers.** suaegi persists a single **plaintext** JSON (`persistence.rs:189` `to_string_pretty`); there is no keychain integration. Orca's Jira/Linear tokens live in **safeStorage-encrypted credential files** (`main/integration-credential-file.ts`, `electron.safeStorage`). **The `gh` shell-out path sidesteps this entirely** (that's a core reason to pick it). But any embedded-forge or issue-tracker work must first add an OS-keychain story (e.g. a `keyring`-style crate on macOS/Windows/Linux). Flag this as a hard prerequisite for the Linear/Jira plan, and note that env-var-only (the Gitea/Bitbucket/Azure model, `bitbucket/client.ts:49`) is a lighter interim option if suaegi ever wants a token forge without building a vault.
5. **No PR/review UI panels.** Orca has `PullRequestPage.tsx`, PR filter/compose components, merge-state helpers, rate-limit pills, and per-provider settings cards. suaegi has none — 7a needs at minimum a "Create PR" dialog (title/body/base/draft) and a per-worktree PR-status indicator; 7b needs a PR panel. This is net-new iced UI (suaegi is the `suaegi-app` iced shell).
6. **No forge auto-detection.** Orca probes remotes through `getForgeProviderForRepository` (`forge-provider.ts:277`). For a GitHub-only 7a this can be as simple as "does `git remote get-url origin` look like GitHub / does `gh` claim the repo," but the plan should decide detection scope explicitly.
7. **No PR-body-template reader.** Orca reads the repo's PR template (`source-control/pull-request-template.ts`). Minor, but a nice parity touch for 7a's create flow.

---

## §4 Open questions / risks for the plan author

1. **`gh` as a hard dependency — acceptable?** 7a makes `gh` required for PR features (though the app still runs without it). Confirm that's the intended posture (it matches suaegi's git shell-out thesis and Orca's own GitHub model). If "must work without `gh`" is a requirement, the whole calculus flips toward embedding + a secrets vault — a much bigger plan.
2. **Enterprise / GHES routing.** Orca leans on `gh` already being authenticated to the enterprise host (`forge-provider.ts:128-129`). Confirm suaegi 7a inherits this "whatever host `gh` is logged into" behavior rather than trying to configure hosts itself.
3. **`gh` version skew & output stability.** Parsing `--json` output couples suaegi to `gh`'s schema. Decide: pin a minimum `gh` version in preflight (recommended) vs. carry fallbacks like Orca (`client.ts:1727`). Risk is low if `--json` fields are chosen conservatively.
4. **Async cancellation & timeouts.** `gh` calls (checks, merge) can hang; `GitRunner` already has `run_with_timeout` (`runner.rs:143`). Ensure `GhRunner` uses the same timeout discipline and that a stalled `gh` can't block the UI (Orca caps best-effort lookups, `client.ts:1641`).
5. **Error surfacing vs. cache poisoning.** Orca is careful to distinguish "no PR" from "lookup failed" so a transient error doesn't erase known PR state (`forge-provider.ts:112-123`). suaegi's per-worktree PR indicator must make the same distinction — decide the state model (found / none / unavailable) up front.
6. **Scope of 7a writes.** Does 7a stop at "create PR + show status," or also include merge? Merge/auto-merge (`client.ts:4552`,`:4602`) is high-value but raises the "destructive action confirmation" bar. Recommend 7a = create + status (read), 7b = merge + reviews.
7. **Issue trackers = separate plan?** Confirm Linear/Jira are explicitly out of Plan 7's scope (they need the §3.4 secrets vault first and are a different subsystem). If the "launch agent against a ticket" flow is a near-term goal, that dependency needs sequencing now.
8. **Does the `ForgeProvider` trait earn its keep at N=1?** Recommend yes (it's cheap and keeps 7c/embedded doors open), but the plan author should consciously accept a one-impl trait rather than have it flagged as premature abstraction in review.
