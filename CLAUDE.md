# CLAUDE.md

## Language Policy

- All code, comments, commit messages, and documentation must be written in English.

## Gemini API Development Guidelines

- Always refer to Google's official documentation when using the Gemini API:
  - API Reference: https://ai.google.dev/gemini-api/docs
  - Supported file types: https://ai.google.dev/gemini-api/docs/vision (images), https://ai.google.dev/gemini-api/docs/document-processing (documents)
  - Embedding API: https://ai.google.dev/gemini-api/docs/embeddings
- Default models: `gemini-3-flash-preview` (embedding: `gemini-embedding-001`, 1536 dimensions)
- Model IDs, supported formats, and parameters may change — always verify with the latest official docs before implementation
- Do not blindly trust LLM-generated information (supported formats, API specs, etc.) — validate against official documentation

## Development Environment Principles

- All tasks requiring package installation must be developed and tested using Docker
- Do not run pip install, npm install, etc. directly on the local environment — execute inside Docker containers
- All experiments, builds, and tests must be reproducible via Dockerfile or docker-compose
- Keep a single `.env` file at the project root; reference it from each docker-compose.yml via `env_file: ../../.env`

## Git Configuration

- All commits must use the local git config `user.name` and `user.email` for both author and committer. Verify with `git config user.name` and `git config user.email` before committing.
- The expected git `user.name` is `Yonghye Kwon`. If the local git config `user.name` does not match, you **MUST** ask the user to confirm their identity before the first commit or push in the session. Once confirmed, do not ask again for the rest of the session.

## Branching & PR Workflow

- **All changes MUST go through a PR** — never commit directly to `main`, including doc-only edits
- Branch naming: `<type>/<short-description>` (e.g., `feat/indexing-pipeline`, `fix/gemini-rate-limit`)
- One focused unit of work per branch. For existing PRs, push to that branch instead of creating a new PR.

**Worktree workflow (mandatory for PR branch changes):**
- Create: `git worktree add ../semantic-file-finder-<branch-name> -b <type>/<short-description>`
- Work and run all PR commands (`gh pr create`, `git push`, etc.) **from inside the worktree**, not the main repo
- Do NOT remove a worktree while your working directory is inside it — return to main repo first: `cd /Users/yhkwon/Documents/Projects/semantic-file-finder && git worktree remove ../semantic-file-finder-<branch-name>`
- Do NOT remove a worktree immediately after completing a task — only when starting a new task or user confirms
- `git checkout`/`git switch` may be used only for local-only inspection tasks (no PR changes)

### PR Merge Procedure

Follow all steps in order — do not skip any.

1. **Review PR description** — rewrite with `gh pr edit` if empty/lacking. Include what changed, why, key changes.
2. **Search related issues** — `gh issue list`, reference with "Related: #N" (no auto-close keywords unless instructed)
3. **Check conflicts** — if `main` advanced, use `git merge-tree` to check; rebase/merge to resolve if needed
4. **Wait for CI** — `gh pr checks <number> --watch`. If CI fails, do NOT merge.
5. **Final review** — `gh pr diff <number>`, check for debug code, hardcoded paths, secrets, unused imports. Mandatory even if CI is green.
6. **Merge** — `gh pr merge <number> --merge` (**NEVER** use `--delete-branch` — worktree still uses the branch)
7. **Update local main** — `cd /Users/yhkwon/Documents/Projects/semantic-file-finder && git pull`

## anytomd Integration Policy

- Use the `anytomd` Rust crate for document conversion (see PRD)
- If bugs, unsupported formats, or conversion quality issues are discovered while using anytomd, contribute improvements directly to the anytomd repository
  - anytomd repo: https://github.com/nicholasgasior/anytomd
  - File issues or submit PRs with fixes
  - Use a fork or git dependency for temporary integration until changes are merged upstream
- Always write tests alongside anytomd improvements and ensure existing tests are not broken

## Experiments Structure

- Organize experiments in sub-folders under `experiments/`
- Each experiment folder must be independently runnable (with its own Dockerfile, docker-compose.yml, requirements.txt)
- Example structure:
  ```
  experiments/
  ├── image-understanding/    # Gemini comprehension comparison for image-containing documents
  ├── embedding-quality/      # Embedding quality evaluation experiment
  └── ...
  ```
