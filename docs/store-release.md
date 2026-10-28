# 스토어 실행 (App Store, Google Play)

1.9.1부터 App Store와 Google Play에 닿는 일은 **스토어 실행**이 합니다. 앱을 만드는 **빌드 실행**(`build.yml` → `client-build.yml`, [ci.md](ci.md))과 따로입니다. 빌드 실행은 TestFlight 내부 테스트에 올리는 일만 하고, Google Play에는 아무것도 올리지 않습니다.

- **시작**: 비공개 빌드 저장소 `yonsei-projecttf/hjviewer-private`의 `.github/workflows/store.yml`을 손으로(dispatch) 시작합니다. 태그 푸시, 커밋 푸시, 일정으로는 시작하지 않습니다. 스토어에 내는 결정은 사람이 하고, Actions 시간도 아끼기 위해서입니다.
- **코드**: `store.yml`은 이 저장소의 `.github/workflows/store-release.yml`을 확인한 커밋(40자리 SHA)에 고정해 부릅니다. 모든 잡이 그 파일의 저장소와 커밋(`job.workflow_repository`, `job.workflow_sha`)을 자격 증명 없이 체크아웃합니다. 불려 온 워크플로에서 `github.repository`와 `github.sha`는 부른 쪽(비공개 저장소)을 가리키기 때문입니다.
- **러너**: ubuntu-24.04만 씁니다. API만 부르므로 macOS 시간은 늘지 않습니다.
- **스크립트**: 표준 라이브러리만 씁니다(fastlane 없음). Google Play는 `scripts/play-upload.py`, App Store Connect는 `scripts/asc-store.mjs`(Node 20 이상), 출시 기록은 `scripts/store-record.py`, 스토어 글 검사는 `scripts/check-store-text.py`입니다. 스토어 글은 [store/](../store/README.md)에 있습니다.
- 설계는 개발 PC의 `build/v191/notes/auto-deploy-design.md`(2–7절)이고, 소유자의 결정은 `build/v191/notes/auto-deploy-decisions.md`입니다.

## 잡

| 잡 | 언제 | environment·키 | 권한 | 하는 일 |
|---|---|---|---|---|
| Checks | status 밖의 동작 | 없음 | contents: read, actions: read | 입력의 짝과 값, 이 워크플로의 저장소, 태그 `client-<버전>`(그 커밋이 소스 커밋), `store/<스토어>/version.txt`와 스토어 글 검사(release는 글을 보내지 않으므로 건너뜀), 출시 기록, upload의 빌드 실행(`run_id`, 아래), 다른 스토어 실행, 데모 계정의 끝나는 날(`REVIEW_DEMO_UNTIL`, 14일 전부터 경고) |
| Google Play | platform=android | environment `store`: `PLAY_SA_EMAIL`, `PLAY_SA_PRIVATE_KEY` | contents: read, actions: read | status, upload |
| App Store | platform=ios | environment `store`: `ASC_STORE_KEY_ID`, `ASC_STORE_PRIVATE_KEY`, `REVIEW_*`. 저장소 시크릿 `ASC_APP_ID`, `ASC_ISSUER_ID` | contents: read | status, prepare, submit, release. prepare·submit은 TestFlight 처리를 60분까지 기다립니다 |
| Release record | upload·submit·release가 성공했고 dry_run이 아닐 때 | 없음 | contents: write | 비공개 저장소의 `store-releases.txt`에 한 줄을 더합니다 |

- 시크릿은 `secrets: inherit`로 넘기지 않고 이름으로만 넘깁니다. 스토어 키와 심사 정보는 environment `store`(배포 브랜치 main)에 있고, `environment: store`를 선언한 Google Play·App Store 잡만 그 값을 받습니다. 빌드 실행의 잡(서드파티 빌드 코드가 도는 곳)은 이 값을 참조할 수 없습니다.
- 동시 실행: upload는 `google-play-<저장소>`, prepare·submit·release는 `app-store-<저장소>` 그룹에서 하나씩 돕니다. status는 편집을 열지 않으므로 그룹에 넣지 않습니다. GitHub는 그룹마다 기다리는 실행을 하나만 두므로, status가 그룹에 있으면 기다리던 upload가 취소될 수 있습니다. 그와 별개로 Checks가 다른 스토어 실행이 진행 중이거나 기다리면 시작하지 않습니다.

## 입력

| 입력 | 값 | 쓰임 |
|---|---|---|
| `platform` | `android`, `ios` | |
| `action` | `status`, `upload`(android), `prepare`·`submit`·`release`(ios) | 맞지 않는 짝은 거부합니다 |
| `version` | 예: `1.9.1` | status 밖에서는 꼭 줍니다. `store/<스토어>/version.txt`와 같아야 하고, 태그 `client-<버전>`이 있어야 합니다 |
| `build` | Android versionCode 또는 iOS CFBundleVersion | status 밖에서는 꼭 줍니다. Android는 번들의 versionCode와 같아야 하고(다르면 Play에 커밋하지 않고 멈춤), iOS는 App Store Connect에 있는 빌드 번호입니다. submit·release가 '이미 됨'으로 끝날 때도 그 버전에 붙은 빌드가 이 번호여야 합니다(기록에 이 번호가 적히므로) |
| `run_id` | 빌드 실행 ID: 실행 페이지 주소 `…/actions/runs/<이 숫자>`(화면의 `#N`이 아님) | android upload만. hjv-release가 패키지의 출처를 보는 것과 같은 규칙으로 봅니다(`scripts/store-record.py check-run`): 이 저장소의 `build.yml`을 main에서 손으로 시작해 성공한 실행, 그 실행이 부른 워크플로가 공개 `client-build.yml@<태그 커밋>` 하나, 그 실행 커밋의 `build.yml`이 문서의 모양(job 하나, 그 워크플로만 부름, `source_ref`도 태그 커밋). 그 실행의 `HJViewer-<버전>-<태그 커밋 7자리>-android-aarch64-play` 아티팩트(만료 전)를 받습니다 |
| `in_review` | `error`(기본), `cancel` | android upload. 앞 변경이 Google 심사 중이면 `error`는 편집을 열기 전에 멈추고, `cancel`은 그 심사를 취소하고 이번 출시를 보냅니다(심사가 처음부터) |
| `release_type` | `manual`(기본), `after-approval` | ios prepare·submit. 실수로 내보내지 않게 기본은 수동 출시입니다. **App Store에 나온 적이 없는 첫 버전은 manual만 받습니다**(`asc-store.mjs`가 쓰기 전에 거부. Unlisted 승인 전에 공개 목록에 나가지 않게) |
| `phased` | `false`(기본), `true` | ios prepare·submit. 단계적 출시(after-approval과 함께). Apple은 업데이트에만 적용하므로 첫 버전에는 거부합니다 |
| `dry_run` | `false`(기본), `true` | Google Play: 편집을 Play에 검증받고 지웁니다(아무것도 출시하지 않음). App Store Connect: GET만 하고, 할 쓰기를 비밀값 없이(크기와 칸 이름만) 출력합니다 |

