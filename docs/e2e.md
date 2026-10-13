# 방화벽 E2E 검증

실제 HJViewer 클라이언트 두 대와 서버를 리눅스 한 대에서 띄웁니다. **회사 방화벽처럼 서버로 가는 HTTPS(TCP 443)만 허용한 네트워크**에서 접속, 데이터 전달, 화면 공유가 되는지 확인합니다. 1.6.0부터는 같은 구성에 계정 API를 더해 연세포탈 로그인과 같은 계정 기기끼리의 비밀번호 없는 연결을 확인하는 [계정 단계](#계정-단계-160)와, 무권한 네트워크 네임스페이스에서 NAS 중계기의 켜기를 확인하는 [원격 켜기 단계](#원격-켜기-단계-160)가 있습니다.

```
 netns hjv-pc (피제어 PC)            호스트 (서버)                      netns hjv-ctl (컨트롤러)
 ┌──────────────────────────┐        ┌──────────────────────────┐        ┌──────────────────────────┐
 │ HJViewer --server        │──443──▶│ Caddy :443 (TLS)         │◀──443──│ HJViewer (헤드리스)       │
 │ Xvfb 가상 화면            │        │  /ws/id    → hbbs        │        │  포트 포워딩·화면 캡처     │
 │ 테스트 웹페이지 :8099     │        │  /ws/relay → hbbr        │        │                          │
 │ 방화벽: 443 외 전부 차단  │        └──────────────────────────┘        │ 방화벽: 443 외 전부 차단  │
 └──────────────────────────┘                                            └──────────────────────────┘
```

## 검증 항목

1. **방화벽 동작 확인**: 컨트롤러 네트워크에서 hbbs의 기본 포트(TCP), WebSocket 포트 직접 접속, UDP, 인터넷이 모두 **막혀 있는지** 먼저 확인합니다.
2. **등록**: PC가 wss(443)로 등록해서 온라인이 되는지 확인합니다. 컨트롤러 네트워크에서 `hjv-probe`로 확인합니다.
3. **세션과 데이터**: 컨트롤러가 비밀번호로 로그인하고 포트 포워딩으로 PC 루프백(127.0.0.1:8099)의 웹페이지를 가져옵니다. 바이트가 신호 → 중계/P2P → 종단 간 암호화 세션을 모두 통과해야 성공합니다.
4. **화면 공유**: 원격 데스크톱 세션을 열고 첫 영상 프레임(VP9)을 디코딩합니다. 디코딩한 화면이 PC의 가상 화면(파란 배경 + 배너)과 같은지 픽셀로 확인합니다.

모드는 두 가지입니다.

| 모드 | 방화벽 | 기대 경로 |
|---|---|---|
| `strict` | 서버로 가는 TCP 443만 허용 | hbbr 중계 (`wss://…/ws/relay`) |
| `open` | 위 + 피어 간 UDP 허용 | WebRTC P2P (직접 연결) |

## 실행

개발 PC에서는 먼저 `scripts/ci.sh server`로 서버(hbbs, hbbr)와 `hjv-probe`를 빌드합니다. 그다음 `scripts/ci.sh e2e`가 나머지를 합니다. 패치를 적용한 클라이언트를 시험용으로 빌드하고(계정 단계면 hjv-api와 가짜 SSO도), Caddy를 고정한 SHA-256으로 받고, strict와 open을 차례로 돌립니다(sudo가 비밀번호 없이 되어야 함, [ci.md](ci.md)). 서버가 빌드되어 있지 않으면 `scripts/ci.sh server`를 먼저 하라고 하며 멈춥니다. 손으로 돌릴 때는 다음과 같습니다.

```bash
# 1) 서버와 진단 도구 빌드
scripts/fetch-upstream.sh
(cd upstream/rustdesk-server && cargo build --release)
(cd tools/hjv-probe && cargo build --release)

# 2) 테스트 서버 키 생성 (hbbs는 처음 실행할 때 키를 만듭니다)
export HJV_E2E_WORK=/tmp/hjv-e2e
mkdir -p $HJV_E2E_WORK/srv
HBBS=$PWD/upstream/rustdesk-server/target/release/hbbs
(cd $HJV_E2E_WORK/srv && timeout 2 $HBBS -p 41116; true)

# 3) 이 서버를 가리키는 테스트용 클라이언트 빌드 (hjv-e2e 기능은 제품 빌드에 들어가지 않습니다)
#    Ubuntu에는 libyuv.pc가 없으므로 직접 만들어 PKG_CONFIG_PATH로 지정합니다
mkdir -p $HJV_E2E_WORK/pkgconfig
printf 'libdir=/usr/lib/x86_64-linux-gnu\nincludedir=/usr/include\nName: libyuv\nDescription: libyuv\nVersion: 0\nLibs: -L${libdir} -lyuv\nCflags: -I${includedir}\n' \
  > $HJV_E2E_WORK/pkgconfig/libyuv.pc
(cd upstream/rustdesk && PKG_CONFIG_PATH=$HJV_E2E_WORK/pkgconfig HJV_SERVER=hjv.test \
   HJV_SERVER_KEY=$(cat $HJV_E2E_WORK/srv/id_ed25519.pub) \
   cargo build --release --features linux-pkg-config,hjv-e2e)

# 4) 실행 (sudo로 네트워크 네임스페이스와 iptables를 쓰고, 끝나면 정리합니다)
CADDY=/path/to/caddy tools/e2e/firewall-e2e.sh strict
CADDY=/path/to/caddy tools/e2e/firewall-e2e.sh open
# 세션 영상 비트레이트도 재려면 (정지 화면, 문서 스크롤, 화면 전체 변화)
HJV_E2E_BITRATE=1 CADDY=/path/to/caddy tools/e2e/firewall-e2e.sh strict
```

필요한 패키지(Ubuntu 24.04): `iptables xvfb imagemagick`, 그리고 upstream 클라이언트 빌드 의존성(`libgtk-3-dev libvpx-dev libyuv-dev libopus-dev libaom-dev libxdo-dev libpulse-dev libpam0g-dev clang nasm` 등)입니다. 전체 목록은 RustDesk CI를 참고하세요.

## 결과 (2026-10-10, WSL2 Ubuntu 24.04)

| 단계 | strict (443만 허용) | open (+ 피어 간 UDP) |
|---|---|---|
| 방화벽: hbbs 네이티브 포트·WS 포트 직접 접속·UDP·인터넷 | 모두 차단 확인 | UDP는 피어끼리만 허용, 나머지 차단 |
| `hjv-probe` (컨트롤러 네트워크) | 4/4 통과, PC `online` | 4/4 통과, PC `online` |
| 포트 포워딩으로 PC 페이지 가져오기 | 성공, **relay** (wss://…/ws/relay) | 성공, **direct (P2P)** |
| 원격 데스크톱 첫 화면(1280×720, VP9) | 수신·디코딩, PC 화면과 RMSE 0.070 | **WebRTC**로 수신, RMSE 0.070 |
| 결과 | **PASS** | **PASS** |

세션 비트레이트(1280×720, VP9, 중계): 정지 화면 약 0, 문서 스크롤 0.33Mb/s(19fps), 화면 전체 변화 30Mb/s(11fps). 서버 용량 산정은 [deploy.md](deploy.md)에 있습니다.

RMSE는 정규화 값입니다. 같은 화면이면 0.07~0.08(VP9 손실 압축), 다른 이미지면 약 0.56입니다.

```
== controller opens a remote desktop session and decodes the PC's screen
hjv-e2e: transport: relay
hjv-e2e: connected over WebSocket
hjv-e2e: logged in to hjv-pc (Linux 1.5.0)
hjv-e2e: screen 1280x720 written to …/run-strict/screen.ppm
screen: …/run-strict/screen.png, RMSE against the PC's desktop: 0.0701106
PASS (strict): transport: relay
```

### 이 검증으로 찾아서 고친 버그

- **hbbr 중계 짝 맺기 경쟁 상태**(upstream 버그): 양쪽이 거의 동시에(15µs 차이) 중계에 붙으면 둘 다 상대를 못 찾고 대기하다가, 세션이 30초 후 실패했습니다. `server/patches/0003`에서 고쳤고, 회귀 테스트(`tests/relay_pairing.rs`, 동시 도착 100쌍)는 패치 전 실패, 패치 후 통과합니다.

### 이 검증이 다루지 않는 것

- 실제 Windows/Android/iOS 기기. 같은 Rust 코어지만 플랫폼별 화면 캡처(DXGI/WGC)와 입력 주입, Flutter UI는 실기기 테스트가 필요합니다.
- 마우스·키보드 제어. 같은 세션 채널을 쓰지만 이 테스트는 화면 수신까지만 확인합니다.
- 실제 인터넷 경로(공유기 NAT, 통신사망)와 명시적 프록시 환경. 프록시는 hbb_common 단위 테스트로만 검증했습니다.

## 계정 단계 (1.6.0)

연세포탈 로그인과 같은 계정 기기끼리의 비밀번호 없는 연결을 위 방화벽 구성에서 실제 클라이언트, 실제 hjv-api, 패치한 hbbs로 확인합니다(설계 3판 7-4). `tools/e2e/firewall-e2e.sh`를 `HJV_E2E_ACCOUNT=1`로 돌리며, `scripts/ci.sh e2e`는 패치에 계정 코드가 있고 스크립트에 이 단계가 있으면 알아서 켭니다. 이 단계의 시나리오는 아직 돌리지 않았습니다. 계정 E2E 작업이 스크립트를 쓰고 결과를 이 절에 적습니다.

**구성**
- 443만 열린 네트워크 네임스페이스 `hjv-pc`(PC), `hjv-ctl`, `hjv-ctl2`(컨트롤러 둘), 선택으로 `hjv-pc2`(1.5.1 PC나 두 번째 1.6.0 PC).
- 호스트 루프백에 hbbs·hbbr(패치 0004), Caddy(계정 경로 추가), hjv-api(`--listen 127.0.0.1:41120`), 가짜 SSO `hjv-fake-sso`(127.0.0.1:41600, 시험 계정 `2026000001`, `2026000002`, `Y9001`). 방화벽 확인에 hjv-api 포트와 hbbs 콘솔을 더합니다. 실제 연세 SSO는 부르지 않습니다.
- hjv-api 시험 설정: `HJV_API_BIND_OS=windows,linux`, `HJV_API_TRUST_REFRESH=5`, `HJV_API_TRUST_TTL=30`, E2E 시험 키(`$HJV_E2E_WORK/api`, 출시 빌드의 공개키 확인이 거부하는 키). 비밀번호 없는 연결 스위치는 운영처럼 **꺼진 채로 시작**하고, S24가 `--owner-of`로 켠 뒤 끝에서 전체를 켭니다.
- 클라이언트는 시험 공개키(`HJV_LOGIN_SEAL_PUBKEY`, `HJV_TRUST_PUBKEYS`)를 넣어 빌드합니다. PC 쪽 실행 파일은 `/usr/local/lib/hjviewer-e2e/rustdesk`에 설치해 그 경로로 실행합니다. 리눅스는 `/usr` 아래의 실행 파일만 설치형으로 보고, hjv-api는 설치된 기기만 계정에 묶기 때문입니다.

**헤드리스 명령**(시험 기능 `hjv-e2e`에만 있음): `--hjv-e2e-login <ID> <pw>`, `--hjv-e2e-logout`, `--hjv-e2e-devices`, `--hjv-e2e-remove <id>`, `--hjv-e2e-keys`, `--hjv-e2e-revoke-key <ckid>`, `--hjv-e2e-option <키> [값]`. 로그인은 앱과 같은 Rust 경로(봉인, 계정 키, 요청 서명, 응답 관찰)를 쓰므로 PC에서는 로그인만으로 묶입니다. 화면 세션이 비밀번호 창을 띄우면 `hjv-e2e: password required <제목>`을 찍고 **종료 코드 3**, 'error' 창(키 바뀜 경고)이면 `hjv-e2e: error <제목>`을 찍고 **종료 코드 4**로 끝납니다. 비밀번호 없는 연결의 데이터 경로는 원격 화면(`--hjv-e2e-screenshot <ID> ""`)으로 봅니다. 포트 포워딩은 증명을 보내지 않으므로 S22에만 씁니다.

**시나리오**(아래 `""`는 비밀번호 없이 연 원격 화면. 시간 상한은 같은 실행의 비밀번호 연결 시간 + 2초)

| # | 단계 | 기대 |
|---|---|---|
| S0 | 기존 단계 전부 | 통과 |
| S24 | (맨 앞, 스위치 꺼짐) PC 로그인 → `""` → `admin passwordless on --owner-of <PC ID>` → `""` | 처음에는 `disabled`와 종료 3, 켠 뒤 갱신 주기 안에 성공 |
| S1–S3 | PC 로그인, 컨트롤러 로그인, 기기 목록 | PC가 묶이고 목록 `active`, 한글 성명, PC가 `pk`와 함께 목록에 나옴 |
| S4–S6 | `""`, 5초 스트림, PC 로그 | 비밀번호 창 없이 화면(RMSE < 0.2), 프레임 수신, `same-account login` 한 줄 |
| S7–S10 | 다른 계정(bob)의 컨트롤러 | 목록에 PC 없음, 증명 없이 비밀번호 창(종료 3), 비밀번호로는 성공, 틀린 비밀번호는 다시 입력 |
| S11 | 명단 밖 ID | 같은 실패 문구, 가짜 SSO 호출 0 |
| S12–S13b | hjv-api 멈춤(`kill -STOP`), 종료, 목록 수명(30초)을 넘긴 종료 | 받아 둔 목록으로 성공, 수명이 지나면 거절 문구와 종료 3 |
| S14 | 다른 컨트롤러가 다시 로그인(새 키) → 바로 `""` | 키 일련번호로 즉시 동기화해 성공 |
| S15 | PC `--server` 재시작, PC 사용자 토큰 삭제 | 허용 표시와 캐시로 성공 |
| S16, S16b | 다른 묶음 ID의 묶기 요청, 캐시·허용 파일 위조, 로그아웃 뒤 예전 묶기 요청 재전송 | 켜지지 않음(종료 3) |
| S17–S19b | 키 폐기, 다른 기기에서 PC 빼기, PC 로그아웃(온라인·hjv-api 꺼짐) | 갱신 주기 안에 종료 3, 로그아웃은 1초 안에 |
| S20 | 실행 폴더 전체에서 포탈 비밀번호·토큰·개인키 문자열 찾기 | 정해진 파일 밖에는 없음 |
| S21 | open 모드에서 S4 | WebRTC P2P로 성공 |
| S22 | 포트 포워딩 | 증명 없음, 종료 3 |
| S23 | 토큰만 복사해 다른 기기에서 목록 요청 | 401 |
| S25 | PC의 [끄기], 설정 다시 쓰기, 다시 로그인 | 1초 안에 종료 3, 설정을 다시 써도 그대로, 다시 로그인한 뒤에만 성공 |
| S26 | `admin binding revoke`와 별칭 `device remove` | 서명된 `revoked`로 허용 표시 삭제, 종료 3 |
| S27 | 컨트롤러 시계 +15분(`faketime`) | 첫 요청은 409 `clock`, 로그아웃 없음, 다음 명령부터 성공 |
| X1–X5 | 클릭 승인 모드, 1.5.1 PC·컨트롤러, PC에서 PC로, 임시 비밀번호 전용 PC | 각각 수락 대기, 멈춤 없는 비밀번호 창, 비밀번호 성공, 성공, 거절 |
| X6 | `passwordless off` | 갱신 주기 안에 종료 3 |
| X7, X7b | PC 기기 키를 바꿔 다시 등록 | 키 바뀜 경고(종료 4). 서버가 1분 안에 `needs-relogin`으로 바꾸고, 목록을 새로 받은 컨트롤러는 경고 없이 비밀번호 창. PC에서 다시 로그인하면 성공 |
| X8 | PC를 '내 기기만'으로(승인 password, 영구만, 비밀번호 없음) | 같은 계정은 성공, 다른 계정은 비밀번호 실패 |

- 예상 시간: 계정 단계 4~6분, `scripts/ci.sh e2e` 전체 9~13분(추정).
- 1.6.0 작업 중(2026-10-12)의 `scripts/ci.sh e2e` 실행에서는 open 모드도 중계로 연결됐습니다(위 2026-10-10 결과는 P2P). 지금 스크립트는 open 모드에서 P2P인지 확인하지 않으므로, S21 전에 원인을 먼저 봅니다.

## 원격 켜기 단계 (1.6.0)

NAS 중계기(hjv-relay)가 서버가 서명한 켜기 명령을 받아 잠든 PC에 매직 패킷을 보내는지 확인합니다(설계 3판 8-10). `tools/e2e/wol-e2e.sh`이고, `scripts/ci.sh wol`이 sudo 없이 돌립니다(서버와 `hjv-probe`는 `scripts/ci.sh server`로 먼저 빌드). 이 스크립트는 아직 없습니다. 원격 켜기 E2E 작업이 쓰고 결과를 이 절에 적습니다.

- **구성**: 사용자 네임스페이스 안에 스위치와 라우터, 랜카드 둘인 NAS, 잠든 PC(IP 없는 랜카드에서 프레임을 포착), 다른 LAN을 둡니다. 바깥에 hjv-api, hbbs, Caddy(시험 CA)를 둡니다. hjv-api 시험 설정은 `HJV_API_BIND_OS=windows,linux`입니다. PC 쪽 클라이언트는 무권한 사용자·마운트 네임스페이스(`unshare -Urm`) 안에서 `/usr/local/lib` 위에 tmpfs를 올리고 `hjviewer-e2e/rustdesk`로 복사해 실행합니다(설치형으로 보이게 하려는 것이고, 바깥의 `/usr`는 바뀌지 않음).
- **흐름**: alice로 로그인해 PC 묶기 → PC가 `--hjv-e2e-wol-report <MAC> <IPv4/접두사> <게이트웨이>`로 켜기 정보를 동기화에 실음 → hjv-relay가 등록 요청 → `admin relay approve <코드> --owner-of <PC ID>` → PC를 "잠재움"(클라이언트 종료, IP 삭제) → 앱처럼 토큰으로 켜기 요청 → 5초 안에 PC 랜카드에서 255.255.255.255와 NAS 랜카드의 브로드캐스트, 포트 9·7, 3번 = 12프레임이 잡히고 다른 LAN에는 0 → `sent` → PC를 "깨움" → 30초 안에 `confirmed`.
- **거부 사례**: 서명 없음, 다른 키, 만료, 다른 중계기 앞 명령, 명령 재사용, MAC 5개, 모르는 네트워크, 분당 11번째 → 모두 프레임 0과 이유. 다른 계정의 켜기 요청 → 404. 중계기가 꺼져 있으면 503.
- 예상 시간: 빌드를 빼고 1~2분. 시험 값은 문서용 주소(RFC 5737)와 MAC(`00:00:5e:00:53:xx`)만 씁니다.
