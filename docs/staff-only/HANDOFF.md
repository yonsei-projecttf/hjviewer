# 교직원 전용 출입 제한: 남은 작업 인계 (2026-10-17)

이 브랜치(`staff-only-gate`)를 이어받는 에이전트가 먼저 읽는 문서입니다. 설계는 [brief.md](brief.md), 형식은 [../access-tickets.md](../access-tickets.md)(계약), 1차 작업의 에이전트 보고는 [wave1-notes.md](wave1-notes.md)에 있습니다.

## 1. 목적과 사용자 결정

HJViewer를 연세대학교 교직원 전용 서비스로 만듭니다. 연세 SSO로 로그인한 사람은 쓰고, 로그인하지 않은 사람은 운영자가 정한 **이용 비밀번호**를 아는 경우에만 그때그때 씁니다. 완전히 막지는 않습니다.

사용자가 2026-10-16에 정한 것:

1. **양쪽 모두 제한**: 연결을 거는 쪽(컨트롤러)과 서버에 등록하는 PC 모두. PC는 로그인(계정에 묶임)했거나 이용 비밀번호를 넣어야 온라인이 됩니다.
2. **이용 비밀번호는 하나**: 운영자가 서버 파일에 해시로 두고 바꿉니다.
3. **앱은 이용 비밀번호를 기억하지 않음**: 컨트롤러는 연결할 때마다, PC는 앱을 켤 때마다 넣습니다.
4. **1.8.0을 낼 때 바로 강제**: 유예가 없습니다.
5. **이름은 "이용 비밀번호"**(영어 "access password"): PC의 "일회용 비밀번호"와 다릅니다. 사용자에게 보이는 글에 "손님 비밀번호"를 쓰지 않습니다(내부 식별자 `guest`는 그대로 둬도 됨).
6. **한 번 넣으면 그때 연결하는 PC 하나**에만 닿습니다.
7. **App Store·Play 심사관**: 데모 계정(review.conf)에 데모 PC만 허용합니다(review.conf의 새 `PC=` 줄, `UNTIL`까지). 심사관에게 이용 비밀번호는 주지 않습니다.
8. **교직원이 만드는 일회용 코드**(10분, PC 하나)는 1.8.x에서 더합니다. 1.8.0에는 넣지 않습니다.

정하지 않아 기본값으로 둔 것(brief 5절): 티켓 수명 교직원 24시간·묶인 PC 72시간·이용 비밀번호 12시간, hjv-api가 멈춰도 가진 티켓으로 계속 연결, 1.8.0은 직접 IP 접속과 LAN 검색을 끔, hbbr은 지금 검사하지 않음, brief Q15의 작은 기본값. 사용자가 아직 정할 것: 데모 PC를 둘지와 어느 PC로 할지, App Store 배포 방식(Unlisted 또는 Custom App).

## 2. 지금 상태 (1차 작업)

방식은 brief의 A안입니다. hjv-api가 Ed25519 출입 티켓에 서명하고, hbbs가 오프라인으로 확인합니다.

| 묶음 | 이 브랜치에 있는 것 | 시험 |
|---|---|---|
| 계약 | 0707e18: `docs/access-tickets.md`, `server/api/tests/vectors/access-tickets.json`(티켓 84건, RegisterPk 7건, access.conf 39건)과 생성기 `access-tickets.py`, `scripts/public-test-keys.txt`의 시험 키 | 생성기 `--check` 통과 |
| hjv-api | 957ecbd, 292b031: 티켓 서명 키(새 자격 증명 `hjviewer-api-ticket-key`), `POST /api/hjv/ticket`, 로그인 답과 pc/sync 답의 티켓, `admin ticket status`. 계약 문서의 일부(2-4 등)를 고쳤음 | `cargo test --locked` 409건 통과 |
| hbbs | `server/patches/0006`, `0007`(리뷰 수정): access.conf(`MODE=off|count|enforce`), punch hole·RequestRelay·RegisterPk(TCP/WS) 확인, 등록 만료, `hjv-access` 콘솔. 기본은 꺼짐 | 서버 단계 명령 통과(lib 46, tests/access.rs 18 등) |
| 클라이언트 | `client/patches/hbb_common/0003`(RegisterPk 필드 1001), `client/patches/rustdesk/0058`, `0059`: 티켓 전달·갱신, 거절 표시 인식(아직 UI 없음), 고정 빌드에서 직접 IP·LAN 검색·안전하지 않은 TLS 대체를 끔 | lib 571건, 고정 계정 빌드 `hjv` 257건 통과 |