트랙은 늘 비공개 테스트 트랙 **alpha**입니다(소유자 결정, 2026-10-26). 내부 테스트 트랙과 승격은 쓰지 않습니다.

```sh
R=yonsei-projecttf/hjviewer-private
gh workflow run store.yml -R $R -f platform=android -f action=status
gh workflow run store.yml -R $R -f platform=ios -f action=status
gh workflow run store.yml -R $R -f platform=android -f action=upload -f version=1.9.1 -f build=<versionCode> -f run_id=<빌드 실행 ID>
gh workflow run store.yml -R $R -f platform=ios -f action=prepare -f version=1.9.1 -f build=<CFBundleVersion>
gh workflow run store.yml -R $R -f platform=ios -f action=submit -f version=1.9.1 -f build=<CFBundleVersion>
gh workflow run store.yml -R $R -f platform=ios -f action=release -f version=1.9.1 -f build=<CFBundleVersion>
```

versionCode와 CFBundleVersion은 빌드 실행의 요약(Play 번들 줄)과 산출물의 `release.json`에 있습니다.

## 동작

- **status** (읽기만): 한 줄 요약을 실행 요약에 남기고, JSON을 `store-status-<플랫폼>-<실행 ID>` 아티팩트(7일)로 둡니다. 정책의 `--play-live`와 `--appstore-live`(hjv-release)는 이 결과를 근거로 냅니다.
  - Android: `applications.tracks.releases.list`로 alpha와 internal의 출시(이름, versionCode, 상태 `IN_REVIEW`·`NOT_APPROVED`·`PUBLISHED` 등)를 봅니다. 편집을 열지 않습니다. `build`를 주면 그 versionCode의 상태도 적습니다.
  - iOS: TestFlight 처리 상태, App Store 버전마다의 상태(appVersionState)와 출시 방식, 열린 심사 제출과 그 안의 버전, App Store에 나온 버전, 단계적 출시 상태.
- **upload** (android): 확인한 빌드 실행의 AAB를 alpha에 completed로 올리고 Google 심사로 보냅니다.
  1. 편집을 열기 전에 releases.list로 alpha와 internal을 봅니다. 이 versionCode가 이미 심사 중·승인됨·게시됨이면 아무것도 하지 않고 성공으로 끝납니다(응답을 못 받은 커밋을 다시 돌린 경우). `IN_REVIEW`인 출시가 있으면 `in_review=cancel`일 때만 계속합니다. `NOT_APPROVED`(거절)나 `NOT_SENT_FOR_REVIEW`(보내지 않은 변경)가 있으면 멈추고, Play Console에서 정리하라고 알립니다.
  2. 편집 하나: Play가 이미 가진 번들(같은 sha256)이면 다시 올리지 않습니다. 트랙이 내보내는 versionCode보다 낮은 번들은 올리지 않습니다. 커밋은 늘 `changesInReviewBehavior`를 적습니다(`ERROR_IF_IN_REVIEW`, cancel이면 `CANCEL_IN_REVIEW_AND_SUBMIT`). 적지 않으면 Google의 기본값이 심사를 취소하기 때문입니다. 어디서 실패하든 편집을 지웁니다.
  3. 출시 노트(ko-KR)는 `store/play/ko-KR/release_notes.txt` 뒤에 두 줄 `HJViewer <버전> (<versionCode>)`와 `소스 코드(AGPL-3.0): https://github.com/yonsei-projecttf/hjviewer/tree/<태그 커밋>`을 더한 것이고, 합쳐 500자 안이어야 합니다(Checks의 글 검사가 봅니다).
- **prepare** (ios): 제출하지 않고 App Store 버전을 준비합니다.
  1. 빌드(처리 VALID, 만료 전, 버전과 번호가 같음)를 찾습니다. 아직 처리 중이면 60분까지 기다립니다.
  2. 이 버전의 App Store 버전을 찾습니다. 없으면 편집할 수 있는 버전(앱을 만들 때 생긴 '제출 준비 중' 1.0 등)의 번호를 바꾸고, 그것도 없으면 만듭니다. 이 버전이 심사 중이거나 승인·출시된 상태면 바꾸지 않고 멈춥니다. 쓰기 전에 두 가지를 더 봅니다. 첫 버전이면 `after-approval`과 `phased`를 거부하고, 앱의 기본 언어(App Store Connect의 primaryLocale)가 `store/appstore/locales.txt`의 첫 줄이 아니면 멈춥니다. `store/`에 없는 언어의 현지화는 그대로 두고 알리기만 합니다.
  3. `store/appstore/`의 글(버전 페이지의 설명·키워드·프로모션 텍스트·지원 URL·마케팅 URL, 앱 정보의 이름·부제목·개인정보 처리방침 URL, 저작권), 출시 방식, 빌드, 단계적 출시, 앱 심사 정보(연락처, 데모 계정, 메모)를 넣습니다. 바뀐 칸만 보냅니다. **App Store에 나온 적이 없는 첫 버전에는 '새로운 기능'을 보내지 않습니다.**
  4. 메모는 `review_notes.txt`의 `{{DEMO_PC_ID}}`, `{{DEMO_PC_PASSWORD}}`를 시크릿으로 채운 것입니다. 데모 PC ID는 16바이트, 비밀번호는 32바이트(인쇄 가능한 ASCII)까지 받고, 채운 메모가 4000바이트(줄바꿈을 CRLF로 셈)를 넘으면 멈춥니다. 로그에는 바이트 수만 나오고, 채운 메모·데모 값·그 해시는 나오지 않습니다.
