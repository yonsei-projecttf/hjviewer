// SPDX-License-Identifier: AGPL-3.0-only

//! Where a package came from (design 4-5): before a policy may name a new Windows or Android
//! package, the tool checks with GitHub and with this repository that
//!
//! 1. the run `release.json` names is a successful run of the build repository's `build.yml`,
//!    started by hand;
//! 2. GitHub recorded that the run called the public repository's `client-build.yml` at exactly
//!    `release.json`'s source commit, and nothing else;
//! 3. `build.yml`, as it was at the run's commit, is the documented file (docs/ci.md): one job
//!    that calls `client-build.yml` with `uses:` and `source_ref` pinned to that same 40-digit
//!    commit, no steps of its own, no other inputs;
//! 4. that commit is what the tag `client-<version>` points to in this repository;
//! 5. the package in the release directory and its `release.json` are, byte for byte, files of
//!    that run's artifacts: the tool downloads them itself (`gh run download <run>`) and looks
//!    for both by their SHA-256.
//!
//! So a changed `build.yml`, a commit that is not the reviewed one, another workflow or a failed
//! run all show up before anything is signed. `release.json` only says where to look; nothing in
//! it is believed before the fifth check has found it among what the reviewed build wrote.
//! Without that check a package from anywhere would pass under a `release.json` that names a
//! good run: whoever can write to the build repository could make one with another run. The
//! artifacts of a run are kept for a few days only (7, docs/ci.md), so the policy for a new
//! package is made while they are there.
//!
//! With `--attest` the package must also pass `gh attestation verify`, for the called workflow
//! at the reviewed commit.

use crate::release::{hash_file, is_commit, Artifact};
use std::{
    fs,
    os::unix::fs::DirBuilderExt,
    path::{Path, PathBuf},
    process::Command,
};

/// The inputs `client-build.yml` takes when called (`on.workflow_call.inputs`). Another input
/// could change what is built, so `build.yml` may pass only these.
pub const KNOWN_INPUTS: [&str; 5] = [
    "platforms",
    "testflight",
    "play",
    "play_release",
    "source_ref",
];

/// The programs the checks run. Tests replay recorded output instead.
pub trait Tools {
    /// `gh <args>`: the standard output of a successful run.
    fn gh(&self, args: &[&str]) -> Result<Vec<u8>, String>;
    /// `git -C <repo> <args>`: the standard output of a successful run.
    fn git(&self, repo: &Path, args: &[&str]) -> Result<Vec<u8>, String>;
}

/// The real `gh` and `git`.
pub struct SystemTools;

fn run(program: &str, command: &mut Command) -> Result<Vec<u8>, String> {
    let output = command
        .output()
        .map_err(|e| format!("{program} 명령을 실행하지 못했습니다({e})"))?;
    if output.status.success() {
        return Ok(output.stdout);
    }
    let stderr = String::from_utf8_lossy(&output.stderr);
    let first = stderr.lines().find(|l| !l.trim().is_empty()).unwrap_or("");
    let first: String = first.chars().take(200).collect();
    Err(format!(
        "{program} 명령이 실패했습니다({}): {first}",
        output.status
    ))
}

impl Tools for SystemTools {
    fn gh(&self, args: &[&str]) -> Result<Vec<u8>, String> {
        run("gh", Command::new("gh").args(args))
    }

    fn git(&self, repo: &Path, args: &[&str]) -> Result<Vec<u8>, String> {
        run("git", Command::new("git").arg("-C").arg(repo).args(args))
    }
}

#[derive(Clone, Debug)]
pub struct Settings {
    /// The private repository whose `build.yml` builds the apps, `owner/name`.
    pub build_repo: String,
    /// The public repository with the code and `client-build.yml`, `owner/name`.
    pub public_repo: String,
    pub build_workflow: String,
    pub called_workflow: String,
    /// A clone of the public repository, where the `client-<version>` tags are.
    pub repo_dir: PathBuf,
    /// Inputs of `client-build.yml` newer than this tool, allowed by the operator.
    pub extra_inputs: Vec<String>,
    /// Also require `gh attestation verify` (build provenance, if the plan has it).
    pub attest: bool,
}

impl Settings {
    pub fn new(repo_dir: PathBuf) -> Settings {
        Settings {
            build_repo: "yonsei-projecttf/hjviewer-private".to_owned(),
            public_repo: "yonsei-projecttf/hjviewer".to_owned(),
            build_workflow: ".github/workflows/build.yml".to_owned(),
            called_workflow: ".github/workflows/client-build.yml".to_owned(),
            repo_dir,
            extra_inputs: Vec::new(),
            attest: false,
        }
    }

    pub fn check(&self) -> Result<(), String> {
        let repo_ok = |s: &str| {
            let mut parts = s.split('/');
            let part_ok = |p: Option<&str>| {
                p.is_some_and(|p| {
                    !p.is_empty()
                        && p.len() <= 100
                        && !p.starts_with('.')
                        && p.bytes()
                            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b'-'))
                })
            };
            part_ok(parts.next()) && part_ok(parts.next()) && parts.next().is_none()
        };
        let path_ok = |s: &str| {
            s.starts_with(".github/workflows/")
                && s.ends_with(".yml")
                && !s.contains("..")
                && s.bytes()
                    .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b'-' | b'/'))
        };
        if !repo_ok(&self.build_repo) || !repo_ok(&self.public_repo) {
            return Err("저장소 이름은 owner/name 형식입니다".to_owned());
        }
        if !path_ok(&self.build_workflow) || !path_ok(&self.called_workflow) {
            return Err("워크플로 경로는 .github/workflows/<이름>.yml 형식입니다".to_owned());
        }
        Ok(())
    }
}

/// What was checked, for the operator and for the person who signs.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Proof {
    pub lines: Vec<String>,
}

/// The most files the artifacts of one run may have (a build has a few dozen).
const MAX_RUN_FILES: usize = 10_000;

/// The files of runs' artifacts, as GitHub hands them out: size and SHA-256 of each. A run is
/// downloaded once, however many of its packages are checked.
#[derive(Debug, Default)]
pub struct Runs {
    seen: Vec<Run>,
}

#[derive(Debug)]
struct Run {
    build_repo: String,
    id: String,
    /// Size and SHA-256 of every file of the run's artifacts.
    files: Vec<(u64, String)>,
}

/// A directory of this program's own under the system's temporary directory, for nobody else to
/// read (the packages are builds for the real server), removed when this drops.
struct Scratch(PathBuf);

