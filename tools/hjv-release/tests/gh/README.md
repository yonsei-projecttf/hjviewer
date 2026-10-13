# Recorded `gh` answers

What `gh` answers to the two questions `hjv-release policy` asks about a build
(src/provenance.rs), recorded from a finished run of the build repository and cut down to the
fields that matter, with ids and hashes replaced by test values:

- `run.json`: `gh api repos/<build repo>/actions/runs/4242`, a successful run of `build.yml`;
- `run-failed.json`, `run-other-workflow.json`, `run-other-commit.json`: the same answer for a
  run that failed, a run of another workflow, and a run that called `client-build.yml` at another
  commit than the one in `release.json`;
- `build.yml`: `gh api -H "Accept: application/vnd.github.raw+json"
  repos/<build repo>/contents/.github/workflows/build.yml?ref=<the run's commit>`, the documented
  file (docs/ci.md); `build-other-commit.yml`: one that pins another commit; `build-extra-job.yml`:
  one with a second job.

`1111111111111111111111111111111111111111` stands for the source commit: tests/cli.rs puts the
commit of its temporary repository there, and a small script named `gh` on the PATH replays the
files. `2222222222222222222222222222222222222222` is the commit of the build repository the run
ran at; `3333333333333333333333333333333333333333` is another commit of the public repository.