- **submit** (ios): prepare가 하는 일을 한 뒤 심사에 냅니다. 다시 돌려도 안전합니다.
  - 이 버전이 이미 심사 대기·심사 중·승인·출시 상태면 성공으로 끝냅니다.
  - 열린 제출(Apple은 앱·플랫폼마다 하나만 둠)이 다른 항목을 담고 있으면 멈춥니다. prepare가 번호를 바꿀 편집할 수 있는 버전(바이너리 거절 뒤의 옛 번호 버전)의 항목은 이 버전의 것으로 셉니다. 스토어 실행은 아무것도 취소하지 않습니다.
  - `READY_FOR_REVIEW`나 `UNRESOLVED_ISSUES`(메타데이터 거절 뒤) 제출이 있으면 그 제출을 다시 쓰고, 이 버전이 없으면 더합니다. 열린 제출이 없을 때만 새로 만듭니다. 그다음 `submitted=true`.
- **release** (ios): 승인된 수동 출시 버전(`PENDING_DEVELOPER_RELEASE`)을 출시합니다. 이미 `PROCESSING_FOR_DISTRIBUTION`이나 `READY_FOR_DISTRIBUTION`이면(승인 뒤 Apple이 자동 출시한 버전 포함) 성공으로 끝내고, 출시 기록에 release 줄을 남깁니다. 어느 경우든 그 버전에 붙은 빌드가 `build`와 같아야 합니다. 단계적 출시의 일시 중지·재개는 App Store Connect에서 합니다.

## 출시 기록 `store-releases.txt` (비공개 저장소)

스토어에 무엇이 나갔는지만 적습니다. 한 줄에 하나이고, 줄을 지우거나 고치지 않습니다(지우면 그 버전을 다시 낼 수 있게 됩니다).

```text
# store version build action track source-commit date(UTC) note
play 1.9.1 <versionCode> upload alpha <소스 커밋 40자리> 2026-10-28 store #12 build-run <run_id>
appstore 1.9.1 <빌드> submit - <소스 커밋 40자리> 2026-10-28 store #13
appstore 1.9.1 <빌드> release - <소스 커밋 40자리> 2026-11-02 store #15
```

- **쓰는 곳**: Release record 잡(upload, submit, release가 성공했을 때, contents API로, 다른 실행이 먼저 쓰면 다시 읽고 5번까지). 커밋 메시지에 `[skip ci]`가 붙습니다. 콘솔에서 손으로 한 일은 같은 꼴로 손으로 더합니다.
- **읽는 곳**: 스토어 실행의 Checks, 빌드 실행의 Android·iOS 잡 첫 단계, TestFlight 업로드 실행의 첫 단계, `hjv-release policy`(App Store 관문 T3). 주석과 빈 줄 밖의 모든 줄이 `스토어 버전 숫자빌드 동작 …` 꼴이어야 합니다. 빌드 번호는 숫자 1–18자리입니다. 줄은 `\n`에서만 나누고(CRLF 줄 끝의 `\r`은 뺌), 칸은 공백과 탭으로만 나눕니다. 탭 밖의 제어 문자나 U+2028·U+2029가 든 줄은 주석이어도 모르는 줄입니다. 두 읽는 쪽(`scripts/store-record.py`, `tools/hjv-release`)이 같은 규칙으로 읽습니다. 모르는 줄이 있으면 셋 다 멈춥니다. 자리 표시(`<…>`)가 남은 줄을 넣지 않습니다.
- **규칙은 (스토어, 버전) 단위**입니다. 같은 1.9.0도 Android와 iOS의 빌드 번호가 달랐고, 버전이 갈라지기도 하므로 다른 스토어의 줄로는 거부하지 않습니다.
  - 같은 버전·빌드·동작은 통과합니다(다시 실행).
  - 같은 버전의 다른 빌드, 그 스토어에 기록된 가장 높은 버전보다 낮은 버전은 거부합니다(소유자 규칙: 새 빌드는 더 높은 버전).
  - release된 App Store 버전의 새 submit은 거부합니다.
  - 빌드 실행은 그 플랫폼의 스토어(android → play, ios → appstore)에 이미 있는 버전이나 그보다 낮은 버전을 빌드하지 않습니다.
- **appstore 줄의 소스 커밋은 확인한 값이 아닙니다.** 태그 `client-<버전>`의 커밋을 적을 뿐이고, App Store 제출은 빌드를 번호로만 고릅니다(그 빌드가 태그 커밋의 빌드 실행에서 왔는지는 보지 않음). play 줄은 Checks가 빌드 실행의 출처를 확인한 값입니다.
- after-approval로 Apple이 출시한 버전(둘째 App Store 버전부터)은 release를 돌리지 않으면 release 줄이 생기지 않습니다. status가 `READY_FOR_DISTRIBUTION`을 보이면 `store.yml ios release`를 한 번 돌립니다(이미 출시됨으로 성공하고 줄을 남김).
- TestFlight의 중복은 기록이 아니라 App Store Connect를 봅니다. TestFlight 업로드 실행(1.9.2부터, 빌드 없이 빌드 실행의 IPA를 올림)의 첫 단계(`asc-store.mjs check-new`)가 그 버전의 빌드가 이미 있으면 멈춥니다.

## 릴리스마다