세 패치 묶음은 새로 받은 upstream에 적용하면 시험한 트리와 정확히 같습니다(2026-10-17, 트리 해시로 확인). 1차 작업은 운영 서버나 앱 배포에 아직 아무 영향이 없습니다.

알아 둘 한계(자세한 것은 wave1-notes.md): upstream의 uuid 신뢰 방식(오프라인 ID의 키를 그 uuid와 마지막 주소를 아는 쪽이 바꿀 수 있음), 티켓은 소지자 증명(복사하면 수명 동안 쓰임), hbbr은 열린 중계기로 남음.

### 2차 작업 (2026-10-17, 클라우드 세션)

3절의 3-1~3-5를 이 브랜치에 커밋하고, `staff-only-gate`에서 `main`으로 가는 초안 PR을 열었습니다. 클라우드에서 돌린 시험과 돌리지 못한 것은 다음과 같습니다.

| 묶음 | 커밋 | 클라우드에서 돌린 시험 |
|---|---|---|
| 3-1 계약 | 01c9908 | 벡터 생성기 `--check`, hjv-api `cargo test --locked` 409 통과(무시 10), hbbs 서버 단계 명령 96 통과. 새 SHA-256을 hjv-api, hbbs(0008), 클라이언트(0060)가 고정 |
| 3-2 hjv-api | 075322d | hjv-api 424 통과(무시 10, root로 돌려 건너뛴 시험 1: "a policy that cannot be read"), `test_guest_conf.py`, `test_review_conf.py`, nginx의 `/api/hjv/guest` 한도 |
| 3-3 클라이언트 | e52cda5(패치 0061) | Dart 순수 시험 134 통과(새 5), 바꾼 Dart 파일의 정적 분석 오류 0, 패치 61개를 고정 커밋에 다시 적용해 같은 트리(f0a1a15). `guest-conf.py`가 앞뒤 공백의 비밀번호를 거절(앱이 지우므로). **Rust는 컴파일하지 못함**: chromium.googlesource.com이 막혀 webm의 하위 모듈(libwebm)을 받지 못해 cargo가 의존성을 풀지 못함. rustfmt로 문법과 형식만 봄 |
| 3-4 문서 | 0bfc058 | 문서의 링크와 고리, `deploy/access.conf.example`을 빌드한 hbbs가 `access off`로 읽음, 런북의 콘솔 한 줄(`HJVACCESS1 mode=…`). deploy 단계(Docker, systemd)는 클라우드에 없어 못 돌림 |
| 3-5 E2E와 버전 | 75cec78, 마지막 커밋 | E2E는 시나리오만(e2e.md "출입 제한 단계", sudo와 네트워크 이름공간과 클라이언트 빌드가 없음). 버전 1.8.0(`client/version.env`) |

정한 것(문서에 적음): 컨트롤러의 이용 비밀번호 출입증은 그 프로세스에서 그 대상으로 가는 마지막 세션이 닫힐 때 버립니다. 연결 문제(400, 503, 네트워크)는 모두 E3입니다. 앱은 입력의 앞뒤 공백을 지우고, `guest-conf.py`는 그런 비밀번호를 만들지 않습니다. 앱 시작과 앞으로 돌아올 때 `hjv-access-refresh`를 보냅니다(1차 노트의 남은 일).

### 3차 작업: 사용자 쪽 안내 (2026-10-17, 클라우드 세션)

2차 작업 뒤 사용자 흐름을 코드로 따라가 찾은 14가지를 사용자 결정("모두 진행")으로 고쳤습니다. 같은 PC에 새로 연결할 때마다 이용 비밀번호를 묻는 것(1차의 결정 3)은 정책이라 그대로 둡니다.

