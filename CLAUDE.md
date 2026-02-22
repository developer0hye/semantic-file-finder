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

## Rust Toolchain Policy

- Rust development follows stable toolchain only (no nightly features).
- Rust version baseline follows `anytomd-rs`: **Rust 1.90**.
- When this project introduces/updates Rust crates (`Cargo.toml`), set and keep:
  - `edition = "2024"`
  - `rust-version = "1.90"` (MSRV)
- Do not bump `rust-version` in unrelated PRs. If a bump is required, use a dedicated `chore` PR and document the reason.
- If Docker images define a Rust version (e.g., `RUST_VERSION` ARG), it must match `Cargo.toml` `rust-version` in the same commit.

## Rust Development Rules

- Production app code under Rust/Tauri paths must be implemented natively in Rust; avoid Python sidecars or subprocess-based core logic unless explicitly approved.
- Keep code `rustfmt`-compatible and `clippy`-clean (`-D warnings` for CI-grade checks).
- Prefer explicit error propagation (`Result`) over panics in application logic.
- Use structured error types (e.g., `thiserror`) for backend domain errors and IPC-facing error mapping.
- Prefer pure-Rust crates for core indexing/search/conversion pipeline dependencies.
- Minimize dependencies; do not add a new crate for logic that can be implemented clearly in a small local module.
- Before adding or upgrading a Rust dependency, verify the latest stable version on crates.io and check MSRV compatibility.

## Rust Testing and Verification — TDD Required

**TDD is mandatory for all Rust features and bug fixes:** write failing test first, then implement minimum code to pass, then refactor. No exceptions.

### TDD Workflow

1. **Red:** Write a test that describes the expected behavior. Run it — it must fail.
2. **Green:** Write the minimum code to make the test pass. No more, no less.
3. **Refactor:** Clean up the code while keeping all tests green.

### Test Requirements

- Every bug fix must include a regression test that reproduces the bug before the fix.
- Every public function and non-trivial private function must have at least one test.
- Every new module must include a `#[cfg(test)] mod tests` block with unit tests.
- Edge cases must be covered: empty input, malformed data, boundary values, Unicode/CJK text.

### Test Integrity — Non-Negotiable

- **NEVER** delete, modify, or `#[ignore]` passing tests to work around failures — fix the implementation instead.
- **NEVER** weaken assertions (e.g., changing `assert_eq!` to `assert!`) to make tests pass.
- Obsolete tests require documented justification in the commit message before removal.
- If a test is flaky, fix the root cause — do not disable it.

### Test Naming

Use descriptive names: `test_<what>_<condition>_<expected>` or `test_<what>_<scenario>`
(e.g., `test_normalize_path_windows_backslashes_converted`, `test_search_offline_fallback_to_keyword_only`)

### Integration Tests

- Place integration tests in `src-tauri/tests/` with fixtures in `src-tauri/tests/fixtures/`.
- Test end-to-end flows: file in → indexed → searchable → results returned.
- Use in-memory SQLite and temporary directories for isolation.
- Mock external services (Gemini API) using trait-based injection.

### Verification Loop

**Run after every code change in `src-tauri/src/` or `src-tauri/tests/`:**

```bash
cargo fmt --check
cargo clippy --all-targets --all-features -- -D warnings
cargo test --all-targets --all-features
cargo build --release
```

**Non-negotiable:** Do NOT proceed if any step fails — fix first, re-run, then continue. Never skip steps.

## CI — GitHub Actions

CI must pass on every push and PR. **Never merge code that breaks CI.**

### CI Pipeline (Required Checks)

All checks run on: `ubuntu-latest`, `macos-latest`, `windows-latest` with stable Rust matching `rust-version`.

```yaml
# Step order is mandatory — later steps depend on earlier ones passing
1. cargo fmt --check
2. cargo clippy --all-targets --all-features -- -D warnings
3. cargo test --all-targets --all-features
4. cargo build --release
5. npm run build  # Frontend TypeScript compilation + Vite build
```

### CI Rules

- **All 5 checks must pass** on all 3 OS targets before merging any PR.
- If CI fails, do NOT merge — fix the issue and push again.
- Clippy warnings are treated as errors (`-D warnings`). No `#[allow(...)]` without documented justification.
- Tests must be deterministic. Flaky tests are CI-blocking bugs — fix immediately.
- CI must complete within 15 minutes. If builds are slow, investigate and optimize.

### Gemini API CI Tests

Gemini API tests require a live API key and should be handled specially:

| Event | Runs? | Reason |
|-------|-------|--------|
| `push` (any branch) | Yes | Owner/collaborators only — trusted |
| `pull_request` (default) | No | External PRs — gated |
| `pull_request` + `ci:gemini` label | Yes | Owner explicitly approved after code review |

- `GEMINI_API_KEY` stored as GitHub Actions repository secret.
- Gemini test failures (rate limits, transient errors) must NOT block CI — allowed-to-fail.
- Gemini tests must use a lightweight model (`gemini-2.5-flash-lite`) for CI cost savings.
- Gemini tests should only assert structural correctness (non-empty response, valid JSON), not exact content (LLM output is non-deterministic).

### Pre-Merge Checklist (Enforced by CI + Manual Review)

Before merging any PR, verify:
- [ ] All CI checks green on all 3 OS targets
- [ ] No debug code (`dbg!`, `println!`, `console.log` for debugging)
- [ ] No hardcoded paths, secrets, or API keys
- [ ] No unused imports or dead code
- [ ] No `#[allow(...)]` or `#[ignore]` added without documented justification
- [ ] New code has corresponding tests
- [ ] Existing tests not weakened or removed

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
  - anytomd repo: https://github.com/developer0hye/anytomd-rs
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