| # | Android | iOS | 누가 |
|---|---|---|---|
| 0 | 빌드 전: `store/`의 글을 이 버전으로(두 `version.txt`, 두 언어의 `whats_new.txt`, `release_notes.txt`, [store/README.md](../store/README.md)). 소유자가 공개될 글을 확인한 뒤 push. 비공개 `build.yml`·`store.yml`의 고정 SHA를 그 커밋으로(공개 main의 조상인지 `git merge-base --is-ancestor <sha> origin/main`으로 확인). 첫 App Store 버전의 판을 정하기 전이거나 그 판이 main의 글보다 앞 판일 때의 `store.yml`은 아래 "첫 App Store 버전만"대로 | 같음 | 에이전트 초안, 소유자 확인 |
| 1 | `build.yml`(`platforms=all`): Windows·Android·iOS(와 hjv-relay)를 한 실행에서 함께 빌드합니다(2026-10-27 사용자 결정). 이 실행은 아무것도 나눠 주지 않습니다(Play에 올리지 않음, `-play` AAB는 30일) → **Windows 게이트**: 그 실행의 Windows 설치 시험 단계가 성공하고 소유자가 그 설치본을 Windows에서 확인한 것. 나눠 주는 일은 모두 게이트 뒤입니다 | 같은 실행이 서명된 IPA를 산출물로만 남깁니다 | 에이전트, 소유자 |
| 2 | 게이트 뒤에 따로 할 빌드 없음(1의 APK와 `-play` AAB를 씀) | 게이트 뒤: `build.yml -f platforms=ios -f testflight=true -f testflight_run=<1의 실행 ID>`(빌드 없이 1의 IPA를 TestFlight에 올림, 7일 안) | 에이전트 |
| 3 | 같은 실행의 APK로 확인(APK 설치본 기기) | `store.yml ios status`로 처리를 본 뒤 TestFlight에서 확인 | 소유자 |
| 4 | 태그 `client-<버전>`, 산출물을 `build/release-<버전>/`에(스토어 정책을 낼 때까지 지우지 않음) | 같음 | 에이전트 |
| 5 | 패키지 정책(Windows·APK, 빌드 뒤 7일 안) | ios는 그대로(`--hold ios`). 스토어 항목(android_play, ios)은 늘 스토어 정책(9번)에서 올리고, App Store 제출 중에는 반드시 그대로 | 에이전트 초안, 소유자 서명 |
| 6 | 전제: 서버에 이 버전의 hjv-api(1.9.1: 계정 삭제)가 돌고 있음([server-runbook.md](server-runbook.md) 9-10). "내보내도 됨" → `store.yml android upload run_id=<1의 실행 ID>` | 같은 전제. "내보내도 됨" → `store.yml ios submit`(첫 App Store 버전은 아래) | 소유자 승인, 에이전트 실행 |
| 7 | Google 심사(몇 시간–7일) | App Review | — |
| 8 | `store.yml android status` → `PUBLISHED` | 수동 출시면 `store.yml ios release`. after-approval이면 status가 `READY_FOR_DISTRIBUTION`을 보인 뒤 `store.yml ios release`를 한 번(이미 출시됨으로 성공하고 기록 줄을 남김) | 에이전트 |
| 9 | 스토어 정책: `--play-live <versionCode>`. 첫 android_play 항목은 `--min android_play=<android의 min>`으로 | 스토어 정책: `--appstore-live <버전>` | 에이전트 초안, 소유자 서명 |
| 10 | Release record 잡이 남긴 줄 확인 | 같음 | 에이전트 |

- 글만 고친 커밋(메타데이터 거절 뒤 등)이면 `store.yml`의 SHA가 `build.yml`보다 앞서도 됩니다. 빌드는 `build.yml`의 커밋, 스토어 글은 `store.yml`의 커밋에서 옵니다.

- 둘째 App Store 버전부터는 `release_type=after-approval`, `phased=true`로 냅니다(소유자 결정: 승인 즉시 자동 + 단계적 출시). 문제가 보이면 App Store Connect에서 단계적 출시를 일시 중지합니다(합계 30일까지).
- **첫 스토어 실행은 Android upload와 iOS submit을 하루 이상 엇갈립니다.** 두 심사자가 데모 계정 하나를 같은 때 쓰면 틀린 비밀번호 한도에 함께 걸릴 수 있습니다. 심사 중에는 status와 함께, 데모 계정이 잠기지 않았는지 허브-운영에게 봐 달라고 합니다.
- **App Store 제출이 열려 있는 동안**(제출부터 승인·거절까지) 정책의 ios latest와 min은 심사 중인 버전 이하로 둡니다. 심사 기기는 TestFlight 화면을 보므로, 정책이 앞서면 배너와 [TestFlight 열기]가 나옵니다. 정책 초안을 만들기 전에 `store.yml ios status`의 심사 중인 버전과 대조합니다.
- **상태 확인 예산**: status는 플랫폼마다 하루 4번까지만 돌립니다. 심사 결과는 스토어의 알림 메일을 먼저 봅니다. status를 되풀이해 dispatch하지 않습니다(`/loop`, Monitor도 쓰지 않음).

### 첫 App Store 버전만

배포 방법은 **등록되지 않은 앱(Unlisted)**이고, 첫 버전은 **수동 출시**입니다(소유자 결정). App Store에 나온 버전이 아직 없는 동안에는 거절 뒤에 다시 내는 제출도 이 절을 따릅니다.

**낼 판은 소유자가 정합니다.** 2026-10-26의 결정은 1.9.1이었고([distribution.md](distribution.md)), 지침 3.2 거절에 보낼 답장의 초안(개발 PC의 `build/appstore-listing/submit/reply-3.2-2026-10-28.md`)은 아래 1번의 조건이 갖춰진 때 TestFlight에서 처리를 마친 가장 높은 판을 권합니다(그때 1.9.2가 있으면 1.9.2). 스토어 실행은 `store.yml`이 고정한 커밋의 글을 보내고, 그 글의 `version.txt`가 입력 `version`과 다르면 Checks가 멈춥니다. main의 글은 413641d(`client: 1.9.2`)부터 1.9.2의 것입니다. 정하기 전에 1.9.2를 push하면 `store.yml`은 아래 1.9.1의 커밋(ac6aaf3)에 둡니다. 1.9.2의 스토어 실행은 태그 `client-1.9.2` 뒤에야 되므로 잃는 것이 없고, 1.9.1의 길도 열려 있습니다.

- **1.9.2 이상으로 낼 때**: "릴리스마다" 0번대로 `store.yml`을 그 판의 커밋으로 옮깁니다. Play에 1.9.1을 올리지 않고 그 판을 올리면 Alpha의 테스터는 1.9.0에서 바로 오므로, `store/play/ko-KR/release_notes.txt`에 1.9.1의 변경(앱 안 계정 삭제 등)도 씁니다(덧붙는 두 줄까지 500자 안).
- **1.9.1로 낼 때**: `store.yml`을 ac6aaf3에 고정합니다. 1.9.1의 글과 고친 심사 메모가 있는 마지막 커밋입니다. 1.9.2를 push할 때 `build.yml`은 1.9.2의 커밋으로, `store.yml`은 ac6aaf3으로 옮기고, `store.yml`은 1.9.1의 prepare·submit(과 1.9.1의 Play upload)이 끝날 때까지 그대로 둡니다. 그동안 1.9.2의 Play upload는 Checks가 멈춥니다(release는 글을 보내지 않으므로 어느 커밋에서도 돕니다). main에서는 1.9.1의 글을 더 고칠 수 없습니다. main의 글은 앱 버전의 것이어야 하고(`scripts/test_check_store_text.py`), 고정 SHA는 main의 조상이어야 하기 때문입니다(G2). 그래서 메모에 ac6aaf3에 없는 문장이 필요하면 1.9.1은 스토어 실행으로 낼 수 없습니다. 1번이 요구하는 학교의 허락서(B4)를 첨부할 때의 문장과 Unlisted 승인 뒤의 문장([store/README.md](../store/README.md)의 조건부 문장)이 그렇습니다. 그때는 1.9.2 이상으로 내거나(prepare가 편집할 수 있는 버전의 번호와 빌드를 바꾸고 submit이 열린 제출을 다시 냄. "실패와 대처"의 바이너리 거절과 같음), 소유자가 정하면 콘솔에서 손으로 냅니다(출시 기록에 submit 줄을 손으로 더함).

