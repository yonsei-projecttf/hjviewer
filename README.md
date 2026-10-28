# HJViewer

모바일 앱(iOS/Android)에서 Windows PC를 원격으로 보고 제어하는 서비스입니다. 프로젝트TF가 만듭니다. **방화벽·NAT 뒤에서도 HTTPS(443)만으로 연결**되는 것이 핵심입니다.
[RustDesk](https://github.com/rustdesk/rustdesk) 클라이언트와 [rustdesk-server](https://github.com/rustdesk/rustdesk-server)를 포크해서 만듭니다.
1.6.0부터는 연세포탈 ID로 로그인하면 앱에 내 Windows PC 목록이 나오고, 같은 계정으로 로그인한 내 기기끼리는 PC 비밀번호 없이 연결하며, 꺼진 PC를 같은 네트워크의 NAS 중계기로 켭니다. 로그인은 연세 SSO를 거치고, 연세 SSO는 연세대학교 교직원 계정만 받습니다. 그 SSO를 통과한 사람은 처음 로그인할 때 저절로 등록되고, 운영자는 계정을 막을 수 있습니다(`hjv-api admin block`). 서버에서 `HJV_API_LOGIN=list`로 바꾸면 운영자가 미리 명단에 올린 ID만 로그인합니다. 운영자가 정한 최소 버전보다 낮은 앱은 연결이 거절되고, 새 버전은 앱 안에서 받습니다(서명된 버전 정책). 1.8.0부터 서버는 교직원 전용으로 둘 수 있습니다: 운영자가 출입 확인을 켜면 로그인한 앱과 계정에 묶인 PC, 그리고 운영자가 정한 이용 비밀번호를 넣은 쪽만 서버를 씁니다(출입증). 1.9.0부터는 로그인한 교직원이 앱에서 만든 일회용 지원 코드와 지원 링크로 로그인하지 않은 PC 하나가 2시간 동안 연결을 받고, 그 PC의 연결 창은 서명으로 확인한 "연세대 교직원(로그인 확인)" 표시를 보입니다. 내 계정의 기기에는 "같은 계정 확인됨" 표시가 붙고, 내 PC에 연결한 세션은 연결할 때 잠겨 있던 PC를 끝날 때 다시 잠그며, 로그아웃한 기기의 비밀번호 없는 연결은 끊깁니다. 설치한 PC는 들어온 연결의 목록과 원격 준비 점검을 보이고, 휴대폰 앱은 Face ID·지문·기기 암호로 여는 앱 잠금을 고를 수 있습니다(기본은 꺼짐).

- 설계와 동작 원리: [docs/architecture.md](docs/architecture.md)
- 서버 배포: [docs/deploy.md](docs/deploy.md)
- CI(개발 PC)와 앱 빌드: [docs/ci.md](docs/ci.md)
- 방화벽 E2E 검증과 결과: [docs/e2e.md](docs/e2e.md)
- 앱 배포와 업데이트(현재 Windows·Android 최신 1.9.2·최소 1.9.0, iOS 최소·최신 1.9.0, 정책 `e1-s7`), 1.9.0·1.9.1·1.9.2에서 사용자에게 달라지는 것, 스토어 자동화에 필요한 것: [docs/distribution.md](docs/distribution.md)
- 서명된 버전 정책을 만들고 확인하는 운영자 도구(hjv-release): [tools/hjv-release/README.md](tools/hjv-release/README.md)
- 서버에서 작업할 사람·에이전트용 배포 절차: [docs/server-runbook.md](docs/server-runbook.md) (1.6.0의 계정 API는 7절, 1.8.0의 교직원 전용 출입 제한은 9절)
- 교직원 전용 출입 제한의 형식(출입증, hbbs의 access.conf, 이용 비밀번호, 지원 코드)과 앱이 보이는 글: [docs/access-tickets.md](docs/access-tickets.md)
- 개인정보 처리방침: [docs/privacy.md](docs/privacy.md)
- 계정 API(hjv-api)의 설정, 관리 명령, 형식: [server/api/README.md](server/api/README.md)
- 원격 켜기 중계기(hjv-relay)의 NAS 설치와 규약: [docs/relay-dsm.md](docs/relay-dsm.md), [relay/PROTOCOL.md](relay/PROTOCOL.md)
- 원격 켜기로 켤 PC의 준비(랜카드, BIOS, 절전과 종료, 빠른 시작): [docs/wol-pc-setup.md](docs/wol-pc-setup.md)

## 현재 상태와 출시 이력 (2026-10-28)

현재 Windows·Android는 최신 **1.9.2**, 최소 **1.9.0**이고, iOS는 최소·최신 **1.9.0**입니다(정책 `e1-s7`, 2026-10-28). iOS의 1.9.2는 TestFlight에 있고, App Store에는 지침 3.2로 거절된 첫 제출 뒤의 다시 제출을 기다립니다. 정책의 iOS 항목은 App Store에 출시된 뒤에 올립니다. 운영 반영 상태는 2026-10-28 운영 에이전트 보고 기준입니다. 아래 1.6.x·1.7.0·1.8.0·1.9.0·1.9.1·1.9.2 행은 출시 이력입니다. 1.8.2로 만든 작업은 1.9.0에 들어 있고, 1.8.2는 따로 나가지 않았습니다.

| 항목 | 상태 |
|---|---|
| 개발 PC CI (`scripts/ci.sh`): 서버 패치의 신호·중계 테스트, 런북대로 한 배포 | 통과. Release 패키지의 systemd 유닛과 compose 구성을 nginx(새 사이트, HTTP/2 기존 사이트), Apache, Caddy 뒤에서 시험. WebSocket, 등록, 중계, 키, 권한, 헤더 위조 차단 확인 |
| 앱 빌드: Windows x64 / Android arm64 / iOS | 세 플랫폼 1.9.2 출시 빌드 완료(`c3b20f7`, 태그 `client-1.9.2`). iOS는 TestFlight에 업로드(빌드 263010510). 공개 소스의 `client/version.env`가 앱 버전의 기준입니다. GitHub CI는 직접 실행할 때만 돕니다. |
| 방화벽 E2E: 실제 클라이언트, 서버로 가는 443만 허용 | 등록, 비밀번호 로그인, 데이터, **원격 화면** 모두 통과 (중계). UDP를 허용하면 WebRTC P2P로 통과 |
| 운영 | 운영 서버 배포 완료(런북 B 경로: systemd, 기존 웹 서버에 include). 실제 서버 값의 앱은 비공개 빌드 저장소에서 빌드하고, iOS는 TestFlight 내부 테스트로 배포 |
| 1.6.x: 연세포탈 로그인, 내 기기 목록, 비밀번호 없는 연결, 원격 켜기, 최소 버전과 업데이트(1.6.0의 기능을 1.6.1로 출시) | 2026-10-14에 1.6.1로 출시: 태그 `client-1.6.1`(Android 최소 7.0, 로그인 오류 문구), 세 플랫폼 출시 빌드, iOS는 TestFlight. 운영 서버: 계정 API와 nginx 전환, 첫 버전 정책(최소 1.6.1, 유예 없음), 문 2(2026-11-30까지), hbbs가 1.5.x 컨트롤러를 거절(2026-10-14 23:43부터), hjv-api `server-2026.10.14.3`. NAS 중계기는 `relay-1.6.0`으로 공개하고 2026-10-15에 설치·승인. 아이폰은 1.6.2(한글 입력 고침, 태그 `client-1.6.2`, 2026-10-15)를 TestFlight로. 들어 있는 것: 계정 API hjv-api(계정 키, 요청 서명, PC 동기화와 신뢰 목록, 켜기 API, 감시·백업, 열린 로그인과 막기, 최고운영자의 사용자·PC 보기, 새 버전 내려받기), hbbs 패치 0004·0005(최소 컨트롤러 버전), NAS 중계기 hjv-relay와 DSM 설치 안내, 운영자 도구 hjv-release와 신뢰 규칙의 시험 벡터, 서버의 버전 정책 확인 스크립트와 nginx 위치, 앱의 계정 기능·켜기 메뉴(클라이언트 패치 0030–0036), 앱의 최소 버전과 업데이트(서명된 정책, PC의 컨트롤러 거절, Windows 무인 설치, 업데이트 화면, 사이드로드 Android의 앱 안 설치), Android 제어 전용, 최고운영자 화면, 만든 곳(프로젝트TF)과 시범 안내, 스플래시(앱 바탕색 위의 아이콘과 '연세대학교 프로젝트TF' 줄)(클라이언트 패치 0037–0045), Windows 패커가 `taskkill`을 Windows 시스템 폴더에서 실행함(클라이언트 패치 0046). 2026-10-14의 통합에서 최소 버전·열린 로그인·최고운영자 화면·Play 업로드·CI·스플래시의 묶음을 합침. 배포 절차: [docs/server-runbook.md](docs/server-runbook.md) 7절, 8절. 남은 일은 아래 줄 |
| 1.7.0: 새 화면(세 플랫폼) | 2026-10-15 구현과 빌드: 태그 `client-1.7.0`(4c3baaf), 비공개 빌드 저장소의 출시 실행 38724716986에서 세 플랫폼 빌드, iOS는 같은 실행이 TestFlight에 올림(빌드 262881109). 정책 seq 3(세 플랫폼 최신 1.7.0, 최소 1.6.1 그대로, Windows 무인 설치 2026-10-17 22:13부터)은 2026-10-16에 서버에 넣음. 문 2도 그때부터 1.7.0을 내줌. 앱 아이콘의 파랑 하나를 강조색으로 쓰는 차분한 색 체계, 얇은 테두리의 카드, 둥근 모서리, 글꼴 Pretendard(SIL OFL 1.1, `client/overlay`의 `flutter/assets/fonts`; 고지는 NOTICE와 빌드의 THIRD_PARTY_NOTICES). 홈(ID·비밀번호 카드, 연결 상자, PC 카드), 목록 탭과 빈 화면, 데스크톱 설정·창 막대·메뉴·설치 화면·연결 관리 창, 휴대폰 설정, 대화상자·로그인·계정·업데이트 화면, 원격 세션 도구 모음(클라이언트 패치 0050–0057). 테마는 RustDesk의 정의를 그대로 두고 `hjv_theme.dart`의 `hjvRestyle`로 덧입힘. 개발 PC의 Linux 미리보기(Xvfb)로 데스크톱·휴대폰 화면을 밝은/어두운 테마에서 확인했고, 원격 세션 화면과 로그인이 필요한 화면은 코드와 시험용 하네스로만 봄. 사용자가 2026-10-15에 기기에서 확인함(운영자 PC의 설치본은 1.6.1 그대로) |
| 1.8.0: 교직원 전용 출입 제한 | `staff-only-gate`를 main에 통합. 출입증·이용 비밀번호·심사 계정과 앱 안내를 포함합니다([계약](docs/access-tickets.md), [운영 절차](docs/server-runbook.md)). 2026-10-18에 패키지 → 정책 `e1-s4` → hbbs 최소 버전 설정 순서로 반영했습니다. Windows·iOS·Android 모두 최소·최신 1.8.0, 유예 없음. hbbs는 `MIN=1.8.0`, `FROM=2026-10-18T06:53:56Z`. 문 2 종료는 기존대로 2026-11-30 00:00 KST입니다. 2026-10-19에 서버 묶음 `server-2026.10.17`(출입증을 아는 hjv-api, hbbs, hbbr)을 넣고, 사용자가 출입증 키 1번을 만들고, 이용 비밀번호(2027-01-15 00:00 KST까지)를 넣은 뒤 16:39:56(KST)에 출입 확인을 켰습니다(`MODE=enforce`, 런북 9-6). 로그인하지 않은 기기는 이용 비밀번호가 있어야 연결하고 연결을 받습니다. 심사용 데모 PC는 아직 없습니다. |
| 1.9.0: 지원 링크와 연결 창의 표시, 세션 끝의 잠금, 들어온 연결, 원격 준비, UI/UX 검토 | 2026-10-20에 출시: 태그 `client-1.9.0`(2404ffa), 비공개 빌드 저장소에서 Windows → (사용자의 Windows 11 확인) → Android → iOS(TestFlight) 순서로 한 번씩 빌드. 운영 서버는 같은 날 서버 묶음 `server-2026.10.20`(hjv-api와 nginx의 `/api/hjv/s/` 줄, 13:48~13:50 KST) → 정책 `e1-s5`(세 플랫폼 최소·최신 1.9.0, 유예 없음, 17:01:43 KST) → hbbs `MIN=1.9.0`(17:28:47 KST) 순서로 반영했습니다(런북 8-2, 8-3, 9-9의 기록). 클라이언트 패치 0065–0148(0148은 E2E 빌드만, `ed1515e`). 1.8.2로 만든 작업(UI/UX 검토의 P1 항목과 U69, 교직원의 지원 코드 W1, 휴대폰 앱 잠금 W13)이 들어 있고, 1.8.2는 따로 나가지 않습니다. 1.9의 기능: PC 연결 창의 같은 계정·교직원 표시와 사칭 주의 줄, 녹화 표시(TV-W2), 지원 링크·대기실·교직원 표시(TV-W6), 세션이 끝날 때의 잠금과 화면 가리기(TV-W3), 원격 준비 카드와 G4(TV-W4), 로그아웃한 기기의 비밀번호 없는 연결 끊기(TV-W5), 휴대폰 세션의 제스처 안내와 파일 전송 바꾸기(TV-W7), 들어온 연결 목록(TV-W9), 연결 전의 이용 비밀번호 A0와 링크 확인 LK1(UX-G1, TV-C6), 트레이 경고(UX-G2), 홈의 계정 줄(UX-H1), 말과 모양의 정리(UX-W1·W2, UX-S1–S4), 키보드·화면 낭독기·Windows 11 스냅 레이아웃·휴대폰 글자 크기(UX-K1–K4), 세션 창의 모양(U15). 서버 쪽은 hjv-api의 지원 링크·대기실·지원 확인과 내 PC 목록의 `synced`·`wake`, nginx의 `/api/hjv/s/` 한 줄입니다(런북 9-9). 출시 빌드는 확인 빌드 없이 한 번씩 했습니다. 사용자에게 달라지는 것: [docs/distribution.md](docs/distribution.md)의 "1.9.0: 사용자에게 달라지는 것" |
| 1.9.1: 설정과 고급 설정, 앱 안의 계정 삭제, App Store 준비 | 2026-10-27에 출시: 태그 `client-1.9.1`(b389ea2), 비공개 빌드 저장소에서 Windows → (사용자의 Windows 확인) → Android → iOS(TestFlight) 순서로 한 번씩 빌드. 운영 서버는 서버 묶음 `server-2026.10.27`(hjv-api, 00:14 KST) → 정책 `e1-s6`(Windows·Android 최신 1.9.1, 최소 1.9.0 그대로, iOS 그대로, Windows 무인 설치 2026-10-27 22:00 KST부터, 09:18:43 KST) 순서로 반영했습니다(런북 9-10, 8-2의 기록). hbbs는 그대로(`MIN=1.9.0`). `client/version.env` 1.9.1, 클라이언트 패치 0149–0160, 하한 `HJV_POLICY_FLOOR=1:5`. 데스크톱 설정을 일상 탭(계정, 일반, 보안, 디스플레이)과 고급 설정으로 나눔(휴대폰은 고급 설정 머리), 앱 안의 계정 삭제(App Store 지침 5.1.1(v), hjv-api `POST /api/hjv/account/delete`, 런북 9-10), 시범 안내를 모든 곳에서 뺌(정식 운영), App Store 설치본의 App Store 업데이트, 개인정보 매니페스트 7유형, 휴대폰의 `rustdesk://`와 QR 화면, 카메라·사진 권한을 뺌, 정보 화면의 링크를 시스템 브라우저로('개인정보 처리방침'), 터미널의 '(베타)'를 뺌. 운영 순서는 1.9.1의 hjv-api를 먼저 넣고, 앱과 App Store 심사는 그 뒤입니다. 1.9.1부터 스토어에 닿는 일(Play 업로드, App Store 준비·제출·출시, 상태 확인)은 빌드 실행이 아니라 손으로 시작하는 스토어 실행이 하고, 스토어에 나간 것은 비공개 저장소의 출시 기록에 남습니다([docs/store-release.md](docs/store-release.md)). 사용자에게 달라지는 것: [docs/distribution.md](docs/distribution.md)의 "1.9.1: 사용자에게 달라지는 것" |
| 1.9.2: '로그인이 풀렸습니다' 안내, RustDesk의 페이지로 가는 단추 없음 | 2026-10-28에 출시: 태그 `client-1.9.2`(c3b20f7), 비공개 빌드 저장소의 빌드 실행 40636035123에서 세 플랫폼을 한 번에 빌드(25분), iOS는 TestFlight 업로드 실행 40640292457이 올림(빌드 263010510). 운영 서버에는 1.9.2의 패키지와 정책 `e1-s7`(Windows·Android 최신 1.9.2, 최소 1.9.0 그대로, iOS 그대로, Windows 무인 설치 2026-10-30 14:51 KST부터, 16:34 KST)을 넣었습니다(런북 8-2의 기록). 서비스 재시작과 nginx reload는 없었고, hbbs는 그대로(`MIN=1.9.0`). `client/version.env` 1.9.2, 클라이언트 패치 0162–0167(0162는 E2E 빌드만), 하한 `HJV_POLICY_FLOOR=1:6`. App Store는 지침 3.2로 거절된 첫 제출 뒤의 다시 제출을 기다립니다. 사용자에게 달라지는 것: [docs/distribution.md](docs/distribution.md)의 "1.9.2: 사용자에게 달라지는 것" |
| 남은 일 | Microsoft Store 제출 형식·코드 서명 경로 결정과 심사, 추가 테스터 검증. Google Play는 비공개 테스트 트랙 Alpha에 1.9.0이 있고(소유자가 손으로 출시), 그다음 판부터 스토어 실행이 Alpha에 올립니다. App Store의 첫 버전은 등록되지 않은 앱이고 수동 출시입니다(store-release.md의 "첫 App Store 버전만"). 첫 제출은 지침 3.2로 거절되었고, 다시 낼 판은 1.9.2(빌드 263010510)이며 App Store Connect에서 손으로 냅니다(2026-10-28 결정, [store/README.md](store/README.md)의 "심사 메모"). 스토어 실행: [docs/store-release.md](docs/store-release.md). 루트 키 연습은 기존 보류 상태입니다. 출입 제한은 2026-10-19부터 켜져 있습니다. 남은 것: 1.9.2의 hjv-api(런북 9-11) → 심사용 데모 PC 묶기(런북 7-17, `review.conf`의 `PC=`), 2027-01-15 전 이용 비밀번호 갱신, 2026-11-30 전 데모 계정과 문 2의 기한 정리. 첫 스토어 실행(Google Play Alpha 업로드, 손으로 낸 1.9.2 뒤의 App Store 제출. 스크린샷은 낼 판의 화면으로 다시 찍기)과 그 뒤의 스토어 정책. App Store 배포를 시작하면 정책의 iOS 항목은 App Store에 출시된 버전으로만 올립니다(런북 8-2). |

## 빠른 시작: 실제 기기로 연결하기

1. **서버**: 도메인 하나와 리눅스 VM에서 `deploy/`를 실행합니다(80/443만 공개). → [docs/deploy.md](docs/deploy.md). 이미 nginx나 Apache가 80/443을 쓰는 서버는 Docker 없이 Release의 hbbs/hbbr을 systemd로 띄웁니다. → [docs/server-runbook.md](docs/server-runbook.md)의 B 경로
   ```bash
   cd deploy && cp .env.example .env   # HJV_DOMAIN=rd.example.com
   sudo docker compose build && sudo docker compose up -d && sudo cat data/id_ed25519.pub
   ```
2. **앱 빌드**: 서버 값을 저장소 변수로 넣고 GitHub Actions의 client 워크플로를 실행합니다. GitHub CI는 지금 일시 중지 상태라 직접 실행할 때만 돕니다. → [docs/ci.md](docs/ci.md)
   ```bash
   gh variable set HJV_SERVER --body rd.example.com
   gh variable set HJV_SERVER_KEY --body '<data/id_ed25519.pub 내용>'
   gh workflow run client-build.yml -f platforms=all
   ```
   계정 기능(1.6.0)은 서버에 계정 API를 더하고([docs/server-runbook.md](docs/server-runbook.md) 7절) 그 공개키 `HJV_LOGIN_SEAL_PUBKEY`, `HJV_TRUST_PUBKEYS`를 빌드 변수로 넣은 앱에만 있습니다([docs/ci.md](docs/ci.md)의 "계정 기능의 공개키"). 넣지 않으면 1.5.1처럼 ID와 비밀번호로만 연결합니다. 로그인은 연세 SSO를 쓰므로 연세대 배포에서만 됩니다.
3. **설치**
   - **PC(Windows)**: 아티팩트 `HJViewer-…-windows-x86_64`(zip) 안의 `HJViewer.exe`를 실행하면 화면에 ID와 비밀번호가 표시됩니다. 상시 접속이 필요한 PC는 `HJViewer-install.exe`로 설치합니다. 설치하면 PC를 켤 때 Windows 로그인 전부터 자동으로 실행됩니다. 사람 없이 접속하려면 영구 비밀번호를 정하세요([docs/distribution.md](docs/distribution.md)의 "Windows 피제어 PC 설치"). 서명이 없어서 SmartScreen 경고가 뜰 수 있습니다. 1.6.0 계정 빌드에서는 설치한 PC에서 연세포탈 ID로 로그인하면 그 PC가 내 PC 목록에 오르고, 같은 계정으로 로그인한 내 휴대폰과 PC가 비밀번호 없이 연결할 수 있게 됩니다.
   - **Android**: `…-android-aarch64*.apk`를 설치합니다. 1.6.0부터 Android 앱은 다른 기기를 제어하기만 합니다.
   - **iOS**: 서명 시크릿을 넣고 빌드한 IPA를 TestFlight나 기기에 설치합니다. 서명 없는 IPA는 설치할 수 없습니다.
4. **연결**: 앱에서 PC의 ID를 입력하고 비밀번호로 접속합니다. 1.6.0 계정 빌드에서 같은 계정으로 로그인했다면 "내 PC" 탭의 PC를 눌러 비밀번호 없이 연결합니다. 출입 확인을 켠 서버(1.8.0)에서는 로그인해 두거나, 거절 창에서 이용 비밀번호를 넣습니다. 로그인하지 않은 PC는 이용 비밀번호나, 1.9.0부터는 교직원에게 받은 지원 코드를 넣어야 연결을 받습니다([docs/distribution.md](docs/distribution.md)의 "1.8.0: 교직원 전용").
5. **방화벽 점검**: 고객사 네트워크에서 `tools/hjv-probe`로 서버까지의 경로를 확인합니다.

## 저장소 구조

upstream 소스는 저장소에 넣지 않습니다. **고정 커밋 + 패치 파일**로 관리해서 upstream 업데이트를 따라가기 쉽게 했습니다.

```
scripts/upstream.env         고정한 upstream 커밋 (rustdesk 1.5.0, rustdesk-server 1.1.17)
scripts/fetch-upstream.sh    upstream/ 에 소스를 받고 패치 적용 (hjviewer 브랜치)
scripts/export-patches.sh    upstream/ 의 커밋을 다시 패치 파일로 내보내기
scripts/ci.sh                개발 PC의 CI: 서버 패치·hjv-api·hjv-relay 테스트, 런북대로 한 배포 시험(Docker, systemd),
                             앱 패치 테스트, 방화벽·원격 켜기 E2E, push 전 검사(leak) (docs/ci.md)
scripts/leak-check.py        push 전 검사: 로컬 목록(build/leak-patterns.txt)의 실제 값이 올라갈 내용에 없는지
scripts/check-build-keys.py  앱·중계기 빌드에 고정하는 공개키(계정 기능, 업데이트 키 client/update-keys.env)의 형식과 시험 키 차단, 지문
scripts/release-info.py      앱 결과물마다 release.json과 SHA256SUMS(정책의 출처 확인이 읽음), 1.6.0부터 Android 매니페스트 확인
scripts/check-api-settings.py hjv-api 운영 설정값 검사
scripts/package-server.sh    서버 Release 패키지: systemd로 띄울 hbbs/hbbr, hjv-api, 유닛, 웹 서버 설정, 라이선스 고지
scripts/package-relay.sh     NAS 중계기 hjv-relay의 묶음(정적 실행 파일, 고지, 따로 쓰는 SHA256SUMS)
scripts/package-notices.sh   앱·서버 결과물에 넣는 고지 파일 (LICENSE, NOTICE, SOURCE, THIRD_PARTY_NOTICES)
scripts/set-client-version.sh 빌드 전에 client/version.env의 앱 버전을 Cargo.toml·Cargo.lock·pubspec.yaml에 적기
scripts/make-icons.sh        client/icons의 SVG 원본으로 client/overlay의 아이콘·스플래시 파일을 다시 만들기
server/api/                  hjv-api: 계정 API (연세포탈 로그인, 내 PC 목록, 계정 키, PC 동기화와 신뢰 목록, 켜기 명령)
relay/                       hjv-relay: NAS에서 도는 원격 켜기 중계기와 서버와의 규약(PROTOCOL.md)
server/patches/              hbbs/hbbr 패치 (TCP/WS 등록, WebRTC 시그널링 중계, 중계 짝 맺기 경쟁 상태 수정,
                             계정 API용 hjv-pk 콘솔 명령과 중계 요청 정리, 최소 컨트롤러 버전, 출입증 확인)
client/patches/rustdesk/     앱 패치 (서버 고정, WebSocket+WebRTC 기본, 브랜딩, E2E용 헤드리스 명령, 보안 강화,
                             Windows 실행 파일 이름, 마우스·터치·키보드 입력과 한글 입력 개선,
                             1.6.0의 연세포탈 로그인과 같은 계정 기기끼리 비밀번호 없는 연결, 최소 버전과 업데이트,
                             Android 제어 전용, 최고운영자 화면, 스플래시, 1.8.0의 출입증과 이용 비밀번호 화면,
                             1.9.0의 지원 코드·지원 링크와 연결 창의 표시, 세션 끝의 잠금, 들어온 연결,
                             원격 준비, 휴대폰 앱 잠금, UI/UX 검토의 다듬기)
client/patches/hbb_common/   공용 라이브러리 패치 (WebSocket 연결의 HTTP/SOCKS5 프록시 지원, TCP_NODELAY, RegisterPk의 출입증 필드)
client/overlay/rustdesk/     패치 대신 통째로 덮어쓰는 파일 (아이콘·스플래시 이미지, scripts/make-icons.sh로 생성)
client/icons/                아이콘·스플래시 SVG 원본 (스플래시 글자는 scripts/make-splash-text.py로 만듦)
client/version.env           앱 버전 (사용자에게 나눠 줄 새 빌드마다 올림)
client/update-keys.env       앱이 믿는 업데이트 공개키와 정책 하한 (공개 파일, 빌드가 이 파일에서만 읽음)
client/play/                 Google Play 번들에서 사이드로드 전용 항목을 빼는 매니페스트
deploy/                      서버 구성: Docker Compose + Caddy(443, 자동 TLS), 또는 기존 nginx/Apache 뒤의 systemd 서비스(deploy/systemd).
                             계정 API의 nginx 위치(nginx-hjviewer-api.conf)와 그 한도(nginx-hjviewer-api-zones.conf), 유닛(hjviewer-api*),
                             hbbs의 출입 파일(access.conf.example), 이용 비밀번호와 심사 계정 파일의 도구(guest-conf.py, review-conf.py)
tools/hjv-probe/             방화벽 진단 도구: 이 네트워크에서 서버까지 연결되는지 단계별로 확인
tools/e2e/                   방화벽 E2E: 443만 허용하는 네임스페이스 두 개에서 실제 클라이언트로 접속·화면 수신 검증 (docs/e2e.md)
                             원격 켜기 E2E: sudo 없이, NAS 중계기가 잠든 PC의 랜카드에 보내는 매직 패킷과 켜짐 확인까지 (같은 문서)
tools/hjv-load/              부하 측정 도구: 상시 등록 PC와 중계 세션을 흉내 내어 서버 용량 확인 (docs/deploy.md 용량 산정)
tools/hjv-release/           운영자 도구: 서명된 버전 정책(최소 버전과 강제 업데이트)을 만들고 서명하고 확인. 신뢰 규칙의
                             참조 모델(model.py)과 앱·도구·서버 확인이 함께 읽는 시험 벡터(tests/vectors)
upstream/                    (git 제외) 패치가 적용된 작업 사본
```

## 개발 흐름

```bash
scripts/fetch-upstream.sh            # upstream/rustdesk-server, upstream/rustdesk 생성
cd upstream/rustdesk-server          # 수정 → hjviewer 브랜치에 커밋
cargo test --release --test tcp_signaling
cd ../.. && scripts/export-patches.sh server   # 패치 파일 갱신 후 이 저장소에 커밋
scripts/ci.sh                        # 커밋 전에 개발 PC의 CI
scripts/ci.sh client                 # 앱 패치(client/patches)를 바꿨다면 이것도
```

서버 빌드에는 Rust와 `pkg-config`, `libssl-dev`가 필요하고, `scripts/ci.sh`의 배포 시험에는 운영 서버와 같은 Docker와, hjv-relay 묶음을 만들 Rust의 `x86_64-unknown-linux-musl` 타깃과 `musl-gcc`(`musl-tools`)가 필요합니다([docs/ci.md](docs/ci.md)). 앱(Windows/Android/iOS)은 네이티브 의존성이 많아서 GitHub Actions 워크플로로 빌드합니다. 지금은 GitHub CI를 일시 중지해서 직접 실행할 때만 돕니다.

## License / 라이선스

HJViewer, made by 프로젝트TF, is a modified version of [RustDesk](https://github.com/rustdesk/rustdesk) and [rustdesk-server](https://github.com/rustdesk/rustdesk-server) (Copyright Purslane Tech Pte. Ltd., open-trade and the RustDesk contributors). It is licensed under the GNU Affero General Public License v3.0 only ([LICENSE](LICENSE)), as RustDesk is, and comes with no warranty. The account service (hjv-api, `server/api`) and the Wake-on-LAN relay (hjv-relay, `relay/`) are programs written in this repository, under the same license. [NOTICE](NOTICE) lists the upstream versions and what HJViewer changes; every downloadable build ships its license, source location and third-party notices (TestFlight builds carry only the in-app source link). HJViewer is not affiliated with or endorsed by RustDesk.

HJViewer는 프로젝트TF가 만듭니다. RustDesk가 AGPL-3.0-only이므로 이 저장소도 AGPL-3.0-only입니다. 이 저장소에서 새로 쓴 계정 API(hjv-api, `server/api`)와 NAS 중계기(hjv-relay, `relay/`)도 같습니다. 앱이나 서버를 쓰는 사람에게 수정한 소스를 제공해야 하고, 이 공개 저장소가 그 소스입니다. 계정 API는 `https://<서버>/api/hjv/source`로 라이선스와 소스 위치를 알립니다. 사람들에게 나눠 주는 빌드는 반드시 이 저장소에 올라간 커밋으로 만들고, 결과물에 들어 있는 LICENSE·NOTICE·SOURCE·THIRD_PARTY_NOTICES 파일도 함께 전달하세요. 나눠 준 빌드의 커밋에는 `client-<앱 버전>` 태그를 붙입니다. TestFlight 빌드에는 이 파일들이 없고 앱 정보 화면의 소스 링크만 있으므로, 내부 테스터가 아닌 사람에게 나눠 주기 전에 고지를 따로 전달하세요. 앱 이름과 아이콘은 HJViewer 것입니다. 앱 정보 화면에 RustDesk의 저작권 줄이 있고, "Powered by RustDesk"(RustDesk 제공) 링크는 데스크톱과 앱에서 기본으로 숨깁니다. Windows 실행 파일은 `HJViewer.exe`입니다. 휴대폰은 1.9부터 `hjviewer://`를 쓰고, 옛 `rustdesk://`는 1.9.0까지만 함께 받았습니다(1.9.1에서 뺌). Android 패키지 안쪽 같은 일부 내부 이름은 아직 rustdesk입니다. 개인정보 처리방침: [docs/privacy.md](docs/privacy.md)