impl Scratch {
    fn new() -> Result<Scratch, String> {
        let mut random = [0u8; 8];
        getrandom::getrandom(&mut random)
            .map_err(|_| "시스템에서 난수를 얻지 못했습니다".to_owned())?;
        let path = std::env::temp_dir().join(format!(
            "hjv-release-run-{}-{}",
            std::process::id(),
            crate::trust::hex(&random)
        ));
        // made here and now: a directory (or a link) that is there already is not used
        fs::DirBuilder::new()
            .mode(0o700)
            .create(&path)
            .map_err(|e| format!("{}: 만들 수 없습니다({e})", path.display()))?;
        Ok(Scratch(path))
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

/// Size and SHA-256 of every file under `dir`. Links are not followed and not counted.
fn files_of(dir: &Path, out: &mut Vec<(u64, String)>) -> Result<(), String> {
    let unreadable = |e: std::io::Error| format!("{}: 읽을 수 없습니다({e})", dir.display());
    for entry in fs::read_dir(dir).map_err(unreadable)? {
        let path = entry.map_err(unreadable)?.path();
        let meta = fs::symlink_metadata(&path)
            .map_err(|e| format!("{}: 읽을 수 없습니다({e})", path.display()))?;
        if meta.is_dir() {
            files_of(&path, out)?;
        } else if meta.is_file() {
            if out.len() >= MAX_RUN_FILES {
                return Err(format!("산출물의 파일이 {MAX_RUN_FILES}개를 넘습니다"));
            }
            out.push(hash_file(&path)?);
        }
    }
    Ok(())
}

impl Runs {
    /// The files of a run's artifacts: downloaded with `gh run download`, hashed and removed.
    fn files(
        &mut self,
        run_id: &str,
        settings: &Settings,
        tools: &dyn Tools,
    ) -> Result<&[(u64, String)], String> {
        let seen = self
            .seen
            .iter()
            .position(|run| run.build_repo == settings.build_repo && run.id == run_id);
        let at = match seen {
            Some(at) => at,
            None => {
                let scratch = Scratch::new()?;
                let dir = scratch
                    .0
                    .to_str()
                    .ok_or("임시 디렉터리의 경로가 UTF-8이 아닙니다")?;
                tools
                    .gh(&[
                        "run",
                        "download",
                        run_id,
                        "-R",
                        &settings.build_repo,
                        "-D",
                        dir,
                    ])
                    .map_err(|e| {
                        format!(
                            "실행 {run_id}의 산출물을 받지 못했습니다: {e}. 산출물은 보관 기간(7일)이 \
                             지나면 지워집니다. 지워졌다면 그 빌드로는 정책을 만들 수 없습니다\
                             (새 버전으로 다시 빌드하세요)"
                        )
                    })?;
                let mut files = Vec::new();
                files_of(&scratch.0, &mut files)?;
                self.seen.push(Run {
                    build_repo: settings.build_repo.clone(),
                    id: run_id.to_owned(),
                    files,
                });
                self.seen.len() - 1
            }
        };
        Ok(&self.seen[at].files)
    }
}

fn text(v: &serde_json::Value, key: &str) -> String {
    v.get(key).and_then(|x| x.as_str()).unwrap_or("").to_owned()
}

/// The commit the tag `client-<version>` points to in the local clone.
fn tag_commit(tag: &str, settings: &Settings, tools: &dyn Tools) -> Result<String, String> {
    let reference = format!("refs/tags/{tag}^{{commit}}");
    let out = tools
        .git(&settings.repo_dir, &["rev-parse", "--verify", "--quiet", &reference])
        .map_err(|_| {
            format!(
                "태그가 없습니다: {tag}({}). 검토하고 올린 커밋에 태그를 먼저 만드세요(git tag -a {tag} <커밋>)",
                settings.repo_dir.display()
            )
        })?;
    let commit = String::from_utf8_lossy(&out).trim().to_owned();
    if !is_commit(&commit) {
        return Err(format!("태그 {tag}의 커밋을 읽지 못했습니다"));
    }
    Ok(commit)
}

/// Runs the checks for one artifact. `runs` holds the runs downloaded so far.
pub fn check(
    artifact: &Artifact,
    settings: &Settings,
    tools: &dyn Tools,
    runs: &mut Runs,
) -> Result<Proof, String> {
    settings.check()?;
    let info = &artifact.info;
    let what = format!("{} {}", info.platform.name(), info.version);
    let fail = |why: String| format!("출처 확인 실패({what}): {why}");

    // 4. the reviewed commit, by this repository's tag
    let tag = format!("client-{}", info.version);
    let tagged = tag_commit(&tag, settings, tools).map_err(fail)?;
    if tagged != info.source_sha {
        return Err(fail(format!(
            "release.json의 소스 커밋({})이 태그 {tag}의 커밋({tagged})과 다릅니다",
            info.source_sha
        )));
    }

    // 1. the run
    let run_path = format!("repos/{}/actions/runs/{}", settings.build_repo, info.run_id);
    let run = tools.gh(&["api", &run_path]).map_err(fail)?;
    let run: serde_json::Value = serde_json::from_slice(&run)
        .map_err(|_| fail("gh가 준 실행 정보가 JSON이 아닙니다".to_owned()))?;
    let repository = run
        .get("repository")
        .map(|r| text(r, "full_name"))
        .unwrap_or_default();
    if repository != settings.build_repo {
        return Err(fail(format!(
            "실행 {}: 빌드 저장소 {}의 실행이 아닙니다({repository:?})",
            info.run_id, settings.build_repo
        )));
    }
    let path = text(&run, "path");
    if path != settings.build_workflow {
        return Err(fail(format!(
            "실행 {}: {} 말고 다른 워크플로의 실행입니다({path:?})",
            info.run_id, settings.build_workflow
        )));
    }
    let (status, conclusion) = (text(&run, "status"), text(&run, "conclusion"));
    if status != "completed" || conclusion != "success" {
        return Err(fail(format!(
            "실행 {}: 성공으로 끝나지 않았습니다(상태 {status:?}, 결과 {conclusion:?})",
            info.run_id
        )));
    }
    let event = text(&run, "event");
    if event != "workflow_dispatch" {
        return Err(fail(format!(
            "실행 {}: 손으로 시작한 실행(workflow_dispatch)이 아닙니다({event:?})",
            info.run_id
        )));
    }
    let head_sha = text(&run, "head_sha");
    if !is_commit(&head_sha) {
        return Err(fail("실행의 커밋(head_sha)을 읽지 못했습니다".to_owned()));
    }
    let claimed = &info.workflow;
    let claimed_ref = format!("{}/{}@", settings.build_repo, settings.build_workflow);
    if *claimed != settings.build_workflow && !claimed.starts_with(&claimed_ref) {
        return Err(fail(format!(
            "release.json의 workflow 값({claimed:?})이 실행의 워크플로({})와 다릅니다",
            settings.build_workflow
        )));
    }

    // 2. what GitHub recorded the run called
    let pinned = format!(
        "{}/{}@{}",
        settings.public_repo, settings.called_workflow, info.source_sha
    );
    let referenced = run
        .get("referenced_workflows")
        .and_then(|r| r.as_array())
        .cloned()
        .unwrap_or_default();
    let all_pinned = !referenced.is_empty()
        && referenced
            .iter()
            .all(|r| text(r, "path") == pinned && text(r, "sha") == info.source_sha);
    if !all_pinned || referenced.len() != 1 {
        let seen: Vec<String> = referenced.iter().map(|r| text(r, "path")).collect();
        return Err(fail(format!(
            "실행이 부른 워크플로가 {pinned} 하나가 아닙니다({seen:?})"
        )));
    }

    // 3. build.yml as it was at the run's commit
    let file_path = format!(
        "repos/{}/contents/{}?ref={head_sha}",
        settings.build_repo, settings.build_workflow
    );
    let file = tools
        .gh(&[
            "api",
            "-H",
            "Accept: application/vnd.github.raw+json",
            &file_path,
        ])
        .map_err(fail)?;
    let file = String::from_utf8(file).map_err(|_| {
        fail(format!(
            "워크플로 파일({})이 텍스트가 아닙니다",
            settings.build_workflow
        ))
    })?;
    check_build_yml(
        &file,
        &settings.public_repo,
        &settings.called_workflow,
        &info.source_sha,
        &settings.extra_inputs,
    )
    .map_err(|e| fail(format!("{}({head_sha}): {e}", settings.build_workflow)))?;

    // 5. the bytes: the package is a file of that run's artifacts, and so is the release.json
    // read here. The package's size and hash are those of the file on disk (release.rs compared
    // them with release.json's when it was read). With release.json itself the build's, what it
    // says besides (the platform, the version, the build number) is the build's word too, not
    // that of whoever put the directory together.
    let files = runs.files(&info.run_id, settings, tools).map_err(fail)?;
    let again = format!(
        "gh run download {} -R {}로 다시 받으세요",
        info.run_id, settings.build_repo
    );
    if !files
        .iter()
        .any(|(size, sha256)| *size == info.size && *sha256 == info.sha256)
    {
        return Err(fail(format!(
            "패키지 {}(sha256 {})가 실행 {}의 산출물에 없습니다. 릴리스 디렉터리의 파일은 그 실행이 \
             만든 것이 아닙니다({again})",
            info.file, info.sha256, info.run_id
        )));
    }
    if !files
        .iter()
        .any(|(_, sha256)| *sha256 == artifact.release_json_sha256)
    {
        return Err(fail(format!(
            "release.json(sha256 {})이 실행 {}의 산출물에 없습니다. 패키지는 그 실행의 것이지만 \
             release.json은 그 실행이 쓴 파일이 아닙니다({again})",
            artifact.release_json_sha256, info.run_id
        )));
    }

    let mut lines = vec![
        format!(
            "{what}: 실행 {} 성공({} @ {}), 소스 {} = 태그 {tag}",
            info.run_id,
            settings.build_workflow,
            &head_sha[..12],
            info.source_sha
        ),
        format!(
            "{what}: 패키지와 release.json이 실행 {}의 산출물과 같음(패키지 sha256 {})",
            info.run_id, info.sha256
        ),
    ];
    if settings.attest {
        let signer = format!("{}/{}", settings.public_repo, settings.called_workflow);
        let file = artifact.path();
        let file = file
            .to_str()
            .ok_or_else(|| fail("패키지 경로가 UTF-8이 아닙니다".to_owned()))?;
        // the attestation is the called workflow's, at the reviewed commit and no other
        tools
            .gh(&[
                "attestation",
                "verify",
                file,
                "--repo",
                &settings.build_repo,
                "--signer-workflow",
                &signer,
                "--signer-digest",
                &info.source_sha,
            ])
            .map_err(|e| fail(format!("출처 증명(gh attestation verify): {e}")))?;
        lines.push(format!(
            "{what}: 출처 증명 확인함(gh attestation verify, 서명자 커밋 {})",
            info.source_sha
        ));
    }
    Ok(Proof { lines })
}

/// A line of YAML without its comment and trailing spaces.
fn without_comment(line: &str) -> &str {
    let line = line.strip_suffix('\r').unwrap_or(line);
    if line.trim_start().starts_with('#') {
        return "";
    }
    match line.find(" #") {
        Some(at) => line[..at].trim_end(),
        None => line.trim_end(),
    }
}

/// `key:` or `key: value` after the indentation.
fn key_value(content: &str) -> Option<(&str, &str)> {
    let (key, value) = content.split_once(':')?;
    let key_ok = !key.is_empty()
        && key
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'_' | b'-'));
    let value_ok = value.is_empty() || value.starts_with(' ');
    (key_ok && value_ok).then(|| (key, value.trim()))
}