1. 막는 것 B1–B10(제출 분석 `build/appstore-listing/submit/SUBMIT-1.9.0.md` 0-3, 1.9.1 기준)을 점검합니다. 데모 Windows PC(B3), 학교의 서면 허락(B4), 낼 판의 화면의 스크린샷(B5), 처리방침(B7), 계정 종류마다의 심사 계정(B10)이 남아 있으면 제출하지 않습니다. 서버에 1.9.1의 hjv-api(앱 안 계정 삭제)가 돌고 있어야 합니다([server-runbook.md](server-runbook.md) 9-10의 확인 curl이 401). 심사자가 데모 계정을 지워 봅니다.
2. `prepare`(release_type=manual). 데모 PC 값(`REVIEW_DEMO_PC_*`)을 포함한 `REVIEW_*` 여덟 개가 모두 있어야 돕니다. 그 전(B3 전)에는 `dry_run=true`만 됩니다. 그다음 소유자가 App Store Connect에서 모든 칸, App Privacy, 스크린샷, 연령 등급을 봅니다. **글은 콘솔에서 고치지 않습니다.** 고칠 것이 있으면 `store/`를 고쳐 커밋하고, `store.yml`의 고정 SHA를 그 커밋으로 바꾼 뒤 prepare를 다시 돌립니다(`dry_run=true`가 콘솔과 다른 칸을 보여 줍니다). 글만 고친 커밋이면 `store.yml`의 SHA만 옮깁니다(`build.yml`보다 앞서도 됨).
3. 심사 첨부 영상(SUBMIT 7-6)을 손으로 올립니다.
4. `submit`. 심사 메모에 Apple이 요구하는 문장 "This app is intended for unlisted distribution."이 있어야 합니다(글 검사가 봅니다).
5. 등록되지 않은 앱 요청 양식을 냅니다(SUBMIT 8-3). Apple은 이미 App Store에 있거나 최종 배포용으로 심사에 제출된 앱의 요청만 받습니다.
6. 심사 승인과 Unlisted 승인을 모두 받은 뒤 `release`. 그 전에 두 가지를 App Store Connect에서 봅니다. 가격 및 사용 가능 여부의 배포 방법이 **등록되지 않은 앱(미등록)**으로 바뀌었는지, 앱 심사가 **개발자 출시 대기 중**인지. 그 전에 공개 목록에 나가지 않도록 수동 출시를 씁니다(첫 버전은 스크립트가 after-approval을 거부합니다).
7. App Store에 나오면 다음 정책에서 `--appstore-live <그 판>`으로 App Store 표시를 켭니다. 서버의 `hjv-update-verify.sh`는 63c18d8 이후의 것이어야 그 정책을 받습니다([server-runbook.md](server-runbook.md) 8-2). 2026-10-27에 서버 묶음 `server-2026.10.27`의 것으로 바꿨으므로(9-10의 기록), 그 정책을 넣기 전에 그대로인지 봅니다.

## 실패와 대처