| 묶음 | 커밋 | 한 것 | 클라우드에서 돌린 시험 |
|---|---|---|---|
| U1 hjv-api | b99d8d2 | `GET /api/hjv/access`(토큰 없음, hbbs 콘솔 `hjv-access`의 모드만, 30초 묶음, 계약 3-6), 이용 비밀번호 429의 `retry_after`(주소의 실패·출입증 한도, 모두의 시도) | hjv-api `cargo test --locked` 426 통과(무시 10), 새 시험: 모드 답·묶음·`unknown`, 429마다 그 한도의 초, `parse_access` |
| U2 클라이언트 | 277815b(패치 0062) | A1을 "교직원 전용 서버"로, [로그인]을 기본 단추로, 이용 비밀번호는 펼쳐서. 화면 글에서 표시 `(access not allowed)`를 뺌(msgbox의 Rust 글에는 남김). 이용 비밀번호 뒤 토스트. A2에 [다시 연결]. PC 홈의 알림을 ID 위로(P1, P2의 "HH:mm까지"와 [끝내기], G2). 거절된 동안 상태 줄과 Windows 트레이 안내. 설치했고 로그인하지 않은 Windows PC는 로그인 창을 한 번 엶. 연결 화면의 G3, 로그아웃과 "내 목록에서 빼기"의 L1·L2(서버 모드에 따라, 원격으로 보고 있으면 한 줄 더). E2의 "약 N분". 세션 창의 로그인 알림은 주 창에서 | Dart 순수 시험 135 통과, 바꾼 Dart 파일의 정적 분석 오류 0(경고·정보 3은 전부터 있던 것), 패치 62개를 고정 커밋에 다시 적용해 같은 트리(ec94cde), 바꾼 Rust 파일 rustfmt. **Rust는 이번에도 컴파일하지 못함**(2차와 같은 까닭) |
| U3 문서 | 이 커밋 | 계약 5-3(새 글과 규칙, G2·G3·L1·L2), distribution, architecture 9-2·9-3, 런북 9-3(앱이 보는 모드 확인)과 9-6(정책 seq 4·5의 알림 글), e2e.md(A1·A3·A13에 모드 확인), privacy, README | 문서 링크 검사 |

정한 것: 로그아웃의 "한 번 더 확인"은 두 번째 창이 아니라 같은 확인 창의 경고 줄로 했습니다. 트레이는 안내(툴팁)만 바꾸고 알림은 띄우지 않습니다. G2는 서버 모드와 상관없이 보입니다(`enforce` 전의 예고). P2의 끝나는 시각은 그 창의 메모리에만 둡니다(앱을 다시 켜면 줄은 사라지고, 출입증이 없으면 P1이 나옴).

### 4차 작업: 클라우드에서 마무리 검증 (2026-10-17, 클라우드 세션)

개발 PC의 에이전트를 쓸 수 없어서, 개발 PC에서 하려던 검사 가운데 클라우드에서 할 수 있는 것을 모두 했습니다. 사용자가 chromium.googlesource.com을 허용해 클라이언트 Rust를 처음 컴파일할 수 있었습니다.

| 묶음 | 커밋 | 한 것 | 클라우드에서 돌린 시험 |
|---|---|---|---|
| C1 클라이언트 | eb4fcd2 | 0058–0062의 Rust를 처음 컴파일함. `scripts/ci.sh client`가 hbb_common의 WebSocket 시험에서 세 번에 두 번 실패하던 것을 고침. 전부터 있던 문제로, 프록시 시험이 프로세스 전체의 프록시 설정을 바꾸는 동안 옆의 시험이 그 프록시로 붙었음. 그 단계만 시험을 하나씩 돌림 | `scripts/ci.sh client` 통과(건너뜀 1: 1.5.1 실물 산출물의 릴리스 기록 시험. `build/release-1.5.1`은 개발 PC에만 있음). Rust 1.75 check, 공유 벡터, Dart 135 |
| C2 E2E 출입 제한 단계 | cf0e5ef, fe81e15, 5c1c749(클라이언트 패치 0063), 91d973d, e2c6858 | e2e.md의 A1~A14를 `tools/e2e/firewall-e2e.sh`에, 감지와 하한을 `scripts/ci.sh`에 넣음(`E2E_LEAST_STRICT` 91, `UNIT_LEAST[hjv]` 260). 돌리며 고친 것: 감지가 찾는 이름(릴리스 빌드는 명령 이름을 글자로 남기지 않음), E2E의 화면 명령이 세션 시작의 거절을 앱의 창 A1·A2로 보이게 함(패치 0063, hjv-e2e 기능 안), A4의 기대(PC로 돈 적이 없는 컨트롤러의 출입증은 `ctl`), PASS 줄의 시간 | 아래 관문 |
| 관문 | e2c6858 | `scripts/ci.sh prepush server e2e`(로그 `build/ci/prepush/20261017-040049-e2c6858`)와 `prepush wol`(`20261017-042156-e2c6858`). root가 아닌 사용자, `HJV_CI_STRICT=1` | server 통과(791, 무시 12, 건너뜀 0: hjv-api 426, hbbs·hbbr 96, hjv-relay 102, 나머지는 hjv-release), e2e 통과(`--lib hjv` 260, 키 없는 `hjviewer_update` 159, strict PASS 91(A1~A14 포함), open PASS 2), wol 통과(PASS 20) |
| 그 밖 | 마지막 커밋 | 클라이언트 lib 시험 전체(관문의 E2E 빌드가 만든 시험 바이너리, 화면은 Xvfb). 패치 0063 뒤의 `scripts/ci.sh client` | lib 579 통과, 무시 5. TCP 펀치 시험도 통과(WSL이 아닌 리눅스, e2e.md "TCP 펀치 시험"). 패치 0063 뒤의 `scripts/ci.sh client`도 통과(건너뜀 1, 위와 같은 것. Rust 1.75 check, Dart 135) |

