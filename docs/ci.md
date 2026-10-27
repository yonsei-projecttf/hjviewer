# CI와 앱 빌드

**지금은 push나 태그로 도는 GitHub Actions를 쓰지 않습니다**(2026-10-10부터 일시 중지). 검증은 개발 PC에서 `scripts/ci.sh`로 합니다. 개발 PC는 운영 서버와 같은 환경으로 맞춥니다. Ubuntu 24.04에 [server-runbook.md](server-runbook.md) 3-1단계와 같은 Docker 패키지를 설치합니다(docker.io 29.1.3, docker-compose-v2 2.40.3, docker-buildx 0.30.1).

두 워크플로(`server.yml`, `client-build.yml`)는 저장소에 남아 있습니다. 다만 push나 태그로는 실행되지 않고, Actions 화면에서 직접 실행하거나 비공개 빌드 저장소가 `client-build.yml`을 불러 쓸 때만 돕니다(아래 "앱 빌드"). 다시 켜려면 `on:`에 `push` 트리거를 되돌립니다.

## 개발 PC의 CI: `scripts/ci.sh`

```bash
scripts/ci.sh            # = all: server + deploy만 (처음에는 15분 안팎, 이후에는 캐시로 더 빠름). 나머지 단계는 따로 실행
scripts/ci.sh prepush    # push 전 관문: 깨끗한 트리에서 엄격 모드로 여섯 단계 전부를 차례로, 단계마다 로그 (아래)
scripts/ci.sh server     # 서버 테스트만 (hbbs·hbbr, hjv-api, hjv-relay, hjv-release)
scripts/ci.sh deploy     # 배포 시험만 (server를 먼저 돌려 이 트리의 hbbs·hbbr·hjv-api가 있어야 함. hjv-probe·hjv-load는 이 단계가 빌드)
scripts/ci.sh client     # 앱 시험: Play 업로드 스크립트, 스토어 실행(글 검사, 출시 기록, App Store Connect, 워크플로 규칙), 공개키 확인, actionlint, 릴리스 기록, hbb_common WebSocket, Rust 1.75 check, 공유 벡터, 순수 Dart (server를 먼저 돌려야 hjv-release와 맞춰 봄)
scripts/ci.sh e2e        # 방화벽 E2E (docs/e2e.md): sudo, 클라이언트 빌드 포함. server를 먼저 돌려야 함
scripts/ci.sh wol        # 원격 켜기 E2E (docs/e2e.md): sudo 없이, 클라이언트 빌드 포함. server를 먼저 돌려야 함
scripts/ci.sh leak       # push 전 검사: 로컬 목록 build/leak-patterns.txt의 값이 올라갈 내용에 없는지
```

아직 들어오지 않은 작업 묶음이 필요한 부분이나 이 PC에 없는 도구가 필요한 부분은 단계가 "skipped: …" 한 줄을 찍고 건너뜁니다. 패치가 들어온 부분은 필수가 되어, 그 패치가 빠지면 건너뛰지 않고 실패합니다. hbbs 패치 0004의 `hjv_pk` 시험, 서버 패치 0006의 `access` 시험, 클라이언트 패치 0033의 `hjviewer_account_test.dart`, 클라이언트 패치 0035가 있을 때의 원격 켜기 E2E, 서버 패치 0005가 있을 때 hbbs의 최소 버전 옵션이 그렇습니다.

- **건너뛴 것은 셉니다.** `scripts/ci.sh`가 직접 건너뛴 것과, E2E 스크립트·운영 설정 검사·컨테이너 설치 시험이 찍은 `skipped:` 줄을 모두 셉니다. 마지막 줄은 `CI passed: <단계> (<n> skipped)`이고, n이 0이 아니면 그 앞에 목록이 나옵니다. cargo는 통과한 시험의 출력을 보여 주지 않으므로, 이 PC에서 돌 수 없을 때 스스로 건너뛰고 통과하는 시험이 있는 hjv-api·hjv-relay의 시험(systemd 도구가 없을 때, `HJV_SERVER`를 넣은 빌드일 때)과 hjv-relay 묶음 시험(`scripts/package-relay.sh`가 빈 루트 확인을 못 할 때)은 `--show-output`으로 돌려 그 `skipped:` 줄도 셉니다. hjv-relay의 네트워크 네임스페이스 시험을 `HJV_RELAY_NETNS`로 건너뛸 수 있게 한 실행은 그 시험의 줄이 `skipped:`로 시작하지 않으므로 `scripts/ci.sh`가 하나로 셉니다.
- **엄격 모드**: `HJV_CI_STRICT=1`이면 건너뛴 것이 하나라도 있는 실행은 끝에서 그 목록과 함께 실패합니다(종료 1). `prepush`는 늘 엄격 모드입니다. 개발 중에 도구가 없어 건너뛰는 것(예: actionlint)은 엄격 모드가 아닐 때만 통과로 끝납니다.
- **건너뛴 줄이 있는 통과는 push 전의 통과가 아닙니다.** push 전에는 `scripts/ci.sh prepush`를 쓰거나, 단계를 따로 돌렸다면 마지막 줄이 모두 `(0 skipped)`인지 봅니다. 건너뛴 것이 없는 실행은 E2E의 `PASS` 줄이나 클라이언트 단위 시험의 통과 수가 마지막 전체 실행보다 적어도 실패합니다(아래 "e2e와 wol").
- **한 번에 한 실행**: 한 CI 폴더(`HJV_CI_DIR`, 기본 `build/ci`)에서는 한 실행만 돕니다. 실행은 `build/ci/ci.lock`을 `flock`으로 잡고, 그동안 시작한 다른 실행은 바로 멈춥니다. 잠금은 실행이 끝나면 풀리고, 실행이 띄운 프로그램이 남아도 잠금을 쥐지 않습니다.
- **시간**: 단계 안의 일마다 찍는 `== ` 줄 아래에 `-- <시각>, <초> s in, load <부하>; the step before: <초> s` 줄이 붙습니다. 그 일을 시작한 시각, 실행을 시작한 뒤 지난 초, 그때 기계의 부하(`/proc/loadavg`의 1분 값. 다른 일이 기계를 같이 쓰면 같은 일도 느려집니다), 앞 일이 걸린 초입니다. 실행이 끝나면 마지막 줄 `CI passed: …` 바로 앞에 일을 걸린 시간 순으로 늘어놓은 표(`-- time by step, <초> s in all:`)가 나옵니다. 마지막 줄, prepush 로그의 머리와 `EXIT` 줄은 바뀌지 않습니다. 깨끗한 컨테이너 설치 시험의 `== container: …` 줄에도 시각과 지난 초가 붙고(시각은 컨테이너의 시계라 UTC이고, 그래서 `UTC`를 붙임), deploy 단계는 이미지 빌드가 걸린 초와 Docker 캐시에서 가져온 일의 수(`FROM`은 캐시에서 가져오는 일이 아니라 세지 않으므로, 모두 캐시면 `10 of 10`처럼 두 수가 같음), 그 빌드가 고른 기반 이미지의 digest를 찍습니다(`image build: …`). 이미지를 다시 빌드하면 몇 분이 더 걸리므로, 느린 deploy 실행은 이 줄로 가려냅니다. E2E 스크립트의 부분마다 걸린 시간은 [e2e.md](e2e.md)의 "실행"에 있습니다.

