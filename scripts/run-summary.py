#!/usr/bin/env python3
# SPDX-License-Identifier: AGPL-3.0-only
"""The run summary of client-build.yml, its "Run summary" job (GHA-09 of the 2026-10-27 CI audit):
what each job and step of a run took, from the jobs API, and the runner images and cache results
the jobs hand on as outputs.

    gh api --paginate repos/<repo>/actions/runs/<run>/jobs --jq '.jobs[]' > jobs.json
    NEEDS='<toJSON(needs) of the job>' python3 -I scripts/run-summary.py jobs.json

Prints Markdown, for $GITHUB_STEP_SUMMARY. Without NEEDS (a finished run looked at later from the
development machine) it gives the times only. The file holds the jobs as JSON values one after
another, or the API's pages ({"jobs": [...]}). Step names are printed as the API gives them, never
matched by prefix. The script never fails a run: what it cannot read becomes a line of the summary.

Offline tests: python3 -I -B -m unittest discover -s scripts -p 'test_store_workflows.py'
(RunSummaryTest).
"""
from __future__ import annotations

import datetime
import json
import os
import sys

# what the jobs' cache-hit outputs say: rust-cache's is true for an exact key only (a fallback or
# nothing gives false), the Bridge codegen job's for its generated files. That job's output is
# 'false' also when it failed before or while it generated them, so its line is for a job that
# succeeded only.
RUST_CACHE = {"true": "hit", "false": "miss or fallback"}
BRIDGE_CACHE = {"true": "from the cache", "false": "generated"}


def moment(text) -> datetime.datetime | None:
    """A time of the API ("2026-10-26T15:55:33Z"), or None."""
    if not isinstance(text, str) or not text:
        return None
    try:
        return datetime.datetime.fromisoformat(text.replace("Z", "+00:00"))
    except ValueError:
        return None


def minutes(start, end) -> str:
    began, ended = moment(start), moment(end)
    if began is None or ended is None or ended < began:
        return ""
    return f"{(ended - began).total_seconds() / 60:.1f}"


def cell(value) -> str:
    """A table cell: no line breaks, no column separators."""
    return " ".join(str(value).split()).replace("|", "\\|")


def read_jobs(text: str) -> list[dict]:
    """The jobs of the file: JSON values one after another, each a job or a page of jobs."""
    decoder, at, jobs = json.JSONDecoder(), 0, []
    while True:
        while at < len(text) and text[at].isspace():
            at += 1
        if at == len(text):
            return jobs
        value, at = decoder.raw_decode(text, at)
        for job in (value.get("jobs") or []) if isinstance(value, dict) and "jobs" in value else [value]:
            if isinstance(job, dict):
                jobs.append(job)


def result(item: dict) -> str:
    if item.get("status") != "completed":
        return str(item.get("status") or "").replace("_", " ")
    return str(item.get("conclusion") or "")


def times(jobs: list[dict]) -> list[str]:
    jobs = sorted(jobs, key=lambda j: (j.get("started_at") or "", j.get("name") or ""))
    starts = [moment(j.get("created_at")) or moment(j.get("started_at")) for j in jobs]
    starts = [s for s in starts if s is not None]
    first = min(starts) if starts else None
    ends = [moment(j.get("completed_at")) for j in jobs if j.get("status") == "completed"]
    ends = [e for e in ends if e is not None]
    lines = ["### Run times", ""]
    if first is not None and ends:
        lines += [f"From the first job's start to the last job's end: "
                  f"{(max(ends) - first).total_seconds() / 60:.1f} minutes.", ""]
    lines += ["| Job | Result | Start | Minutes | Runner |", "|---|---|---:|---:|---|"]
    for job in jobs:
        began = moment(job.get("started_at"))
        start = f"{(began - first).total_seconds() / 60:.1f}" if began is not None and first is not None else ""
        took = minutes(job.get("started_at"), job.get("completed_at")) if job.get("status") == "completed" else ""
        labels = ", ".join(str(label) for label in job.get("labels") or [])
        lines.append(f"| {cell(job.get('name', ''))} | {cell(result(job))} | {start} | {took} | {cell(labels)} |")
    for job in jobs:
        steps = [s for s in job.get("steps") or [] if isinstance(s, dict) and s.get("conclusion") != "skipped"]
        if not steps:
            continue
        took = minutes(job.get("started_at"), job.get("completed_at"))
        lines += ["", f"<details><summary>Steps of {cell(job.get('name', ''))}"
                      + (f" ({took} minutes)" if took else "") + "</summary>", "",
                  "| Step | Result | Minutes |", "|---|---|---:|"]
        for step in steps:
            lines.append(f"| {cell(step.get('name', ''))} | {cell(result(step))} | "
                         f"{minutes(step.get('started_at'), step.get('completed_at'))} |")
        lines += ["", "</details>"]
    return lines


def caches(needs: dict) -> list[str]:
    """The table of what the jobs report through their outputs (client-build.yml)."""
    rows = []
    for job, entry in needs.items():
        outputs = entry.get("outputs") if isinstance(entry, dict) else None
        if not isinstance(outputs, dict):
            continue
        image = outputs.get("image") or ""
        rust = RUST_CACHE.get(outputs.get("rust_cache") or "", "")
        restored, packages = outputs.get("vcpkg_restored") or "", outputs.get("vcpkg_packages") or ""
        vcpkg = f"{restored} of {packages}" if restored and packages else ""
        other = []
        if outputs.get("bridge_cache") and entry.get("result") == "success":
            other.append("bridge files " + BRIDGE_CACHE.get(outputs["bridge_cache"], outputs["bridge_cache"]))
        if outputs.get("disk_free"):
            other.append("disk left " + outputs["disk_free"])
        if image or rust or vcpkg or other:
            rows.append(f"| {cell(job)} | {cell(image)} | {cell(rust)} | {cell(vcpkg)} | {cell('; '.join(other))} |")
    if not rows:
        return []
    return ["### Runner images and caches", "",
            "| Job | Runner image | Rust cache | vcpkg packages from the cache | Other |",
            "|---|---|---|---|---|"] + rows


def main(argv: list[str], env=os.environ) -> int:
    if len(argv) != 1:
        print(__doc__.split("\n\n")[1], file=sys.stderr)
        return 2
    lines = []
    try:
        with open(argv[0], encoding="utf-8") as handle:
            lines += times(read_jobs(handle.read()))
    except (OSError, ValueError) as error:
        lines.append(f"The run's jobs could not be read: {cell(error)}")
    try:
        needs = json.loads(env.get("NEEDS") or "{}")
    except ValueError:
        needs = {}
    if isinstance(needs, dict) and caches(needs):
        lines += [""] + caches(needs)
    print("\n".join(lines))
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