클라우드 컨테이너에서 맞춘 것(커밋하지 않음, e2e.md "실행"):
- E2E는 root로 돌지 않으므로 사용자 `hjvci`(sudo는 비밀번호 없이)로 돌렸습니다.
- systemd가 없어 두 가지를 데스크톱처럼 맞췄습니다. `/usr/local/bin/getent`가 빈 이름의 질의에 "없음"으로 답하게 하고(없으면 PC가 자기를 로그인 화면으로 보고 S6이 실패), `/var/lib/dbus/machine-id`를 `/etc/machine-id`의 링크로 바꿨습니다(없으면 모든 기기가 한 기기로 보여 S15가 실패).
- 그 사용자의 환경에는 클라우드 프록시가 있어 `NO_PROXY`에 `hjv.test`를 더했고(없으면 `wol-e2e.sh`의 curl이 시험 도메인을 프록시로 보냄), umask는 022로 했습니다(hjv-relay 시험 하나가 그것을 전제함).

찾았지만 이 PR에서 고치지 않은 것(따로 할 일, 둘 다 전부터 있던 것):
- `tools/e2e/firewall-e2e.sh`는 네임스페이스마다 `/etc/machine-id`만 따로 줍니다. `/var/lib/dbus/machine-id`가 따로 된 파일인 호스트에서는 앱이 그것을 먼저 읽어 계정 단계가 실패합니다.
- `relay/tests/cli.rs`의 `status_and_test_wake_write_only_to_a_plain_out_file`은 umask 022를 전제합니다(0002이면 0664가 나와 실패).

## 3. 남은 일 (순서대로)

### 3-1. 계약 하나로 맞추기 (먼저)

- `docs/access-tickets.md`를 하나뿐인 계약으로 맞춥니다. hjv-api 묶음이 고친 부분, hbbs 리뷰가 문서에 남긴 지적 7·8(wave1-notes.md의 S review), 클라이언트 0058/0059가 바꾼 요청·응답 모양을 반영합니다.
- 생성기의 `=` 없는 줄 처리(S 지적 8)를 고치고 벡터를 다시 만듭니다. hjv-api, hbbs, 클라이언트의 벡터 시험이 모두 통과해야 합니다.
- 사용자에게 보이는 글(계약의 5-3 문구 포함)에서 "손님 비밀번호"를 "이용 비밀번호"로 바꿉니다.

### 3-2. hjv-api: 이용 비밀번호와 데모 PC

- `POST /api/hjv/guest`: 이용 비밀번호를 받아 티켓을 줍니다.
  - 비밀번호는 `guest` 용도로 봉인합니다.
  - `/etc/hjviewer/guest.conf`는 review.conf처럼 PBKDF2 해시와 `UNTIL`을 담고, 바뀌면 다시 읽습니다. 만드는 도구는 `deploy/review-conf.py`처럼 사용자 터미널에서만 비밀번호를 묻습니다.
  - 컨트롤러 티켓은 `to=[대상 ID 하나]`, PC 티켓은 그 PC의 id·pk에 묶고(기기 서명, hbbs의 `hjv-pk`), 둘 다 12시간입니다.
  - 한도: 주소마다 실패 10분에 5번·하루 20번, 전체 시도 시간당 60번, 주소마다 티켓 하루 50개. 해시 계산은 한도 뒤, 동시에 2개까지입니다.
  - 틀린 비밀번호와 꺼진 이용 비밀번호는 같은 401과 같은 지연입니다.
  - 기록에는 주소의 꼬리표만 남기고 6개월 둡니다.