| 단계 | 하는 일 |
|---|---|
| client | 먼저 오프라인 시험을 돌립니다. Google Play 업로드 스크립트(`scripts/test_play_upload.py`, 가짜 Play API와 버리는 키), 스토어 실행([store-release.md](store-release.md))의 것들, 앱 빌드의 공개키 확인(`scripts/test_check_build_keys.py`, 아래 "계정 기능의 공개키")입니다. 스토어 실행의 것은 `store/`의 글 검사(`scripts/check-store-text.py`와 그 시험), 출시 기록(`scripts/test_store_record.py`, 가짜 contents API), 워크플로의 스토어 규칙(`scripts/test_store_workflows.py`), App Store Connect 스크립트(`node --test scripts/asc-store.test.mjs`, 가짜 App Store Connect와 버리는 키. Node 20 이상이 없으면 건너뛴 것으로 셉니다)입니다. 네트워크와 자격 증명이 필요 없습니다. actionlint가 있으면 워크플로도 검사합니다. server 단계가 만든 `hjv-release` 릴리스 빌드(`build/ci/target-hjv-release`)가 있으면 공개키 확인 시험이 업데이트 키 파일을 도구와도 맞춰 봅니다(없으면 건너뛴 것으로 셉니다). 그다음 `build/ci/upstream`에 upstream rustdesk를 새로 받아 `client/patches`를 적용하고, 앱 자신의 Dart 파일(`hjv_*.dart`, `hjviewer_*.dart`)에서 날 글자 크기(`fontSize: <숫자>`) 줄이 `scripts/hjv-fontsize-baseline.txt`보다 늘어난 파일을 경고로 알린 뒤(실패가 아님. 검사 스크립트 `scripts/hjv-fontsize-check.sh`의 자체 시험이 틀리면 실패), 릴리스 기록의 시험(`scripts/test_release_info.py`: 패치를 적용한 트리에 워크플로가 요구하는 Dart 시험이 있는지, 실제 APK와 IPA로 매니페스트 확인. 실제 패키지는 `scripts/testdata/release-samples`에 둔 1.9.1의 것입니다. 아래 "개발 PC에서 확인하기"), `hbb_common`의 WebSocket 테스트(프록시, `TCP_NODELAY`), 앱 빌드와 같은 Rust 1.75로 `cargo +1.75.0 check --features linux-pkg-config,hjv-e2e`, 클라이언트 코어가 `tools/hjv-release/tests/vectors`의 공유 벡터를 모두 참조 모델대로 받고 거절하는지(`--lib hjviewer_update::vectors -- --ignored`, 출력은 `build/ci/client-vectors.log`. 돈 사례 수가 manifest의 수와 같아야 하고 하한 `VECTORS_LEAST`도 봅니다), 앱의 순수 Dart 테스트(`CLIENT_DART_TESTS`: iPhone 한글 입력과 키보드 기록, 계정 화면의 `hjviewer_account_test.dart`, 업데이트 화면의 `hjv_update_test.dart`, 제어 전용 Android의 `hjviewer_android_test.dart`, 최고운영자 화면의 `hjviewer_admin_test.dart`, 1.8.0 출입 화면의 `hjviewer_access_test.dart`, 1.9.0의 토스트·ID 제안·휴대폰 막대·고정 설정·디자인 토큰·앱 잠금 테스트 `hjviewer_toast_test.dart`, `hjviewer_suggestions_test.dart`, `hjviewer_phone_test.dart`, `hjviewer_pins_test.dart`, `hjviewer_style_test.dart`, `hjviewer_applock_test.dart`, 그리고 1.9.0 PC 연결 창의 `hjviewer_cm_test.dart`, `hjviewer_support_test.dart`, `hjviewer_cm_fit_test.dart`(overlay의 Pretendard를 읽음), 세션이 끝날 때 잠그는 규칙의 `hjviewer_end_lock_test.dart`, 원격 준비의 `hjviewer_ready_test.dart`, 휴대폰 세션의 제스처 안내와 바꾸기의 `hjviewer_session_test.dart`, 들어온 연결 목록의 `hjviewer_incoming_test.dart`, 홈 계정 줄의 `hjviewer_home_test.dart`, 데스크톱 키보드(PC 카드의 키, 초점 고리) `hjviewer_keys_test.dart`, 화면 낭독기가 듣는 선택과 상태(선택됨, 제목, 라이브 영역) `hjviewer_semantics_test.dart`, 세션 창(파일 전송, 채팅)의 색 `hjviewer_session_look_test.dart`, 로그인한 기기 목록의 화살표가 제목 줄에 놓이는지 `hjviewer_title_line_test.dart`, Windows 11 최대화 단추가 Windows에 알리는 사각형과 끄는 옵션, 연결 창·설치 창 제외 `hjv_caption_test.dart`, 휴대폰 글자 크기 130%의 `hjviewer_text_scale_test.dart`, 1.9.1 데스크톱 설정의 일상 탭과 고급 설정 나누기 `hjviewer_settings_split_test.dart`)를 돌립니다. 1.75 툴체인이 없으면 실패합니다(`rustup toolchain install 1.75.0 --profile minimal`). Flutter는 앱 빌드와 같은 3.24.5를 처음 한 번 받아 Flutter 릴리스 목록의 SHA-256과 맞춰 봅니다. 생성된 bridge가 필요한 제스처·화질 표시 테스트와 계정·최고운영자 화면 위젯, 업데이트 게이트, 1.9.0 대화상자 틀의 테스트는 GitHub Actions의 Dart tests 잡에서만 돕니다. |
| server | `build/ci/upstream`에 upstream rustdesk-server를 새로 받아 `server/patches`를 적용하고, 그 시험 전체를 릴리스 빌드로 돌립니다. 라이브러리와 실행 파일(hbbs, hbbr)의 단위 시험, 신호·중계 통합 테스트(`tcp_signaling`, `relay_pairing`), `protobuf_recursion`, hbbs 패치 0004의 `hjv_pk` 테스트입니다. 패치가 바꾼 코드의 upstream 단위 시험도 이렇게 함께 돕니다. `hjv_pk` 테스트가 쓰는 벡터(서버 키, 기기 키, 서명된 답)가 hjv-api 시험의 것(`server/api/src/hbbs.rs`)과 같은지도 봅니다. 다르면 두 시험이 각각 통과해도 hjv-api가 hbbs의 답을 읽는다는 보장이 없으므로 실패합니다. `hjv-probe`, `hjv-load`도 빌드합니다. 서버 패치 0005의 최소 컨트롤러 버전 시험(`min_version`)도 이렇게 돕니다. 서버 패치 0006의 출입증 시험(`tests/access.rs`와 `src/hjv_access.rs`의 단위 시험)은 필수입니다. 그 시험이 읽는 벡터 사본(`tests/vectors/access-tickets.json`)이 hjv-api의 공유 벡터(`server/api/tests/vectors/access-tickets.json`)와 바이트까지 같은지 보고, 단위 시험은 `HJV_ACCESS_VECTORS`로 hjv-api의 파일을 직접 읽어 고정한 SHA-256과 맞춰 봅니다([access-tickets.md](access-tickets.md) 8절). 이어서 hjv-api(`server/api`)의 시험과 릴리스 빌드, hjv-relay(`relay/`)의 시험을 돌립니다. 끝으로 `tools/hjv-release`의 `cargo fmt --check`, clippy(`-D warnings`), 시험(공유 벡터를 참조 모델로 다시 만들어 맞춰 보는 `tests/vectors.rs` 포함, python3 필요), 릴리스 빌드를 합니다(`build/ci/target-hjv-release`, client와 e2e 단계가 씀). hjv-relay의 네트워크 네임스페이스 시험은 무권한 네임스페이스가 없으면 건너뛰지 않고 실패합니다. 무권한 네임스페이스를 막아 둔 GitHub 러너(`server.yml`)에서는 건너뛸 수 있고, 다른 곳에서도 `HJV_RELAY_NETNS=`로 비우면 건너뜁니다. |
| deploy | 작업 트리 사본(`build/ci/src`, 커밋하지 않은 변경 포함)으로 런북의 배포를 Docker에서 그대로 합니다. B 경로는 hbbs/hbbr을 127.0.0.1에 띄우고, 그 앞에 `deploy/nginx-hjviewer.conf`와 `deploy/apache-hjviewer.conf`를 차례로 둡니다. nginx는 기존 사이트에 `deploy/nginx-hjviewer-locations.conf`를 include 한 줄로 넣은 경우도 함께 시험합니다. 이 기존 사이트는 HTTP/2를 켜고 모든 경로를 로그인 페이지로 보내는 WordPress 같은 사이트입니다. 마지막으로 런북 B 경로의 Release 패키지를 `scripts/package-server.sh`로 만들고, 그 안의 systemd 유닛(hbbs, hbbr, hjv-api의 소켓과 서비스)을 띄워 같은 nginx 뒤에서 시험합니다. 개발 PC를 건드리지 않도록 런타임 유닛, 임시 사용자, 별도 디렉터리로 바꾼 이름만 다르고, 보안 옵션과 자원 상한은 배포 파일 그대로입니다. hjv-api의 자격 증명은 버리는 시험 값을 평문 파일(`LoadCredential=`)로 주고, 저널 네임스페이스 줄은 빼고 띄웁니다. 자격 증명은 유닛의 `LoadCredentialEncrypted=` 줄을 따라 만들며, 1.8.0부터는 일곱째인 출입증 키 `hjviewer-api-ticket-key`(`hjv-api keygen ticket --kid 1`의 버리는 값)도 줍니다. 유닛에 `scripts/ci.sh`가 시험 값을 모르는 자격 증명이 생기면 실패합니다(`api_test_credentials`). 웹 서버는 운영 서버와 같은 Ubuntu 패키지를 내려받아 풀어서 실행할 뿐, 설치하지 않습니다. 그다음 A 경로(Caddy)를 같은 이미지와 키로 띄웁니다. 끝으로 hjv-api의 운영 설정값, 계정 API의 nginx 한도, 깨끗한 Ubuntu 24.04 컨테이너에 서버 묶음을 설치하는 시험을 합니다(아래). Docker 시험 전에는 hjv-relay 묶음(`scripts/package-relay.sh`)을 만드는 시험을 돌립니다. `relay/tests/package.rs`의 `--ignored` 시험으로, 정적 빌드, 그 묶음만의 SHA256SUMS, 현장 시험 묶음을 봅니다. 다른 단계는 이 시험을 돌리지 않습니다. 이 시험은 컴파일된 트리의 `scripts/package-relay.sh`로 그 트리를 묶으므로, 출력의 `commit:` 줄이 이 트리의 HEAD이고 `wrote … to` 줄이 이 트리의 `build/package-relay/`인지도 봅니다. 아니면 실패합니다. x86_64-unknown-linux-musl용 정적 빌드라서 그 Rust 타깃과 musl-gcc가 있어야 하고, 없으면 실패합니다(`rustup target add x86_64-unknown-linux-musl`, Ubuntu는 `apt-get install musl-tools`). |
| e2e | `tools/e2e/firewall-e2e.sh`를 strict와 open으로 돌립니다([e2e.md](e2e.md)). Caddy는 버전과 SHA-256을 `scripts/ci.sh`에 고정해 처음 한 번 받습니다. upstream rustdesk를 새로 받아 시험 서버 `hjv.test`용 클라이언트(`hjv-e2e` 기능)를 빌드하고, PC 쪽 실행 파일을 `/usr/local/lib/hjviewer-e2e/rustdesk`에 설치합니다(리눅스는 `/usr` 아래 실행 파일만 설치형으로 봅니다). 계정 코드가 패치에 들어오고 `firewall-e2e.sh`에 계정 단계(`HJV_E2E_ACCOUNT`)가 생기면, 클라이언트에 E2E 시험 공개키를 넣어 빌드하고, 그 빌드로 클라이언트의 HJViewer 코드 단위 시험 전체(`--lib hjv`: 계정 코드, 컨트롤러 쪽 훅, `hjviewer.rs`, 켜기 정보 보고)와 계정 시나리오도 돌립니다. 그 전에는 계정 공개키 없이 빌드합니다. 이 단계는 `E2E_CLIENT`를 받지 않습니다(주면 바로 실패). 패치에서 새로 빌드한 클라이언트여야 그 빌드의 단위 시험과 계정 시나리오가 함께 돌기 때문입니다. 최소 버전의 두 부분도 더합니다([e2e.md](e2e.md#최소-버전-단계-160)). hbbs에 서버 패치 0005의 `--hjv-min-controller-file`이 있으면 V1·V2·V4를 돌리고, `server/patches`에 0005가 있는데 hbbs에 그 옵션이 없으면 실패합니다(전의 server 단계가 빌드한 hbbs). 패치에 업데이트 코드(`src/hjviewer_update`)가 있으면 클라이언트와 그 단위 시험을 업데이트 시험 키(`hjv-release test-keys --env`와 같은 키, 하한 `1:0`)로 빌드합니다. 그래서 업데이트 코드의 시험이 정책을 따르는 빌드로 돕니다. 업데이트 키 없는 빌드에서 정책 기능이 꺼져 있는지 보는 시험은 키가 든 빌드에서는 첫 줄에서 끝나므로, 업데이트 코드의 시험(`--lib hjviewer_update`)을 업데이트 키만 뺀 빌드로 한 번 더 돌립니다(출력은 `build/ci/e2e-unit-hjviewer_update-nokeys.log`). 같은 빌드 폴더에서 클라이언트 크레이트의 시험을 다시 컴파일하므로 1분쯤 더 걸립니다. 다시 컴파일하지 않았으면 키가 빠지지 않은 것이므로 실패합니다. `tools/hjv-release`가 있으면 빌드해(`build/ci/target-hjv-release`) 시험 정책에 서명하게 하고 V5–V7·V9를 돌립니다. 셋 중 없는 것은 건너뛴 것으로 셉니다. 스크립트의 출력은 `build/ci/e2e-strict.log`, `e2e-open.log`에, 단위 시험의 출력은 `build/ci/e2e-unit-hjv.log`에도 남습니다. 단계는 모드마다 `PASS` 줄의 수와 단위 시험의 통과 수를 찍고, 건너뛴 것이 없는데 하한보다 적으면 실패합니다(아래 "e2e와 wol"). sudo가 비밀번호 없이 되어야 합니다(네트워크 네임스페이스, iptables). |
| wol | `tools/e2e/wol-e2e.sh`(원격 켜기 E2E, [e2e.md](e2e.md#원격-켜기-단계-160))를 sudo 없이 돌립니다. e2e와 같은 방법으로 클라이언트를 새로 받아 빌드하고, 그 빌드에서 켜기 정보 보고(`src/hjviewer_wol`)의 단위 시험을 돌린 뒤, e2e와 같은 Caddy·hjv-api·가짜 SSO에 E2E의 명령 키로 빌드한 hjv-relay(`HJV_SERVER=hjv.test`)를 더해 스크립트에 넘깁니다. 클라이언트에 `--hjv-e2e-wol-report` 명령이 없을 때, 패치에 켜기 정보 보고(클라이언트 패치 0035)가 없으면 E2E를 건너뛰고, 있으면 실패합니다. `E2E_CLIENT=<빌드한 클라이언트>`를 주면 클라이언트 빌드와 단위 시험을 건너뛰고 그 파일로 돌립니다. 이것도 건너뛴 것으로 세므로 엄격 모드에서는 실패합니다. `prepush`에서는 같은 실행의 e2e 단계가 빌드한 클라이언트를 손으로 값을 주지 않아도 그대로 씁니다. 그 빌드의 입력(`client/` 전체, upstream 고정값과 받는 스크립트, 시험 서버 키, 시험 공개키, 빌드 설정, 툴체인)과 실행 파일의 해시가 e2e 단계가 남긴 기록과 같을 때만이고, 단위 시험은 e2e 단계의 `--lib hjv`(켜기 정보 보고의 시험 포함)로 갈음합니다. 스크립트의 출력은 `build/ci/wol-e2e.log`에도 남고, 단계는 `PASS` 줄의 수(직접 빌드했으면 단위 시험의 통과 수도)를 찍으며, 건너뛴 것이 없는데 하한보다 적으면 실패합니다. 무권한 사용자 네임스페이스가 있어야 하고, 없으면 실패합니다. 스크립트의 네트워크와 포트는 그 실행만의 네임스페이스 안에 있어 방화벽 E2E와 겹치지 않지만, 두 단계는 같은 클라이언트 체크아웃과 빌드 폴더를 쓰므로 `scripts/ci.sh`는 한 번에 하나만 돌립니다. |
| leak | push 전 검사입니다(아래 "push 전 검사"). |

e2e·wol 단계가 시험 스크립트에 넘기는 값은 `HJV_E2E_WORK`, `CADDY`, `HBBS`, `HBBR`, `PROBE`, `CLIENT`, `HJV_E2E_KEYS`이고, e2e에는 `PC_CLIENT`(설치 경로의 PC 실행 파일), 계정 E2E에는 `HJV_E2E_ACCOUNT=1`, `HJV_API`, `HJV_FAKE_SSO`, 최소 버전에는 `HJV_E2E_MIN_CONTROLLER=1`(hbbs의 0005), `HJV_E2E_UPDATE_KEYS=1`(업데이트 시험 키로 빌드한 클라이언트), `HJV_RELEASE`(`tools/hjv-release`), wol에는 `HJV_API`, `HJV_FAKE_SSO`, `HJV_RELAY`가 더해집니다. wol은 PC 실행 파일을 설치하지 않습니다. PC 쪽 명령의 네임스페이스 안에서만 `/usr/local/lib`에 실행 폴더의 사본이 보입니다. `HJV_E2E_KEYS` 폴더에는 E2E의 시험 키가 있습니다. 같은 바이트를 32번 되풀이한 seed로 만든 키라서(봉인 `e2`, 목록 키 1·2 `e3`·`e4`, 명령 키 `e5`, 기록 키 `e6`) 출시 빌드의 공개키 확인이 거부합니다. `*.secret`은 비밀 쪽(base64), `seal.pub`, `trust.pub`, `command.pub`, `build.env`는 공개 값입니다.

계정 E2E에는 시계 오차 시험용 `FAKETIME_LIB`도 넘깁니다. 시스템에 있거나 Ubuntu 패키지를 `build/ci/faketime`에 풀어 둔 `libfaketime.so.1`입니다(설치하지 않음). e2e 단계의 클라이언트는 LTO 없이 빌드하는 시험용입니다([e2e.md](e2e.md)). 최소 지원 버전을 1.8.0으로 정한 뒤에는 구버전 실행 파일과 비교하는 X2·X3·V3·V8·A12를 퇴역했습니다. 현재 빌드의 최소 버전 거부, 인증·서명 검증, 키 없는 빌드와 시계 오차 시험은 계속 돕니다.

배포 시험이 매번 확인하는 것은 다음과 같습니다.

- hbbs/hbbr이 127.0.0.1에서만 listen하는지
- 런북 7단계의 WebSocket 확인(`/ws/id`, `/ws/relay`가 101), `/api/`가 404인지, 첫 화면에 소스 안내가 있는지
- `hjv-probe` 전 단계, 그리고 `hjv-load`로 PC 3대 등록과 중계 1세션. 등록한 PC는 12초 동안 저마다 hbbs의 heartbeat(10초마다 보내는 빈 프레임)를 하나 이상 되돌려 보내야 하고(`at least <n> by each PC`), 중계는 한쪽이 보낸 바이트를 다른 쪽이 모두 받아야 합니다(`<n> bytes received of <n> sent`). hbbs는 30초 동안 프레임이 없는 PC를 끊으므로, 빈 프레임을 버리는 웹 서버 뒤에서는 운영의 PC가 30초마다 끊깁니다. `hjv-load relay`는 한 바이트도 오지 않은 세션을 실패로 셉니다
- hbbs가 PC마다 실제 포트를 받는지(`X-Real-Port` 전달), 세 키 값(`.pub`, hbbs, hbbr)이 같은지
- 개인키 권한(700/600, root), 컨테이너 권한 제거(`cap_drop: ALL`), `.env`의 `COMPOSE_FILE`과 대역폭 상한이 적용되는지
- Apache 오류 로그에 fail2ban이 인증 실패로 세는 `AH01630`이 없는지
- 기존 사이트에 추가한 경우: HTTP/2를 켠 listener에서도 WebSocket이 열리는지, 사이트의 다른 경로(`/`, `/wp-json/`, `.php`)와 HTTP/2가 그대로인지
- 클라이언트가 보낸 `X-Real-IP`, `X-Forwarded-For`, `X-Real-Port`가 nginx와 Apache에서 실제 주소로 덮어써지는지(받은 헤더를 그대로 돌려주는 서버로 확인)
- Apache와 Caddy의 계정 경로: Apache가 `/api/peers`(쿼리 포함), `/api/login-options`, `/api/hjv/…`를 hjv-api 소켓으로 넘기며 주소 헤더를 덮어쓰고 쿠키를 빼는지, 그 밖의 `/api/`는 404인지, hjv-api가 없을 때 Apache와 Caddy가 앱 배너의 503 JSON을 주는지, Caddy가 `/api/login-options`와 `/api/device-group/accessible`에 직접 답하는지. hjv-api가 떠 있을 때의 Caddy는 e2e 단계가 봅니다
- systemd 유닛: 127.0.0.1에만 listen하는지, 키 파일이 처음부터 600인지, 권한이 없는지, `hjviewer.env`의 대역폭 상한이 적용되는지, 샌드박스 때문에 생긴 오류가 없는지
- hjv-api의 systemd 유닛: 소켓이 `660 root:www-data`인지, 소켓으로 `/api/hjv/source`에 답하는지, 권한이 없는지, 상태 디렉터리 700과 DB 600, 관리 소켓(`admin report`), 샌드박스 오류가 없는지, 비밀번호 없는 연결 스위치가 설치 직후 꺼져 있는지(묶음의 hjv-api 실행 파일에 그 기능이 생긴 뒤부터. 그 뒤로는 `admin passwordless status`가 답하지 않아도 실패)
- Release 패키지: hbbs·hbbr·hjv-api, 서버의 버전 정책 확인 스크립트(`bin/hjv-update-verify.sh`), 유닛, env 예시, nginx·Apache 파일, zones 파일, 고지(hjv-api의 크레이트 포함)가 들어 있는지, `SHA256SUMS`가 그 패키지 한 줄인지
- 서버의 버전 정책 확인(`deploy/hjv-update-verify.sh`, 런북 8절): 먼저 자체 시험 `scripts/test_hjv_update_verify.py`가 돕니다. 시험 키로 서명한 봉투로 도구의 정식 형식, 엄격한 서명(R이 작은 차수인 서명, S가 군의 차수 이상인 서명), 앱의 단계 A-C와 이유 코드와 상태, 기록 읽기(LOAD), 설치 확인 1-8번, 벡터 실행기를 봅니다. 서명은 그 파일의 Ed25519로 만들고 스크립트는 OpenSSL로 확인하므로 두 구현이 서로를 확인합니다. 이어서 `tools/hjv-release/tests/vectors`가 있으면 그 공유 벡터 전체를 돌립니다(`--vectors`): 정식 형식이 아닌 것은 `Canonical`로 거절하고, 나머지는 앱과 같은 결과, 이유, 상태여야 하고, `covers.json`의 256쌍도 같아야 합니다. 공유 벡터가 없는 트리(도구가 아직 들어오지 않음)에서는 건너뛴 것으로 셉니다. `HJV_UPDATE_VECTORS=<폴더>`로 다른 벡터 폴더를 줄 수 있습니다. 벡터가 바뀌면 deploy 단계를 다시 돌립니다.
- nginx의 버전 정책 위치 `/api/hjv/version`: 위치 파일의 `alias`만 `build/ci/nginx/version-policy.json`(시험 파일)로 바꿔 넣고(공개 파일의 운영 경로는 그대로), 새 사이트, HTTP/2를 켠 기존 사이트, 모든 404를 PHP 페이지로 보내는 사이트에서 봅니다. 파일이 없을 때는 그 위치 자신의 404 JSON(`{"error":"no version policy"}`, 사이트의 오류 페이지로 넘어가지 않음), 있을 때는 파일과 같은 바이트, `application/json; charset=utf-8`, `Cache-Control: no-cache`, `X-Content-Type-Options: nosniff`, ETag를 보낸 요청에 304, POST에 405입니다. 접속 기록에 그 경로의 줄이 없고 오류 기록에 오류가 없어야 합니다.
- hjv-api의 운영 설정값(설계 3판 5-6, `scripts/check-api-settings.py`): 패키지의 `hjviewer-api.env.example`에서 `HJV_API_BIND_OS=windows`, `HJV_API_SECOND_FACTOR=off`, `HJV_API_TRUST_REFRESH`가 60–300초, `HJV_API_TRUST_TTL`이 72시간 이하인지. 설정 파일은 systemd가 읽는 대로 읽습니다. 들여쓴 줄, `=` 둘레의 공백, 따옴표로 감싼 값도 적용되고, `#`나 `;`로 시작하는 줄은 주석이며, 같은 이름이 여러 번 나오면 마지막 줄이 이깁니다. 값이 없거나 비어 있으면 hjv-api가 기본값을 쓰므로, 예시가 주석 줄(`#이름=값`, `# 이름=값`)로 적어 둔 기본값으로 봅니다. 묶음의 hjv-api 실행 파일이 그 이름을 모를 때만(그 설정을 더하는 작업 묶음이 아직 없을 때) 건너뜁니다. 실행 파일이 아는 이름인데 값도 기본값 줄도 없으면 실패하므로, 새 설정은 예시에 기본값 줄을 적어야 합니다. 서버의 설정 파일 사본을 받아 `HJV_API_ENV_FILE=<파일> scripts/ci.sh deploy`로 같은 검사를 할 수 있습니다. 비밀 값은 없지만, 사본에서는 `HJV_API_OPERATORS`와 `HJV_API_CAMPUS_NETS` 줄을 뺍니다(최고운영자의 포탈 ID와 학교 주소 대역. 서버의 파일은 root만 읽음, 런북 7-3). 이 검사의 오프라인 시험(`scripts/test_check_api_settings.py`)은 단계 처음에 돕니다.
- App Review 데모 계정의 파일을 쓰는 `deploy/review-conf.py`(런북 7-17): 오프라인 시험 `scripts/test_review_conf.py`가 쓰는 파일(0600, 세 줄, hjv-api 시험과 같은 해시 벡터, 예시 파일, 데모 PC의 `PC=` 줄)과 거절(ID 규칙, 90일 넘는 날짜, 비밀번호, 터미널이 아닌 입력, 덮어쓰기)을 봅니다. 단계 처음에 돕니다.
- 이용 비밀번호의 파일을 쓰는 `deploy/guest-conf.py`(1.8.0, 런북 9-4): 오프라인 시험 `scripts/test_guest_conf.py`가 쓰는 파일(0600, `HASH`와 `UNTIL` 두 줄, hjv-api 시험과 같은 해시 벡터, 예시 파일), `--random`이 만드는 비밀번호와 그것을 터미널에만 보이는지, 거절(90일 넘는 날짜, 12~256자의 출력 가능한 ASCII가 아니거나 공백으로 시작하거나 끝나는 비밀번호, 터미널이 아닌 입력, 덮어쓰기)을 봅니다. 단계 처음에 돕니다.
- 계정 API의 nginx 한도: 무권한 네트워크 네임스페이스에서 `nginx-hjviewer-api-zones.conf`와 `nginx-hjviewer-api.conf`로, 중계기 등록(`/api/hjv/relay/hello`)이 IPv4 주소마다, IPv6는 /64마다 1분에 10번(버스트 5)이고 넘으면 중계기가 읽는 JSON 429인지, 다른 /64는 따로 세는지, 한 소켓으로 IPv4와 IPv6를 함께 받는 listener(`listen [::]:443 ipv6only=off;`)에서도 IPv4 주소마다 따로 세는지(IPv4 클라이언트가 `::ffff:a.b.c.d`로 들어와도 한 /64로 묶이지 않음), 로그인 한도와 이용 비밀번호(`/api/hjv/guest`, 1.8.0)의 한도도 같은지
- 깨끗한 컨테이너 설치: Ubuntu 24.04 컨테이너(`public.ecr.aws/docker/library/ubuntu:24.04`, `HJV_REGISTRY`로 바꿈)에 런북대로 패키지를 설치하고, 라이브러리(`ldd`), `systemd-analyze verify`, zones 파일과 계정 API 위치로 바꾼 nginx의 `nginx -t`, 그 nginx 뒤의 hjv-api(`/api/hjv/source`, `/api/login-options`, `/api/heartbeat`, 중계기 등록 한도)를 확인합니다. hjv-api는 `systemd-socket-activate`로 `/run/hjviewer-api.sock`에서 서비스 사용자로 띄웁니다. 버전 정책은 운영 경로 그대로 봅니다. 계정 API 위치로 바꾼 뒤에도 `/api/hjv/version`은 nginx가 직접 답하고(정책 파일 전 404, `/etc/hjviewer/version-policy.json`을 넣은 뒤 200, nginx 작업 프로세스 `www-data`가 읽음), 묶음의 `bin/hjv-update-verify.sh --self-test`가 그 Ubuntu의 python3와 openssl로 통과해야 합니다.

시험용 스택은 compose 프로젝트 `hjv-ci`로 띄우고, 끝나면 그 프로젝트만 지웁니다. 웹 서버와 Caddy는 127.0.0.1의 18080–18082, 18443–18448, 18599 포트를 쓰므로 이 PC의 80/443은 건드리지 않습니다. 다만 B 경로는 운영과 같이 21115–21119를 쓰므로, 이 PC에서 실제 HJViewer 서버가 돌고 있으면 시험이 시작하지 않고 멈춥니다. Docker 명령은 런북처럼 sudo로 실행합니다.

### push 전 전체 검사 (`scripts/ci.sh prepush`)

설계 3판 11장 1단계와 런북 7-1의 2번이 말하는 "모든 단계 통과"를 한 명령으로 합니다.

- **조건**: 작업 트리가 깨끗해야 합니다(`git status`에 아무것도 없음). push하는 것은 커밋이므로 커밋을 검사합니다. `E2E_CLIENT`가 있으면 멈추고, leak 단계의 목록(`build/leak-patterns.txt`나 `HJV_LEAK_PATTERNS`)이 없으면 한 시간 뒤가 아니라 시작할 때 멈춥니다. 비밀번호 없는 sudo와 Docker가 필요합니다(e2e, deploy).
- **하는 일**: 잠금을 잡은 채 `HJV_CI_STRICT=1`로 server, client, deploy, e2e, wol, leak을 이 순서로 하나씩 돌립니다. 단계가 실패하거나, 마지막 줄이 `CI passed: <단계> (0 skipped)`가 아니거나, `skipped:` 줄이 있거나, 그 사이에 HEAD나 작업 트리가 바뀌면 그 자리에서 멈춥니다. 캐시가 없는 첫 실행은 한 시간을 넘길 수 있습니다.
- **로그**: 단계마다 `build/ci/prepush/<시각>-<커밋>/<단계>.log` 하나입니다(`HJV_CI_LOGS`로 폴더를 바꿈). 머리에는 이 저장소, `upstream/rustdesk`(클라이언트 시리즈), `upstream/rustdesk-server`(hbbs)의 HEAD와 트리, 각각의 `git status --porcelain` 줄 수(`upstream/rustdesk`는 덮어쓰기 파일 55개와 하위 모듈 포인터 때문에 보통 56), 툴체인, 시작 시각이 있고, 끝 줄은 `EXIT <종료 코드> <시각>`입니다. prepush 밖에서 단계를 따로 돌린 실행도 같은 머리로 시작합니다(첫 줄이 `LOG scripts/ci.sh <단계> (on its own, …)`). 그 실행은 `CI passed: …`나 `FAIL: …` 줄로 끝나고 `EXIT` 줄은 없습니다. 같은 폴더의 `summary.txt`에는 prepush가 단계마다 찍는 줄(종료 코드, 걸린 초, `skipped:` 줄 수, 로그)과, 모두 통과하면 끝에 `prepush: passed, <초> s`가 남고, `times.tsv`에는 각 단계의 일별 시간(`<단계>`, `<초>`, `<일>`을 탭으로 나눈 줄)이 쌓입니다. `HJV_CI_LOGS`로 같은 폴더를 다시 쓰면 둘 다 새로 시작합니다. deploy 단계의 곁 로그(이미지 빌드의 `build.log`, 서버 묶음의 `package.log`, `container.log`, `api-settings.log`, `relay-package.log`) 가운데 그 실행이 쓴 것은 `deploy-<이름>`으로 함께 남습니다. 다음 실행이 `build/ci`의 원본을 덮어쓰기 때문입니다. server, client, e2e 단계는 패치를 적용한 트리의 해시도 찍으므로 시리즈의 트리와 맞춰 볼 수 있습니다. 커밋 하나에 대한 로그는 그 뒤 그 단계가 읽는 파일을 바꾼 커밋이 생기면 아무것도 증명하지 않습니다.
- **고친 뒤**: 단계 이름을 주면 그 단계만 위 순서로 돕니다(예: `scripts/ci.sh prepush e2e wol leak`). 처음부터 다시 돌리지 않고, 고친 파일을 읽는 단계만 아래 "고친 뒤 다시 돌릴 단계"의 표대로 다시 돌립니다. 커밋마다 leak의 입력이 바뀌므로 leak은 늘 마지막 커밋에서 다시 돌립니다.
- **E2E가 실패했을 때**: `HJV_E2E_HOLD=<초>`(최대 3600)를 주고 돌리면(`HJV_E2E_HOLD=1800 scripts/ci.sh e2e`) 실패한 `tools/e2e/firewall-e2e.sh`가 정리하기 전에 그 시간 동안, 또는 `build/ci/e2e/run-<모드>/release` 파일이 생길 때까지 네임스페이스·서버·클라이언트를 그대로 둡니다. 실패를 살피려고 다시 돌리지 않고, 로그와 서비스 상태(`--hjv-e2e-option` 등)를 그 자리에서 봅니다([e2e.md](e2e.md)). 고친 뒤에는 위처럼 그 파일을 읽는 단계만 다시 돌립니다.
- **알려진 불안정한 시험**: server 단계의 `tests/tcp_signaling.rs`의 `peer_registered_over_websocket_receives_signaling`(server/patches 0002)이 hbbs가 listen을 시작한 직후 `connect to hbbs: Protocol(HandshakeIncomplete)`로 실패할 때가 있습니다(2026-10-26에 한 번, 0.3초 만에. 혼자 5번과 다시 돌린 실행은 통과). 이 시험만 이렇게 실패했으면 코드를 고치지 않고 그 단계부터 다시 돌립니다.
- **e2e와 wol**: 건너뛴 것이 없는 실행은 `PASS` 줄의 수와 클라이언트 단위 시험의 통과 수가 `scripts/ci.sh`의 하한보다 적으면 실패합니다. 현재 하한은 e2e strict 128, open 2(`E2E_LEAST_STRICT`, `E2E_LEAST_OPEN`), wol 20(`WOL_LEAST`), `--lib hjv` 345, `--lib hjviewer_wol` 6(`UNIT_LEAST`), 업데이트 키를 뺀 빌드의 `--lib hjviewer_update` 159(`UNIT_LEAST[hjviewer_update-nokeys]`)입니다. 앞의 둘(`hjv`, 키를 뺀 `hjviewer_update`)은 2026-10-26에 1.9.0 실행이 센 344와 159에서, 1.9.1 시리즈가 더한 계정 시험 하나를 더해 345와 159로 올렸습니다(그 전 260, 141). strict 하한은 2026-10-17 출입 제한 단계까지 91이었고, 2026-10-18에 구버전 실행 파일 비교 다섯 개(X2·X3·V3·V8·A12)를 퇴역하면서 86으로 내렸고, 2026-10-20에 지원 코드의 C1–C7을 더해 93이 되었고, 같은 날 1.9.0의 26줄(TV-W2의 H-hide 1, TV-W3의 EL1–EL4와 EL3의 always 5, TV-W5의 S33–S38과 S38의 [끄기] 7, TV-W9의 I1–I5 5, TV-W6의 W6-E1–E7과 E4b 8)을 더해 119가 되었고, 2026-10-26에 1.9.1의 앱 안 계정 삭제 9줄(DEL1 4, DEL2 1, DEL3 2, DEL4 2, [e2e.md](e2e.md)의 계정 삭제 단계)을 더해 128입니다. 나머지 시나리오 번호와 하한은 유지합니다. 시나리오가 늘 거짓인 조건문 안으로 들어가거나 시험 이름 필터가 아무것도 고르지 못하면 이렇게 잡힙니다. 시나리오나 시험을 더하면 하한을 올리고, 일부러 뺄 때만 내립니다. 로그에는 `e2e: PASS lines (strict): 128 (at least 128)` 같은 줄로 남습니다. 건너뛴 것이 있는 실행은 수가 적어도 그 줄 아래에 그렇다고만 적고, 엄격 모드에서는 끝에서 실패합니다.

### 고친 뒤 다시 돌릴 단계

고친 파일을 읽는 단계입니다. 한 커밋이 여러 줄에 걸리면 그 단계를 모두 합칩니다. server 단계가 빌드하는 것(hbbs·hbbr, hjv-api, hjv-release)이 바뀌면 그것을 쓰는 단계도 다시 돌립니다. 엄격 모드에서는 그 실행 파일의 기록이 이 트리와 맞지 않으면 그 단계가 실패하므로 빠뜨릴 수 없습니다(아래 "작업 트리 여럿이 한 CI 폴더를 쓸 때"). 운영 연습은 `scripts/ci.sh`의 단계가 아닙니다([hjv-api-drill.md](hjv-api-drill.md)).

| 고친 것 | 다시 돌릴 단계 | 읽는 곳 |
|---|---|---|
| `client/` (패치, 덮어쓰기 파일, `play/`, `version.env`, `update-keys.env`) | client, e2e, wol | `fetch-upstream.sh client`가 패치와 덮어쓰기 파일을 적용한 트리. client의 공개키 확인 시험이 키 파일을, e2e·wol이 같은 트리의 시험용 클라이언트를 빌드(wol이 e2e의 빌드를 쓸 때의 기록 `e2e-client.stamp`는 `client/` 전체의 해시) |
| `server/patches/` | server, deploy, e2e, wol | server가 hbbs·hbbr을 빌드하고 시험. deploy는 Docker 이미지가 같은 패치로 hbbs를 빌드하고 서버 묶음이 server의 빌드를 씀. e2e·wol은 server의 hbbs·hbbr |
| `server/api/`의 `Cargo.*`, `src/` | server, deploy, e2e, wol, 운영 연습 | hjv-api의 시험과 릴리스 빌드(`src/hbbs.rs`의 벡터는 hbbs의 `hjv_pk` 시험의 것과도 비교), deploy의 서버 묶음·컨테이너, e2e·wol의 계정 API(`build_e2e_api`), 운영 연습의 빌드. `src/seal.rs`의 시험 키는 client도(맨 아래 줄) |
| `server/api/hjviewer-api.env.example` | server, deploy, 운영 연습 | hjv-api의 config 시험, 서버 묶음과 운영 설정 검사(`check-api-settings.py`), 운영 연습의 설치 |
| `server/api/README.md` | server | hjv-api의 keys·pcsync·tickets·guest·update 시험이 읽음 |
| `server/api/tests/` | server, 운영 연습 | hjv-api의 시험(운영 연습은 `tests/drill.rs`와 `tests/common`). `tests/vectors/access-tickets.json`은 hbbs의 출입증 시험도 사본과 바이트까지 비교하고 `HJV_ACCESS_VECTORS`로 읽음 |
| `relay/` (`PROTOCOL.md` 포함) | server, deploy, wol | hjv-relay의 시험(`PROTOCOL.md`의 벡터), hjv-api의 `tests/relay_loop.rs`(개발 의존성), deploy의 hjv-relay 묶음 시험, wol의 hjv-relay |
| `tools/hjv-release/`의 `Cargo.*`, `src/` | server, client, e2e | 형식·lint·시험·릴리스 빌드, client의 키 파일·릴리스 기록 시험(`HJV_RELEASE_BIN`), e2e의 시험 정책 서명(V5–V7·V9) |
| `tools/hjv-release/tests/`(`vectors/` 말고) | server | 도구의 시험 |
| `tools/hjv-release/tests/vectors/` | server, client, deploy | 도구의 시험(server)에 더해 hjv-api의 `src/update.rs`가 `accepted/e2-s2.json`과 제 사본을 비교(server), 클라이언트 코어의 벡터 시험과 그 사례 수(client), `deploy/hjv-update-verify.sh --vectors`(deploy) |
| `tools/hjv-probe/` | server, deploy, e2e, wol | 빌드, `check_site`, E2E 스크립트의 `PROBE`(쓰는 단계가 이 트리에서 빌드) |
| `tools/hjv-load/` | server, deploy | 빌드, `check_site`의 PC 등록·heartbeat와 중계 |
| `tools/e2e/firewall-e2e.sh` | e2e, wol | e2e가 돌림. wol이 클라이언트를 직접 빌드할 때 이 스크립트의 계정·업데이트 부분을 보고 키를 넣음(`build_e2e_client`) |
| `tools/e2e/wol-e2e.sh`, `wol-helper.py`, `capture.py` | wol | |
| `tools/e2e/hjv-api-drill.sh` | 운영 연습 | |
| `deploy/Caddyfile` | deploy, e2e, wol | A 경로(Caddy), 두 E2E 스크립트의 서버 |
| `deploy/guest-conf.py` | deploy, e2e | `scripts/test_guest_conf.py`, `firewall-e2e.sh`가 이용 비밀번호의 해시를 만듦 |
| `deploy/systemd/`, `deploy/journald@hjviewer.conf` | server, deploy, 운영 연습 | hjv-api의 `tests/ops.rs`, 서버 묶음과 그 systemd 유닛 시험 |
| `deploy/`의 나머지(nginx·Apache 파일, compose 파일, `Dockerfile`, `.env.example`, `hjv-update-verify.sh`, `review-conf.py`, `guest.conf.example`, `review.conf.example`) | deploy | `access.conf.example`, `door2.conf.example`, `min-controller.conf.example`은 읽는 시험이 없음(leak만) |
| `LICENSE`, `NOTICE` | deploy | Docker 이미지, 서버 묶음과 hjv-relay 묶음의 고지 |
| `scripts/fetch-upstream.sh`, `scripts/upstream.env` | server, client, deploy, e2e, wol | 모든 upstream 체크아웃, deploy는 Docker 이미지 |
| `scripts/package-server.sh`, `scripts/package-notices.sh` | deploy | 서버 묶음(고지는 hjv-relay 묶음도) |
| `scripts/package-relay.sh` | server, deploy | `relay/tests/package.rs`의 거절 시험과 기본 폴더 시험(`--ignored` 없이, server), 묶음 시험(deploy) |
| `scripts/check-build-keys.py`, `scripts/public-test-keys.txt` | server, client, e2e, wol | hjv-relay 묶음의 거절 시험(`package-relay.sh`가 부름), 공개키 확인 시험, E2E의 시험 키(`e2e_keys`, `e2e_update_keys`) |
| `scripts/release-info.py`, `scripts/testdata/`, 스토어 실행의 스크립트(`play-upload.py`, `asc-store.mjs`, `store-record.py`, `check-store-text.py`), `hjv-fontsize-check.sh`, `hjv-fontsize-baseline.txt`, 그리고 `test_play_upload.py`, `asc-store.test.mjs`, `test_store_record.py`, `test_store_workflows.py`, `test_check_store_text.py`, `test_check_build_keys.py`, `test_release_info.py` | client | |
| `scripts/check-api-settings.py`, `test_check_api_settings.py`, `test_review_conf.py`, `test_guest_conf.py`, `test_hjv_update_verify.py` | deploy | |
| `scripts/leak-check.py`, `test_leak_check.py` | leak | |
| `scripts/set-client-version.sh`, `export-patches.sh`, `make-icons.sh`, `make-splash-text.py` | leak만 | 앱 빌드 워크플로와 손으로 쓰는 도구. 읽는 시험이 없음 |
| `scripts/ci.sh` | 고친 부분을 쓰는 단계 | 단계 함수와 그 단계만 부르는 함수·값은 그 단계: `server_stage`, `hjv_pk_vector`는 server, `client_stage`, `CLIENT_DART_TESTS`, `FLUTTER_VERSION`, `VECTORS_LEAST`는 client, `deploy_stage`, `check_site` 등 배포 확인 함수(`check_stamp`를 뺀 `check_*`), `api_test_credentials`, `start_systemd_api`, `start_echo`, 컨테이너 시험(`container-test.sh`), `SIDE_LOGS`는 deploy, `e2e_stage`, `e2e_minimum_version`, `e2e_access`, `e2e_unit_without_update_keys`, `fetch_faketime`, `E2E_LEAST_*`는 e2e, `wol_stage`, `WOL_LEAST`는 wol, `leak_stage`는 leak. 여럿이 쓰는 것: `build_e2e_client`, `e2e_keys`, `e2e_server_key`, `e2e_update_keys`, `e2e_client_stamp`, `build_e2e_api`, `fetch_caddy`, `E2E_PROFILE`, `UNIT_LEAST`는 e2e·wol, `pkgconfig`는 client·e2e·wol, `build_tools`는 server·deploy·e2e·wol, `stamp_binary`, `check_stamp`, `inputs_hash`, `*_INPUTS`는 server와 client·deploy·e2e·wol. 공통 함수(`step`, `skip`, `count_skips`, `with_skips`, `at_least`, `passed_tests`, `own_target`, `fail`, `finish`)와 맨 위의 값(잠금, `PATH`)은 모든 단계. `prepush`, `log_head`는 prepush로 도는 단계 하나(가장 짧은 leak)로 확인 |
| `.github/workflows/` | client | actionlint, 워크플로의 스토어 규칙, 공개키 확인·릴리스 기록 시험이 `client-build.yml`을 읽음 |
| `store/` (`README.md` 포함) | client | `check-store-text.py`가 `README.md`의 파일 목록대로 읽음 |
| `docs/ci.md` | client | 릴리스 기록 시험이 이 문서의 빌드 저장소 `build.yml`을 읽음 |
| `docs/relay-dsm.md` | server, deploy | hjv-relay의 watchdog 시험, 묶음의 `README-DSM.md`(묶음 시험이 비교) |
| 그 밖의 문서(`docs/`, `README.md`들, `tools/hjv-release/README.md` 포함) | leak | 위 줄의 문서(`server/api/README.md`, `relay/PROTOCOL.md`, `store/README.md`, `docs/ci.md`, `docs/relay-dsm.md`)는 그 줄의 단계도 |
| 32바이트 값(시험 키, 벡터의 키)을 더하거나 바꾼 파일, 어디에 있든 | client | 공개키 확인 시험이 git이 추적하는 모든 파일에서 그런 값을 찾아 시험 키로 셈 |

### 작업 트리 여럿이 한 CI 폴더를 쓸 때

레인의 작업 트리들은 `HJV_CI_DIR=<주 체크아웃>/build/ci`로 데운 CI 폴더 하나를 같이 씁니다. 그러면 `build/ci/target*`의 cargo 빌드 폴더도 같이 씁니다. cargo는 파일 시각으로 다시 빌드할지 정하고, 단위의 이름은 어느 트리에서 빌드했든 같습니다. 그래서 파일이 더 오래된 트리는 다른 트리가 빌드한 것을 제 것으로 씁니다. 그 트리의 경로가 `CARGO_MANIFEST_DIR`로 들어간 시험 실행 파일도 그렇습니다. 2026-10-26에는 deploy 단계의 hjv-relay 묶음 시험이 다른 작업 트리의 스크립트와 소스로 묶음을 만들고도 통과했습니다.

- **빌드 폴더의 트리**: 빌드 폴더마다 마지막으로 빌드한 트리를 `.hjv-tree`에 적어 둡니다. 다른 트리가 그 폴더에 빌드하기 전에, 그 작업 공간에서 경로로 빌드하는 패키지(기능을 모두 켰을 때의 것)를 두 프로필(debug, release)에서 모두 다시 빌드하게 합니다(`scripts/ci.sh`의 `own_target`). crates.io와 git 의존성은 그대로 남습니다. 이 트리의 크레이트(`target-api`·`target-api-e2e`는 hjv-api와 hjv-relay, `target-relay`는 hjv-relay, `target-hjv-release`는 hjv-release)는 `cargo clean -p`로 지웁니다. 다른 트리가 남긴 단위도 이때 함께 지워집니다. hjv-relay가 `server/api` 밖에 있어 hjv-api의 시험 단위는 이름에 트리가 들어가고 트리마다 따로 쌓이므로, 이것이 `target-api`가 커지지 않게 합니다(2026-10-27에 다른 레인들이 그 폴더를 쓴 뒤의 한 번은 12GB를, 표시를 처음 남긴 실행은 여러 트리와 툴체인이 남긴 증분 캐시 23GB를 지움). `cargo clean -p`는 대상의 이름으로 지우므로 hjv-api의 시험 `http`와 이름이 같은 의존성 `http`(그리고 그 위의 hyper 묶음)도 함께 다시 빌드됩니다. 이 트리의 패치로 만든 upstream 체크아웃의 크레이트(`target`은 hbbs와 hbb_common, `target-client`·`target-175`·`target-e2e`는 rustdesk와 그 `libs/`)는 지우지 않고, 그 단위의 fingerprint와 빌드 스크립트 폴더만 지웁니다. 그러면 cargo가 그 단위를 다시 빌드하고 증분 캐시는 남습니다. 체크아웃은 실행마다 같은 자리에 새로 만들므로 그 단위의 이름은 어느 트리가 빌드했든 같고, 쌓이지 않습니다. 2026-10-27에 모든 폴더를 `cargo clean -p`로 지우던 때 잰 값으로, 다른 트리가 그 폴더를 쓴 다음 처음 도는 단계는 이만큼 더 걸렸습니다. server 약 40초(네 폴더를 지우는 데 모두 2초, hjv-api의 시험 빌드 28초, hjv-release 7초, hjv-relay 2초), client 약 43초(체크아웃을 새로 받아 어차피 다시 컴파일하는 rustdesk와 그 `libs/`의 증분 캐시가 함께 지워져, Rust 1.75 check가 14초, 공유 벡터 시험의 빌드가 27초, hbb_common이 2초 더 걸림), e2e 약 18초(hjv-api와 가짜 SSO의 릴리스 빌드), wol 약 17초(hjv-relay의 릴리스 빌드)입니다. 모든 폴더를 다른 트리가 쓴 뒤의 prepush는 2분쯤 더 걸리는 셈이었습니다. client의 43초는 지운 증분 캐시를 다시 만든 것이라 upstream 체크아웃의 폴더를 위처럼 다루면 들지 않습니다. hbbs와 E2E 클라이언트는 증분 캐시가 없는 릴리스 빌드이고 어차피 실행마다 다시 컴파일하므로 더 걸리지 않습니다. 실행이 끝나거나 `fail`로 멈추면 그 실행이 빌드한 폴더의 `.hjv-tree`를 새로 찍어 둡니다. cargo가 실패해 `set -e`로 바로 끝난 실행(hbbs의 시험, hjv-api의 릴리스 빌드, hbb_common의 시험)은 찍지 못하므로, 다음 실행이 그 폴더에서 이 트리의 크레이트를 한 번 더 다시 빌드합니다. 같은 트리라도 표시 뒤에 컴파일된 단위가 있으면(`invoked.timestamp`가 더 새로움) 다시 빌드하게 합니다. 표시를 모르는 `scripts/ci.sh`(이 변경이 들어오기 전의 작업 트리)나 손으로 돌린 cargo가 그 사이에 빌드한 것을 제 것으로 쓰지 않기 위해서입니다. 로그에는 `target-relay: built last from <트리>; this tree's packages (hjv-relay) cleaned in <초> s` 같은 줄이 남습니다. upstream 체크아웃의 폴더는 그 끝에 지운 fingerprint와 빌드 스크립트 폴더의 수를 붙입니다. hjv-relay 묶음(`scripts/package-relay.sh`)의 빌드 폴더는 트리 안(`build/package-relay/target`)에 있어 같이 쓰지 않습니다.
- **다른 단계가 쓰는 실행 파일의 출처**: server 단계가 빌드해 다른 단계가 쓰는 실행 파일에는 옆에 `<실행 파일>.stamp`를 둡니다. hbbs와 hbbr(`build/ci/target/release`, deploy의 서버 묶음과 e2e·wol), hjv-api(`build/ci/target-api/release`, deploy의 서버 묶음), hjv-release(`build/ci/target-hjv-release/release`, client 단계의 키 파일·릴리스 기록 시험. e2e 단계도 같은 곳에 빌드해 기록을 새로 씀)입니다. 기록에는 커밋과 바뀐 파일 수, 재료의 해시, 실행 파일의 해시, 빌드한 트리가 있습니다(경로에 공백이 있어도 읽도록 트리가 맨 끝). 재료는 hbbs·hbbr이 `server/patches`, `scripts/upstream.env`, `scripts/fetch-upstream.sh`, hjv-api가 `server/api`의 `Cargo.toml`·`Cargo.lock`·`src/`, hjv-release가 `tools/hjv-release`의 같은 것(시험과 문서는 빼고)에서 git이 추적하거나 무시하지 않는 파일과 툴체인(`cargo --version`)입니다. 쓰는 단계는 출처를 한 줄 찍습니다(`hbbs: built from what this tree has of … (in <트리> at <커밋>, …)`). 재료가 이 트리의 것과 다르거나, 기록이 없거나, 실행 파일이 그 뒤 바뀌었으면 엄격 모드에서는 바로 실패하고(`scripts/ci.sh server`로 다시 빌드), 아니면 건너뛴 것으로 셉니다. 재료가 같으면 다른 트리에서 빌드한 것도 이 트리의 것으로 봅니다. 같은 재료와 툴체인으로 빌드한 것이기 때문입니다. 이 폴더의 hjv-release는 마지막으로 빌드한 트리의 것이므로, 손으로 쓰기 전에는 `hjv-release.stamp`로 어느 트리의 어느 커밋인지 봅니다.
- `build/ci/target*`에 cargo를 `scripts/ci.sh` 밖에서 직접 돌리지 않습니다. 실행 사이에 한 빌드는 위의 시각 비교가 잡아 다음 실행이 이 트리의 크레이트를 다시 빌드하지만(그만큼 느려짐), 실행 도중에 한 빌드는 잡지 못합니다. 레인이 따로 빌드할 때는 자기 `CARGO_TARGET_DIR`을 씁니다.

## 서버 Release 만들기 (런북 B 경로용)

1. 변경을 커밋해 공개 저장소의 main에 올립니다. 패키지에 들어가는 고지가 그 커밋을 가리키므로, 공개된 커밋으로만 만듭니다. 스크립트가 확인합니다.
2. `scripts/package-server.sh <버전>`을 실행합니다(예: `2026.10.10`). 새로 받은 upstream에 `server/patches`를 적용해 빌드하고, 바이너리, systemd 유닛, 웹 서버 설정, 라이선스 고지를 `build/release/hjviewer-server-<버전>-linux-x86_64.tar.gz`로 묶습니다. 그 파일 한 줄만 든 `SHA256SUMS`도 함께 만듭니다.
   - `bin/`: hbbs, hbbr, hjv-api(계정 API, `server/api`), `hjv-update-verify.sh`(서버의 버전 정책 확인, python3와 openssl만 씀)
   - `systemd/`: hbbs·hbbr 유닛, hjv-api의 소켓과 서비스, 감시·백업 유닛(`deploy/systemd/hjviewer-api-*`가 생기면), `hjviewer.env.example`, `hjviewer-api.env.example`, `journald@hjviewer.conf`(hjv-api 저널의 보관, 생기면)
   - `web/`: nginx·Apache 파일. 계정 API의 위치 `nginx-hjviewer-api.conf`와 그 http 단계 zones 파일 `nginx-hjviewer-api-zones.conf`(로그인과 중계기 등록의 한도) 포함
   - 고지: `THIRD_PARTY_NOTICES.txt`에 hjv-api의 크레이트도 들어갑니다. `server/api`가 없는 저장소에서는 hjv-api 부분을 빼고 묶습니다(설계 3판 10-2).
3. `gh release create server-<버전> build/release/hjviewer-server-<버전>-linux-x86_64.tar.gz build/release/SHA256SUMS`로 올립니다. Release를 만들어도 워크플로는 돌지 않습니다.

hjv-relay의 묶음은 `scripts/package-relay.sh`가 따로 `build/release-relay/`에 만들고, `SHA256SUMS`도 그 묶음만의 것을 씁니다. 그 `SHA256SUMS`는 두 줄입니다: 묶음과 그 안의 `hjv-relay`(아래 "hjv-relay 빌드" 3번). 나눠 주는 hjv-relay는 비공개 빌드 저장소에서 만듭니다(아래 "hjv-relay 빌드"). 개발 PC의 `package-relay.sh`는 시험과 해시 대조에만 씁니다.

개발 PC는 운영 서버와 같은 Ubuntu 24.04라서, 여기서 빌드한 바이너리(glibc 2.39)가 그대로 돕니다.

## 앱 빌드

앱(Windows/Android/iOS)은 개발 PC에서 빌드하지 않습니다. iOS는 macOS, Windows 앱은 Windows가 필요하기 때문입니다. 실제 서버 값으로 빌드하는 앱은 **비공개 빌드 저장소** `yonsei-projecttf/hjviewer-private`의 GitHub Actions에서 만듭니다. 2026-10-10에 사용자가 정한 방식이고, GitHub CI는 그 저장소에서만 씁니다.

- 빌드 저장소에는 앱 원본 코드를 복제하지 않습니다. 이 저장소의 `client-build.yml`을 부르는 워크플로와 Secrets를 관리하며, 앱 코드는 실행에서 고정한 공개 저장소 커밋으로 받습니다.
- 서버 주소와 공개키, 서명 시크릿은 그 저장소의 Secrets에만 있습니다. 빌드 로그와 결과물도 그 저장소에만 남습니다. 실제 서버 값이 든 앱과 서명된 앱이 공개 아티팩트로 남지 않게 하기 위해서입니다.
- 빌드 저장소의 워크플로는 이 저장소의 **확인을 마친 커밋**에 고정해서 부릅니다(아래 "비공개 빌드 저장소"). main이 바뀌어도, 시크릿을 쓰는 빌드는 확인한 빌드 방법으로만 돕니다.

## 앱 빌드 워크플로 (GitHub Actions, 지금은 직접 실행할 때만)

`.github/workflows/client-build.yml`이 앱을 빌드합니다. 빌드 대상은 다음 세 가지입니다.

- Windows x64: 피제어 PC와 제어 측 겸용
- Android arm64 APK. 비공개 저장소의 서명 빌드는 Google Play용 AAB도 만듭니다.
- iOS: 제어 측
- hjv-relay: NAS에서 도는 원격 켜기 중계기(`relay/`, [relay-dsm.md](relay-dsm.md)). x86_64 musl 정적 실행 파일입니다. 앱이 아니지만 같은 워크플로에서 같은 커밋으로 만듭니다(`platforms`의 `relay`, `all`에 포함).

macOS와 Linux 앱은 빌드하지 않습니다.

앱 빌드 잡은 같은 흐름으로 동작합니다.

1. 이 저장소를 체크아웃하고 `scripts/fetch-upstream.sh client`를 실행합니다. 고정 커밋의 RustDesk 1.5.0과 `libs/hbb_common`을 받아 `client/patches`를 적용합니다.
2. `upstream/rustdesk` 안에서 upstream CI(`.github/workflows/flutter-build.yml`, `bridge.yml`)와 같은 단계로 빌드합니다. Flutter 3.24.5, Rust 1.75, NDK r28c, vcpkg 커밋, LLVM 15.0.6 같은 도구 버전도 upstream 값을 그대로 씁니다. 각 단계가 따라 한 upstream 줄 번호는 워크플로 주석에 적혀 있습니다.

## 실행

- **수동 실행**: Actions → client → Run workflow에서 `platforms`(windows / android / ios / relay / all)를 고릅니다. CLI로는 `gh workflow run client-build.yml -f platforms=android`입니다. 이 버튼은 워크플로 파일이 기본 브랜치에 있어야 나타납니다.
- **실제 서버용 빌드**: 비공개 빌드 저장소에서 실행합니다. 예: `gh workflow run build.yml -R yonsei-projecttf/hjviewer-private -f platforms=all`
- **TestFlight 업로드**: `testflight`를 켜면 서명된 App Store IPA를 App Store Connect에 올립니다. 예: `gh workflow run build.yml -R yonsei-projecttf/hjviewer-private -f platforms=ios -f testflight=true`. 비공개 저장소, App Store 프로파일, 아래의 App Store Connect 시크릿이 필요합니다.
- **Google Play 업로드**: 빌드 실행은 Google Play에 올리지 않습니다(1.9.1부터). `play=true`는 Bridge codegen 잡이 바로 거부합니다. 비공개 서명 빌드의 Android 잡이 만든 AAB(`…-android-aarch64-play`, 30일 보관)를, 소유자가 같은 실행의 APK로 확인한 뒤 **스토어 실행**이 비공개 테스트 트랙 alpha에 올립니다([store-release.md](store-release.md)). 예: `gh workflow run store.yml -R yonsei-projecttf/hjviewer-private -f platform=android -f action=upload -f version=<버전> -f build=<versionCode> -f run_id=<빌드 실행 ID>`. Play의 심사 상태(앞 버전이 심사 중 등)가 빌드 실행을 실패시키지 않으므로, 그 실행의 APK와 Windows 패키지도 막히지 않습니다. 입력 `play`, `play_release`는 비공개 `build.yml`이 넘기므로 선언만 남아 있습니다.
- **확인 빌드(`check_only`)**: 출시 전에 코드가 세 플랫폼에서 빌드되는지만 봅니다(설계 3판 6-4, 결정 O2: 마지막 push 바로 뒤). 1.6.0의 공개키 필수 검사를 건너뛰므로 키를 만들기 전에도 돌릴 수 있습니다. 산출물 이름에 `-check`가 붙고 하루만 남으며, `testflight`나 `play`를 함께 켜면 실행이 바로 실패합니다. hjv-relay는 서버 값 없는 시험 빌드가 되고 Release를 만들지 않습니다. 예: `gh workflow run build.yml -R yonsei-projecttf/hjviewer-private -f platforms=all -f check_only=true`. 확인 빌드의 산출물은 나눠 주지 않습니다.
- **태그**: GitHub CI를 다시 켜면 `v`로 시작하는 태그를 푸시할 때 전체를 빌드합니다(지금은 꺼 둠). 예: `git tag v0.1.0 && git push origin v0.1.0`
- **결과물**: 실행 페이지 아래의 Artifacts에서 받거나 `gh run download <run-id>`를 씁니다. 보관 기간은 7일(워크플로의 `ARTIFACT_RETENTION_DAYS`)이므로 필요한 파일은 바로 받아 두세요. Play 번들(`…-android-aarch64-play`)만 30일입니다. 스토어 실행이 Play 심사가 늦어져도 그 번들을 올릴 수 있게 하기 위해서입니다(확인 빌드는 하루). 이름의 `<이름>`은 태그 빌드면 태그(`v0.1.0`), 수동 실행이면 `<앱 버전>-<커밋 7자리>`(예: `1.5.1-d2fa660`)입니다.

## 잡 구성

| 잡 | 러너 | 하는 일 | 결과물 (artifact) |
|---|---|---|---|
| Bridge codegen | ubuntu-22.04 | 입력·서버 변수·고정 공개키를 확인하고 소스 커밋을 고정합니다. 앱을 빌드할 때는 패치가 `src/flutter_ffi.rs`를 바꾸므로 flutter_rust_bridge 코드를 생성합니다. `relay` 단독 실행에서는 이 생성 단계와 도구 설치를 건너뜁니다. | 앱 빌드의 내부용 `bridge-artifact` (다른 아티팩트처럼 7일, 확인 빌드는 1일 보관. 아래 "릴리스 기록") |
| Dart tests | ubuntu-24.04 | 생성된 bridge로 `flutter/test`의 Dart 테스트를 모두 돌립니다. 앱 패치가 더하거나 고친 테스트(제스처, iPhone 한글 입력, 화질 표시)가 하나라도 없으면 실패합니다. Windows·Android 잡은 이 잡을 기다리지 않습니다. iOS 잡은 이 잡이 통과한 뒤에 시작하므로, 테스트가 실패한 빌드는 TestFlight에 올라가지 않습니다. | 없음 |
| Windows x64 | windows-2022 | vcpkg 의존성 → `build.py --flutter --hwcodec --vram`(Rust + Flutter) → 가상 디스플레이 드라이버, 프라이버시 모드용 `WindowInjection.dll` 포함 → 자동 압축 해제 exe (RustDesk의 프린터 드라이버는 소스가 공개되지 않아 넣지 않습니다. 원격 인쇄는 꺼져 있습니다.) | `HJViewer-<이름>-windows-x86_64` (`HJViewer.exe`와 같은 파일인 `HJViewer-install.exe`, 서명 없음) |
| Android arm64 | ubuntu-24.04 | 첫 단계(서드파티 빌드 코드보다 먼저): 이 저장소의 출시 기록 `store-releases.txt`에 이 버전의 Google Play 줄이 없는지(`scripts/store-record.py check-build`, 실행의 토큰으로 읽기만, 확인 빌드는 건너뜀) → vcpkg(NDK) → `cargo ndk`로 `librustdesk.so` → `flutter build apk --split-per-abi` → 키스토어가 있으면 릴리스 서명. 비공개 저장소의 서명 빌드는 이어서 `flutter build appbundle`로 AAB를 만들어 업로드 키로 서명하고 검사합니다(아래 "Google Play 업로드"). | `…-android-aarch64` 또는 `…-android-aarch64-debugsigned`. 비공개 서명 빌드는 `…-android-aarch64-play`(AAB)도 |
| iOS | macos-26 (Xcode 26.6) | 첫 단계: 출시 기록에 이 버전의 App Store 줄이 없는지, `testflight`를 켰으면 App Store Connect에 이 버전의 빌드가 없는지(`scripts/asc-store.mjs check-new`, GET만, 업로드 키는 그 단계와 업로드 단계에만) → vcpkg(arm64-ios) → Rust 정적 라이브러리 → `flutter build ipa` → 인증서가 있으면 서명된 IPA → `testflight`를 켜면 App Store Connect에 업로드 | `…-ios` 또는 `…-ios-unsigned` |
| hjv-relay | ubuntu-24.04 | 명령 공개키 확인 → musl-tools와 Rust(`RELAY_RUST_VERSION`, 개발 PC와 같은 판) → `scripts/package-relay.sh`로 정적 빌드와 묶음. 비공개 저장소의 출시 빌드만 서버 값(`HJV_SERVER`, `HJV_COMMAND_PUBKEY`)을 넣고, 나머지는 서버 값 없는 시험 빌드(`-test`, `run`을 거부)입니다. 실행 요약에 묶음의 SHA-256과 명령 키 지문이 나옵니다. | `hjviewer-relay-<판>-linux-x86_64[-test]` (`tar.gz`와 `SHA256SUMS`) |
| hjv-relay draft release | ubuntu-24.04 | 비공개 저장소의 출시 빌드(`check_only` 아님)에서만 돕니다. hjv-relay 묶음을 그 저장소의 **초안** Release `relay-<판>`에 올립니다. 같은 이름의 Release가 이미 있으면 건드리지 않고 알림만 남깁니다. 저장소에 쓰는 유일한 잡이고, 서드파티 빌드 코드를 돌리지 않습니다. | 없음 (초안 Release) |

앱을 포함한 실행에서는 Bridge codegen이 끝나면 Dart tests와 선택한 Windows, Android, hjv-relay 잡이 병렬로 실행되고, iOS 잡은 Dart tests가 통과한 뒤에 시작합니다. hjv-relay draft release 잡은 hjv-relay 잡이 성공한 뒤에 시작합니다. `relay`만 선택하면 커밋 고정과 사전 확인 뒤 hjv-relay로 바로 진행하며, Flutter 설치·bridge 생성·Dart 검사는 실행하지 않습니다.

호출 워크플로의 `platforms` 값도 `windows`, `android`, `ios`, `relay`, `all` 중 하나여야 합니다. `testflight=true`는 `ios` 또는 `all`에서만 허용하고, `play=true`는 늘 거부합니다(스토어 실행의 upload를 씀). 맞지 않는 조합은 플랫폼 빌드를 시작하기 전에 실패합니다.

Windows exe 하나로 피제어 PC와 제어 측을 모두 씁니다. 실행하면 압축이 풀리고 앱이 뜹니다. 피제어 PC에서는 앱의 설치 버튼으로 서비스로 설치합니다. 서명이 없으므로 SmartScreen 경고나 백신 오탐이 나올 수 있습니다.

### upstream과 다른 점

- **vcpkg 캐시**: upstream은 `VCPKG_BINARY_SOURCES=clear;x-gha,readwrite`를 씁니다. 그런데 vcpkg가 2025년에 x-gha 제공자를 없앴고 지금은 경고만 출력하므로, upstream은 매번 ffmpeg 등 모든 포트를 새로 빌드합니다. 이 워크플로는 vcpkg의 `files` 제공자와 `actions/cache`로 빌드된 포트를 저장하고, 그 실행에서 설치한 패키지만 남깁니다.
- **서명**: upstream의 비공개 서명 서비스 대신 아래 시크릿으로 선택적으로 서명합니다. Windows는 서명하지 않습니다.
- **Xcode 고정**: upstream은 `macos-latest`의 기본 Xcode를 씁니다. 이 워크플로는 `macos-26`에서 Xcode 26.6(`XCODE_VERSION`)을 고르고, 26.x가 아니면 실패합니다. App Store Connect는 Xcode 26 이상으로 만든 빌드만 받습니다. 반면 Xcode 27(iOS 27 SDK)로 만들면 UIScene 수명 주기가 없는 Flutter 3.24.5 앱이 실행되지 않습니다.
- **제외한 것**:
  - macOS·Linux·웹·Sciter 빌드
  - Windows arm64, Android armv7/x86_64/universal APK
  - MSI, SBOM, GitHub Release 업로드
- **Windows 보조 DLL**: `WindowInjection.dll`(RustDeskTempTopMostWindow)은 별도 잡 대신 Windows 잡 안에서 빌드합니다.
- **Bridge codegen의 패키지 해석**: 이 잡은 upstream처럼 Flutter 3.22.3(Dart 3.4)으로 돌고, 코드 생성에 쓸 패키지만 받습니다. upstream이 `extended_text`를 14.0.0에서 13.0.0으로 낮춰 받듯, 1.9.0부터 앱 잠금의 `local_auth_android`도 이 잡에서만 1.0.47에서 1.0.43으로 낮춰 받습니다. 1.0.44부터 Dart 3.5가 필요하기 때문입니다. 플랫폼 잡은 Flutter 3.24.5와 `pubspec.yaml`의 버전 그대로 빌드합니다. 1.9.0의 첫 배포용 빌드(실행 39363651119)는 이 줄이 없어 이 잡에서 멈췄습니다.
- **Dart 테스트**: upstream CI에는 없는 잡입니다. 앱 패치가 고친 제스처와 키보드 동작을 Dart 테스트로 확인합니다. 순수 Dart 테스트는 개발 PC의 `scripts/ci.sh client`에서도 돕니다.
- **앱 버전**: 각 플랫폼 잡이 빌드 전에 `scripts/set-client-version.sh`로 `client/version.env`의 버전을 적습니다(아래 "앱 버전").
- **Windows 실행 파일 이름**: 서버 값이 있는 빌드는 앱 이름이 HJViewer입니다. 그런데 설치 프로그램은 `<앱 이름>.exe`를 서비스·시작프로그램·시작 메뉴·앱 제거에 등록하면서 파일은 원래 이름대로 복사합니다. 그래서 패키지 안의 실행 파일을 `HJViewer.exe`로 바꿔 묶습니다. 1.5.0까지는 `rustdesk.exe`라서 설치해도 자동 시작이 되지 않았습니다.
- **Windows 설치 시험**: 서버 값이 있는 빌드는 러너에서 `--silent-install`로 설치해 보고, `HJViewer.exe`, 자동·실행 중인 서비스, 시작프로그램의 트레이 바로가기를 확인합니다. 설치 전에 서버 이름을 127.0.0.1로 돌려 러너가 운영 서버에 기기를 등록하지 않게 합니다. 몇 번 통과할 때까지는 실패해도 빌드를 멈추지 않습니다(continue-on-error). 결과는 그 단계의 표시로 확인합니다.
- **Google Play용 AAB와 업로드 잡**: upstream은 APK만 만듭니다. 이 워크플로는 비공개 저장소의 서명 빌드에서 APK 다음에 arm64 AAB를 만들고, jarsigner로 한 번 서명한 뒤 검사합니다. apksigner로는 번들을 서명할 수 없습니다. 그 AAB는 스토어 실행이 Google Play 비공개 테스트 트랙 alpha에 올립니다([store-release.md](store-release.md)).
- **시간 제한**: 잡마다 `timeout-minutes`를 걸어, 멈춘 잡이 계속 비용을 쓰지 않게 했습니다.

## 비공개 빌드 저장소

`yonsei-projecttf/hjviewer-private`에는 아래 `.github/workflows/build.yml`과 README만 있습니다. 시크릿은 아래 "서명 시크릿"과 같습니다. 서버 값 `HJV_SERVER`, `HJV_SERVER_KEY`도 시크릿으로 넣습니다.

- 빌드 실행의 시크릿은 빌드 저장소의 **저장소 시크릿**(Repository secrets)이나 조직 시크릿으로 넣습니다. `client-build.yml`의 잡은 environment를 선언하지 않으므로 environment 시크릿을 받지 않습니다. 스토어 키와 심사 정보는 그래서 environment `store`에 둡니다. 그 값은 environment를 선언한 스토어 실행의 잡(`store-release.yml`의 Google Play·App Store 잡)만 받습니다([store-release.md](store-release.md)).
- 빌드 저장소에는 `build.yml` 말고도 스토어 실행의 `store.yml`(공개 `store-release.yml`을 같은 방식으로 고정 SHA에 고정해 부르고, 시크릿은 이름으로만 넘김)과 출시 기록 `store-releases.txt`가 있습니다.
- `secrets: inherit`는 같은 조직 안의 저장소끼리만 시크릿을 넘깁니다. 그래서 두 저장소는 모두 `yonsei-projecttf`에 있어야 합니다. 서버 값이 넘어오지 않으면 Bridge codegen 잡이 실패합니다. 서버 값 없이 빌드한 앱은 순정 RustDesk처럼 공개 서버를 쓰기 때문입니다.

```yaml
name: build
on:
  workflow_dispatch:
    inputs:
      platforms: { type: choice, options: [windows, android, ios, relay, all], default: all }
      testflight: { type: boolean, default: false }
      play: { type: boolean, default: false }
      play_release: { type: choice, options: [completed, draft, validate], default: completed }
      check_only: { type: boolean, default: false }
permissions:
  contents: read
jobs:
  client:
    # 불려 오는 워크플로에서 hjv-relay draft release 잡만 contents: write를 씁니다.
    permissions:
      contents: write
    # 확인을 마친 공개 저장소 커밋의 40자리 전체 SHA(git rev-parse HEAD).
    # 빌드 방법(uses)과 코드(source_ref)를 같은 커밋으로 둡니다.
    uses: yonsei-projecttf/hjviewer/.github/workflows/client-build.yml@<커밋>
    with:
      platforms: ${{ inputs.platforms }}
      testflight: ${{ inputs.testflight }}
      play: ${{ inputs.play }}
      play_release: ${{ inputs.play_release }}
      check_only: ${{ inputs.check_only }}
      source_ref: <커밋>
    secrets: inherit
```

- 새 버전을 빌드하려면, 공개 저장소에 올린 커밋을 확인한 뒤 `<커밋>` 두 곳을 그 커밋의 40자리 전체 SHA(`git rev-parse HEAD`)로 바꿉니다. 짧은 SHA(`e6b1cfd`)는 체크아웃이 브랜치·태그 이름으로 찾다가 실패합니다.
- `play`, `play_release`는 Google Play 업로드가 들어간 커밋부터 워크플로에 있는 입력입니다. 불려 오는 워크플로에 없는 입력을 넘기면 실행이 거절되므로, `build.yml`에 두 입력을 더하는 일과 `<커밋>`을 그 이후 커밋으로 바꾸는 일은 같은 커밋에서 합니다. 반대로 두 입력이 없는 옛 `build.yml`은 새 워크플로를 가리켜도 그대로 돕니다. 두 입력에 기본값이 있기 때문입니다. 1.9.1부터 `play=true`는 거부되지만, `hjv-release`의 출처 확인이 보는 `build.yml`의 모양을 바꾸지 않도록 두 입력은 그대로 둡니다.
- `check_only`와 `platforms`의 `relay`, 그리고 `client` 잡의 `permissions: contents: write`는 1.6.0 커밋(hjv-relay 잡이 들어간 커밋)부터입니다. 그 워크플로의 hjv-relay draft release 잡이 `contents: write`를 요청하므로, 위처럼 허락하지 않은 `build.yml`이 그 커밋을 가리키면 GitHub가 실행을 시작하지 않고 거절합니다. 그래서 이 셋과 `<커밋>`도 같은 커밋에서 바꿉니다. 쓰기 권한은 그 잡 하나만 받고, 나머지 잡은 워크플로 맨 위의 `contents: read`를 그대로 씁니다.
- 불려 온 워크플로는 빌드 저장소의 실행으로 돕니다. 그래서 시크릿, 로그, 결과물, 캐시, 실행 시간은 모두 빌드 저장소 쪽에 속합니다. iOS 서명도 실행하는 저장소가 비공개일 때만 합니다.
- 각 잡은 Bridge codegen 잡이 받은 커밋을 체크아웃합니다. 빌드 중에 브랜치가 움직여도 모든 플랫폼이 같은 커밋으로 빌드됩니다.
- 나눠 줄 빌드가 끝나면 그 커밋에 공개 저장소에서 `client-<앱 버전>` 태그를 붙입니다(예: `client-1.8.0`). 앱을 받은 사람이 자기 빌드의 소스 커밋을 찾을 수 있게 하기 위해서입니다. `v`로 시작하지 않으므로, 다시 켤 태그 빌드 트리거와도 겹치지 않습니다.

### 미러에서 빌드 전용으로 바꾸기 (한 번만)

1.5.0까지 `hjviewer-private`는 이 저장소의 미러였습니다. 미러에 딸린 `client`·`server` 워크플로도 실제 시크릿으로 실행할 수 있었습니다. 그 워크플로로 빌드하면 1.5.0 코드(옛 아이콘, 자동 시작이 안 되는 `rustdesk.exe`)가 실제 서버 값으로 빌드되고, TestFlight에도 올라갈 수 있습니다. 그래서 공개 저장소에 main을 올린 직후, 다른 실행보다 먼저 바꿉니다.

1. 미러 워크플로를 끕니다: `gh workflow disable client -R yonsei-projecttf/hjviewer-private`, 같은 방법으로 `server`도 끕니다.
2. 빌드 저장소에 일반 커밋 하나를 올립니다. 미러 파일을 모두 지우고 위 `build.yml`과 README만 남기는 커밋입니다. 강제 push는 쓰지 않습니다.
3. 개발 PC에서 `git remote remove private`로 미러 remote를 지웁니다. 이후로는 이 저장소를 빌드 저장소에 올리지 않습니다.
4. 첫 빌드를 실행하고 모든 잡을 확인합니다. Windows 설치 시험은 실패해도 잡이 성공으로 끝나므로, 그 단계의 표시까지 확인합니다.

## 앱 버전

사용자에게 나눠 주는 빌드는 **매번 이전보다 높은 버전**이어야 합니다(2026-10-10 사용자 결정). Android는 더 낮은 versionCode로 업데이트하지 못하고, App Store Connect는 같은 빌드 번호를 받지 않기 때문입니다.

| 값 | 어디에 쓰이나 | 정하는 곳 |
|---|---|---|
| 앱 버전 (`1.6.0`) | iOS CFBundleShortVersionString, Android versionName, Windows 파일·제품 버전, 앱 정보 화면, 설치 프로그램 목록 | `client/version.env`의 `HJV_VERSION`. `scripts/set-client-version.sh`가 빌드 전에 `Cargo.toml`(Rust 코어와 압축 해제 실행기), `Cargo.lock`, `pubspec.yaml`에 적습니다. |
| iOS·Android 빌드 번호 | iOS CFBundleVersion, Android versionCode | 빌드 시각(UTC `YYDDDHHMM`). 실행할 때마다 자동으로 커집니다. |
| Windows 빌드 번호 | Windows 파일 버전의 네 번째 숫자(예: `1.6.0.12`) | 워크플로 실행 번호. Windows 버전 숫자는 65535까지라 시각을 쓸 수 없습니다. 실행할 때마다 자동으로 커집니다. |

- 새 버전을 나눠 줄 때마다 `HJV_VERSION`을 올립니다. upstream RustDesk 버전보다 낮으면 스크립트가 실패합니다. 상대 기기가 이 버전을 비교해 기능을 켜기 때문입니다.
- Android versionCode는 빌드 번호에 2000을 더한 값입니다(1.5.1: 262840251 → 262842251). `--split-per-abi`가 arm64 APK에 2000을 더하기 때문입니다. Play용 AAB는 이 오프셋이 붙지 않으므로 같은 값을 직접 받습니다. 그래서 같은 실행의 APK와 AAB는 versionCode가 같고, 워크플로가 이를 확인합니다. Android 잡의 Build number 단계가 같은 분(UTC)에 돈 두 실행은 versionCode가 같으므로 Play는 두 번째를 받지 않습니다. 그때는 다시 빌드합니다.
- iOS가 허용하는 형식은 숫자 세 개(`1.6.0`)뿐입니다.
- Windows에서 내려받는 파일 이름은 `HJViewer.exe`이고, 같은 파일을 `HJViewer-install.exe`로도 냅니다(더블클릭하면 바로 설치 화면). 설치하면 `C:\Program Files\HJViewer\HJViewer.exe`가 됩니다. 압축 해제 실행기는 파일 이름을 읽습니다. 이름이 `install.exe`로 끝나면 바로 설치하고, `-qs-`가 들어가면 일회용 지원 모드로 실행합니다. 그러니 이름을 바꿀 때는 이 둘을 피하세요.

## 서버 값 (필수)

비공개 빌드 저장소에서는 두 값을 **시크릿**으로 넣습니다(위 "비공개 빌드 저장소"). 시크릿은 로그에서 가려집니다. 이 저장소에서 직접 실행할 때는 Settings → Secrets and variables → Actions의 **Variables** 탭에 변수로 만들어도 됩니다. 변수는 로그에 그대로 찍힙니다.

| 이름 | 값 | 예 |
|---|---|---|
| `HJV_SERVER` | 서버 도메인 | `rd.example.com` |
| `HJV_SERVER_KEY` | hbbs 공개키(base64 ed25519) | 서버의 `deploy/data/id_ed25519.pub` 내용 ([deploy.md](deploy.md)) |

```bash
gh variable set HJV_SERVER --body rd.example.com
gh variable set HJV_SERVER_KEY --body "$(ssh <서버> cat hjviewer/deploy/data/id_ed25519.pub)"
```

두 값은 빌드할 때 Rust 코드에 박힙니다. 그래서 값을 바꾸면 앱을 다시 빌드해 배포해야 합니다. 값이 비어 있으면 앱이 순정 RustDesk처럼 공개 서버를 씁니다. 이를 막기 위해 빌드 저장소가 부른 실행과 태그 빌드는 실패하고, 이 저장소에서 직접 실행하면 경고만 남깁니다.

수동 실행에서는 `server`, `server_key` 입력으로 이번 실행에만 쓸 값을 줄 수 있습니다. 저장소 변수보다 우선합니다. 예: 테스트 서버용 빌드 `gh workflow run client-build.yml -f platforms=android -f server=rd-test.example.com -f server_key=...`

## 계정 기능의 공개키 (1.6.0부터)

연세포탈 로그인과 비밀번호 없는 연결(설계 3판)은 서버 키의 공개 쪽을 앱과 중계기에 고정합니다. 서버 값처럼 빌드할 때 박히고, 값을 바꾸면 다시 빌드해야 합니다. 모두 공개값이라 비밀은 아니지만, 비공개 빌드 저장소에는 서버 값과 같은 방식으로 **저장소 시크릿**으로 넣습니다(워크플로는 `secrets.X || vars.X`로 받음). 개인키는 저장소, 개발 PC, 에이전트 세션 어디에도 두지 않습니다. 키는 사용자가 만들고 공개키와 번호만 알려 줍니다(설계 3판 5-3, 11장 5단계).

| 이름 | 값 | 받는 곳 |
|---|---|---|
| `HJV_LOGIN_SEAL_PUBKEY` | 로그인 비밀번호를 봉인하는 X25519 공개키: `hjv-api keygen seal`이 출력하는 표준 base64 한 개(32바이트) | 세 앱 |
| `HJV_TRUST_PUBKEYS` | 신뢰 목록 키(Ed25519) 둘, 현재와 다음: `1:<base64>,2:<base64>` | 세 앱 |
| (업데이트 키) | 시크릿이나 변수로 넣지 않습니다. 공개 파일 `client/update-keys.env`에서만 읽습니다(아래 "업데이트 키와 릴리스 기록") | 세 앱 |
| `HJV_COMMAND_PUBKEY` | 켜기 명령 키(Ed25519): `1:<base64>[,2:<base64>]` ([relay/PROTOCOL.md](../relay/PROTOCOL.md) 3.2) | hjv-relay만 (앱은 받지 않음) |

```bash
# 사용자가 키를 만든 곳에서 받은 공개값을 넣습니다. 출력에 개인키가 없는지 먼저 봅니다.
gh secret set HJV_LOGIN_SEAL_PUBKEY -R yonsei-projecttf/hjviewer-private --body '<base64>'
gh secret set HJV_TRUST_PUBKEYS -R yonsei-projecttf/hjviewer-private --body '1:<base64>,2:<base64>'
gh secret set HJV_COMMAND_PUBKEY -R yonsei-projecttf/hjviewer-private --body '1:<base64>'
gh secret list -R yonsei-projecttf/hjviewer-private   # 이름과 날짜만 보입니다
```

- **확인 단계**: Bridge codegen 잡의 "Check the pinned public keys"(`scripts/check-build-keys.py`)가 봅니다. 빌드 저장소가 부른 실행이고 `client/version.env`가 1.6.0 이상이면 `HJV_LOGIN_SEAL_PUBKEY`와 `HJV_TRUST_PUBKEYS`가 꼭 있어야 하고(`check_only`면 건너뜀), 넣은 키는 모두 형식(공백과 줄바꿈 없는 한 줄, 키 번호 1–65535, 표준 base64 32바이트, 목록 키는 정확히 두 개, 번호와 키가 겹치지 않음)과 약한 키(작은 위수의 점) 여부를 봅니다. 빌드는 값을 바이트 그대로 받으므로, 파일에서 넣다가 끝에 붙은 줄바꿈도 실패로 잡습니다. 이 저장소에 개인키가 공개된 **시험 키**면 실패합니다. 같은 바이트를 32번 되풀이하거나 32개의 연속한 바이트로 된 seed로 만든 모든 키, RFC 8032·7748의 예시 키, `scripts/public-test-keys.txt`의 키, 그리고 git이 추적하는 파일에 base64나 16진수로 적힌 32바이트 값마다 그 값을 Ed25519 seed나 X25519 비밀키로 본 공개키입니다. 마지막 것은 시험 벡터처럼 비밀키가 공개키 옆에 적힌 경우(예: `server/api/src/seal.rs`의 봉인 시험 키)를 목록에 적지 않아도 잡습니다. 거부할 때는 그 비밀키가 있는 파일과 줄을 알려 줍니다. `scripts/public-test-keys.txt`가 없으면 확인 단계가 실패합니다. 이 저장소에서 직접 돌리는 실행에서는 같은 문제가 경고로만 나옵니다. hjv-relay 잡은 `HJV_COMMAND_PUBKEY`를 같은 방식으로 보고, `package-relay.sh`가 같은 검사(`check-build-keys.py --relay`, 엄격 모드)를 빌드 전과 빌드한 실행 파일의 키로 한 번 더 하며 서버 이름도 봅니다.
- 계정 공개키가 없는 빌드는 로그인도 비밀번호 없는 연결도 없이 1.5.1처럼 동작합니다(설계 3판 6-5). 확인 단계가 막는 것은 빌드 저장소의 1.6.0 출시 빌드에서 그런 앱이 나가는 일입니다.
- **지문 대조**: 확인 단계는 고정한 공개키마다 SHA-256 지문(원래 32바이트의 SHA-256, 16진수)을 실행 요약에 표로 적습니다. `HJV_SERVER_KEY`도 함께 적습니다. 출시 태그를 붙이기 전에 이 표를 서버의 `hjv-api selfcheck`가 출력한 지문과 사람이 대조합니다(설계 3판 11장 8단계). 봉인 키의 키 ID 칸은 지문의 앞 16자리이고, `hjv-api keygen seal`이 출력하는 키 ID와 같습니다.

## 업데이트 키와 릴리스 기록 (1.6.0부터)

최소 버전과 강제 업데이트(최소 버전 설계 6-4)를 위해 앱 빌드가 하는 일은 다섯입니다. 새 시크릿은 없습니다.

- 업데이트 키를 공개 파일 `client/update-keys.env`에서 읽어 앱에 박습니다.
- 산출물마다 `release.json`과 `SHA256SUMS`를 넣습니다.
- Android 잡이 APK의 매니페스트를 확인합니다. 앱 안 업데이트가 있고 제어받는 쪽은 없어야 합니다.
- iOS 앱에 내보내기 방식을 알려 줍니다.
- Windows 잡이 업데이트와 패커 보호를 연기 시험으로 확인합니다.

### 업데이트 키 파일 `client/update-keys.env`

앱은 서버가 내주는 버전 정책을 이 파일의 공개키로 확인합니다. 값은 세 줄입니다.

| 이름 | 값 |
|---|---|
| `HJV_UPDATE_PUBKEY` | 서명 키의 공개키. 현재 키와 다음 키, 둘까지입니다: `1:<base64>[,2:<base64>]`. 번호는 1–100입니다 |
| `HJV_UPDATE_ROOT_PUBKEY` | 루트 키의 공개키 둘까지: `101:<base64>,102:<base64>`. 번호는 101–199이고, 1.6.0은 루트 키 둘을 박습니다(신뢰 규칙 1-4). 번호와 키가 서명 키와 달라야 합니다 |
| `HJV_POLICY_FLOOR` | 이 빌드를 만들 때의 정책 위치 `<세대>:<seq>`. 첫 정책이 나오기 전에는 `1:0`입니다 |

- **시크릿이나 변수로 넣지 않습니다.** 계정 기능의 공개키와 다른 점입니다. 세 플랫폼 잡의 "Pin the update keys" 단계가 체크아웃한 소스의 이 파일을 읽어 빌드에 넘기고, 다른 곳에서는 받지 않습니다. 그래서 빌드 저장소의 시크릿이나 변수로는 앱이 믿는 업데이트 키를 바꿀 수 없습니다. 키를 바꾸려면 공개 저장소에서 이 파일을 고쳐야 하고, 그 변경은 검토를 거쳐 소스 태그에 남습니다. 워크플로의 `env:`에 이 세 이름을 더하지 마세요(`scripts/test_check_build_keys.py`가 막습니다).
- **키는 2026-10-14에 넣었습니다**(서명 키 1·2, 루트 키 101·102, 커밋 8622975, 1.6.0의 출시 빌드부터). 키는 사용자만 만듭니다(아래 "업데이트 키 보관 규칙"). 키 줄이 빈 빌드는 정책을 확인하지 않고 1.5.1처럼 동작합니다. 하한은 릴리스마다 `hjv-release floor --live`로 정합니다(`tools/hjv-release/README.md` 2장 1번). `client-1.7.0`은 이를 빠뜨려 `1:1`입니다(빌드할 때 걸린 정책은 `1:2`, 태그 메모).
- **형식**: 세 이름이 한 번씩, 줄 맨 앞에서 `이름=값`으로 있어야 합니다. 값 뒤에는 아무것도 둘 수 없습니다. 공백, 따옴표, `export`, 줄 끝 주석이 모두 안 됩니다. 그 밖에는 빈 줄과 `#`으로 시작하는 줄만 둘 수 있습니다. 줄 끝은 LF이고, BOM은 안 됩니다. LF 말고 다른 프로그램이 줄바꿈으로 읽는 문자(U+000B, U+000C, U+001C–U+001E, U+0085, U+2028, U+2029)는 주석에도 둘 수 없습니다. 이렇게 한 가지 모양만 받는 이유는 이 파일을 여러 곳이 읽기 때문입니다(CI, `hjv-release`, 서버의 확인 스크립트).

확인은 두 곳에서 같은 코드(`scripts/check-build-keys.py`)로 합니다. Bridge codegen 잡의 "Check the pinned public keys" 단계가 한 번 보고 실행 요약에 적습니다. 세 플랫폼 잡의 "Pin the update keys" 단계가 값을 빌드에 넘기기 직전에 다시 봅니다.

| 파일의 상태 | 빌드 저장소의 출시 빌드 | 빌드 저장소의 확인 빌드(`check_only`) | 이 저장소에서 직접 실행 |
|---|---|---|---|
| 형식이 틀림 | 실패 | 실패 | 실패 |
| 키가 없음(2026-10-14 전의 상태) | 실패 | 통과 | 경고 |
| 이 저장소의 시험 키가 있음 | 실패 | 실패 | 경고 |
| 올바른 키 | 통과 | 통과 | 통과 |

- "출시 빌드"는 빌드 저장소가 부른 실행이고 `client/version.env`가 1.6.0 이상이며 `check_only`가 아닌 실행입니다. 계정 기능의 공개키와 같은 기준입니다.
- **형식이 틀린 경우**는 다음과 같습니다. 어느 실행에서든 실패합니다.
  - 위의 줄 모양이 아니거나, 이름이 빠졌거나 두 번 나옴
  - 키 번호가 1–65535가 아니거나, 키가 표준 base64 32바이트가 아님
  - Ed25519 공개키가 아니거나, 약한 키(작은 위수의 점)이거나, 주 부분군 밖의 점임
  - 서명 키나 루트 키가 셋 이상임. 서명 키 번호가 1–100 밖이거나 루트 키 번호가 101–199 밖임(선언은 서명 키 번호만 폐기합니다). 번호나 키가 서로 겹침
  - 두 키 줄 가운데 하나만 채워져 있음. 키는 있는데 하한이 비어 있음
  - 하한이 `<세대>:<seq>`가 아님(세대는 1 이상, 앞의 0 없는 숫자)
  - 환경 변수에 같은 이름이 파일과 다른 값으로 들어 있음(시크릿이나 변수로 넣으려 한 경우)
- 이렇게 엄격한 이유: 앱은 키 하나라도 규칙을 어기면 키 전체를 쓰지 않습니다. 그런 빌드는 정책을 영영 확인하지 않습니다. 규칙은 앱의 코어(`src/hjviewer_update/trust.rs`)와 `hjv-release keys --production`의 것과 같습니다.
- **시험 키**의 기준은 계정 기능의 공개키와 같습니다(위 "확인 단계"). `hjv-release test-keys`가 내는 업데이트 시험 키는 `scripts/public-test-keys.txt`에도 적어 두었습니다.
- 오류 문장은 줄 번호만 말하고 줄의 내용은 출력하지 않습니다. 공개키가 아닌 것을 잘못 붙여 넣었을 때 로그에 남지 않게 하기 위해서입니다.
- 통과하면 실행 요약의 표에 키마다 번호와 SHA-256 지문이 나오고, 하한도 함께 나옵니다.

키를 넣는 순서는 다음과 같습니다.

1. 사용자가 `hjv-release keygen`으로 키 셋을 만듭니다(`tools/hjv-release/README.md` 1장). 화면에 나오는 공개 줄 `N:<base64>`만 이 파일에 옮깁니다.
2. 개발 PC에서 확인합니다. 둘 다 통과해야 합니다.

   ```bash
   hjv-release keys --production                                   # 도구의 확인
   CALLED=1 python3 -I scripts/check-build-keys.py --pin-update-keys   # 출시 빌드가 하는 확인. 통과하면 세 줄을 출력
   ```
3. 커밋합니다. 이 파일이 마지막 push에 들어가야 출시 빌드가 됩니다.
4. 릴리스마다 `HJV_POLICY_FLOOR`를 서버에 걸린 정책의 위치로 올립니다. `hjv-release floor --live <서버에서 받은 정책 파일>`이 그 줄을 출력합니다(보관소에 있는 파일만 받습니다. `tools/hjv-release/README.md` 2장).

### 업데이트 키 보관 규칙

사용자 결정 D-K(최소 버전 설계 6-3)입니다.

| 키 | 두는 곳 |
|---|---|
| 서명 키 1 | 개발 PC의 `~/hjviewer-secrets/hjviewer-update-1.key`(비밀번호로 암호화, 0600)와 사용자의 오프라인 사본 |
| 서명 키 2(다음 교체용) | 오프라인 매체에만 |
| 루트 키 둘(번호 101, 102) | 키마다 오프라인 매체 두 곳에만. 개발 PC 디스크에 쓰지 않고, 깨끗한 컴퓨터에서만 엽니다 |
| 공개키(서명 키 둘, 루트 키 둘) | 이 저장소의 `client/update-keys.env`, 서버의 `/etc/hjviewer/update-keys.env` |

- 비밀키는 저장소, GitHub 시크릿과 변수, 빌드 저장소, 운영 서버, `build/` 폴더, 메모, 에이전트 세션 어디에도 두지 않습니다.
- 키 만들기(`keygen`)와 서명(`sign`)은 **사용자만** 합니다. 에이전트는 실제 키를 만들지 않고, 실제 정책에 서명하지 않고, 키 파일을 읽지 않습니다. 도구도 터미널이 없으면 두 명령을 거부합니다.
- CI에는 업데이트 키에 관한 시크릿이 없습니다. 빌드 저장소나 운영 서버가 뚫려도 정책을 위조할 수 없게 하려는 것입니다.
- 키를 바꾸는 일은 이 파일을 고친 새 빌드로만 됩니다. 키가 샜을 때의 절차는 `tools/hjv-release/README.md` 3장에 있습니다.

정해진 교체(키가 새지 않았을 때)는 다음 순서로 합니다(최소 버전 설계 6-3). 루트 키는 쓰지 않습니다.

1. 빌드에는 늘 서명 키 둘을 박습니다. 지금 서명하는 키와 다음 키입니다. 예를 들어 `HJV_UPDATE_PUBKEY=1:<키 1>,2:<키 2>`이면 키 1로 서명합니다.
2. 정책을 확인하는 앱이 모두 다음 키를 박은 빌드로 올라온 뒤에, 다음 정책부터 키 2로 서명해 서버에 넣습니다. 남은 앱 버전은 `hjv-api admin report`와 운영자의 기기 목록으로 봅니다. 업데이트 키가 없는 빌드와 1.5.x는 정책을 확인하지 않으므로 세지 않습니다. 이때부터 키 2를 위 표의 서명 키 1처럼 둡니다.
3. 서버의 정책이 키 2로 서명된 뒤에 내는 빌드에서 옛 키를 빼고 새 다음 키를 넣습니다: `HJV_UPDATE_PUBKEY=2:<키 2>,3:<키 3>`. 키 3은 사용자가 `hjv-release keygen --role signing --id 3 --out <오프라인 매체의 파일>`로 만들어 오프라인 매체에만 둡니다. 새 키에는 쓴 적 없는 번호를 줍니다. 키 폐기는 번호로 하기 때문입니다. 루트 키 줄은 그대로 둡니다.
4. 3단계의 빌드를 낸 뒤 사용자가 물러난 키 1의 비밀키를 모두 지웁니다. 개발 PC의 `~/hjviewer-secrets/hjviewer-update-1.key`, 오프라인 사본, 그 밖에 만든 사본입니다. 그리고 다른 이유로 다음 루트 선언을 만들 때(`--relax`, `--new-epoch`, 다른 키의 유출 등) 그 `policy` 명령에 `--revoke-key 1`을 더해 키 1을 폐기합니다. 폐기만을 위해 따로 루트 선언을 만들 필요는 없습니다(신뢰 규칙 명세 5장 7번).

- 순서를 지키는 이유: 앱은 자기 빌드에 박힌 키로 서명한 정책만 받습니다. 다음 키를 모르는 앱이 남았는데 그 키로 서명하면, 그 앱은 새 정책을 받지 못해 손으로 설치해야 합니다. 서버의 정책이 아직 옛 키로 서명돼 있는데 옛 키를 뺀 빌드를 내면, 그 빌드는 지금 정책을 받지 못합니다.
- 키 1을 지우는 이유: 키 1을 박은 빌드는 3단계 뒤에도 기기에 남아 있고, 키 1로 서명한 정책을 받습니다. 폐기 선언을 받은 앱은 키 1을 더 믿지 않습니다. 하지만 선언을 아직 받지 못한 앱은 여전히 믿습니다. 옛 설치 파일로 새로 설치한 앱도 그렇습니다. 그래서 비밀키가 남아 있다가 새면 그 앱들에 가짜 정책을 낼 수 있습니다.
- **키를 바꾼 빌드마다 `HJV_POLICY_FLOOR`를 서버에 걸린 정책의 위치로 올립니다**(`hjv-release floor --live <서버에서 받은 정책 파일>`). 앱은 키가 다른 빌드로 바뀌면 저장한 정책 상태를 버리고 그 빌드의 하한에서 다시 시작합니다(최소 버전 설계 1-4). 하한이 낮으면 그 빌드로 바꾼 앱이 이미 지난 옛 정책을 다시 받을 수 있습니다.
- 다음 교체도 같습니다. 정책을 확인하는 앱이 모두 키 3을 박은 빌드로 올라오면 키 3으로 서명하고, 그다음 빌드에 `3:<키 3>,4:<키 4>`를 넣습니다. 그 빌드를 낸 뒤 키 2의 비밀키를 지우고, 다음 루트 선언에서 `--revoke-key 2`로 폐기합니다.

### 릴리스 기록 `release.json`, `SHA256SUMS`

세 플랫폼 잡은 고지 파일을 넣은 다음 "Write release.json and SHA256SUMS" 단계(`scripts/release-info.py`)에서 두 파일을 만들어 기존 아티팩트에 함께 넣습니다. 아티팩트 이름과 보관 기간(7일)은 그대로입니다.

```json
{"platform": "windows", "version": "1.6.0", "build": "1.6.0.41", "file": "HJViewer.exe",
 "size": 25329664, "sha256": "<소문자 hex 64자>", "source_sha": "<공개 저장소 커밋 40자리>",
 "run_id": "<실행 번호>", "workflow": ".github/workflows/build.yml"}
```

| 필드 | 어디서 오나 |
|---|---|
| `platform` | `windows`, `android`, `ios` |
| `version` | `client/version.env`. APK의 versionName, IPA의 CFBundleShortVersionString이 다르면 잡이 실패합니다 |
| `build` | Windows는 `<앱 버전>.<실행 번호>`(설치된 앱의 파일 버전). Android는 APK에서 읽은 versionCode, iOS는 IPA에서 읽은 CFBundleVersion입니다 |
| `file`, `size`, `sha256` | 그 아티팩트의 패키지 파일. Windows는 `HJViewer.exe`, Android는 올라가는 APK, iOS는 IPA입니다 |
| `source_sha` | 잡이 체크아웃한 공개 저장소 커밋 |
| `run_id` | 실행 번호. 빌드 저장소가 부른 실행이면 빌드 저장소의 실행입니다 |
| `workflow` | 그 실행의 워크플로 파일. 빌드 저장소가 부른 실행이면 `.github/workflows/build.yml`입니다 |

- **`SHA256SUMS`**는 그 아티팩트에 올라가는 파일의 해시입니다(`sha256sum` 형식). 패키지, Windows의 `HJViewer-install.exe`, 고지 파일 넷, `release.json`이 들어갑니다. 받은 폴더에서 `sha256sum -c SHA256SUMS`로 확인합니다.
- **실행 요약**에 패키지의 SHA-256이 나옵니다. 정책에 서명하는 사람은 `hjv-release sign`이 보여 주는 해시를 이 값과 대조합니다.
- **쓰는 곳**: `hjv-release policy`의 출처 확인입니다. `gh run download <실행 번호> -R <빌드 저장소> -D build/release-<버전>`으로 받은 폴더를 그대로 `--release-dir`에 줍니다. 도구는 그 실행의 산출물을 직접 다시 받아, 패키지와 `release.json`이 그 안에 바이트 그대로 있는지 봅니다.
  - 그래서 받은 `release.json`을 고치면 안 됩니다.
  - 도구는 그 실행의 `build.yml`이 위 "비공개 빌드 저장소"의 모양인지도 봅니다. 도구가 모르는 입력을 넘기면 거부하므로, `check_only` 입력을 도구가 알기 전에는 `--allow-input check_only`를 함께 줍니다. 확인 빌드에는 `release.json`이 없어서, 이 입력을 허락해도 확인 빌드가 릴리스로 받아들여지지 않습니다.
  - **새 패키지가 든 정책 초안은 빌드한 지 7일 안에 만듭니다.** 아티팩트가 7일 뒤 지워지기 때문입니다. 더 길게 필요하면 워크플로의 `ARTIFACT_RETENTION_DAYS`를 올립니다(저장 공간을 그만큼 더 씁니다).
  - **정책에 쓰는 실행은 성공으로 끝나야 합니다.** 도구는 잡 하나가 아니라 실행 전체의 결과를 봅니다. 그래서 Google Play나 TestFlight 업로드 하나만 실패해도 그 실행의 Windows·Android 패키지를 받지 않습니다. 이때는 실행 화면의 "Re-run failed jobs"로 실패한 잡만 다시 돌려 실행을 성공으로 끝낸 뒤 초안을 만듭니다. 다시 도는 잡은 그 실행의 아티팩트를 받습니다. Bridge codegen의 `bridge-artifact`도 다른 아티팩트처럼 7일 두므로, 7일 안에 다시 돌립니다.
  - 그래도 안 되면 같은 태그 커밋으로 새로 실행합니다. 실패한 실행이 TestFlight나 Google Play에 빌드를 하나도 올리지 않았을 때만 버전을 그대로 둡니다(그 패키지를 적은 정책은 아직 없습니다). 새 실행은 빌드 번호가 달라지므로 Play와 TestFlight에도 새 실행의 빌드를 올리고, `--play-live`와 `--ios-valid`에는 그 번호를 줍니다. 하나라도 올라갔으면 테스터가 같은 버전을 두 번 받지 않게 "앱 버전"대로 `client/version.env`의 버전을 올려 새로 빌드합니다. 판단하기 어려우면 사용자에게 묻습니다.
- **나눠 줄 수 없는 빌드에는 `release.json`이 없습니다.** 확인 빌드(`check_only`), 디버그 서명 APK(키스토어 시크릿이 없을 때), 서명 없는 IPA입니다. 확인 빌드는 출시에 필요한 공개키 없이도 만들어지고, 디버그 서명 APK는 사용자의 앱 위에 설치되지 않고, 서명 없는 IPA는 어디에도 설치되지 않습니다. `hjv-release`는 `release.json`이 없는 폴더를 릴리스로 받지 않습니다. 단계 자체는 이런 빌드에서도 돌아 같은 검사를 하고 `SHA256SUMS`를 만들며, 실행 요약에 "Not for release"와 그 이유를 적습니다.
- Google Play용 AAB 아티팩트에는 `release.json`을 넣지 않습니다. 같은 Android 빌드이고, 도구는 한 실행에서 Android 패키지를 하나만 받습니다.
- 이 저장소에서 직접 돌린 실행의 `release.json`은 `workflow`가 `client-build.yml`이라 도구가 받지 않습니다.
- `client/version.env`의 버전이 도구가 받는 모양(앞의 0이 없는 숫자 셋, 앞의 둘은 세 자리까지, 마지막은 두 자리까지)이 아니면 이 단계가 실패합니다. 확인 빌드에서 먼저 드러납니다.
- **출처 증명(artifact attestation)은 넣지 않았습니다.** 비공개 저장소에서 쓸 수 있는 요금제인지 확인하지 못했습니다. 될 때는 세 플랫폼 잡에 `actions/attest-build-provenance`(커밋 SHA로 고정)를 더하고, 빌드 저장소 `build.yml`의 잡에 `id-token: write`와 `attestations: write`를 허락한 뒤, `hjv-release policy --attest`를 씁니다. 그 전에는 위의 `gh` 확인이 출처 확인의 전부입니다.

### Android 매니페스트 확인 (1.6.0부터)

`client/version.env`가 1.6.0 이상이면 Android 잡의 "Write release.json and SHA256SUMS" 단계가 APK의 매니페스트도 봅니다. Gradle이 앱, 클라이언트 패치, 플러그인과 라이브러리의 매니페스트를 합친 결과입니다. 맞지 않으면 단계가 실패하고 APK는 올라가지 않습니다. 확인 빌드와 디버그 서명 APK도 같습니다.

- **있어야 하는 것**: 사이드로드 APK의 앱 안 업데이트(최소 버전 설계 5-4)가 쓰는 두 항목입니다. `REQUEST_INSTALL_PACKAGES` 권한(`uses-permission`이나 `uses-permission-sdk-23`)과 `com.carriez.flutter_hbb.HjvUpdateFileProvider` provider입니다.
- **없어야 하는 것**: 제어받는 쪽입니다. 1.6.0부터 모든 Android 빌드는 제어만 합니다(Google Play 설계의 결정 D1). 접근성 서비스(`BIND_ACCESSIBILITY_SERVICE`로 묶는 서비스), `foregroundServiceType`이 있는 요소, 권한 `FOREGROUND_SERVICE`와 `FOREGROUND_SERVICE_*`, `SYSTEM_ALERT_WINDOW`, `RECEIVE_BOOT_COMPLETED`, `REQUEST_IGNORE_BATTERY_OPTIMIZATIONS`, 그리고 앱의 `InputService`, `MainService`, `FloatingWindowService`, `BootReceiver`입니다. 라이브러리가 가져온 것도 실패합니다.
- 그 밖의 항목은 보지 않습니다. 그래서 1.9.0의 앱 잠금이 더하는 `USE_BIOMETRIC`, `USE_FINGERPRINT`(아래 "Play 선언 대상")는 이 단계와 `scripts/release-info.py`의 목록을 바꾸지 않습니다. 시험(`scripts/test_release_info.py`)이 그 둘이 든 매니페스트도 통과하는 것을 봅니다.
- 실패하면 클라이언트 패치(제어 전용, 앱 안 업데이트)가 빠졌거나, 플러그인이나 라이브러리 갱신이 이런 항목을 다시 가져온 것입니다. 오류 문장이 빠진 항목과 있으면 안 되는 항목을 모두 적습니다. 일부러 바꿀 때는 `scripts/release-info.py`의 목록과 이 절을 같은 커밋에서 고칩니다.
- 이 확인이 없으면 패치가 통째로 빠져도 빌드는 초록입니다. Play 번들의 매니페스트 검사는 공개 저장소의 실행과 서명 없는 실행에서는 돌지 않고, `play`도 `check_only`도 아닌 실행에서는 경고만 남깁니다(확인 빌드에서는 잡이 실패합니다). 앱 안 업데이트 두 항목이 빠진 것은 번들 검사가 어느 실행에서도 잡지 못합니다.
- Play 번들은 이 단계가 보지 않습니다. 번들에서는 앱 안 업데이트의 두 항목이 빠져야 하고, "Sign and check Play bundle"이 그것을 봅니다(아래 "Google Play 업로드").

### iOS 내보내기 방식

서명 빌드는 `flutter build ipa`에 `--dart-define=HJV_IOS_EXPORT=<내보내기 방식>`을 넘깁니다. 값은 "Set up code signing" 단계가 프로파일을 보고 정한 것입니다(`app-store-connect`, `release-testing`, `debugging`, `enterprise`). 앱은 이 값이 `app-store-connect`일 때만 업데이트 화면에서 TestFlight를 엽니다. Ad Hoc 같은 다른 설치본은 TestFlight로 업데이트할 수 없기 때문입니다(최소 버전 설계 5-5). TestFlight와 App Store는 같은 빌드를 내주므로, 1.9.1부터 앱은 그 값일 때 설치본의 영수증(`package_info_plus`의 `installerStore`)도 봅니다. App Store의 영수증(`com.apple`)이면 App Store를, 샌드박스 영수증(`com.apple.testflight`, 심사하는 앱도)이면 TestFlight를 엽니다. 서명 없는 빌드에는 값이 없습니다.

### Dart 테스트

Dart tests 잡의 필수 목록에 업데이트 화면의 테스트 둘을 더했습니다. 순수 테스트 `test/hjv_update_test.dart`와, 생성된 bridge가 필요한 게이트 테스트 `test/hjv_update_gate_test.dart`입니다. 1.8.0의 출입 화면은 순수 테스트 `test/hjviewer_access_test.dart`(문구, 실패 코드, Rust와 주고받는 값)를 더했습니다. 1.9.0은 순수 테스트 여섯을 더했습니다. 토스트의 `test/hjviewer_toast_test.dart`, ID 제안의 `test/hjviewer_suggestions_test.dart`, 휴대폰 터치 대상과 시스템 막대의 `test/hjviewer_phone_test.dart`, 빌드가 고정한 설정을 빼는 `test/hjviewer_pins_test.dart`, 색 대비와 한국어 줄바꿈의 `test/hjviewer_style_test.dart`(1.9.0부터 알림 상자 `HjvCallout`과 옅은 톤의 대비, 단추 `hjvButton`의 모양과 높이, 대화상자 폭과 알림 창의 틀, core 오류 토스트의 아이콘과 낭독, 글자 크기 역할 `HjvText.caption`·`captionStrong`·`dialogTitle`의 값, 관리자 화면의 알약 탭 `HjvPillTabs`도), 앱 잠금(묻는 때, 인증 결과, 문구)의 `test/hjviewer_applock_test.dart`입니다. 여섯 모두 `CLIENT_DART_TESTS`에도 있습니다. 1.9.0은 생성된 bridge가 필요한 `test/hjviewer_dialog_frame_test.dart`(앱 테마의 데스크톱 단추 높이 36, 세션 메시지 창의 아이콘과 제목 한 줄)를 더했습니다. 이것은 `CLIENT_DART_TESTS`에 없고 Dart tests 잡에서만 돕니다. 1.9.0은 순수 테스트 열넷도 더했습니다. PC 연결 창의 `test/hjviewer_cm_test.dart`(확인 표시, 주의 줄과 관리자 연락처 줄의 글, 녹화 때 창을 띄우는 1분 규칙, 1.8.x 코어의 JSON), 교직원 지원 링크와 대기실, 다시 시작 줄의 `test/hjviewer_support_test.dart`, 연결 창 카드가 Pretendard로 300 x 490에 들어가는지 보는 `test/hjviewer_cm_fit_test.dart`(overlay의 글꼴 `assets/fonts/Pretendard-*.otf`를 읽음), 세션이 끝날 때 잠그는 규칙의 `test/hjviewer_end_lock_test.dart`, 원격 준비 글의 `test/hjviewer_ready_test.dart`, 휴대폰 세션의 제스처 안내와 바꾸기의 `test/hjviewer_session_test.dart`, 들어온 연결 목록의 답과 글, 키보드의 `test/hjviewer_incoming_test.dart`, 홈 계정 줄의 `test/hjviewer_home_test.dart`, 데스크톱 키보드의 `test/hjviewer_keys_test.dart`, 화면 낭독기의 `test/hjviewer_semantics_test.dart`, 세션 창 색의 `test/hjviewer_session_look_test.dart`, 기기 목록 제목 줄의 `test/hjviewer_title_line_test.dart`, Windows 11 최대화 단추의 `test/hjv_caption_test.dart`(`hjv-snap-layouts`, 창 메뉴의 스위치, 연결 창·설치 창 제외, runner와 주고받는 채널), 휴대폰이 시스템 글자 크기를 130%까지 따르는 것(묶는 값, 원격 화면·카메라 화면·터미널의 100%, 메뉴 줄과 알림 상자 제목의 자리)의 `test/hjviewer_text_scale_test.dart`입니다. 열넷 모두 `CLIENT_DART_TESTS`에도 있습니다. 1.9.1은 순수 테스트 `test/hjviewer_settings_split_test.dart`를 더했습니다. 데스크톱 설정이 일상 탭(계정, 일반, 보안, 디스플레이)과 고급 설정으로 나뉘는지를 `desktop_setting_page.dart`와 `desktop_home_page.dart`의 소스로 봅니다(탭 목록에 네트워크 탭이 없음, 'ID 변경'이 고급 설정으로 감, 일상 탭에 남는 행, 고급 설정의 묶음과 카드 순서). 이것도 `CLIENT_DART_TESTS`에 있습니다. 패치된 트리에 이 파일들이 없으면 잡이 실패합니다.

### Windows 업데이트·패커 연기 시험

Windows 잡은 설치 시험(위 "Windows 설치 시험") 다음에 두 단계를 더 돕니다. 둘 다 서버 값이 있는 빌드에서만 돌고, 설치 시험처럼 실패해도 빌드를 멈추지 않습니다(continue-on-error). 결과는 그 단계의 표시와 로그로 봅니다. 몇 번 통과한 뒤 필수로 바꿉니다.

- **Update smoke test**: 설치된 PC는 서비스가 새 파일을 SYSTEM으로 `--update`와 함께 실행해 스스로 업데이트합니다(최소 버전 설계 5-1). 러너에는 이전 버전이 없으므로, 방금 설치한 같은 빌드를 예약 작업으로 SYSTEM에서 `HJViewer-<버전>.exe --update`로 한 번 더 돌립니다. 확인하는 것은 다음과 같습니다.
  - 업데이트 로그에 `Update completed.`가 있는지
  - 서비스가 자동 시작이고 실행 중인지
  - 앱 목록의 `DisplayVersion`이 업데이트 전과 같고 `client/version.env`와 같은지
  - 설치된 `HJViewer.exe`가 새로 쓰였는지
  - 서버 이름을 127.0.0.1로 돌린 hosts 줄이 그대로인지(러너가 운영 서버에 등록하지 않게 하는 가드)

  설치 시험이 실패하면 이 단계는 건너뜁니다.
- **Packer guard smoke test**: 옛 HJViewer가 압축을 푼 폴더에서 아직 실행 중이면, 새 압축 해제 실행기는 파일을 바꿀 수 없으므로 아무것도 실행하지 않고 종료 코드 32로 끝나야 합니다(최소 버전 설계 5-3). 러너에서는 그 폴더의 `HJViewer.exe` 자리에 다른 내용을 두고, 실행 중인 프로그램의 파일처럼 쓰기와 삭제를 막은 채 열어 둡니다. 그 상태에서 실행기를 `--update`로 돌려 종료 코드가 32인지, 그 폴더에서 시작된 프로세스가 없는지, 잡아 둔 파일이 그대로인지 봅니다. 실행기는 `--update`나 `--silent-install`로 시작하면 안내 창(K1)을 띄우지 않습니다. 창이 뜨면 누를 사람이 없어 단계가 멈추므로 `--update`로 돌립니다.
- **한계**: 같은 버전을 다시 설치하므로 버전이 오르는 것은 보지 못합니다. 예약 작업은 세션 0에서 돌고, 실제 PC에서는 서비스가 사용자 세션에서 실행합니다. 서명된 정책 받기, 내려받기와 해시 확인, 안내 창은 이 시험에 없습니다. 기기 시험으로 봅니다.

### 개발 PC에서 확인하기

GitHub 실행 없이 볼 수 있는 것은 아래 두 오프라인 시험이 봅니다. 네트워크와 자격 증명이 필요 없습니다.

```bash
python3 -I -B -m unittest discover -s scripts -p 'test_check_build_keys.py'   # 키 파일 확인, 워크플로의 확인·고정 단계
python3 -I -B -m unittest discover -s scripts -p 'test_release_info.py'       # release.json, SHA256SUMS, 워크플로의 나머지 단계
```

- `HJV_RELEASE_BIN=<빌드한 hjv-release>`를 주면 두 가지를 더 봅니다. 키 파일을 `hjv-release keys --production`과 같게 판정하는지, 그리고 `hjv-release policy`가 이 스크립트가 만든 산출물과 위 "비공개 빌드 저장소"의 `build.yml`로 초안을 만드는지입니다. `gh`의 답은 기록해 둔 것을 씁니다.
- 실제 APK와 IPA에서 버전과 빌드 번호를 읽고, APK의 매니페스트가 위 "Android 매니페스트 확인"을 통과하는지 봅니다. 실제 패키지는 저장소의 `scripts/testdata/release-samples`입니다. 1.9.1 빌드(`b389ea2`)의 서명한 APK에서 꺼낸 `AndroidManifest.xml`과 IPA의 `Info.plist`를 바이트 그대로, 읽는 쪽이 보는 것만 남긴 압축 없는 묶음에 넣었습니다. `build/`의 받아 둔 산출물은 정리하면 사라지고 작업 트리에는 없어서, 그것을 읽던 때는 엄격한 prepush가 두 번 실패했습니다(2026-10-26). 매니페스트의 문자열은 UTF-16이라 leak 단계(UTF-8로 찾음)가 보지 못하므로, 표본을 바꿀 때는 문자열을 풀어 leak 목록과 맞춰 봅니다(2026-10-27에 그렇게 넣음). `HJV_RELEASE_SAMPLES=<받아 둔 산출물 폴더>`(예: `build/release-1.9.1`)를 주면 표본 대신 그 폴더의 패키지를 읽습니다. 제어받는 쪽 항목을 거절하는 시험은 합성 입력으로 유지하므로, 구버전 APK와 IPA를 보관할 필요가 없습니다.
- `HJV_PATCHED_FLUTTER=<패치를 적용한 트리의 flutter 폴더>`를 주면 Dart tests 잡의 필수 목록에 있는 파일이 그 트리에 모두 있는지 봅니다. 목록과 패치가 어긋난 것을 GitHub 실행 전에 알 수 있습니다.

아래는 아직 러너에서 돌려 보지 못한 것입니다. 첫 확인 빌드에서 봅니다.

- 세 러너(Windows의 Git Bash, Linux, macOS)의 `python3`로 두 스크립트가 도는지, "Pin the update keys"가 넘긴 값으로 cargo가 빌드하는지
- 빌드 저장소가 부른 실행에서 `release.json`의 `run_id`와 `workflow`가 빌드 저장소의 것으로 나오는지(`GITHUB_RUN_ID`, `GITHUB_WORKFLOW_REF`). 확인 빌드도 "Write release.json and SHA256SUMS" 단계의 로그에 기록을 그대로 찍으므로 거기서 봅니다. `workflow`가 `.github/workflows/build.yml`이 아니면 그 단계가 경고를 남깁니다.
- Windows의 두 연기 시험 전체(PowerShell 단계는 개발 PC에서 실행할 수 없습니다)
- 1.6.0 APK의 합친 매니페스트가 "Android 매니페스트 확인"을 통과하는지(Gradle 빌드는 개발 PC에서 하지 않습니다)
- "Re-run failed jobs"로 다시 돈 잡이 같은 이름의 아티팩트를 다시 올릴 때 GitHub가 받아 주는지

## hjv-relay 빌드

NAS의 hjv-relay는 비공개 빌드 저장소의 hjv-relay 잡에서 만듭니다(설계 3판 8-6). NAS가 부팅 때 root로 시작하는 파일이 에이전트가 도는 개발 PC를 거치지 않게 하려는 것입니다.

1. `HJV_COMMAND_PUBKEY` 시크릿을 넣습니다(위 표). `HJV_SERVER`는 앱과 같은 값을 씁니다.
2. 공개 저장소 main의 커밋으로 `platforms=relay`(또는 `all`)를 실행합니다. 출시 빌드는 그 커밋이 공개 저장소 main에 있어야 합니다(`package-relay.sh`가 확인).
3. 잡이 끝나면 빌드 저장소에 **초안** Release `relay-<판>`이 생깁니다. 묶음(`hjviewer-relay-<판>-linux-x86_64.tar.gz`)과 `SHA256SUMS`가 들어 있습니다. `SHA256SUMS`는 두 줄입니다: 묶음과 그 안의 실행 파일 `hjviewer-relay-<판>-linux-x86_64/hjv-relay`. 같은 이름의 Release가 이미 있으면 새로 만들지 않으므로, 다시 만들려면 옛 Release(초안 포함)를 지웁니다.
4. 실행 요약의 명령 키 지문을 `hjv-api selfcheck`의 지문과 대조한 뒤 Release를 게시하고, 공개 저장소의 그 커밋에 `relay-<판>` 태그를 붙입니다. NAS 설치는 [relay-dsm.md](relay-dsm.md)를 따릅니다. 작업 화면에 적는 SHA-256은 실행 요약이나 Release의 `SHA256SUMS`에서 `…/hjv-relay` 줄의 값을 옮깁니다(relay-dsm.md 3장). 1.6.0은 2026-10-15에 했습니다: 8622975의 태그 `relay-1.6.0`과 Release 'hjv-relay 1.6.0'의 게시(런북 7-7, 7-11).

## push 전 검사 (`scripts/ci.sh leak`)

공개 저장소로 push하기 전에 반드시 돌립니다(설계 3판 11장 1·2단계의 관문). `scripts/ci.sh prepush`의 마지막 단계이기도 합니다. 운영 서버의 도메인, 학교의 공인 주소 대역, NAS·PC의 주소와 MAC, 부서 공유 폴더 이름, 실제 포탈 ID가 올라갈 내용에 들어가지 않았는지 봅니다.

- **목록**: `build/leak-patterns.txt`. `build/`는 git이 무시하므로 저장소에 올라가지 않습니다. 사용자가 직접 만들고, 에이전트는 값을 대화나 다른 파일에 옮기지 않습니다. UTF-8 텍스트(BOM이 있어도 됨)로, 한 줄에 고정 문자열 하나입니다. `#`부터 줄 끝까지는 주석이라 값 뒤에 메모를 달아도 되고(`값  # 메모`), 빈 줄과 값 앞뒤의 공백은 건너뜁니다. 그래서 값에는 `#`를 넣을 수 없습니다. 영어 대소문자는 구분하지 않고, 한글은 NFC·NFD 두 형태로 찾습니다. MAC 주소는 콜론·하이픈·구분자 없음·점 세 칸(`0000.5e00.5377`) 가운데 어느 형태로 적어도 네 형태를 모두 찾습니다. 아무것도 찾지 못하게 되는 목록은 검사를 멈춥니다. Windows PowerShell 5.1의 `>`나 `Out-File`이 만드는 UTF-16 파일, UTF-8이 아닌 파일(EUC-KR 등), 값 안의 제어 문자·보이지 않는 문자·특수 공백(NBSP 등)이 그렇습니다. 메모장의 "UTF-8"과 "UTF-8(BOM)"은 둘 다 됩니다. 주소 대역은 앞부분(첫 두 옥텟과 점)으로 적습니다. 너무 짧은 값(흔한 단어, 두세 글자)은 엉뚱한 곳에서 걸립니다. worktree(`build/wt/…`)에서 돌리면 그 안에 목록이 없을 때 본 작업 트리의 목록을 씁니다. 다른 파일은 `HJV_LEAK_PATTERNS=<파일>`로 줍니다. 목록은 가장 민감한 값을 한데 모은 사본이므로, 여러 에이전트가 일하는 프로젝트 폴더(`grep -r` 한 번이면 값이 읽힘)와 그 백업 대상보다는 저장소 밖(예: `~/.config/hjviewer/leak-patterns.txt`, 모드 600)에 두고 `HJV_LEAK_PATTERNS`로 주기를 권합니다. 포탈 ID는 명단 전체가 아니라 저장소에 들어갈 수 있었던 값만 넣습니다. 에이전트는 이 파일을 열지 않고, 그 폴더를 값의 꼴로 훑지 않습니다. 목록이 없거나 값이 하나도 없거나 git이 추적하는 파일이면 실패합니다.
- **찾는 곳**: git이 추적하는 모든 파일의 작업 트리 내용(`client/patches`, `server/patches` 포함, 바이너리 파일도)과 경로, 스테이징된 내용, 그리고 push하면 올라갈 커밋(`origin/main..HEAD`, `origin/main`이 없으면 HEAD의 모든 커밋)의 메시지, 작성자·커미터, 더하거나 바꾼 파일의 경로, 더한 줄입니다. 나중 커밋에서 지우거나 이름을 바꾼 값도 그 앞 커밋에 남아 있으면 찾습니다.
- **출력**: 걸린 곳(`파일:줄`, `파일 (in the path)`, `staged 파일:줄`, `commit <해시>: message line 3`, `commit <해시> 파일:줄`, `commit <해시>: 파일 (in the path)`, `commit <해시>: author`)과 목록의 줄 번호만 출력합니다. 값과 그 줄의 내용은 출력하지 않습니다. 경로에 값이 들어 있으면 그 부분을 `<listed value>`로 바꿔 출력합니다.
- 걸리면 그 내용을 자리표시자(`rd.example.com`, RFC 5737의 `192.0.2.x`, 문서용 MAC `00:00:5e:00:53:xx`, "부서 공유 폴더", 시험 ID)로 바꾸고, 커밋에 들어갔으면 push 전에 그 커밋을 고칩니다.

## 서명 시크릿 (선택)

Settings → Secrets and variables → Actions의 **Secrets** 탭에 만듭니다. 시크릿이 없으면 서명 없이 빌드합니다. Android는 디버그 서명으로 빌드합니다.

### Android

| 이름 | 내용 |
|---|---|
| `ANDROID_KEYSTORE_BASE64` | 키스토어(.jks)를 base64로 인코딩한 값. 이 값이 있으면 서명합니다. |
| `ANDROID_KEYSTORE_PASSWORD` | 키스토어 비밀번호 |
| `ANDROID_KEY_ALIAS` | 키 별칭 |
| `ANDROID_KEY_PASSWORD` | 키 비밀번호. 비우면 키스토어 비밀번호를 씁니다. PKCS12 키스토어는 두 비밀번호가 같습니다. |

```bash
keytool -genkeypair -v -keystore hjviewer-release.jks -storetype PKCS12 \
  -alias hjviewer -keyalg RSA -keysize 4096 -validity 10000 -dname "CN=HJViewer, C=KR"
base64 -w0 hjviewer-release.jks | gh secret set ANDROID_KEYSTORE_BASE64 -R yonsei-projecttf/hjviewer-private   # macOS: base64 -i hjviewer-release.jks | tr -d '\n' | …
gh secret set ANDROID_KEYSTORE_PASSWORD -R yonsei-projecttf/hjviewer-private   # 프롬프트에 입력
gh secret set ANDROID_KEY_ALIAS -R yonsei-projecttf/hjviewer-private --body hjviewer
```

- **키스토어 백업**: 키스토어를 잃어버리면 이미 설치된 앱을 업데이트할 수 없습니다. 비밀번호와 함께 안전한 곳에 백업하세요.
- **Google Play 업로드 키**: Play에 올리는 AAB도 이 키스토어로 서명합니다. 그래서 첫 AAB를 올리면 이 키가 Play의 업로드 키가 됩니다. 업로드 키는 잃어도 Play Console에서 재설정을 요청할 수 있지만(아래 "오류와 대처"의 "다른 키로 서명"), 사이드로드한 APK를 업데이트하려면 이 키가 꼭 있어야 합니다.
- **디버그 서명 APK**: 러너가 매번 새 디버그 키로 서명합니다. 그래서 이전 빌드 위에 덮어 설치할 수 없고, 앱을 지운 뒤 다시 설치해야 합니다.

### iOS

Apple Developer Program(유료) 계정이 필요합니다. `.p12`와 프로파일이 둘 다 있으면 서명합니다.

| 이름 | 내용 |
|---|---|
| `IOS_DIST_CERT_P12_BASE64` | Apple Distribution 인증서와 개인키를 함께 내보낸 .p12의 base64 |
| `IOS_DIST_CERT_PASSWORD` | .p12를 내보낼 때 정한 비밀번호 |
| `IOS_PROVISIONING_PROFILE_BASE64` | 배포용 프로비저닝 프로파일(.mobileprovision)의 base64 |
| `IOS_TEAM_ID` | 10자리 팀 ID(developer.apple.com → Membership details). 넣으면 프로파일의 팀과 같은지 확인하고, 비우면 프로파일의 팀을 씁니다. |
| `ASC_KEY_ID`, `ASC_ISSUER_ID` | TestFlight 업로드용 App Store Connect 팀 API 키의 Key ID와 Issuer ID (Users and Access → Integrations) |
| `ASC_PRIVATE_KEY` | 그 키의 `.p8` 파일 내용 (`gh secret set ASC_PRIVATE_KEY -R yonsei-projecttf/hjviewer-private < AuthKey_XXXX.p8`) |
| `ASC_APP_ID` | App Store Connect 앱 레코드의 숫자 Apple ID (앱 → 앱 정보 → Apple ID). Xcode 26의 `altool --upload-package`에 필요합니다. |

준비 순서는 다음과 같습니다.

1. **App ID**: Certificates, Identifiers & Profiles → Identifiers에서 explicit App ID를 만듭니다. 패치의 번들 ID는 `kr.ac.yonsei.hjviewer`입니다. 다른 ID를 써도 워크플로는 프로파일의 번들 ID를 따릅니다. 와일드카드 App ID는 쓸 수 없습니다.
   - 앱의 entitlements는 Push Notifications와 Access Wi-Fi Information을 요청하므로 두 기능을 켜 두기를 권장합니다.
   - 켜지 않아도 서명은 됩니다. 워크플로가 경고를 남기고 해당 항목을 entitlements에서 뺍니다. 앱 코드는 두 기능을 쓰지 않습니다.
2. **인증서**: 다음 중 한 가지로 발급합니다.
   - Xcode → Settings → Accounts → Manage Certificates → + → Apple Distribution
   - 키체인 접근에서 CSR을 만들어 웹에서 발급

   발급한 뒤 키체인 접근에서 인증서를 펼쳐 개인키가 함께 선택된 상태로 .p12로 내보냅니다. OpenSSL 3로 .p12를 만들면 `-legacy` 옵션을 붙여야 macOS가 읽을 수 있습니다.
3. **프로파일**: Profiles → +에서 종류를 고릅니다. 내보내기 방식(ad-hoc / app-store / enterprise / development)은 워크플로가 프로파일을 보고 정합니다.
   - **Ad Hoc**: 등록한 기기에만 설치됩니다. 먼저 Devices에 아이폰 UDID를 등록하세요.
   - **App Store Connect**: TestFlight와 App Store용입니다.
4. **시크릿 등록**:

```bash
base64 -i dist.p12 | tr -d '\n' | gh secret set IOS_DIST_CERT_P12_BASE64 -R yonsei-projecttf/hjviewer-private
gh secret set IOS_DIST_CERT_PASSWORD -R yonsei-projecttf/hjviewer-private
base64 -i HJViewer_AdHoc.mobileprovision | tr -d '\n' | gh secret set IOS_PROVISIONING_PROFILE_BASE64 -R yonsei-projecttf/hjviewer-private
gh secret set IOS_TEAM_ID -R yonsei-projecttf/hjviewer-private --body ABCDE12345
```

워크플로가 서명하는 과정은 다음과 같습니다.

1. 인증서를 임시 키체인에 넣습니다.
2. Runner 타깃만 수동 서명으로 바꿉니다. 팀, 프로파일, 번들 ID는 프로파일 기준입니다.
3. ExportOptions.plist를 만들어 `flutter build ipa --export-options-plist`로 IPA를 만듭니다.

결과물별 설치 방법은 다음과 같습니다.

- **Ad Hoc IPA**: 기기를 Mac에 연결하고 Finder나 Apple Configurator로 IPA를 끌어다 놓습니다.
- **App Store 프로파일로 만든 IPA**: `testflight`를 켜고 실행하면 워크플로가 App Store Connect에 올립니다(Transporter 앱으로 직접 올려도 됩니다). Apple의 처리가 끝나면 TestFlight에 나타납니다. 내부 테스트 그룹에 넣은 App Store Connect 사용자가 TestFlight 앱으로 설치합니다. TestFlight 빌드는 90일 뒤 만료됩니다.
  - `altool`은 업로드가 실패해도 0으로 끝날 때가 있어서, 워크플로는 출력에서 성공 메시지를 확인합니다.
  - 2026-10-10 첫 업로드(실행 38044728062, 빌드 262831026)는 Apple 처리 후 VALID가 되었고, 내부 테스트 그룹에 자동으로 배포되었습니다. 업로드 경고는 90068 하나였습니다(2027년 4월부터 최소 iOS 15).
  - Info.plist는 upstream처럼 `ITSAppUsesNonExemptEncryption=false`입니다. 앱의 암호화(libsodium, TLS)는 표준 알고리즘이라, Apple 기준으로 문서가 필요한 경우는 프랑스 App Store 판매뿐입니다. App Store에 낼 때 프랑스를 판매 지역에서 빼지 않으려면 프랑스 암호화 신고를 하고 이 값을 바꿔야 합니다.
- **서명 없는 IPA(`-ios-unsigned`)**: 그대로는 설치할 수 없습니다. Sideloadly나 AltStore 같은 도구로 재서명해야 합니다. 무료 Apple ID로 서명하면 7일 뒤 만료됩니다.

## 스토어 실행 (`store-release.yml`, 1.9.1부터)

App Store와 Google Play에 닿는 일(Play 업로드, App Store 버전 준비·심사 제출·출시, 상태 확인)은 빌드 실행이 아니라 **스토어 실행**이 합니다. 비공개 `store.yml`이 이 저장소의 `.github/workflows/store-release.yml`을 고정 SHA로 부르고, 잡은 Linux에서 API만 부릅니다. 스토어 키와 심사 정보는 environment `store`에 있고 Google Play·App Store 잡만 받습니다. 출시 기록 `store-releases.txt`(비공개 저장소)에 스토어에 나간 것을 적고, 빌드 실행의 Android·iOS 잡이 첫 단계에서 그 기록을 읽습니다. 입력, 순서, 실패와 대처, 한 번 할 일은 [store-release.md](store-release.md)에 있습니다.

## Google Play 업로드

Android 앱은 Google Play의 **비공개 테스트 트랙 Alpha**(API 이름 `alpha`)로 나갑니다(소유자 결정, 2026-10-26). 1.9.0은 소유자가 Play Console에서 손으로 올렸고, 1.9.1부터 스토어 실행([store-release.md](store-release.md))이 올립니다. 테스터는 참여 링크로 한 번 참여한 뒤 Play 스토어에서 설치하고, 그 뒤 업데이트는 Play가 합니다. 비공개 테스트의 출시는 Google 심사(몇 시간–7일)를 거칩니다. 내부 테스트 트랙과 승격은 쓰지 않습니다. 1.6.x 때 내부 테스트 트랙을 쓰던 기록은 아래 표에 남아 있습니다. Play 정책과 정할 것은 [distribution.md](distribution.md)의 "Android: Google Play 비공개 테스트(Alpha)"에 있습니다.

빌드 실행(1–2)과 스토어 실행(3)이 하는 일은 다음과 같습니다.

1. **AAB 빌드**: 비공개 저장소의 서명 빌드는 `play`와 상관없이, APK 아티팩트를 올린 다음 arm64 AAB를 만듭니다. 첫 업로드는 Play Console에서 직접 해야 하므로, 그때 쓸 AAB가 늘 있습니다. 아티팩트 이름은 `HJViewer-<이름>-android-aarch64-play`입니다.
   - 번들을 빌드하기 직전에 Play 전용 매니페스트(`client/play/AndroidManifest.xml`)를 넣습니다. APK는 이미 빌드된 뒤라 이 파일은 번들에만 적용됩니다(아래 "Play 번들에서 빼는 것").
   - APK 서명 액션이 작업 폴더에 남긴 키스토어 사본은 번들 빌드(Gradle, 플러그인의 빌드 스크립트)가 시작되기 전에 지웁니다.
   - **번들 단계가 실패하면**: APK 아티팩트는 번들 단계보다 먼저 올라가므로 어느 경우든 그대로 쓸 수 있습니다. 확인 빌드(`check_only`)에서는 Android 잡이 실패합니다(`play`를 켠 실행도 그렇게 되어 있지만, 1.9.1부터 `play=true`는 빌드 전에 거부됩니다). 그 밖의 실행에서는 Android 잡이 성공으로 끝나고, 실행 화면에 경고 "This run has no Play bundle"과 요약 한 줄이 남으며, `…-android-aarch64-play` 아티팩트가 없습니다. Play에 아무것도 올리지 않는 실행이 번들 때문에 실패로 보이지 않게 하려는 것입니다. 이 동작은 번들 단계들의 `if`·`continue-on-error` 줄과 단계 순서로 정해집니다. 그 효과는 GitHub에서 실패한 실행으로만 볼 수 있으므로, 오프라인 시험(`scripts/test_play_upload.py`의 `PlayBundleListsTest`)이 그 줄들과 순서를 지킵니다. 바꿀 때는 시험의 `BUNDLE_STEPS`와 이 문단을 같은 커밋에서 고칩니다.
2. **서명과 검사**: 릴리스 키스토어로 jarsigner 서명을 한 번 합니다. 비밀번호는 환경 변수로만 넘깁니다. 서명을 확인한 뒤에는 키스토어 파일을 지우고 비밀번호를 환경에서 없앤 다음에 bundletool을 내려받아 돌립니다. 이어서 다음을 확인하고, 하나라도 틀리면 이 단계가 실패합니다(잡이 실패하는지는 1번과 같음). 검사에 실패한 번들은 아티팩트로 올리지 않습니다.
   - 서명자가 하나이고, 그 인증서가 키스토어의 것인지
   - `bundletool validate`, 패키지 이름 `kr.ac.yonsei.hjviewer`, versionCode(같은 실행의 APK와 같은지), versionName(`HJV_VERSION`), minSdk 24 이상(Play 자동 보호의 요건), targetSdk 36 이상, 디버그 빌드가 아닌지
   - 네이티브 라이브러리가 arm64-v8a뿐이고 16KB 페이지에 맞게 정렬되었는지
   - Rust 코어 `librustdesk.so`와 `libc++_shared.so`, Flutter의 `libflutter.so`, `libapp.so`가 들어 있는지. Rust 코어가 빠진 번들은 어느 기기에서도 시작되지 않는데, Flutter 라이브러리만으로도 위 검사는 통과하기 때문입니다.
   - Play 전용 매니페스트가 병합되었는지, 사이드로드 전용 항목(`PLAY_FORBIDDEN`)이 없는지(아래 "Play 번들에서 빼는 것")
   - Play 선언 대상 목록(`PLAY_MANIFEST`, 아래)이 그대로인지

   실행 요약에 versionCode, 크기, 업로드 인증서 SHA-256이 나옵니다.
3. **업로드(스토어 실행)**: 스토어 실행의 Google Play 잡이 확인한 빌드 실행(`run_id`)의 AAB를 받아 `scripts/play-upload.py`로 Play Developer API에 올립니다. 빌드 실행에는 Play에 올리는 잡이 없습니다(1.9.1에 지움).
   - 편집을 열기 전에 `applications.tracks.releases.list`(편집 없음)로 alpha와 internal의 출시 상태를 봅니다. 앞 변경이 심사 중(`IN_REVIEW`)이면 `in_review=cancel`일 때만 계속하고, 거절(`NOT_APPROVED`)이나 보내지 않은 변경(`NOT_SENT_FOR_REVIEW`)이 있으면 멈추고 Play Console에서 정리하라고 알립니다. 이 versionCode가 이미 심사 중이거나 게시됐으면 아무것도 하지 않고 성공합니다.
   - 편집 하나를 열어 번들을 올리고, alpha의 출시(completed)를 설정한 뒤 커밋합니다. 커밋은 늘 `changesInReviewBehavior`를 적습니다(`ERROR_IF_IN_REVIEW`, cancel이면 `CANCEL_IN_REVIEW_AND_SUBMIT`). 적지 않으면 Google의 기본값이 진행 중인 심사를 취소하기 때문입니다.
   - 어디서 실패하든 편집을 지우므로, 반쯤 올라간 상태가 남지 않습니다.
   - Play가 이미 가진 같은 번들(같은 sha256)은 다시 올리지 않습니다. 이미 출시된 같은 번들이면 아무것도 바꾸지 않고 성공합니다.
   - 트랙이 이미 내보내는 versionCode보다 낮은 번들은 올리지 않고 실패합니다. 비교 대상은 그 트랙의 출시 가운데 초안이 아닌 것입니다.
   - 업로드는 한 번에 하나만 돌고(`concurrency`), 스토어 실행의 Checks 잡이 다른 스토어 실행이 진행 중이거나 기다리면 시작하지 않습니다. GitHub는 그룹마다 기다리는 잡을 하나만 두기 때문입니다.
   - 출시 노트(ko-KR)는 `store/play/ko-KR/release_notes.txt`에 `HJViewer <버전> (<versionCode>)`와 이 빌드의 소스 커밋(태그 `client-<버전>`) 링크를 더한 것이고, 500자 안이어야 합니다.
   - 서비스 계정 키는 environment `store`에 있고, 스토어 실행의 Google Play 잡만 받습니다. 서드파티 빌드 코드(Gradle, cargo, pub)가 도는 빌드 실행에는 들어가지 않습니다. 스크립트는 키도 토큰도 출력하지 않습니다.

### 시크릿

빌드 저장소의 **environment `store`**(배포 브랜치 main)에 넣습니다([store-release.md](store-release.md)의 "한 번 할 일"). 서비스 계정 JSON 키에서 두 값을 꺼내 넣습니다. 저장소 시크릿에 넣어 두었다면 environment로 옮기고 저장소 시크릿은 지웁니다.

| 이름 | 내용 |
|---|---|
| `PLAY_SA_EMAIL` | JSON 키의 `client_email` (`…@<프로젝트>.iam.gserviceaccount.com`) |
| `PLAY_SA_PRIVATE_KEY` | JSON 키의 `private_key` (여러 줄 PEM) |

- JSON 파일 전체를 시크릿 하나로 넣지 않습니다. GitHub는 로그에서 시크릿과 정확히 같은 문자열을 찾아 가리므로, 구조화된 값은 값마다 시크릿을 만들라고 안내합니다. 여러 줄 시크릿은 줄마다 가려집니다.
- 업로드 키는 Android 키스토어(`ANDROID_KEYSTORE_*`)이므로, 이 둘 말고 새 시크릿은 없습니다.

### 처음 한 번 할 일

Play 개발자 계정은 이미 있습니다. 계정에 남은 확인 요청(신원, 연락처, Android 기기 인증)이 있으면 먼저 끝냅니다.

아래 표는 1.6.x 때 내부 테스트 트랙으로 처음 설정한 기록입니다. 1.9.1부터는 비공개 테스트 트랙 alpha를 쓰고(1.9.0을 소유자가 손으로 올림), 서비스 계정의 시크릿은 environment `store`에 넣습니다. 지금 할 일은 [store-release.md](store-release.md)의 "한 번 할 일"(Play Console P1–P6)입니다. 표의 단계는 모두 지금 할 수 있습니다. 4–5단계에 필요한 워크플로는 공개 저장소에 있고, 비공개 `build.yml`이 그 커밋을 가리킵니다(1.6.0과 1.6.1의 출시 실행이 번들을 만들었습니다). 2·5단계에 필요한 개인정보처리방침의 카메라·마이크 절도 privacy.md 6절 "카메라와 마이크"로 main에 있습니다(표 아래). 서비스 계정 권한은 적용에 시간이 걸릴 수 있으니 6–8단계를 일찍 하는 편이 좋습니다.

| # | 어디서 | 할 일 |
|---|---|---|
| 1 | Play Console → 앱 만들기 | 이름 `HJViewer`, 기본 언어 **한국어(ko-KR)**, 앱, 무료, 선언 동의. 출시 노트가 ko-KR이므로 기본 언어를 한국어로 둡니다. 패키지 이름은 첫 업로드로 `kr.ac.yonsei.hjviewer`로 정해지고, 바꿀 수 없습니다. |
| 2 | 앱 콘텐츠 | 개인정보처리방침 URL `https://github.com/yonsei-projecttf/hjviewer/blob/main/docs/privacy.md`, 광고 없음, 광고 ID 사용 안 함. 방침에는 카메라·마이크 절이 있어야 하고, 이미 있습니다(표 아래). 스토어 등록정보, 콘텐츠 등급, 타깃층, 데이터 보안은 **하지 않습니다.** 내부 테스트는 앱 설정을 마치기 전에 시작할 수 있고, 설정을 다 마친 앱은 첫 내부 출시도 심사를 받기 때문입니다. 1.6.0부터 모든 Android 빌드가 제어 전용이라 접근성 API 선언과 포그라운드 서비스 선언은 필요 없습니다([distribution.md](distribution.md)의 "Android: Google Play 비공개 테스트(Alpha)"). 두 선언이 필요한 1.5.1 이하의 번들은 Play에 올리지 않습니다. |
| 3 | 테스트 및 출시 → 내부 테스트 → 테스터 | 이메일 목록을 만들어 테스터의 Google 계정을 넣습니다(최대 100명). [런북](server-runbook.md) 7-9의 관문이 끝나기 전에는 운영자의 계정만 넣고, 다른 테스터는 관문 뒤에 더합니다. 휴대폰 Play 스토어에 로그인한 계정이어야 합니다. 피드백 이메일을 적고 저장합니다. 참여 링크는 첫 출시를 게시한 뒤에 복사합니다(10단계). |
| 4 | 터미널 | 첫 AAB를 만들어 받습니다. `gh workflow run build.yml -R yonsei-projecttf/hjviewer-private -f platforms=android`가 끝나면 `gh run download <실행 ID> -R yonsei-projecttf/hjviewer-private -n HJViewer-<버전>-<커밋 7자리>-android-aarch64-play`. 실행 요약에서 versionCode와 업로드 인증서 SHA-256을 봅니다. 최소 SDK 24 패치(0047)가 들어간 1.6.1 이후의 커밋으로 빌드합니다. 출시 빌드가 있으면 따로 빌드하지 않고 그 실행의 `-play` 아티팩트를 받습니다. 확인 빌드(`check_only`)의 `-check` AAB(`…-check-android-aarch64-play`)는 올리지 않습니다. 출시에 필요한 공개키 없이도 만들어지는 빌드이기 때문입니다. 실행 화면에 경고 "This run has no Play bundle"이 있으면 번들이 만들어지지 않은 것입니다. Android 잡의 번들 단계 로그에서 까닭을 봅니다. |
| 5 | 내부 테스트 → 새 버전 만들기 | 먼저, 이 트랙에 게시하지 않은 버전(임시 버전)이 남아 있으면 "임시 버전 삭제"로 지웁니다. 1.6.0 번들로 만들다 오류로 멈춘 버전이 남아 있을 수 있습니다. 그런 버전이 있는 동안에는 새 버전을 만들 수 없습니다. ① 새 앱은 첫 번들을 올릴 때 Google이 만든 키로 Play 앱 서명에 저절로 등록됩니다. "앱 서명 키 변경"은 누르지 않습니다. ② 4의 AAB를 올립니다. ③ 출시 노트를 적습니다. ④ 다음을 눌러 오류 요약을 봅니다. 오류는 해결해야 하고(선언 등), 경고는 출시를 막지 않습니다. Play의 자동 보호(Automatic protection)는 최소 SDK 24를 요구합니다. 최소 SDK 22였던 1.6.0 번들은 이 오류로 막혔고, 1.6.1부터 최소 SDK가 24입니다. ⑤ 다음을 눌러 나오는 "미리보기 및 확인" 화면에서 "출시 시작"(또는 "게시")을 누릅니다. "임시보관함에 저장"에서 멈추지 않습니다. "저장"만 있으면 그 변경은 Google의 검토를 받아야 하는 것이므로 거기서 멈춥니다. ⑥ Google Play로 보호됨 → Play 스토어 보호 → Play 앱 서명 관리 → 업로드 키 인증서의 SHA-256 인증서 지문이 실행 요약의 업로드 인증서 SHA-256과 같은지 봅니다. 옛 "앱 무결성" 페이지는 "Google Play로 보호됨"으로 바뀌었습니다. 실행 요약은 콜론 없는 소문자 16진수로, Play는 콜론으로 나눈 대문자 16진수로 보여 줍니다. ⑦ 첫 출시 뒤, 이 패키지가 Android 개발자 인증에 등록됐는지 봅니다. Play Console 홈의 알림(신원 확인을 마친 계정에만 나옴)이나 "Android 개발자 인증" 페이지에서 봅니다. Google이 등록하지 않았으면 멈추고, 직접 등록하는 방법을 메인 세션과 정합니다. 같은 "Android 개발자 인증" 페이지에서 사이드로드 APK의 서명 키도 이 패키지에 등록돼 있는지 봅니다. 그 키는 키스토어, 곧 업로드 키입니다. SHA-256은 실행 요약의 업로드 인증서 SHA-256입니다. 없으면 2027년의 전 세계 시행 전에 더하는 방법을 메인 세션과 정합니다. API로는 첫 업로드를 할 수 없어서 이 단계는 직접 합니다. |
| 6 | Google Cloud 콘솔 | 새 프로젝트를 만듭니다. 조직이 없는 개인 Google 계정의 프로젝트를 권장합니다. API 및 서비스 → 라이브러리에서 **Google Play Android Developer API**를 사용 설정합니다. IAM 및 관리자 → 서비스 계정 → 만들기(예: `play-upload`, Cloud 역할 없음) → 키 → 키 추가 → 새 키 만들기 → **JSON**으로 `key.json`을 내려받습니다. Play 계정과 Cloud 프로젝트를 연결할 필요는 없습니다. |
| 7 | Play Console → 사용자 및 권한 | 새 사용자 초대 → 서비스 계정 이메일. 계정 권한은 모두 끕니다. 앱 권한 → HJViewer → **"앱을 테스트 트랙으로 출시"(Release apps to testing tracks)만** 켜고 초대합니다. 프로덕션에는 출시할 수 없는 권한입니다. 다만 이 권한으로 내부 앱 공유에도 올릴 수 있습니다. 내부 앱 공유의 파일은 어느 키로 서명해도 되고, 그 링크는 기본으로 누구나 내려받을 수 있습니다. 그래서 테스트 및 출시 → 설정 → 내부 앱 공유에서 내려받을 수 있는 사람을 이메일 목록으로 제한하고, JSON 키는 업로드 키처럼 지킵니다. |
| 8 | 키 파일이 있는 PC의 터미널 | 아래 명령으로 시크릿 2개를 넣고 `key.json`을 지웁니다. |
| 9 | 터미널 | 첫 CI 업로드는 검증만 합니다. 1.9.1부터는 스토어 실행의 `dry_run=true`입니다: `gh workflow run store.yml -R yonsei-projecttf/hjviewer-private -f platform=android -f action=upload -f version=<버전> -f build=<versionCode> -f run_id=<빌드 실행 ID> -f dry_run=true`. (1.9.0까지는 `build.yml -f play=true -f play_release=validate`였습니다.) 실행 요약(터미널에서는 표 아래의 명령)이 "validated … nothing was released"이면 됩니다. 5단계에서 손으로 올린 버전에는 이 검증 실행만 합니다. 같은 버전을 `completed`로 다시 올리면 테스터가 같은 버전을 두 번 받기 때문입니다(위 "앱 버전"). 첫 CI 업로드는 1.9.1의 스토어 실행 upload입니다(1.9.0은 소유자가 alpha에 손으로 올림). |
| 10 | 내부 테스트 → 테스터 | 첫 출시가 게시된 뒤에 참여 링크를 복사해 테스터에게 보냅니다. 런북 7-9의 관문 전에는 운영자만 씁니다. 다른 테스터에게는 관문 뒤에 보냅니다. Google의 한국어 도움말은 "공유 가능한 링크 복사"라고 하고, 콘솔에는 "웹에서 참여"로도 나옵니다. 첫 출시 뒤 링크가 동작하기까지 몇 시간 걸릴 수 있습니다. 테스터는 링크에서 참여한 뒤 Play 스토어에서 설치합니다. 앱을 처음 올린 뒤 최대 48시간 동안은 테스터에게 임시 앱 이름과 스토어 등록정보가 보일 수 있습니다. 사이드로드한 HJViewer가 있으면 먼저 지웁니다. 서명이 달라서 Play 앱이 그 위에 설치되지 않기 때문입니다. 이때 그 휴대폰의 로컬 설정, 기기 ID, 최근 접속 목록이 지워집니다. |

**개인정보처리방침의 카메라·마이크 절**: 번들은 `RECORD_AUDIO` 권한을 요청합니다(1.9.0까지는 `CAMERA`도). 이런 번들은 방침 URL이 없으면 내부 트랙에서도 거절된다는 보고가 있고, 방침은 그 권한을 어디에 쓰는지 말해야 합니다. 그래서 2단계(URL 등록)와 5단계(첫 업로드)에는 [privacy.md](privacy.md)에 이 절이 있어야 합니다. 이 절은 공개 저장소 main의 privacy.md 6절 "카메라와 마이크"입니다. 절에는 다음 내용이 있어야 하고, 6절에 둘 다 있습니다.

- 마이크는 세션 중 직접 시작한 음성 통화에만 씁니다. 소리는 상대 기기로 종단 간 암호화되어 가고 저장하지 않습니다.
- 카메라는 쓰지 않습니다. 1.9.1부터는 권한도 없습니다(1.9.0까지는 QR 스캐너 플러그인이 더했고, 서버를 정하는 QR 읽기는 고정 서버 빌드에서 보이지 않았음, U38).

8단계 명령(키를 채팅이나 저장소, `build/`에 두지 않습니다):

```bash
# key.json이 있는 폴더에서. jq와 gh가 필요합니다(gh auth status로 로그인을 확인).
jq -r .client_email key.json | gh secret set PLAY_SA_EMAIL --env store -R yonsei-projecttf/hjviewer-private
jq -r .private_key  key.json | gh secret set PLAY_SA_PRIVATE_KEY --env store -R yonsei-projecttf/hjviewer-private
rm key.json
gh secret list --env store -R yonsei-projecttf/hjviewer-private   # 이름과 날짜만 보입니다
```

- 값은 파이프로 `gh`에 들어가므로 셸 기록이나 명령줄에 남지 않습니다.
- 2024-05-03 이후 만든 Google Cloud 조직은 서비스 계정 키 생성을 기본으로 막습니다. 6단계에서 키를 만들 수 없으면 Workload Identity Federation(키 없는 인증)으로 바꿔야 하고, 그때는 워크플로도 고칩니다.
- 권한 적용에 24–48시간 걸렸다는 보고가 있습니다. 설정 직후 `does not have permission`이나 `insufficient permissions`라는 메시지로 실패하면 기다렸다가 다시 실행합니다. 기다리는 것은 이 메시지일 때만입니다. 다른 메시지의 403은 기다려도 풀리지 않으니 아래 "오류와 대처"를 봅니다. 트랙이나 번들 목록을 읽다가 같은 메시지가 계속 나면 같은 앱 권한에 "앱 정보 보기(읽기 전용)"를 더합니다.

9단계의 결과를 터미널에서 보는 명령입니다(스토어 실행의 잡 이름은 `Google Play`로 끝남). `gh run view … --log`는 불려 온 워크플로의 잡에 대해 gh 2.45에서 아무것도 출력하지 않으므로, 잡 번호로 로그를 받습니다.

```bash
J=$(gh run view <실행 ID> -R yonsei-projecttf/hjviewer-private --json jobs --jq '.jobs[] | select(.name | endswith("Google Play")) | .databaseId')
gh api repos/yonsei-projecttf/hjviewer-private/actions/jobs/$J/logs | grep -E 'validated|Uploaded|already|##\[error\]'
```

### Play 선언 대상 (`PLAY_MANIFEST`)

Play Console의 권한·포그라운드 서비스·접근성 선언 안내는 손으로 올릴 때만 나오고, CI 업로드에서는 나오지 않는다는 보고가 있습니다. 선언이 빠진 번들은 커밋이 막힐 수 있습니다(포그라운드 서비스 선언이 없어 내부 테스트 트랙 커밋이 403으로 실패한 보고가 있음). 그래서 "Sign and check Play bundle" 단계는 번들의 다음 항목을 고정 목록 `PLAY_MANIFEST`와 비교합니다.

- 요청하는 권한 전체(`uses-permission`, `uses-permission-sdk-23`)
- 포그라운드 서비스 유형
- 접근성 서비스

다르면 diff(`>` = 새로 생김, `<` = 없어짐)를 보여 주고 단계가 실패합니다. 패치나 플러그인 업데이트로 항목이 바뀌면, Play Console → 앱 콘텐츠에서 해당 선언을 먼저 확인하거나 고친 뒤 같은 커밋에서 목록을 고칩니다.

지금 목록은 제어 전용 빌드의 것입니다. 1.6.0부터 모든 Android 빌드가 제어 전용입니다. 권한 8개뿐이고(1.9.0의 앱 잠금이 둘을 더하고, 1.9.1에서 `CAMERA`가 빠짐), 포그라운드 서비스 유형과 접근성 서비스는 없습니다. 하나라도 나타나면 실패합니다.

| 권한 | 쓰임 |
|---|---|
| `INTERNET`, `ACCESS_NETWORK_STATE` | 서버와 상대 기기에 연결 |
| `RECORD_AUDIO` | 세션 중 음성 통화 |
| `POST_NOTIFICATIONS`, `WAKE_LOCK` | 앱의 매니페스트에 남아 있는 것 |
| `USE_BIOMETRIC`, `USE_FINGERPRINT` | 휴대폰의 앱 잠금(1.9.0, W13). `local_auth_android` 1.0.47이 `USE_BIOMETRIC`을, 그 의존성 `androidx.biometric` 1.1.0이 둘 다 더합니다. 일반 권한이라 Play Console의 신고 양식이 없습니다([distribution.md](distribution.md)) |
| `kr.ac.yonsei.hjviewer.DYNAMIC_RECEIVER_NOT_EXPORTED_PERMISSION` | AndroidX가 앱마다 더하는 내부 권한 |

- 이 목록은 1.5.1 번들에서 잰 권한 14개에서 제어 전용 패치가 빼는 7개(`FOREGROUND_SERVICE` 4종, `SYSTEM_ALERT_WINDOW`, `REQUEST_IGNORE_BATTERY_OPTIMIZATIONS`, `RECEIVE_BOOT_COMPLETED`)를 빼고, 1.9.0의 앱 잠금이 더하는 2개를 더한 뒤, 1.9.1에서 QR 스캐너 플러그인과 함께 빠진 `CAMERA`를 뺀 값입니다. 1.6.0과 1.6.1의 출시 실행(38559428611, 38568720786)이 러너에서 만든 번들도 그때의 목록과 같았습니다(diff 없음). 플러그인이 같은 권한을 따로 요청하게 되면 diff로 드러납니다. 그때 diff를 보고 패치나 목록을 맞춥니다.
- `MANAGE_EXTERNAL_STORAGE`와 저장소 읽기·쓰기 권한은 upstream이 빼므로 없습니다.
- 사이드로드 전용 권한은 이 목록에 넣지 않습니다(다음 절).

### Play 번들에서 빼는 것 (`client/play/AndroidManifest.xml`)

사이드로드 APK에만 있어야 하는 권한과 구성요소는 Play 번들에서 뺍니다. 지금은 앱 안 업데이트가 쓰는 둘입니다. 앱 안 업데이트는 새 APK를 받아 설치하는데, Play에서 받은 앱은 Play 말고 다른 방법으로 자신을 업데이트하면 안 됩니다.

- 권한 `android.permission.REQUEST_INSTALL_PACKAGES`
- 제공자 `com.carriez.flutter_hbb.HjvUpdateFileProvider`

**빼는 방법**: "Build Play bundle" 단계가 `client/play/AndroidManifest.xml`을 `flutter/android/app/src/release/AndroidManifest.xml`로 복사한 뒤에 번들을 빌드합니다. 빌드 유형(release)의 매니페스트는 앱의 매니페스트와 플러그인의 매니페스트보다 우선합니다. 그래서 이 파일이 `tools:node="remove"`로 적은 항목은 병합된 매니페스트에서 빠집니다. APK는 그 전에 빌드되므로 그대로입니다.

**확인하는 방법**: "Sign and check Play bundle" 단계가 번들의 매니페스트에서 두 가지를 봅니다.

- 표지 `<meta-data android:name="kr.ac.yonsei.hjviewer.PLAY_OVERLAY" android:value="play">`가 있는지. 이 표지는 Play 전용 매니페스트에만 있습니다. 없으면 그 파일 없이 빌드된 번들입니다.
- 고정 목록 `PLAY_FORBIDDEN`(위 두 항목)이 번들에 없는지. 권한은 `uses-permission`과 `uses-permission-sdk-23`을 모두 보고, 제공자는 번들의 모든 `provider`를 봅니다.

**고칠 때의 규칙**

- 사이드로드 전용 권한이나 구성요소가 새로 생기면 `client/play/AndroidManifest.xml`과 `PLAY_FORBIDDEN`에 함께 넣습니다. **`PLAY_MANIFEST`에는 넣지 않습니다.** 거기에 넣으면 검사는 통과하지만 그 권한이 Play로 나갑니다.
- 같은 내용을 두 곳에 적는 까닭이 있습니다. 매니페스트 파일에서 이름을 잘못 적으면 그 항목은 빠지지 않는데, 그때 `PLAY_FORBIDDEN` 검사가 잡습니다.
- 두 곳이 서로 맞는지는 빌드 전에도 확인됩니다. `scripts/ci.sh client`의 오프라인 시험(`scripts/test_play_upload.py`의 `PlayBundleListsTest`)이 다음을 봅니다. 매니페스트 파일이 빼는 항목과 `PLAY_FORBIDDEN`이 같은지, 그 항목이 `PLAY_MANIFEST`에 없는지, `PLAY_MANIFEST`에 피제어 쪽 항목이 없는지, 표지의 이름과 값이 검사 단계의 것과 같은지, 파일을 복사하는 자리가 APK 빌드와 번들 빌드 사이인지. 앱 안 업데이트 패치가 `client/patches`에 들어온 뒤에는, 그 패치가 앱의 매니페스트에 더하는 설치 권한과 제공자(`hjv_update_paths`를 쓰는 것)가 `PLAY_FORBIDDEN`에 있는지도 봅니다. 그 전에는 이 시험만 건너뜁니다.
- 클래스 이름은 전체 이름으로 적습니다(앱의 클래스는 `com.carriez.flutter_hbb`에 있음). 이 파일에는 짧은 이름을 채워 줄 `package` 속성이 없습니다.
- 위 두 이름은 앱 안 업데이트 패치가 쓰기로 한 이름입니다. 그 패치가 다른 이름을 쓰면 위 시험이 실패합니다. 그때는 두 곳을 같은 커밋에서 고칩니다.
- upstream에 `src/release/AndroidManifest.xml`이 생기면 "Build Play bundle" 단계가 실패합니다. 그때는 두 파일을 합치도록 단계를 고칩니다.
- 실제 러너의 Gradle이 이 파일을 병합하는 것은 1.6.0과 1.6.1의 출시 실행(38559428611, 38568720786)에서 표지 검사로 확인했습니다. 그 전에 개발 PC에서는 Android SDK의 매니페스트 병합 도구로 병합 결과를, 1.5.1 APK로 만든 번들로 검사 단계를 확인했습니다.

### 오류와 대처

스크립트는 Google의 메시지 다음 줄에 대처를 알려 줍니다. 업로드 중에 실패하면 편집을 지우므로 커밋된 것은 없습니다. 커밋의 응답을 받지 못한 경우만 예외입니다(아래 `did not answer the commit properly`).

| 메시지 | 대처 |
|---|---|
| `PLAY_SA_EMAIL and PLAY_SA_PRIVATE_KEY are not in the environment store` | environment `store`에 시크릿을 넣고 `store.yml`이 두 이름을 넘기는지 본 뒤, 같은 upload를 다시 dispatch하거나 "Re-run failed jobs"를 누릅니다. 같은 AAB(30일 보관)로 다시 돕니다. |
| `… IN_REVIEW: Google is reviewing an earlier change` | 편집을 열지 않았습니다. 심사가 끝나기를 기다리거나, 소유자가 정하면 `in_review=cancel`로 다시 돌립니다(앞 심사를 취소하고 이번 출시를 보냄). |
| `… NOT_APPROVED` 또는 `… NOT_SENT_FOR_REVIEW` | 편집을 열지 않았습니다. Play Console → 게시 개요에서 거절 뒤의 변경이나 보내지 않은 변경을 정리하고(손으로 '검토를 위해 전송' 또는 삭제) 다시 돌립니다. |
| 404 `Package not found` | Play Console에서 앱을 만들고 첫 번들을 직접 올린 뒤 다시 실행합니다(위 1, 5단계). |
| 403 `does not have permission`, `insufficient permissions`, 401 | 서비스 계정을 "앱을 테스트 트랙으로 출시" 권한으로 초대했는지 봅니다. 적용에 24–48시간 걸릴 수 있으니, 설정 직후라면 기다렸다가 다시 실행합니다. |
| 403 선언·정책: `You must let us know whether your app uses any Foreground Service permissions`, `privacy policy`, `declaration` 등 | 서비스 계정 문제가 아니고, 기다려도 풀리지 않습니다. Play Console → 앱 콘텐츠에서 Google의 메시지가 말하는 항목(선언, 개인정보처리방침)을 끝낸 뒤 "Re-run failed jobs"를 누릅니다. 같은 AAB로 다시 올립니다. |
| 그 밖의 403 | 스크립트는 까닭을 추측하지 않고 "See Google's message above."만 알려 줍니다. Google의 메시지를 읽습니다. |
| `has not been used in project` | 키를 만든 Cloud 프로젝트에서 Google Play Android Developer API를 사용 설정합니다. |
| `invalid_grant` | 키가 지워졌거나 다른 키입니다. 새 JSON 키를 만들고 시크릿 2개를 다시 넣습니다. |
| `already been used` (versionCode) | 같은 versionCode의 다른 번들이 먼저 올라갔습니다. 다시 빌드합니다. |
| `Play already serves versionCode N on alpha; this run's bundle (M) is older` | 더 새 빌드가 이미 트랙에 있습니다. 옛 실행의 잡을 다시 돌렸거나, 두 실행의 잡이 거꾸로 끝난 경우입니다. 아무것도 올리지 않았습니다. 이 빌드가 꼭 필요하면 새로 빌드합니다. |
| `doesn't allow any existing users to upgrade`, `shadowed`, `higher version codes` | Play가 거절했습니다. 이 트랙이나 다른 트랙에 더 높은 versionCode가 있어서 이 번들을 받을 테스터가 없습니다. 새로 빌드합니다. |
| `did not answer the commit properly` | 커밋의 응답을 받지 못했습니다. 출시가 됐을 수도 있고 안 됐을 수도 있습니다. 같은 upload를 다시 실행합니다. 이미 됐으면 "is there already"나 "already released; nothing to do"로 끝나고, 아니면 다시 올립니다. |
| `draft app` | 첫 출시를 Play Console에서 끝냅니다(HJViewer는 1.9.0으로 끝냈음). |
| 다른 키로 서명 | 업로드 키로 서명했는지 봅니다. 업로드 키를 잃었거나 바꿔야 하면 재설정을 요청합니다. 새 업로드 키를 만들고 그 인증서를 PEM으로 내보낸 뒤(`keytool -export -rfc -keystore <새 키스토어> -alias <별칭> -file upload_certificate.pem`), Google Play로 보호됨 → Play 스토어 보호 → Play 앱 서명 관리 → 업로드 키 인증서에서 "업로드 키 재설정 요청"을 누르고 그 파일을 올립니다. 재설정은 Google이 하고, 바로 되지 않습니다. |
| 편집이 지워짐 (`edit` … `deleted`) | 다른 실행이나 Play Console 변경과 겹쳤습니다. 다시 실행합니다. |
| 출시 노트 언어 | 앱의 스토어 등록정보 언어여야 합니다(1단계의 기본 언어 한국어). |
| `The android job passed no bundle name or version` | GitHub는 시크릿 값이 든 잡 출력을 버립니다. 키 별칭 같은 시크릿 값이 번들 이름(`HJViewer-<버전>-<커밋>`)이나 버전에 들어 있지 않은지 봅니다. |

Android 잡의 번들 단계가 내는 메시지는 다음과 같습니다.

| 메시지 | 대처 |
|---|---|
| 경고 `This run has no Play bundle` | 번들 단계가 실패했습니다(확인 빌드가 아닌 실행). 스토어 실행이 올릴 `-play` 아티팩트가 없습니다. 이 실행의 APK는 그대로 씁니다. "Build Play bundle"과 "Sign and check Play bundle"의 로그에서 까닭을 보고 고칩니다. `play`를 켠 실행과 확인 빌드에서는 같은 문제로 Android 잡이 실패합니다. |
| `the bundle has '…', which only the side-loaded APK may have` | 사이드로드 전용 항목이 번들에 들어 있습니다. `client/play/AndroidManifest.xml`이 그 항목을 정확한 이름으로 빼는지 봅니다. `PLAY_MANIFEST`에 넣어서 통과시키지 않습니다. |
| `the bundle was built without the Play manifest overlay` | 번들에 Play 전용 매니페스트의 표지가 없습니다. "Build Play bundle"이 그 파일을 복사했는지, Gradle이 `src/release/AndroidManifest.xml`을 병합했는지 봅니다. |
| `the bundle's permissions, foreground service types or accessibility services changed` | 위 "Play 선언 대상"대로 합니다. |
| `the bundle has no librustdesk.so for arm64-v8a` (또는 다른 라이브러리) | 번들에 네이티브 라이브러리가 빠졌습니다. `jniLibs/arm64-v8a`에 라이브러리를 넣는 APK 빌드 단계와 번들 빌드 사이에 그 폴더를 지우는 것이 없는지 봅니다. |
| `bundletool … could not be downloaded`, `does not have the pinned SHA-256` | 내려받기가 실패했거나 받은 파일이 고정한 해시와 다릅니다. 다시 실행해 보고, 해시가 계속 다르면 쓰지 않고 원인을 확인합니다. |

### 출시된 versionCode 확인

최소 버전 정책은 Play 트랙에 어떤 versionCode가 출시됐는지 확인한 뒤에만 Play 쪽 값을 올립니다(`hjv-release policy --play-live <versionCode>`). 확인하는 곳은 두 가지입니다.

- **스토어 실행의 status**(근거): `gh workflow run store.yml -R yonsei-projecttf/hjviewer-private -f platform=android -f action=status -f build=<versionCode>`. 편집 없이 `applications.tracks.releases.list`로 alpha와 internal의 출시를 읽고, 실행 요약에 "Google Play alpha: versionCode N PUBLISHED" 같은 한 줄을, 아티팩트에 JSON을 남깁니다. Google 심사가 끝나 `PUBLISHED`가 된 뒤에만 `--play-live`를 씁니다.
- 업로드한 실행의 요약 "Uploaded kr.ac.yonsei.hjviewer versionCode N to Google Play alpha (completed)"는 Play가 커밋을 받아들였다는 뜻일 뿐입니다. 그 뒤 Google 심사가 있습니다.
- 출시 기록 `store-releases.txt`의 `play <버전> <versionCode> upload alpha …` 줄. 업로드가 커밋된 뒤 스토어 실행이 더합니다.

upload를 다시 실행하는 것은 상태 확인으로 쓰지 않습니다. 트랙에서 그 번들이 빠져 있고 더 새 번들도 없으면, 그 번들을 다시 출시하기 때문입니다.

### 키 교체와 유출

- 서비스 계정 키는 기본으로 만료되지 않습니다. 바꿀 때는 Google Cloud 콘솔에서 새 JSON 키를 만들어 위 명령으로 두 시크릿을 다시 넣고, 옛 키를 지웁니다.
- 키가 샜다고 의심되면 Cloud 콘솔에서 그 키를 바로 지웁니다. 이 권한으로는 프로덕션에 출시할 수 없지만, 공개 테스트(누구나 볼 수 있음)에는 출시할 수 있습니다. 트랙에 새 번들을 올리려면 업로드 키(키스토어)도 있어야 하지만, 내부 앱 공유에는 어느 키로 서명한 파일이든 올릴 수 있습니다(위 "처음 한 번 할 일"의 7단계).

## 소요 시간

이 저장소의 실측값과 upstream 참고값입니다. 첫 실행은 2026-10-10 비공개 저장소에서 캐시 없이, 1.5.1은 2026-10-11 빌드 전용 저장소에서 캐시가 있는 상태로 돌았습니다(실행 38106290530). upstream 값은 2026-10-09 nightly(공개 저장소, Rust 캐시만 있음)입니다.

| 잡 | 첫 실행 (캐시 없음) | 1.5.1 (캐시 있음) | upstream 참고 |
|---|---|---|---|
| Bridge codegen | 7분 | **4분** | 7분 |
| Dart tests | 없음 | **2분** | 없음 |
| Windows x64 | 72분 | **24분** | 52분 (vcpkg 16, 빌드 22, exe 4, 서명·MSI 5) |
| Android arm64 | 51분 | **18분** | 31분 (vcpkg 9, Rust 11, Flutter 7) |
| iOS | 15분 | **7분** | 22분 (vcpkg 8, Rust 8, Flutter 4) |

1.5.1 결과물 크기는 Windows exe 25MB, Android APK 29MB, App Store용 iOS IPA 44MB입니다.

- **러너 사양 차이**: 비공개 저장소의 Linux·Windows 러너는 2코어/8GB로, 공개 저장소(4코어/16GB)의 절반입니다. 그래서 컴파일이 upstream보다 오래 걸립니다. macOS는 공개·비공개 모두 3코어 M1입니다.
- **전체 시간**: 플랫폼 잡이 병렬로 돌기 때문에 가장 긴 Windows 잡이 결정합니다. 1.5.1 실행은 모두 28분 걸렸습니다.
- **Google Play 경로**: 비공개 서명 빌드의 Android 잡은 번들을 만들고 서명·검사해 올리는 단계 셋으로 약 3분 15초 늘어납니다(1.6.0, 1.6.1 출시 실행의 실측). Play 업로드는 빌드 실행이 아니라 스토어 실행에서 돕니다(Linux 3–5분, [store-release.md](store-release.md)의 "비용").

## 비용 (비공개 저장소)

GitHub 공식 문서(2026-10 확인) 기준입니다.

- **공개 저장소**: 표준 러너 사용이 무료입니다.
- **비공개 저장소**: 매달 포함 시간을 먼저 쓰고, 넘으면 분당 요금을 냅니다.
  - **포함 시간**: Free 2,000분, Pro 3,000분, Free(조직) 2,000분, Team 3,000분
  - **분당 요금**: Linux 2코어 $0.006, Windows 2코어 $0.010, macOS $0.062
  - **올림**: 잡마다 사용 시간을 분 단위로 올림합니다.
  - **저장 공간**: 아티팩트는 Free 500MB, Pro 1GB, Team 2GB입니다. 캐시는 저장소당 10GB입니다.
- **OS별 배수**: 현재 공식 문서에는 Windows·macOS 사용 시간이 포함 시간에서 몇 배로 차감되는지 나와 있지 않습니다. 실제 차감은 Settings → Billing and licensing의 사용량 화면에서 확인해야 합니다. 분당 요금으로 비교하면 macOS가 Linux의 약 10배, Windows가 약 1.7배입니다.

예를 들어 캐시가 찬 상태의 전체 빌드 1회(1.5.1 실측, 잡마다 분 단위로 올림)를 분당 요금으로 환산하면 다음과 같습니다(포함 시간을 넘긴 경우).

- Linux 약 25분(Bridge 4, Dart 3, Android 18): $0.15
- Windows 약 24분: $0.24
- macOS 약 7분: $0.43

합계는 약 $0.8입니다. macOS가 분당 가장 비싸므로 iOS가 필요할 때만 `ios`나 `all`을 고르세요.

Google Play 경로는 Android 실행마다 Linux 약 4분(AAB 단계), 약 $0.03을 더하고, 업로드하는 스토어 실행이 3–5분을 더합니다. AAB 단계(약 3분 15초)는 1.6.0, 1.6.1 출시 실행에서 잰 값입니다. AAB 아티팩트(약 30MB)는 30일 동안 저장 공간을 씁니다.

**저장소 공개도 고려하세요.**

- RustDesk는 AGPL-3.0이라, 앱을 받는 사용자에게 수정한 소스를 제공할 의무가 이미 있습니다.
- 클라이언트 포크(이 저장소)를 공개하면 그 의무를 지키는 방법도 되고, 표준 러너도 무료가 됩니다.
- 공개 저장소에서는 **실행 로그와 아티팩트를 누구나 볼 수 있습니다.**
  - 시크릿은 로그에서 `***`로 가려집니다. 하지만 저장소 **변수**는 각 단계의 로그에 그대로 찍힙니다. 서버 주소와 키를 로그에 남기지 않으려면 `HJV_SERVER`/`HJV_SERVER_KEY`를 시크릿으로 등록하세요(워크플로는 둘 다 받습니다).
  - 서버 주소와 키는 **앱 안에 들어갑니다**. 공개 아티팩트로 받은 앱이나 소스로 직접 빌드한 앱은 그 서버에 접속할 수 있습니다. 소수만 쓰는 서버라면 실제 서버 값으로 빌드한 앱은 공개 아티팩트로 내지 말고, 비공개 빌드 저장소에서 빌드하세요. ([deploy.md](deploy.md)의 "소규모 비공개 운영")
  - 서명된 IPA는 공개 저장소에서 아티팩트로 올리지 않습니다. 프로비저닝 프로파일에 Apple 개발자 이름, 팀 ID, Ad Hoc의 경우 기기 UDID가 들어 있기 때문입니다. 서명 단계의 로그에서도 인증서 이름과 팀을 가립니다.
- 내려받는 모든 결과물(artifact)에는 `LICENSE.txt`, `NOTICE.txt`, `SOURCE.txt`(이 빌드의 소스 위치), `THIRD_PARTY_NOTICES.txt`(Rust 크레이트, vcpkg 포트, 동봉 드라이버, 앱 글꼴 Pretendard의 라이선스)가 함께 들어갑니다. AGPL이 요구하는 고지입니다. 글꼴의 라이선스(SIL OFL 1.1)는 글꼴 파일과 함께 앱 안(`flutter/assets/fonts`, `client/overlay`에서 옴)에도 들어 있어, 고지 파일이 없는 TestFlight와 Google Play 앱에도 따라갑니다. TestFlight로 올리는 IPA와 Google Play에서 설치하는 앱에는 이 파일이 없고, 앱 정보 화면의 소스 링크만 있습니다. Google Play 출시 노트에는 워크플로가 이 빌드의 소스 커밋 링크를 넣습니다. 내부 테스터가 아닌 사람에게 TestFlight로 나눠 주기 전에, 고지를 앱 안에 넣거나 TestFlight의 "테스트할 내용"에 이 빌드의 소스 커밋(`client-<앱 버전>` 태그)과 고지 링크를 적어야 합니다.

## 캐시

- **저장 대상**: Rust(`Swatinem/rust-cache`), vcpkg(도구와 빌드된 포트), Bridge 잡의 Flutter SDK를 GitHub 캐시에 저장합니다.
- **삭제 규칙**: 7일 동안 쓰지 않은 캐시는 지워집니다. 저장소당 10GB를 넘으면 오래된 것부터 지워집니다.
- **태그 빌드**: 다른 태그의 캐시는 쓸 수 없고, 기본 브랜치의 캐시만 씁니다. 태그를 붙이기 전에 기본 브랜치에서 수동 실행을 한 번 해 두면 빨라집니다.

## upstream 버전을 올릴 때

`scripts/upstream.env`를 바꾸면 새 커밋의 `.github/workflows/flutter-build.yml`과 `bridge.yml`을 이 워크플로와 비교하세요. 특히 다음 두 가지를 맞춰야 합니다.

- 워크플로 맨 위 `env`의 도구 버전
- 주석에 적힌 upstream 줄 번호의 단계

## 아직 확인하지 못한 것

2026-10-10 첫 실행에서 세 플랫폼 빌드가 성공했습니다(1.5.0). 2026-10-11 빌드 전용 저장소의 첫 실행(38106290530)에서는 1.5.1의 새 빌드 경로도 모두 통과했습니다.

- `workflow_call`과 다른 저장소에서 하는 체크아웃
- 세 잡의 앱 버전 적기
- `HJViewer.exe` 이름 바꾸기와 압축 해제 실행기
- Windows 설치 시험: 자동 시작 서비스와 트레이 바로가기
- Dart 테스트 160개
- TestFlight 업로드(빌드 262840253, Apple 처리 후 VALID)
- Google Play용 AAB의 빌드·서명·검사: 1.6.0과 1.6.1의 출시 실행(38559428611, 38568720786)이 러너에서 번들을 만들고 서명해 그때의 검사를 모두 통과했습니다(minSdk 검사는 그 뒤에 더함). `PLAY_MANIFEST`는 그대로였고, Play 전용 매니페스트의 표지가 있었고, targetSdk는 36, 번들 설정의 zip 페이지 정렬은 `PAGE_ALIGNMENT_16K`였으며, R8 매핑도 들어 있었습니다. 번들 단계 셋은 약 3분 15초 걸렸습니다.

아래는 아직 실행하지 않은 경로입니다.
- **Google Play 경로**: 스토어 실행의 Play 업로드(`dry_run`의 validate, completed)와 status, alpha 출시의 심사 시간, Google Play 잡만 다시 실행하기. 개발 PC에서는 가짜 Play API로 업로드 스크립트를 시험했습니다.
- **스토어 실행 전체**: environment `store`의 시크릿이 이름으로 넘긴 재사용 워크플로 잡에 오는지, `job.workflow_repository`·`job.workflow_sha` 체크아웃, 다른 실행의 아티팩트 받기, App Store Connect의 prepare·submit·release(처음은 1.9.1). 개발 PC에서는 가짜 서버로 스크립트를, 오프라인 시험으로 워크플로의 규칙을 시험했습니다([store-release.md](store-release.md)).
- **의존성 변경**: 모든 빌드가 `--locked`이므로, 패치가 의존성을 바꾸면 `Cargo.lock`도 패치에 포함해야 합니다.
- **Xcode 26.6 고정**: 러너 이미지에서 26.6이 빠지면 "Select Xcode" 단계가 실패합니다. 그때 `XCODE_VERSION`을 이미지에 있는 26.x로 바꿉니다. Apple은 2027년 4월부터 iOS 27 SDK를 요구할 예정입니다. 그 전에 Flutter를 3.38 이상으로 올리고 AppDelegate를 UIScene으로 옮겨야 합니다.
- **Flutter 플러그인**: 패치 0006이 개인정보 매니페스트 때문에 sqflite, file_picker, package_info_plus, device_info_plus를 올렸습니다. 2026-10-10에 세 플랫폼 모두 빌드되는 것까지 확인했습니다(실행 38042826319의 Windows·Android, 38044728062의 iOS). 실제 기기에서의 동작은 아직 확인하지 않았습니다.
- **외부 파일**: Windows 빌드는 빌드 중에 rustdesk 쪽 GitHub 릴리스에서 파일을 받습니다. 커스텀 Flutter 엔진(`main` 릴리스)과 가상 디스플레이 드라이버입니다. 이 파일이 바뀌거나 사라지면 결과가 달라질 수 있습니다.