/// `${{ inputs.<name> }}`.
fn is_input_expression(value: &str) -> bool {
    value
        .strip_prefix("${{")
        .and_then(|v| v.strip_suffix("}}"))
        .map(str::trim)
        .and_then(|v| v.strip_prefix("inputs."))
        .is_some_and(|name| {
            !name.is_empty() && name.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_')
        })
}

fn is_plain(value: &str) -> bool {
    !value.is_empty()
        && value
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'_' | b'-' | b'.'))
}

/// Checks that `text` is the build repository's documented `build.yml` (docs/ci.md): a workflow
/// whose one job calls `<public_repo>/<called_workflow>` at `source_sha` with `source_ref` set
/// to the same commit, and that can run nothing else.
///
/// The file is read as lines, which is sound only for plain YAML, so everything that could make
/// a line mean something else is refused: quotes, block scalars, anchors, aliases, tags,
/// documents, tabs and brackets left open at the end of a line.
pub fn check_build_yml(
    text: &str,
    public_repo: &str,
    called_workflow: &str,
    source_sha: &str,
    extra_inputs: &[String],
) -> Result<(), String> {
    if !is_commit(source_sha) {
        return Err("소스 커밋이 40자리가 아닙니다".to_owned());
    }
    let uses = format!("{public_repo}/{called_workflow}@{source_sha}");
    // (line number, indentation, text) of every line with content
    let mut lines = Vec::new();
    for (n, raw) in text.lines().enumerate() {
        let content = without_comment(raw);
        if content.is_empty() {
            continue;
        }
        let at = |why: &str| format!("{}번째 줄: {why}", n + 1);
        let allowed = |c: char| c.is_ascii_alphanumeric() || " _.,:@/${}[]-".contains(c);
        if !content.chars().all(allowed) {
            return Err(at(
                "쓸 수 없는 문자가 있습니다(따옴표, 탭, |, >, &, *, ! 등)",
            ));
        }
        let open = |a: char, b: char| content.matches(a).count() != content.matches(b).count();
        if open('{', '}') || open('[', ']') {
            return Err(at("괄호가 줄 안에서 닫히지 않습니다"));
        }
        let indent = content.len() - content.trim_start().len();
        let body = content.trim_start();
        if body.starts_with("---")
            || body.starts_with("...")
            || body.starts_with("- ")
            || body == "-"
        {
            return Err(at("목록이나 문서 구분선은 쓸 수 없습니다"));
        }
        lines.push((n + 1, indent, body));
    }

    let mut section = "";
    let mut jobs_seen = false;
    let mut job_lines: Vec<(usize, usize, &str, &str)> = Vec::new();
    for (n, indent, body) in &lines {
        let at = |why: String| format!("{n}번째 줄: {why}");
        if *indent == 0 {
            let (key, value) =
                key_value(body).ok_or_else(|| at("이름: 값 형식이 아닙니다".to_owned()))?;
            if ![
                "name",
                "run-name",
                "on",
                "permissions",
                "concurrency",
                "jobs",
            ]
            .contains(&key)
            {
                return Err(at(format!("맨 위에 둘 수 없는 항목 {key:?}")));
            }
            if key == "jobs" {
                if jobs_seen || !value.is_empty() {
                    return Err(at("jobs는 한 번, 값 없이 써야 합니다".to_owned()));
                }
                jobs_seen = true;
            }
            section = key;
        } else if section == "jobs" {
            let (key, value) =
                key_value(body).ok_or_else(|| at("이름: 값 형식이 아닙니다".to_owned()))?;
            job_lines.push((*n, *indent, key, value));
        } else if section.is_empty() {
            return Err(at("들여쓴 줄로 시작합니다".to_owned()));
        }
    }

    // exactly one job, and of it only what calls the pinned workflow
    let job_indent = job_lines
        .first()
        .map(|l| l.1)
        .ok_or("jobs에 job이 없습니다")?;
    let key_indent = job_lines.get(1).map(|l| l.1).ok_or("job이 비어 있습니다")?;
    if key_indent <= job_indent {
        return Err("job이 하나가 아닙니다(다른 job이 무엇이든 실행할 수 있습니다)".to_owned());
    }
    let (mut uses_seen, mut source_ref_seen, mut parent) = (false, false, "");
    let mut job_keys: Vec<&str> = Vec::new();
    for (i, (n, indent, key, value)) in job_lines.iter().enumerate() {
        let at = |why: String| format!("{n}번째 줄: {why}");
        if i == 0 {
            if !value.is_empty() {
                return Err(at("job 이름 뒤에 값이 있습니다".to_owned()));
            }
        } else if *indent == job_indent {
            return Err(at(
                "job이 하나가 아닙니다(다른 job이 무엇이든 실행할 수 있습니다)".to_owned(),
            ));
        } else if *indent == key_indent {
            if job_keys.contains(key) {
                return Err(at(format!("{key}: 두 번 나옵니다")));
            }
            job_keys.push(key);
            parent = key;
            match *key {
                "uses" if *value == uses => uses_seen = true,
                "uses" => {
                    return Err(at(format!(
                        "uses가 검토한 커밋으로 고정되어 있지 않습니다(있어야 할 값: {uses})"
                    )))
                }
                "with" | "permissions" if value.is_empty() => {}
                "secrets" if *value == "inherit" => {}
                "name" | "permissions" if is_plain(value) => {}
                _ => return Err(at(format!("job에 둘 수 없는 항목입니다: {key}"))),
            }
        } else if *indent > key_indent && parent == "with" {
            let known = KNOWN_INPUTS.contains(key) || extra_inputs.iter().any(|e| e == key);
            if !known {
                return Err(at(format!(
                    "client-build.yml에 넘길 수 없는 입력입니다: {key}(새 입력이면 --allow-input {key})"
                )));
            }
            if *key == "source_ref" {
                if *value != source_sha {
                    return Err(at(format!(
                        "source_ref가 검토한 커밋으로 고정되어 있지 않습니다(있어야 할 값: {source_sha})"
                    )));
                }
                if source_ref_seen {
                    return Err(at("source_ref가 두 번 나옵니다".to_owned()));
                }
                source_ref_seen = true;
            } else if !is_input_expression(value) && !is_plain(value) {
                return Err(at(format!("입력 {key}의 값을 이해할 수 없습니다")));
            }
        } else if *indent > key_indent && parent == "permissions" {
            if !is_plain(value) {
                return Err(at("permissions의 값을 이해할 수 없습니다".to_owned()));
            }
        } else {
            return Err(at("들여쓰기를 이해할 수 없습니다".to_owned()));
        }
    }
    if !uses_seen {
        return Err(format!(
            "job이 검토한 워크플로({public_repo}/{called_workflow})를 부르지 않습니다"
        ));
    }
    if !source_ref_seen {
        return Err("with에 source_ref가 없습니다".to_owned());
    }
    Ok(())
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::release::tests::{temp_dir, write_artifact, SOURCE_SHA};
    use std::cell::RefCell;

    pub(crate) const HEAD_SHA: &str = "2222222222222222222222222222222222222222";
    const PUBLIC: &str = "yonsei-projecttf/hjviewer";
    const CALLED: &str = ".github/workflows/client-build.yml";

    /// The documented build.yml (docs/ci.md), pinned to `sha` in both places.
    pub(crate) fn build_yml(sha: &str) -> String {
        format!(
            "# Builds the HJViewer apps from the public repository with this repository's secrets.\n\
             name: build\n\
             on:\n\
             \x20 workflow_dispatch:\n\
             \x20   inputs:\n\
             \x20     platforms: {{ type: choice, options: [windows, android, ios, all], default: all }}\n\
             \x20     testflight: {{ type: boolean, default: false }}\n\
             \x20     play: {{ type: boolean, default: false }}\n\
             \x20     play_release: {{ type: choice, options: [completed, draft, validate], default: completed }}\n\
             permissions:\n\
             \x20 contents: read\n\
             jobs:\n\
             \x20 client:\n\
             \x20   # The reviewed public commit, as the full 40-character SHA, in both places.\n\
             \x20   uses: {PUBLIC}/{CALLED}@{sha}\n\
             \x20   with:\n\
             \x20     platforms: ${{{{ inputs.platforms }}}}\n\
             \x20     testflight: ${{{{ inputs.testflight }}}}\n\
             \x20     play: ${{{{ inputs.play }}}}\n\
             \x20     play_release: ${{{{ inputs.play_release }}}}\n\
             \x20     source_ref: {sha}\n\
             \x20   secrets: inherit\n"
        )
    }

    /// What `gh api repos/<repo>/actions/runs/<id>` answers, cut down to the fields of a real
    /// answer that matter here (recorded from a finished run, values replaced).
    pub(crate) fn run_json(change: impl Fn(&mut serde_json::Value)) -> String {
        let mut run = serde_json::json!({
            "id": 4242,
            "name": "build",
            "path": ".github/workflows/build.yml",
            "event": "workflow_dispatch",
            "status": "completed",
            "conclusion": "success",
            "head_branch": "main",
            "head_sha": HEAD_SHA,
            "run_attempt": 1,
            "run_number": 7,
            "referenced_workflows": [{
                "path": format!("{PUBLIC}/{CALLED}@{SOURCE_SHA}"),
                "sha": SOURCE_SHA
            }],
            "repository": {"full_name": "yonsei-projecttf/hjviewer-private", "private": true},
            "head_repository": {"full_name": "yonsei-projecttf/hjviewer-private"}
        });
        change(&mut run);
        run.to_string()
    }

    /// Replays recorded answers and notes what was asked.
    pub(crate) struct Recorded {
        pub run: String,
        pub build_yml: String,
        pub tag: Option<String>,
        pub attestation: Result<(), String>,
        /// What run 4242 built: the files `gh run download 4242` unpacks (path and bytes).
        pub built: Vec<(String, Vec<u8>)>,
        /// Or why the download fails.
        pub download: Result<(), String>,
        pub asked: RefCell<Vec<String>>,
        /// The permission bits of the directories the downloads went to.
        pub download_modes: RefCell<Vec<u32>>,
    }

    impl Recorded {
        /// The answers about a good run. It built nothing yet: see [`Recorded::building`].
        pub(crate) fn good() -> Recorded {
            Recorded {
                run: run_json(|_| {}),
                build_yml: build_yml(SOURCE_SHA),
                tag: Some(SOURCE_SHA.to_owned()),
                attestation: Ok(()),
                built: Vec::new(),
                download: Ok(()),
                asked: RefCell::new(Vec::new()),
                download_modes: RefCell::new(Vec::new()),
            }
        }

        /// The same, with the packages of `artifacts` and their release.json files (as they
        /// are on disk now) among what the run built, each artifact in a directory of its name.
        pub(crate) fn building(mut self, artifacts: &[Artifact]) -> Recorded {
            for artifact in artifacts {
                let name = artifact.dir.file_name().unwrap().to_str().unwrap();
                for file in [artifact.info.file.as_str(), crate::release::RELEASE_JSON] {
                    let bytes = fs::read(artifact.dir.join(file)).unwrap();
                    self.built.push((format!("{name}/{file}"), bytes));
                }
            }
            self
        }
    }

    impl Tools for Recorded {
        fn gh(&self, args: &[&str]) -> Result<Vec<u8>, String> {
            self.asked
                .borrow_mut()
                .push(format!("gh {}", args.join(" ")));
            match args {
                ["api", path] if *path == "repos/yonsei-projecttf/hjviewer-private/actions/runs/4242" => {
                    Ok(self.run.clone().into_bytes())
                }
                ["api", "-H", "Accept: application/vnd.github.raw+json", path]
                    if *path
                        == format!(
                            "repos/yonsei-projecttf/hjviewer-private/contents/.github/workflows/build.yml?ref={HEAD_SHA}"
                        ) =>
                {
                    Ok(self.build_yml.clone().into_bytes())
                }
                ["attestation", "verify", _, "--repo", "yonsei-projecttf/hjviewer-private", "--signer-workflow", signer, "--signer-digest", SOURCE_SHA]
                    if *signer == format!("{PUBLIC}/{CALLED}") =>
                {
                    self.attestation.clone().map(|_| Vec::new())
                }
                ["run", "download", "4242", "-R", "yonsei-projecttf/hjviewer-private", "-D", dir] =>
                {
                    use std::os::unix::fs::PermissionsExt;
                    self.download.clone()?;
                    let dir = Path::new(dir);
                    let mode = fs::metadata(dir).unwrap().permissions().mode() & 0o7777;
                    self.download_modes.borrow_mut().push(mode);
                    assert_eq!(fs::read_dir(dir).unwrap().count(), 0, "a directory of its own");
                    // as gh does: every artifact unpacked into a directory of its name
                    for (path, bytes) in &self.built {
                        let file = dir.join(path);
                        fs::create_dir_all(file.parent().unwrap()).unwrap();
                        fs::write(file, bytes).unwrap();
                    }
                    Ok(Vec::new())
                }
                _ => Err(format!("gh: HTTP 404 ({})", args.join(" "))),
            }
        }

        fn git(&self, _: &Path, args: &[&str]) -> Result<Vec<u8>, String> {
            self.asked
                .borrow_mut()
                .push(format!("git {}", args.join(" ")));
            // every client-<version> tag points to the recorded commit
            match (args, &self.tag) {
                (["rev-parse", "--verify", "--quiet", reference], Some(sha))
                    if reference.starts_with("refs/tags/client-")
                        && reference.ends_with("^{commit}") =>
                {
                    Ok(format!("{sha}\n").into_bytes())
                }
                _ => Err("git: exit status 1".to_owned()),
            }
        }
    }

    fn artifact(name: &str) -> (Artifact, PathBuf) {
        let root = temp_dir(name);
        write_artifact(&root, "windows", "1.6.0", "1.6.0.37", b"windows package");
        let mut found = crate::release::scan(std::slice::from_ref(&root)).unwrap();
        (found.remove(0), root)
    }

    fn settings() -> Settings {
        Settings::new(PathBuf::from("."))
    }

    /// The check of the test artifact against `recorded`, whose run built that artifact.
    fn refused(name: &str, recorded: Recorded) -> String {
        let (a, root) = artifact(name);
        let recorded = recorded.building(std::slice::from_ref(&a));
        let err = check(&a, &settings(), &recorded, &mut Runs::default()).unwrap_err();
        std::fs::remove_dir_all(root).unwrap();
        assert!(err.starts_with("출처 확인 실패(windows 1.6.0)"), "{err}");
        err
    }

    #[test]
    fn a_documented_build_passes() {
        let (a, root) = artifact("prov-good");
        let good = || Recorded::good().building(std::slice::from_ref(&a));
        let recorded = good();
        let proof = check(&a, &settings(), &recorded, &mut Runs::default()).unwrap();
        assert_eq!(proof.lines.len(), 2);
        assert!(proof.lines[0].contains("실행 4242 성공"));
        assert!(proof.lines[0].contains(SOURCE_SHA));
        assert_eq!(
            proof.lines[1],
            format!(
                "windows 1.6.0: 패키지와 release.json이 실행 4242의 산출물과 같음(패키지 sha256 {})",
                a.info.sha256
            )
        );
        // the lines go into the draft's notes, which keep a rule of their own
        crate::draft::check_notes(&proof.lines).unwrap();
        let asked = recorded.asked.borrow();
        assert_eq!(
            asked[..3],
            [
                "git rev-parse --verify --quiet refs/tags/client-1.6.0^{commit}".to_owned(),
                "gh api repos/yonsei-projecttf/hjviewer-private/actions/runs/4242".to_owned(),
                format!("gh api -H Accept: application/vnd.github.raw+json repos/yonsei-projecttf/hjviewer-private/contents/.github/workflows/build.yml?ref={HEAD_SHA}"),
            ]
        );
        // the run's artifacts: downloaded last, into a directory of the tool's own that is gone
        // again afterwards
        assert_eq!(asked.len(), 4);
        let download = asked[3]
            .strip_prefix("gh run download 4242 -R yonsei-projecttf/hjviewer-private -D ")
            .unwrap();
        assert!(Path::new(download).starts_with(std::env::temp_dir()));
        assert!(!Path::new(download).exists());
        assert_eq!(*recorded.download_modes.borrow(), [0o700]);
        // with the attestation, which is asked for the reviewed commit
        let with = Settings {
            attest: true,
            ..settings()
        };
        let recorded = good();
        let proof = check(&a, &with, &recorded, &mut Runs::default()).unwrap();
        assert_eq!(proof.lines.len(), 3);
        assert!(proof.lines[2].contains(&format!("서명자 커밋 {SOURCE_SHA}")));
        crate::draft::check_notes(&proof.lines).unwrap();
        assert!(recorded.asked.borrow()[4].ends_with(&format!(
            "--repo yonsei-projecttf/hjviewer-private --signer-workflow {PUBLIC}/{CALLED} --signer-digest {SOURCE_SHA}"
        )));
        let failing = Recorded {
            attestation: Err("no attestations found".to_owned()),
            ..good()
        };
        assert!(check(&a, &with, &failing, &mut Runs::default())
            .unwrap_err()
            .contains("출처 증명"));
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn the_package_is_a_file_of_that_run() {
        let (a, root) = artifact("prov-bytes");
        let package = a.path();
        let good = Recorded::good().building(std::slice::from_ref(&a));
        fs::copy(a.dir.join("release.json"), root.join("release.json.kept")).unwrap();
        // Another build's package under a release.json that names the good run: the bytes
        // changed, and release.json's size and hash with them, so release.rs is content.
        // Whoever can start runs in the build repository can make such a directory.
        let other = b"not what run 4242 built";
        fs::write(&package, other).unwrap();
        let json = root.join("HJViewer-1.6.0-1111111-windows-x86_64/release.json");
        fs::write(
            &json,
            crate::release::tests::release_json(
                "windows",
                "1.6.0",
                "1.6.0.37",
                "HJViewer.exe",
                other,
            ),
        )
        .unwrap();
        let swapped = crate::release::scan(std::slice::from_ref(&root))
            .unwrap()
            .remove(0);
        assert_ne!(swapped.info.sha256, a.info.sha256);
        let err = check(&swapped, &settings(), &good, &mut Runs::default()).unwrap_err();
        assert!(err.starts_with("출처 확인 실패(windows 1.6.0)"), "{err}");
        assert!(
            err.contains(&format!(
                "패키지 HJViewer.exe(sha256 {})가 실행 4242의 산출물에 없습니다",
                swapped.info.sha256
            )),
            "{err}"
        );
        assert!(err.contains("gh run download 4242 -R yonsei-projecttf/hjviewer-private"));
        // the same size with other bytes is another file as well
        let same_size = Recorded {
            built: vec![(
                "a/HJViewer.exe".to_owned(),
                b"not what run 4242 BUILT".to_vec(),
            )],
            ..Recorded::good()
        };
        assert!(
            check(&swapped, &settings(), &same_size, &mut Runs::default())
                .unwrap_err()
                .contains("산출물에 없습니다")
        );
        // a run that built nothing of the kind, and one whose artifacts are gone
        assert!(check(
            &swapped,
            &settings(),
            &Recorded::good(),
            &mut Runs::default()
        )
        .unwrap_err()
        .contains("산출물에 없습니다"));
        let expired = Recorded {
            download: Err("no valid artifacts found to download".to_owned()),
            ..Recorded::good()
        };
        let err = check(&swapped, &settings(), &expired, &mut Runs::default()).unwrap_err();
        assert!(
            err.contains("실행 4242의 산출물을 받지 못했습니다: no valid artifacts found"),
            "{err}"
        );
        assert!(err.contains("보관 기간(7일)"), "{err}");
        // what the run did build passes, wherever the files lie in the download
        let swapped_json = fs::read(&json).unwrap();
        let elsewhere = Recorded {
            built: vec![
                ("a/notes.txt".to_owned(), b"something else".to_vec()),
                ("b/deep/HJViewer-1.6.0.exe".to_owned(), other.to_vec()),
                ("c/release.json".to_owned(), swapped_json.clone()),
            ],
            ..Recorded::good()
        };
        assert!(check(&swapped, &settings(), &elsewhere, &mut Runs::default()).is_ok());
        // The run's own package under a release.json the run did not write: the bytes are the
        // build's, what is said about them (here the build number) is somebody else's.
        fs::write(&package, b"windows package").unwrap();
        let honest = fs::read_to_string(root.join("release.json.kept")).unwrap();
        let edited = honest.replacen("\"build\":\"1.6.0.37\"", "\"build\":\"1.6.0.99\"", 1);
        assert_ne!(edited, honest);
        fs::write(&json, &edited).unwrap();
        let relabelled = crate::release::scan(std::slice::from_ref(&root))
            .unwrap()
            .remove(0);
        assert_eq!(relabelled.info.sha256, a.info.sha256);
        assert_eq!(relabelled.info.build, "1.6.0.99");
        let err = check(&relabelled, &settings(), &good, &mut Runs::default()).unwrap_err();
        assert!(
            err.contains(&format!(
                "release.json(sha256 {})이 실행 4242의 산출물에 없습니다",
                relabelled.release_json_sha256
            )),
            "{err}"
        );
        // a run that has the package and no release.json at all is no better
        let no_json = Recorded {
            built: vec![("a/HJViewer.exe".to_owned(), b"windows package".to_vec())],
            ..Recorded::good()
        };
        assert!(
            check(&relabelled, &settings(), &no_json, &mut Runs::default())
                .unwrap_err()
                .contains("release.json(sha256")
        );
        // and with the run's own release.json back in place, all is well again
        fs::write(&json, &honest).unwrap();
        let restored = crate::release::scan(std::slice::from_ref(&root))
            .unwrap()
            .remove(0);
        assert!(check(&restored, &settings(), &good, &mut Runs::default()).is_ok());
        // the run is not asked before everything about it has been checked
        let failed = Recorded {
            run: run_json(|r| r["conclusion"] = "failure".into()),
            ..Recorded::good()
        };
        assert!(check(&swapped, &settings(), &failed, &mut Runs::default()).is_err());
        assert!(!failed.asked.borrow().iter().any(|a| a.contains("download")));
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn a_run_is_downloaded_once() {
        let root = temp_dir("prov-once");
        write_artifact(&root, "windows", "1.6.0", "1.6.0.37", b"windows package");
        write_artifact(&root, "android", "1.6.0", "263122251", b"android package");
        let artifacts = crate::release::scan(std::slice::from_ref(&root)).unwrap();
        let recorded = Recorded::good().building(&artifacts);
        let mut runs = Runs::default();
        for artifact in &artifacts {
            check(artifact, &settings(), &recorded, &mut runs).unwrap();
        }
        let downloads = |r: &Recorded| {
            let asked = r.asked.borrow();
            asked.iter().filter(|a| a.contains("run download")).count()
        };
        assert_eq!(downloads(&recorded), 1);
        // two packages and their two release.json files
        assert_eq!(runs.files("4242", &settings(), &recorded).unwrap().len(), 4);
        assert_eq!(downloads(&recorded), 1);
        // another build repository's run of the same number is another run: asked again (the
        // recorded gh knows no such repository)
        let other = Settings {
            build_repo: "someone/else".to_owned(),
            ..settings()
        };
        assert!(runs.files("4242", &other, &recorded).is_err());
        assert_eq!(downloads(&recorded), 2);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn links_in_a_download_are_not_followed() {
        let dir = temp_dir("prov-links");
        fs::create_dir_all(dir.join("artifact/sub")).unwrap();
        fs::write(dir.join("artifact/sub/package"), b"abc").unwrap();
        fs::write(dir.join("outside"), b"a file outside the download").unwrap();
        std::os::unix::fs::symlink(dir.join("outside"), dir.join("artifact/link")).unwrap();
        std::os::unix::fs::symlink(&dir, dir.join("artifact/loop")).unwrap();
        let mut files = Vec::new();
        files_of(&dir.join("artifact"), &mut files).unwrap();
        assert_eq!(
            files,
            [(
                3,
                "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad".to_owned()
            )]
        );
        // the scratch directory: new, for its owner only, and gone when it is dropped
        use std::os::unix::fs::PermissionsExt;
        let scratch = Scratch::new().unwrap();
        let path = scratch.0.clone();
        assert_eq!(
            fs::metadata(&path).unwrap().permissions().mode() & 0o7777,
            0o700
        );
        assert_ne!(Scratch::new().unwrap().0, path);
        fs::write(path.join("file"), b"x").unwrap();
        drop(scratch);
        assert!(!path.exists());
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn another_commit() {
        let other = "3333333333333333333333333333333333333333";
        // the tag is on another commit than the one that was built
        let err = refused(
            "prov-tag",
            Recorded {
                tag: Some(other.to_owned()),
                ..Recorded::good()
            },
        );
        assert!(err.contains("태그 client-1.6.0의 커밋"), "{err}");
        // there is no tag
        let err = refused(
            "prov-notag",
            Recorded {
                tag: None,
                ..Recorded::good()
            },
        );
        assert!(err.contains("태그가 없습니다: client-1.6.0"), "{err}");
        // git answers something that is no commit
        let err = refused(
            "prov-tag-text",
            Recorded {
                tag: Some("client-1.6.0".to_owned()),
                ..Recorded::good()
            },
        );
        assert!(
            err.contains("태그 client-1.6.0의 커밋을 읽지 못했습니다"),
            "{err}"
        );
        // the run called the workflow at another commit
        let run = run_json(|r| {
            r["referenced_workflows"][0]["path"] = format!("{PUBLIC}/{CALLED}@{other}").into();
            r["referenced_workflows"][0]["sha"] = other.into();
        });
        let err = refused(
            "prov-ref",
            Recorded {
                run,
                ..Recorded::good()
            },
        );
        assert!(err.contains("부른 워크플로"), "{err}");
        // the path GitHub recorded names the reviewed commit, the commit it resolved to is another
        // (and the other way round)
        for field in ["sha", "path"] {
            let run = run_json(|r| {
                let entry = &mut r["referenced_workflows"][0];
                entry[field] = match field {
                    "sha" => other.into(),
                    _ => format!("{PUBLIC}/{CALLED}@{other}").into(),
                };
            });
            let err = refused(
                "prov-ref-half",
                Recorded {
                    run,
                    ..Recorded::good()
                },
            );
            assert!(err.contains("부른 워크플로"), "{field}: {err}");
        }
        // build.yml pins another commit, in either place
        let err = refused(
            "prov-yml",
            Recorded {
                build_yml: build_yml(other),
                ..Recorded::good()
            },
        );
        assert!(err.contains("uses가 검토한 커밋"), "{err}");
        let half = build_yml(SOURCE_SHA).replace(
            &format!("source_ref: {SOURCE_SHA}"),
            &format!("source_ref: {other}"),
        );
        let err = refused(
            "prov-yml2",
            Recorded {
                build_yml: half,
                ..Recorded::good()
            },
        );
        assert!(err.contains("source_ref가 검토한 커밋"), "{err}");
        // a branch name instead of a commit
        let branch = build_yml(SOURCE_SHA).replace(&format!("@{SOURCE_SHA}"), "@main");
        let err = refused(
            "prov-branch",
            Recorded {
                build_yml: branch,
                ..Recorded::good()
            },
        );
        assert!(err.contains("uses가 검토한 커밋"), "{err}");
    }

    #[test]
    fn another_workflow() {
        let run = run_json(|r| r["path"] = ".github/workflows/other.yml".into());
        let err = refused(
            "prov-path",
            Recorded {
                run,
                ..Recorded::good()
            },
        );
        assert!(err.contains("다른 워크플로"), "{err}");
        let run = run_json(|r| r["repository"]["full_name"] = "someone/fork".into());
        let err = refused(
            "prov-repo",
            Recorded {
                run,
                ..Recorded::good()
            },
        );
        assert!(err.contains("빌드 저장소"), "{err}");
        // the run also called something else, or GitHub recorded no call at all
        let run = run_json(|r| {
            let extra = serde_json::json!({"path": "someone/else/.github/workflows/x.yml@main", "sha": HEAD_SHA});
            r["referenced_workflows"]
                .as_array_mut()
                .unwrap()
                .push(extra);
        });
        let err = refused(
            "prov-extra",
            Recorded {
                run,
                ..Recorded::good()
            },
        );
        assert!(err.contains("부른 워크플로"), "{err}");
        // the reviewed workflow twice is not one call either
        let run = run_json(|r| {
            let calls = r["referenced_workflows"].as_array_mut().unwrap();
            let again = calls[0].clone();
            calls.push(again);
        });
        let err = refused(
            "prov-twice",
            Recorded {
                run,
                ..Recorded::good()
            },
        );
        assert!(err.contains("부른 워크플로"), "{err}");
        for empty in [serde_json::json!([]), serde_json::Value::Null] {
            let run = run_json(|r| r["referenced_workflows"] = empty.clone());
            let err = refused(
                "prov-none",
                Recorded {
                    run,
                    ..Recorded::good()
                },
            );
            assert!(err.contains("부른 워크플로"), "{err}");
        }
        // a fork of the public repository under another owner
        let run = run_json(|r| {
            r["referenced_workflows"][0]["path"] =
                format!("someone/hjviewer/{CALLED}@{SOURCE_SHA}").into()
        });
        let err = refused(
            "prov-fork",
            Recorded {
                run,
                ..Recorded::good()
            },
        );
        assert!(err.contains("부른 워크플로"), "{err}");
        // not started by hand
        let run = run_json(|r| r["event"] = "push".into());
        let err = refused(
            "prov-event",
            Recorded {
                run,
                ..Recorded::good()
            },
        );
        assert!(err.contains("workflow_dispatch"), "{err}");
    }

    #[test]
    fn a_failed_run() {
        for (status, conclusion) in [
            ("completed", "failure"),
            ("completed", "cancelled"),
            ("completed", "skipped"),
            ("in_progress", ""),
            ("queued", ""),
            // a conclusion without the end: the run of a re-run attempt that is still going
            ("in_progress", "success"),
        ] {
            let run = run_json(|r| {
                r["status"] = status.into();
                r["conclusion"] = if conclusion.is_empty() {
                    serde_json::Value::Null
                } else {
                    conclusion.into()
                };
            });
            let err = refused(
                "prov-failed",
                Recorded {
                    run,
                    ..Recorded::good()
                },
            );
            assert!(err.contains("성공으로 끝나지"), "{err}");
        }
        // gh itself fails: no such run, no network
        let err = refused(
            "prov-404",
            Recorded {
                run: String::new(),
                ..Recorded::good()
            },
        );
        assert!(err.contains("JSON"), "{err}");
        let (mut a, root) = artifact("prov-other-run");
        let good = Recorded::good().building(std::slice::from_ref(&a));
        a.info.run_id = "999".to_owned();
        let err = check(&a, &settings(), &good, &mut Runs::default()).unwrap_err();
        assert!(err.contains("HTTP 404"), "{err}");
        // release.json names another workflow than the run's
        a.info.run_id = "4242".to_owned();
        a.info.workflow = ".github/workflows/other.yml".to_owned();
        assert!(check(&a, &settings(), &good, &mut Runs::default())
            .unwrap_err()
            .contains("workflow"));
        a.info.workflow =
            "yonsei-projecttf/hjviewer-private/.github/workflows/build.yml@refs/heads/main"
                .to_owned();
        assert!(check(&a, &settings(), &good, &mut Runs::default()).is_ok());
        std::fs::remove_dir_all(root).unwrap();
    }

    fn yml(text: &str) -> Result<(), String> {
        check_build_yml(text, PUBLIC, CALLED, SOURCE_SHA, &[])
    }

    #[test]
    fn the_documented_build_yml() {
        let good = build_yml(SOURCE_SHA);
        yml(&good).unwrap();
        // the file as it was before the Google Play inputs, with Windows line ends, with a
        // comment after the pin
        let older = good
            .replace("      play: ${{ inputs.play }}\n", "")
            .replace("      play_release: ${{ inputs.play_release }}\n", "");
        yml(&older).unwrap();
        yml(&good.replace('\n', "\r\n")).unwrap();
        yml(&good.replace(
            &format!("@{SOURCE_SHA}\n"),
            &format!("@{SOURCE_SHA} # 1.6.0\n"),
        ))
        .unwrap();
        yml(&good.replace("${{ inputs.platforms }}", "all")).unwrap();
        yml(&good.replace("    secrets: inherit\n", "")).unwrap();
        yml(&good.replace(
            "    secrets: inherit\n",
            "    secrets: inherit\n    permissions:\n      contents: read\n",
        ))
        .unwrap();
    }

    #[test]
    fn a_changed_build_yml() {
        let good = build_yml(SOURCE_SHA);
        let uses = format!("    uses: {PUBLIC}/{CALLED}@{SOURCE_SHA}\n");
        let changes: Vec<(&str, String)> = vec![
            // a second job, which could run anything
            (
                "job이 하나가",
                format!("{good}  extra:\n    runs-on: ubuntu-latest\n"),
            ),
            (
                "job이 하나가",
                good.replace("  client:\n", "  first:\n  client:\n"),
            ),
            // steps in the job
            (
                "둘 수 없는",
                good.replace("    secrets: inherit\n", "    steps:\n"),
            ),
            (
                "둘 수 없는",
                good.replace("    secrets: inherit\n", "    runs-on: ubuntu-latest\n"),
            ),
            (
                "둘 수 없는",
                good.replace("    secrets: inherit\n", "    secrets:\n"),
            ),
            (
                "둘 수 없는",
                good.replace("    secrets: inherit\n", "    if: always\n"),
            ),
            (
                "둘 수 없는",
                good.replace("    secrets: inherit\n", "    strategy:\n"),
            ),
            (
                "목록",
                good.replace("    secrets: inherit\n", "    steps:\n      - run: evil\n"),
            ),
            // another workflow, a fork, a second call
            (
                "uses가 검토한",
                good.replace(
                    &uses,
                    &format!("    uses: someone/hjviewer/{CALLED}@{SOURCE_SHA}\n"),
                ),
            ),
            (
                "uses가 검토한",
                good.replace(
                    &uses,
                    &format!("    uses: {PUBLIC}/.github/workflows/other.yml@{SOURCE_SHA}\n"),
                ),
            ),
            (
                "uses가 검토한",
                good.replace(
                    &uses,
                    &format!("    uses: {PUBLIC}/{CALLED}@{}\n", &SOURCE_SHA[..7]),
                ),
            ),
            ("두 번", good.replace(&uses, &format!("{uses}{uses}"))),
            ("부르지 않습니다", good.replace(&uses, "")),
            // inputs: an unknown one, an expression that is not an input, no source_ref
            (
                "넘길 수 없는 입력",
                good.replace(
                    "      source_ref",
                    "      server: evil.example.com\n      source_ref",
                ),
            ),
            (
                "값을 이해할 수 없습니다",
                good.replace("${{ inputs.play }}", "${{ secrets.X }}"),
            ),
            (
                "값을 이해할 수 없습니다",
                good.replace("${{ inputs.play }}", "${{ github.event.inputs.play }}"),
            ),
            (
                "source_ref가 없습니다",
                good.replace(&format!("      source_ref: {SOURCE_SHA}\n"), ""),
            ),
            (
                "source_ref가 검토한",
                good.replace(&format!("source_ref: {SOURCE_SHA}"), "source_ref: main"),
            ),
            (
                "source_ref가 두 번",
                good.replace(
                    &format!("      source_ref: {SOURCE_SHA}\n"),
                    &format!("      source_ref: {SOURCE_SHA}\n      source_ref: {SOURCE_SHA}\n"),
                ),
            ),
            // a line under a key that has no lines of its own
            (
                "들여쓰기를 이해할 수 없습니다",
                good.replace(
                    "    secrets: inherit\n",
                    "    secrets: inherit\n      extra: value\n",
                ),
            ),
            (
                "들여쓰기를 이해할 수 없습니다",
                good.replace(&uses, &format!("{uses}      steps: none\n")),
            ),
            (
                "source_ref가 검토한",
                good.replace(
                    &format!("source_ref: {SOURCE_SHA}"),
                    "source_ref: ${{ inputs.ref }}",
                ),
            ),
            // top level
            ("맨 위에 둘 수 없는", format!("env:\n  X: y\n{good}")),
            (
                "맨 위에 둘 수 없는",
                format!("defaults:\n  run:\n    shell: bash\n{good}"),
            ),
            ("jobs는 한 번", format!("{good}jobs:\n")),
            ("jobs는 한 번", good.replace("jobs:\n", "jobs: {}\n")),
            (
                "job이 없습니다",
                good.split("jobs:\n").next().unwrap().to_owned() + "jobs:\n",
            ),
            // YAML that could make a line mean something else
            (
                "쓸 수 없는 문자",
                good.replace("name: build", "name: \"build\""),
            ),
            (
                "쓸 수 없는 문자",
                good.replace("name: build", "name: 'build'"),
            ),
            ("쓸 수 없는 문자", good.replace("name: build", "name: |")),
            ("쓸 수 없는 문자", good.replace("name: build", "name: >-")),
            ("쓸 수 없는 문자", good.replace("  client:", "  client: &a")),
            (
                "쓸 수 없는 문자",
                good.replace("    with:", "    <<: *a\n    with:"),
            ),
            (
                "쓸 수 없는 문자",
                good.replace("name: build", "name: !!str build"),
            ),
            ("쓸 수 없는 문자", good.replace("    with:", "\twith:")),
            (
                "쓸 수 없는 문자",
                good.replace("name: build", "name: build\u{2028}jobs:"),
            ),
            (
                "괄호",
                good.replace(
                    "default: false }\n      play:",
                    "default: false\n      play:",
                ),
            ),
            ("목록", format!("---\n{good}")),
            ("들여쓴 줄", format!("  name: x\n{good}")),
            ("이름: 값", good.replace("  client:", "  client")),
            ("쓸 수 없는 문자", good.replace("name: build", "? name")),
        ];
        for (expected, text) in &changes {
            assert_ne!(*text, good, "{expected}: the change did nothing");
            let err = yml(text).unwrap_err();
            assert!(err.contains(expected), "{expected:?} is not in {err:?}");
        }
        // an input newer than the tool passes only when the operator allows it
        let newer = good.replace("      source_ref", "      channel: beta\n      source_ref");
        assert!(yml(&newer).is_err());
        assert!(
            check_build_yml(&newer, PUBLIC, CALLED, SOURCE_SHA, &["channel".to_owned()]).is_ok()
        );
        // the pin is compared with the commit asked for
        assert!(check_build_yml(&good, PUBLIC, CALLED, HEAD_SHA, &[]).is_err());
        assert!(check_build_yml(&good, PUBLIC, CALLED, "main", &[]).is_err());
    }

    #[test]
    fn settings_are_checked() {
        assert!(settings().check().is_ok());
        for bad in [
            "",
            "owner",
            "owner/",
            "/name",
            "owner/name/x",
            "owner/na me",
            "../x",
            "owner/..",
            "o?x/y",
        ] {
            let s = Settings {
                build_repo: bad.to_owned(),
                ..settings()
            };
            assert!(s.check().is_err(), "{bad:?}");
        }
        for bad in [
            "build.yml",
            ".github/workflows/../../x.yml",
            ".github/workflows/x.yaml",
            ".github/workflows/x y.yml",
        ] {
            let s = Settings {
                build_workflow: bad.to_owned(),
                ..settings()
            };
            assert!(s.check().is_err(), "{bad:?}");
        }
    }
}