| 상황 | 일어나는 일 | 할 일 |
|---|---|---|
| Play: 앞 버전이 심사 중 | upload가 편집을 열기 전에 멈춥니다(`IN_REVIEW`). 빌드 실행과 APK는 상관없습니다 | 기다립니다. 꼭 바꿔야 하면 소유자가 정해 `in_review=cancel`(앞 심사를 취소하고 이번 것을 보냄, 심사가 처음부터) |
| Play: 거절(`NOT_APPROVED`)이나 보내지 않은 변경(`NOT_SENT_FOR_REVIEW`) | upload가 멈춥니다 | Play Console의 게시 개요에서 정리하고 '검토를 위해 전송'(손으로). 코드 문제면 더 높은 버전으로 새로 빌드 |
| Play: 관리형 게시가 켜짐 | 승인 뒤 `APPROVED_NOT_PUBLISHED`에 머뭅니다 | 콘솔에서 게시(API 없음). 관리형 게시는 끄기로 했습니다 |
| Play: 트랙에 더 높은 versionCode | 올리지 않고 멈춥니다 | 새로 빌드 |
| Play: 커밋 응답을 받지 못함 | 출시가 됐는지 모릅니다 | 같은 upload를 다시 dispatch(또는 Re-run failed jobs). 이미 됐으면 "is there already"로 끝납니다 |
| Play: `-play` 아티팩트가 지워짐(30일) | Checks가 아티팩트를 찾지 못합니다 | 더 높은 버전으로 새로 빌드 |
| 스토어의 새 계약·정책 선언 | API 업로드와 제출이 실패합니다(Play는 403 선언 메시지, Apple은 403) | 콘솔에서 처리한 뒤 다시(Apple 계약은 계정 소유자만) |
| 빌드 실행의 'Re-run all jobs' | 새 빌드 번호로 다시 빌드합니다 | 스토어나 TestFlight에 올라간 뒤에는 쓰지 않습니다. 첫 단계 검사가 기록에 같은 버전이 있으면 멈춥니다. TestFlight에 이미 있는 버전의 새 IPA는 TestFlight 업로드 실행의 첫 단계가 받지 않습니다 |
| TestFlight 업로드 실행이 업로드 뒤에 실패 | 다시 돌리면 같은 IPA를 또 올리려 합니다 | 첫 단계 검사가 TestFlight의 같은 버전을 보고 멈춥니다. `store.yml ios status`로 확인 |
| App Store: 편집할 수 있는 버전의 번호가 다름 | | prepare가 번호를 바꿉니다. 이 버전이 심사 중·승인 상태면 바꾸지 않고 멈춥니다 |
| App Store: 제출이 이미 열려 있음 | | submit이 그 제출을 다시 씁니다. 다른 항목이 있으면 멈춥니다 |
| App Store: 응답을 받지 못한 submit·release | | 다시 돌리면 상태를 보고 성공으로 끝냅니다 |
| App Store: 심사 거절 | Apple이 App Store Connect의 앱 심사에 까닭을 적습니다 | 다시 내기 전에 App Store Connect의 앱 심사(App Review)에서 그 제출의 메시지에 답합니다(무엇을 고쳤는지, 또는 설명). 다시 내는 길은 아래 두 줄(메타데이터 거절, 바이너리 거절)대로입니다 |
| App Store: 메타데이터 거절 | 제출이 `UNRESOLVED_ISSUES` | `store/`를 고쳐 커밋 → `store.yml`의 SHA → submit(같은 제출을 다시 냄) |
| App Store: 바이너리 거절 | 제출이 `UNRESOLVED_ISSUES`, 버전이 `REJECTED` | 고친 빌드는 새 버전입니다. 새 버전으로 빌드한 뒤 submit. prepare가 거절된 버전의 번호를 새 버전으로 바꾸고, 열린 제출의 그 항목은 이 버전의 것으로 셉니다(같은 제출을 다시 냄). 열린 제출을 콘솔에서 지우거나 취소하지 않습니다. 심사 중에는 정책의 ios를 그 버전 이하로 둡니다 |
| 잘못된 빌드가 나감 | | App Store: 단계적 출시면 일시 중지한 뒤 더 높은 버전. 수동 출시면 release하지 않습니다. Play alpha: 더 높은 versionCode로 바꿉니다. 정책: min을 올리지 않습니다(`--hold`) |
| 등록되지 않은 앱 요청 대기 | | 수동 출시로 두고 기다립니다. 거절되면 공개로 낼지 소유자가 정합니다 |
| 데모 계정 만료·잠김 | 심사 거절 | Checks가 14일 전부터 경고하고, 끝난 뒤에는 upload·submit을 멈춥니다. 잠기면 허브-운영의 `hjv-api admin unlock`. 새 review.conf를 만들면 `REVIEW_DEMO_*`, `REVIEW_DEMO_UNTIL`, Play의 앱 액세스를 함께 바꿉니다 |
| 스토어 실행이 겹침 | Checks가 멈춥니다 | 앞 실행이 끝난 뒤 다시 |
| Release record 잡 실패 | 스토어에는 나갔는데 `store-releases.txt`에 줄이 없습니다 | 그 실행에서 'Re-run failed jobs'를 누릅니다. 성공한 Checks와 스토어 잡은 다시 돌지 않고 기록만 다시 합니다(같은 줄이 이미 있으면 그대로 끝남). 빠진 줄에 따라 풀리는 검사가 다릅니다. `play … upload` 줄이나 `appstore … submit` 줄이 없으면 그 스토어에 그 버전이 없는 것처럼 보입니다. 그래서 빌드 실행의 첫 단계(`check-build`)가 같은 버전을 다시 빌드하는 것을, 스토어 실행의 Checks(`check`)가 같은 버전의 다른 빌드나 더 낮은 버전을 내는 것을 막지 못합니다. `appstore … release` 줄이 없으면 같은 버전의 submit 줄이 남아 있어서 그 둘은 그대로 막습니다. 대신 Checks가 출시된 그 버전의 submit(같은 빌드)을 다시 실행으로 보고 통과시키고, `hjv-release policy --no-appstore`에 T3-2의 경고와 `yes` 질문이 나오지 않습니다. 다시 돈 뒤 줄을 확인하고, 그래도 없으면 같은 꼴의 줄을 손으로 더합니다 |
| TestFlight 빌드 90일 | 내부 테스터의 앱이 멈춥니다. 만료된 빌드는 prepare도 쓰지 않습니다(`has expired`) | `hjv-release check`가 75일에 알립니다. App Store 설치본과는 상관없습니다 |
| Play 서명 앱이 다른 출처로 설치됨(기기 옮기기 도구, adb 등, 확인 필요) | 앱은 설치 출처로 채널을 정하므로 android 항목으로 판정되고, APK 업데이트는 서명 인증서가 달라 설치되지 않습니다([distribution.md](distribution.md)) | 그 사용자는 앱을 지우고 Play에서 다시 설치합니다 |
| 키 유출 | Play 키: 테스트 트랙과 내부 앱 공유에 올릴 수 있음(앱 단위 권한). App Manager 키: 메타데이터, 제출, 출시. **App Store Connect 팀 키는 앱 하나로 줄일 수 없어 Apple 팀의 모든 앱에 같은 역할로 걸립니다**(키를 만들 때 확인). Developer 키: TestFlight 업로드(팀의 모든 앱) | Play: Cloud 콘솔에서 키 삭제·교체. ASC: 키 폐기 → 새 키 → 시크릿 |

App Store Connect 오류는 Apple의 코드와 설명을 출력합니다(비밀값은 `<masked>`로 가림). 401은 키 문제, 403은 키 권한이나 기다리는 계약입니다.

## 한 번 할 일 (소유자)

### Play Console

| # | 할 일 |
|---|---|
| P1 | 테스트 및 출시 → 테스트 → 비공개 테스트 → Alpha에서 지금 출시(1.9.0)의 versionCode와 상태를 봅니다. 그 줄을 출시 기록에 손으로 넣을 때 씁니다 |
| P2 | 소유자 계정이 내부 테스트에 참여했다면 나가고, Alpha 참여 링크로 다시 참여합니다(내부 테스트에 참여한 사람은 비공개 테스트 버전을 받지 않음). 확인용 Android 기기 하나는 APK 설치본으로 둡니다 |
| P3 | 서비스 계정이 없으면 만들고([ci.md](ci.md)의 "Google Play 업로드" 6단계), HJViewer에 '앱을 테스트 트랙으로 출시' 권한으로 초대합니다. 목록을 읽다가 권한 오류가 계속 나면 '앱 정보 보기(읽기 전용)'를 더합니다 |
| P4 | 게시 개요에서 관리형 게시를 끕니다 |
| P5 | 앱 콘텐츠: 앱 액세스(데모 계정과 데모 PC), 데이터 보안(1.9.1의 앱 안 계정 삭제, 삭제 요청 URL) |
| P6 | Android 개발자 인증: 패키지와 사이드로드 APK의 서명 키(업로드 키)가 등록됐는지 봅니다 |
| P7 | Alpha 테스터 목록은 소유자가 관리합니다(처음 한 번, 그 뒤 사람이 바뀔 때마다). 테스트 및 출시 → 테스트 → 비공개 테스트 → Alpha의 트랙 관리 → 테스터 탭에서 이메일 목록(또는 Google 그룹)에 더하고 뺍니다. 테스터는 그 탭의 참여 링크로 각자 참여해야 받습니다(내부 테스트에 참여한 사람은 먼저 나감, P2). 테스터의 이메일은 저장소, 채팅, `build/`에 적지 않습니다 |

