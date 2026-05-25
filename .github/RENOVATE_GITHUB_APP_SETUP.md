# Switching Renovate from a Personal Access Token to a GitHub App

`.github/workflows/renovate.yml` currently authenticates Renovate with a
fine-grained Personal Access Token stored in the `RENOVATE_TOKEN` secret.
That works, but it has two known sharp edges:

- **Single point of failure.** The PAT is bound to one human GitHub
  account. If that account loses access (offboard, password rotation,
  PAT expiry, scopes revoked) the entire dependency-update pipeline
  stops with no warning.
- **No expiry hygiene.** Fine-grained PATs expire and have to be
  manually rotated. A missed rotation = silently broken Renovate runs.

A GitHub App installation token fixes both: it has no human account
behind it, it auto-renews on each workflow run, and its permissions can
be locked down per-installation rather than per-user.

This document is the runbook for that migration. Nothing in the runbook
is automated — GitHub App creation is a one-time UI flow.

---

## 1. Create the GitHub App

1. Open <https://github.com/settings/apps/new> (personal scope) or
   `https://github.com/organizations/<ORG>/settings/apps/new` (org
   scope). Use the org scope if this repo is org-owned and you want
   multiple repos to share one App.
2. **GitHub App name**: pick something specific, e.g.
   `rxxozqfoe-renovate`. App names are globally unique on GitHub.
3. **Homepage URL**: any URL is fine
   (e.g. <https://github.com/rxxozqfoe/rustdesk-server>).
4. **Webhook**: untick **Active**. Renovate self-hosted does not need
   webhooks because the workflow polls on a cron.
5. **Repository permissions** — set these to *Read and write* unless
   noted otherwise:

   | Permission       | Access         | Why                                                                      |
   | ---------------- | -------------- | ------------------------------------------------------------------------ |
   | Contents         | Read and write | Create branches, push commits.                                           |
   | Pull requests    | Read and write | Open / update / close Renovate PRs.                                      |
   | Workflows        | Read and write | Update `.github/workflows/*.yml` (GITHUB_TOKEN cannot do this).          |
   | Issues           | Read and write | Maintain the Dependency Dashboard issue.                                 |
   | Commit statuses  | Read and write | Set `renovate/stability-days` status used by `minimumReleaseAge`.        |
   | Metadata         | Read           | Required for everything else; GitHub forces this on.                     |

   Leave all other repository permissions at *No access*.
6. **Organization permissions**: leave at *No access* unless you also
   want Renovate to manage org-level secrets or members (you don't).
7. **Account permissions**: leave at *No access*.
8. **Where can this GitHub App be installed?**
   - *Only on this account* if you keep the App private.
   - *Any account* only if you intend to share it. Not recommended.
9. Click **Create GitHub App**.

---

## 2. Generate a private key

On the App page (Settings → Developer settings → GitHub Apps → your App):

1. Scroll to **Private keys** → **Generate a private key**.
2. A `.pem` file is downloaded immediately. Keep it; you cannot
   re-download it from the UI.

---

## 3. Install the App on this repository

1. On the App page, click **Install App** in the left sidebar.
2. Pick the account that owns `rxxozqfoe/rustdesk-server`.
3. Choose **Only select repositories** and tick
   `rxxozqfoe/rustdesk-server`.
4. Confirm install.
5. After install, the URL bar shows
   `https://github.com/settings/installations/<INSTALLATION_ID>`
   (or `https://github.com/organizations/<ORG>/settings/installations/<INSTALLATION_ID>`).
   Copy the numeric `INSTALLATION_ID`.

---

## 4. Record the three values

You now have three things you need:

| Value           | Where it comes from                                                  |
| --------------- | -------------------------------------------------------------------- |
| `APP_ID`        | App page, "About" section, numeric **App ID** field.                 |
| `INSTALLATION_ID` | The numeric ID from the install URL in step 3.5.                   |
| `APP_PRIVATE_KEY` | The contents of the `.pem` file from step 2.                       |

---

## 5. Store the values as repository secrets

Repository → Settings → Secrets and variables → Actions → **New
repository secret**, for each:

- `RENOVATE_APP_ID` ← `APP_ID` (numeric)
- `RENOVATE_APP_INSTALLATION_ID` ← `INSTALLATION_ID` (numeric)
- `RENOVATE_APP_PRIVATE_KEY` ← full `.pem` contents including the
  `-----BEGIN ...-----` / `-----END ...-----` lines

Do NOT delete the existing `RENOVATE_TOKEN` yet — keep it as a fallback
until the App-based run is green.

---

## 6. Switch `.github/workflows/renovate.yml` to use the App

Replace the **Run Renovate** step in `.github/workflows/renovate.yml`
with the snippet below. The official
[`actions/create-github-app-token`](https://github.com/actions/create-github-app-token)
action mints a short-lived installation token from the three secrets
above, and Renovate is given that token instead of the PAT.

```yaml
      - name: Mint Renovate installation token
        id: app-token
        uses: actions/create-github-app-token@<PIN_TO_SHA>  # v2 latest
        with:
          app-id: ${{ secrets.RENOVATE_APP_ID }}
          private-key: ${{ secrets.RENOVATE_APP_PRIVATE_KEY }}
          # Scope the token to this repo only. Without this, the token
          # would be scoped to every repo the App is installed in.
          owner: ${{ github.repository_owner }}
          repositories: ${{ github.event.repository.name }}

      - name: Run Renovate
        uses: renovatebot/github-action@3633cede7d4d4598438e654eac4a695e46004420 # v46.1.7
        with:
          configurationFile: renovate.json
          token: ${{ steps.app-token.outputs.token }}
        env:
          LOG_LEVEL: ${{ inputs.logLevel || 'info' }}
          RENOVATE_DRY_RUN: ${{ inputs.dryRun && 'full' || '' }}
          RENOVATE_REPOSITORIES: ${{ github.repository }}
```

Pin `actions/create-github-app-token` to a commit SHA the same way
the rest of the workflow does (Renovate will manage future bumps via
`helpers:pinGitHubActionDigestsToSemver`).

The token Renovate receives is automatically scoped to the
repositories you listed, expires after 1 hour, and is revoked at job
end. No PAT, no rotation reminder, no per-user surface.

---

## 7. Verify

1. Trigger Renovate manually:
   *Actions → Renovate → Run workflow* (with `dryRun: true` the first
   time).
2. Watch the run. The "Mint Renovate installation token" step should
   succeed; the "Run Renovate" step should pick up the same set of
   updates the PAT-based run would.
3. Once a non-dry-run is green and produces a PR, you can delete the
   `RENOVATE_TOKEN` secret.

---

## Rollback

If anything goes wrong, revert the workflow change (drop the
`Mint Renovate installation token` step, restore
`token: ${{ secrets.RENOVATE_TOKEN }}`). The PAT path keeps working as
long as you have not deleted the secret.

---

## Optional: install on more repos later

If you later want this App to manage another repo, install it from the
App page (step 3) onto that repo, and update the `repositories:` list
on the token step (or drop it to let the token cover every installed
repo).