- review.conf의 `PC=` 줄: 데모 계정에 컨트롤러 티켓을 주되 `to`는 `PC=`의 ID들(1~3개), 끝은 `min(발급+24시간, UNTIL)`입니다. `PC=`가 없으면 티켓이 없습니다.
- README, env 예시, 런북 7-17(데모 PC 줄), 시험.

### 3-3. 클라이언트: 이용 비밀번호 화면 (1.8.0)

- Rust: `POST /api/hjv/guest` 교환, 이용 비밀번호와 그 티켓은 메모리에만 둠, 설치된 PC는 UI 프로세스가 받은 PC 티켓을 서비스에 IPC로 넘김(디스크에 쓰지 않음).
- Flutter, 서버가 거절했을 때만 묻습니다(`MODE=off` 서버에서는 지금과 같음):
  - 컨트롤러: 연결이 `access not allowed`로 거절되면 창을 띄웁니다. [로그인], [이용 비밀번호로 연결], [취소]. 이용 비밀번호를 넣으면 그 대상 PC 하나의 티켓을 받아 다시 연결합니다.
  - PC: 등록이 거절되면(NOT_DEPLOYED와 표시) 홈에 "이 PC는 아직 서버에 연결되지 않았습니다"와 [로그인], [이용 비밀번호]를 보입니다. 앱을 켤 때마다 묻습니다.
  - 휴대폰 앱(제어만 함)은 컨트롤러 창만 있습니다.
  - 한국어·영어 문구, Dart 시험.
- 설치된 PC가 사람 없이도 연결되려면 계정에 로그인해 두어야 한다는 안내를 설정과 설치 화면에 넣습니다.

### 3-4. 문서

architecture.md(출입 제한 절), privacy.md(이용 비밀번호 기록, 티켓, 데모 계정이 Google Play 심사에도 쓰임, 영문 제목 "Privacy policy"), distribution.md(사용자 안내: 로그인 필요, 이용 비밀번호), server-runbook.md(새 절: 사용자가 만드는 티켓 키, access.conf의 MODE 바꾸기, guest.conf, 출시일 순서), README의 현재 상태, ci.md(새 자격 증명), e2e.md.

### 3-5. 시험과 마무리

- E2E(`tools/e2e/firewall-e2e.sh`): MODE off·count·enforce, 교직원·이용 비밀번호·묶인 PC·데모 계정 티켓, 거절 문구, 옛 앱. `scripts/ci.sh`의 옵션 감지와 E2E 하한. E2E는 sudo와 네트워크 이름공간이 필요합니다. 클라우드에서 돌릴 수 없으면 시나리오를 쓰고 "개발 PC에서 실행 필요"로 남깁니다.
- 버전 1.8.0(`client/version.env`)은 모든 것이 끝난 뒤, 마지막 커밋에서 올립니다.

## 4. 작업 규칙

- **브랜치**: `staff-only-gate`에서만 일합니다(필요하면 하위 브랜치를 만들어 이 브랜치로 PR). `main`에 push하지 않고, 태그와 Release를 만들지 않고, PR을 병합하지 않습니다. 끝나면 `staff-only-gate`에서 `main`으로 가는 **초안 PR**을 엽니다.
- **커밋**: 작성자 이메일은 `haejin.cho@yonsei.ac.kr`(`git -c user.name=ysfaculty -c user.email=haejin.cho@yonsei.ac.kr commit`, upstream 패치 커밋은 이름 `hjviewer`). 메시지는 영어이고 마지막 줄은 `Co-Authored-By: Claude <noreply@anthropic.com>` 형식의 공동 작성자 줄입니다.
- **upstream 코드**: `scripts/fetch-upstream.sh all`로 `upstream/rustdesk`(하위 모듈 `libs/hbb_common`)과 `upstream/rustdesk-server`를 받고 패치를 `hjviewer` 브랜치에 적용합니다. 그 브랜치에 커밋한 뒤 `scripts/export-patches.sh client|server`로 패치 파일을 다시 만듭니다. 오버레이 파일(`client/overlay/`)은 패치에 넣지 않습니다.
- **코드 방식**: `upstream/rustdesk/AGENTS.md`를 따릅니다. 기존 코드 옆에 새 함수·모듈을 두고 호출 지점은 얇게, 관계없는 정리는 하지 않습니다. 새 crate는 꼭 필요할 때만.
- **비밀**: 실제 키를 만들지 않습니다. 시험 키(`scripts/public-test-keys.txt`)만 씁니다. 운영 서버, 실제 연세 SSO, 앱 스토어 콘솔에 접속하지 않습니다.
- **공개 저장소**: 운영 서버의 도메인과 주소, 실제 포털 ID, 데모 계정의 ID와 비밀번호를 어디에도 쓰지 않습니다. 개발 PC의 유출 목록(`build/leak-patterns.txt`)은 클라우드에 없으므로, 마지막 유출 검사는 개발 PC에서 합니다.
- **시험**: 묶음마다 마지막 커밋 뒤 그 묶음의 시험과 컴파일 검사를 한 번 돌립니다. 같은 전체 검사를 되풀이하지 않습니다. 돌리지 못한 것은 PR에 적습니다.
- **글**: 문서는 한국어(이 저장소의 쉬운 문체), 코드 주석과 커밋 메시지는 영어입니다.