### App Store Connect

제출 분석 `build/appstore-listing/submit/SUBMIT-1.9.0.md`의 순서를 따릅니다.

| # | 할 일 |
|---|---|
| A1 | 비즈니스: 최신 계약, DSA(거래자 아님), 대한민국 규정 준수 정보 |
| A2 | 앱 정보: 카테고리, 콘텐츠 권한(학교 허락 뒤), 연령 등급 |
| A3 | 가격 무료, 판매 지역 대한민국만, Mac·Vision Pro 끔. 배포 방법은 **공개**로 둡니다. **비공개는 고르지 않습니다**(비공개 = 맞춤형 앱, 승인 뒤 되돌릴 수 없음). 등록되지 않은 앱(Unlisted)은 콘솔에서 고르는 값이 아니라, 제출 뒤 요청이 승인되면 Apple이 바꿉니다(위 "첫 App Store 버전만" 5–6단계) |
| A4 | App Privacy 답과 게시(API 없음) |
| A5 | 첫 App Store 버전으로 낼 판의 화면의 스크린샷(위 "첫 App Store 버전만" 1번의 B5) |
| A6 | 사용자 및 액세스 → 통합 → 팀 키에서 키 둘을 만들고, 이 순서로 바꿉니다. ① **Developer** 역할(TestFlight 업로드용) → 저장소 시크릿 교체(`gh secret set ASC_KEY_ID -R $R`, `gh secret set ASC_PRIVATE_KEY -R $R < AuthKey_<키 ID>.p8`). ② **App Manager** 역할(스토어 실행용) → environment 시크릿 `ASC_STORE_KEY_ID`, `ASC_STORE_PRIVATE_KEY`. ③ 키를 바꾼 뒤의 첫 TestFlight 업로드 실행(Developer 키)과 G6의 ios status(App Manager 키)가 성공한 것을 봅니다. ④ 개발 PC의 로컬 App Store Connect 도우미가 쓰는 옛 Admin 키 파일의 쓰임을 정리한 뒤 Admin 키를 폐기합니다. 배포 인증서·프로파일을 갱신할 때는 웹이나 그때만 쓰는 임시 Admin 키를 씁니다(API로 갱신하려면 Admin 권한이 필요). `.p8`은 한 번만 받을 수 있으므로 넣은 뒤 지웁니다. 팀 키는 팀의 모든 앱에 걸리므로, 팀에 다른 앱이 있는지 소유자가 봅니다. Developer 키로 altool 업로드가 안 되면 메인 세션에 알립니다 |
| A7 | 시크릿 `REVIEW_*`(아래) |

### GitHub (에이전트가 소유자 승인 뒤, 시크릿 값은 소유자가)

| # | 할 일 |
|---|---|
| G1 | 공개 저장소 push(1.9.1은 push됨. push마다 그 전에 `hjv-release floor --live`). 첫 스토어 실행 전에 브라우저에서 `store/appstore/ko/support_url.txt`의 주소(`docs/privacy.md#10-문의와-변경`을 퍼센트 인코딩한 것)를 열어, GitHub의 앵커가 한국어 10절 '문의와 변경'에 닿는지 봅니다(App Store의 한국어 지원 URL) |
| G2 | 비공개 저장소: 고정할 SHA가 공개 main의 조상인지 확인(`git merge-base --is-ancestor <sha> origin/main`, 포크의 커밋도 `@SHA`로 풀리기 때문). 그다음 `build.yml`과 `store.yml`의 고정 SHA, `store-releases.txt`, README |
| G3 | **첫 `store.yml` 실행 전에** environment `store`: Settings → Environments → New environment `store` → Deployment branches and tags를 Selected로, `main`만. 먼저 돌리면 GitHub가 배포 브랜치 제한 없는 `store`를 저절로 만듭니다 |
| G4 | 시크릿(소유자의 터미널, 아래)과 변수 `REVIEW_DEMO_UNTIL`. 저장소 시크릿에 남은 `PLAY_SA_*`를 지웁니다(아래 명령). `gh secret list -R $R`(`--env` 없이)에 `PLAY_SA_*`, `ASC_STORE_*`, `REVIEW_*`가 없어야 합니다. 있으면 store.yml이 그 값을 넘겨 environment로 옮기지 않은 것이 드러나지 않고, 빌드 실행의 `secrets: inherit` 범위에도 남습니다 |
| G5 | 저장소 설정(아래) |
| G6 | 읽기만 하는 확인: `store.yml platform=android action=status`, `platform=ios action=status`(version 없이). 키가 맞는지, alpha의 1.9.0이 보이는지 봅니다. 그 전에 G4의 `gh secret list -R $R`을 한 번 더 봅니다(environment 값이 잡에 닿는지가 이 실행으로 확인됨). Google Play·App Store 잡의 첫 단계에 찍힌 `job.workflow_ref is …` 줄이 `@<고정한 40자리 SHA>`로 끝나는지도 봅니다. `@refs/heads/main`이나 태그로 끝나면 `store.yml`이 SHA에 고정하지 않은 것입니다. 잡은 이 줄로 멈추지 않습니다(`job.workflow_sha`는 늘 40자리라서, 잡의 검사로는 고정했는지 알 수 없음) |
| G7 | 첫 스토어 실행에 낼 판의 빌드와 태그 뒤(1.9.1은 태그 `client-1.9.1`이 있음), 첫 upload·prepare 전에 같은 입력으로 `dry_run=true`를 한 번씩 돌립니다. 요약에 upload는 validated…nothing was released, prepare는 계획만 있고 비밀값이 없는지 봅니다 |

