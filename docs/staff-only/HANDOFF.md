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

PR을 받은 뒤 개발 PC의 세션이 합니다: 유출 검사, `scripts/ci.sh prepush`(server, client, deploy, e2e, wol, leak), 패치가 시험한 트리를 재현하는지 확인, `main`에 병합과 push, 서버 묶음, 허브-운영의 설치(`MODE=off`로 먼저), 사용자의 티켓 키 만들기, 1.8.0 세 플랫폼 빌드와 TestFlight, 출시일 순서(정책 seq 4 → 묶인 PC 업데이트 → seq 5 최소 1.8.0 → hbbs MIN=1.8.0 → `MODE=enforce`).