## 5. 개발 PC에서 할 일 (클라우드가 하지 않음)

4차 작업까지로 개발 PC에 남은 것은 다음입니다(PR의 "개발 PC에서 확인할 것"과 같음). 에이전트 없이 손으로 할 수 있습니다.

1. `upstream/`을 새로 받습니다. `scripts/fetch-upstream.sh`는 있는 작업 사본을 덮어쓰지 않으므로(`exists; remove it to fetch again`) 옛 사본을 먼저 옮깁니다: `mkdir -p build/upstream-old && mv upstream/rustdesk-server upstream/rustdesk build/upstream-old/`, 그다음 `scripts/fetch-upstream.sh all`. 새 사본의 `HEAD^{tree}`는 클라우드 CI가 시험한 것과 같아야 합니다: `upstream/rustdesk` 8586e8c0ae5b, 그 `libs/hbb_common` 0a2c097e1d81, `upstream/rustdesk-server` ce6d09e969c7. 옛 사본은 내보내지 않은 커밋이 없는지 본 뒤 지웁니다(1차 작업의 `build/wt/` 작업 트리는 이미 패치에 들어 있음, [wave1-notes.md](wave1-notes.md) 첫머리). `scripts/ci.sh`의 단계는 `build/ci/` 아래에 따로 받으므로, 이 사본은 시험에 쓰이지 않고 prepush 로그의 머리에만 적힙니다.
2. `scripts/ci.sh prepush client deploy leak`. client는 `build/release-1.5.1`의 실물 산출물로 릴리스 기록 시험까지 돌고(클라우드에서는 그것만 건너뜀), deploy는 Docker와 systemd(새 `hjviewer.env.example`의 주석 줄, `access.conf.example`), leak는 개발 PC의 유출 목록입니다. 원하면 `e2e`와 `wol`도 함께 넣습니다(클라우드에서는 위의 컨테이너 맞춤과 함께 통과함).
3. 비공개 빌드 저장소의 확인 빌드(`check_only`, [ci.md](../ci.md)): 세 플랫폼 빌드, bridge가 있는 Dart 시험 전체(client-build.yml의 Dart tests 잡), **Windows에서만 컴파일되는** `src/tray.rs`의 훅과 `src/hjviewer_access.rs`의 `tray_*`.
4. 화면을 데스크톱과 휴대폰에서 눈으로 봄: A1(펼치기 전과 뒤, 휴대폰에서 키보드가 펼친 뒤에만 올라옴), A2의 [다시 연결], PC 홈의 P1(ID 위)·P2의 줄과 [끝내기]·G2, 상태 줄과 Windows 트레이 안내, 설치했고 로그인하지 않은 Windows PC에서 로그인 창이 한 번만 열림, 연결 화면의 G3(`MODE=enforce`일 때만), 로그아웃과 "내 목록에서 빼기"의 경고(서버 모드마다, 원격 세션 중), 세션 창에서 로그인했을 때 주 창이 알림을 보임.
5. 사용자가 정할 것: 데모 PC를 둘지와 어느 PC로 할지, App Store 배포 방식(Unlisted 또는 Custom App), 이용 비밀번호와 그 `UNTIL`, 정책 seq 4의 알림 글, 출시일.

그다음은 1차 작업 때 정한 대로입니다: `main`에 병합과 push, 서버 묶음, 허브-운영의 설치(`MODE=off`로 먼저), 사용자의 티켓 키 만들기, 1.8.0 세 플랫폼 빌드와 TestFlight, 출시일 순서(정책 seq 4 → 묶인 PC 업데이트 → seq 5 최소 1.8.0 → hbbs MIN=1.8.0 → `MODE=enforce`).