**시크릿과 변수** (이름만. 값은 채팅, 저장소, `build/`에 두지 않습니다)

| 어디 | 이름 | 내용 |
|---|---|---|
| 저장소 시크릿(지금처럼) | `ASC_APP_ID`, `ASC_ISSUER_ID` | TestFlight 업로드 실행과 스토어 실행이 함께 씀 |
| 저장소 시크릿 | `ASC_KEY_ID`, `ASC_PRIVATE_KEY` | TestFlight 업로드 실행의 업로드와 첫 단계의 확인(Developer 키로 바꿈) |
| environment `store` | `PLAY_SA_EMAIL`, `PLAY_SA_PRIVATE_KEY` | 서비스 계정 JSON 키의 두 값. 저장소 시크릿에 있으면 environment로 옮깁니다 |
| environment `store` | `ASC_STORE_KEY_ID`, `ASC_STORE_PRIVATE_KEY` | App Manager 키 |
| environment `store` | `REVIEW_DEMO_USER`, `REVIEW_DEMO_PASSWORD` | 심사용 데모 계정(review.conf의 ID)과 비밀번호. review.conf를 새로 만들 때마다 바꿈 |
| environment `store` | `REVIEW_DEMO_PC_ID`, `REVIEW_DEMO_PC_PASSWORD` | 데모 Windows PC의 ID(16바이트까지)와 영구 비밀번호(16–32자 ASCII). prepare·submit 모두 필요합니다. 데모 PC가 생기기 전에는 `dry_run=true`만 됩니다 |
| environment `store` | `REVIEW_CONTACT_FIRST_NAME`, `REVIEW_CONTACT_LAST_NAME`, `REVIEW_CONTACT_PHONE`, `REVIEW_CONTACT_EMAIL` | App Review 연락처 |
| 저장소 변수(비밀 아님) | `REVIEW_DEMO_UNTIL` | review.conf의 끝나는 날(예: 2026-11-30, 그날 0시 KST에 끝남). Checks가 14일 전부터 경고하고, 끝난 뒤에는 upload·submit을 멈춤 |

```sh
R=yonsei-projecttf/hjviewer-private
gh secret set REVIEW_DEMO_USER --env store -R $R                    # 값은 입력 창에 붙여 넣음
gh secret set ASC_STORE_PRIVATE_KEY --env store -R $R < AuthKey_<키 ID>.p8
jq -r .client_email key.json | gh secret set PLAY_SA_EMAIL --env store -R $R
jq -r .private_key  key.json | gh secret set PLAY_SA_PRIVATE_KEY --env store -R $R
gh secret list --env store -R $R                                    # 이름과 날짜만 보임
gh secret delete PLAY_SA_EMAIL -R $R                                # 저장소 시크릿의 옛 사본(environment로 옮긴 뒤)
gh secret delete PLAY_SA_PRIVATE_KEY -R $R
gh secret list -R $R                                                # PLAY_SA_*, ASC_STORE_*, REVIEW_*가 없어야 함
gh variable set REVIEW_DEMO_UNTIL -R $R --body 2026-11-30           # 그날 0시(KST)에 끝남으로 셈
```

environment를 선언하지 않은 잡(빌드 실행의 잡)은 environment 시크릿을 받지 않습니다. 불려 온 워크플로의 잡이 environment를 선언하면 그 environment의 값을 받습니다. 그래서 `store.yml`은 이 이름들을 넘기기만 하고, 값은 environment에서 옵니다.

**저장소 설정** (2026-10-26에 읽은 지금 값 → 권장)

| 설정 | 지금 | 권장 |
|---|---|---|
| 비공개 저장소 main 보호 | 없음 | 규칙 하나: 강제 push와 삭제만 막습니다. PR 요구는 넣지 않습니다(Release record 잡의 커밋이 막힘) |
| Actions의 SHA 고정 필수 | 두 저장소 모두 꺼짐 | 비공개 저장소에서 켭니다. 공개 저장소도 켤 수 있습니다(`server.yml`의 마지막 태그 고정 `dtolnay/rust-toolchain@stable`을 1.9.2에서 SHA로 바꿈) |
| 조직 2FA 필수 | 꺼짐 | 구성원이 모두 2FA를 켰는지 본 뒤 켭니다 |
| 아티팩트 보관 상한, 기본 GITHUB_TOKEN 권한 | 90일, read | 그대로 |

## 비용

| 실행 | 잡 시간(분) | Linux 요금으로 |
|---|---|---|
| status | 1 | 약 $0.006 |
| upload, submit, release | 3–5 | 약 $0.02–0.03 |
| prepare | 2–21(빌드 처리 대기) | 약 $0.01–0.13 |

빌드 실행의 iOS 잡은 첫 단계 검사(sparse checkout, 기록 확인: 몇 초짜리 GET)만큼 늡니다. 1.9.2부터 TestFlight 업로드는 빌드하지 않는 실행이 macOS에서 합니다(기록·빌드 실행·App Store Connect 확인, IPA 받기, altool 업로드). 잰 값은 아직 없습니다. 첫 실행 뒤 Run summary 잡의 표에서 그 단계들의 실측을 [ci.md](ci.md)의 시간 표에 적습니다.

## 시험

네트워크와 스토어 키 없이 `scripts/ci.sh client`가 돌립니다.

```sh
python3 -I scripts/check-store-text.py
python3 -I -B -m unittest discover -s scripts -p 'test_check_store_text.py'
python3 -I -B -m unittest discover -s scripts -p 'test_play_upload.py'      # 가짜 Play API
python3 -I -B -m unittest discover -s scripts -p 'test_store_record.py'     # 가짜 contents·runs API
python3 -I -B -m unittest discover -s scripts -p 'test_store_workflows.py'  # 워크플로의 규칙
node --test scripts/asc-store.test.mjs                                      # 가짜 App Store Connect
```

키 없이 할 일을 보는 dry run(네트워크 없음, 비밀값 없음):

```sh
python3 -I scripts/play-upload.py --package kr.ac.yonsei.hjviewer --aab <AAB> --mode dry-run --version-code <versionCode> --notes-file store/play/ko-KR/release_notes.txt
node scripts/asc-store.mjs submit --version 1.9.1 --build <CFBundleVersion> --dry-run
```
