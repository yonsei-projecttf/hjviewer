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

`open`에서는 호스트가 두 네임스페이스 사이의 UDP를 넘겨줘야 합니다. Docker가 깔린 호스트는 iptables `FORWARD` 정책이 DROP이라 이 UDP가 버려지고, 세션은 조용히 중계로 붙습니다. 그래서 스크립트는 `open`으로 도는 동안 두 veth 사이의 UDP만 허용하는 `FORWARD` 규칙 둘을 넣고, 끝나면 지웁니다.

## 실행

개발 PC에서는 먼저 `scripts/ci.sh server`로 서버(hbbs, hbbr)와 `hjv-probe`를 빌드합니다. 그다음 `scripts/ci.sh e2e`가 나머지를 합니다(sudo가 비밀번호 없이 되어야 함, [ci.md](ci.md)). 서버가 빌드되어 있지 않으면 `scripts/ci.sh server`를 먼저 하라고 하며 멈춥니다.

- 패치를 적용한 클라이언트를 새로 받아 시험용으로 빌드하고, Caddy를 고정한 SHA-256으로 받고, PC 쪽 클라이언트를 `/usr/local/lib/hjviewer-e2e/rustdesk`에 설치한 뒤 strict와 open을 차례로 돌립니다. 끝나면 설치한 파일을 지웁니다.
- 패치에 계정 코드가 있으면 [계정 단계](#계정-단계-160)도 돕니다. 계정 코드의 단위 시험, hjv-api와 가짜 SSO 빌드, 태그 `client-1.5.1`로 만드는 1.5.1 비교용 클라이언트(태그와 서버 키가 같으면 한 번만 빌드해 `build/ci/e2e-old/rustdesk`에 둠), Ubuntu의 `libfaketime` 패키지 풀기(`build/ci/faketime`, 설치하지 않음)가 더해집니다.
- 시험용 클라이언트는 LTO 없이, 코드 생성 단위 16개로 빌드합니다. 트리를 새로 받으므로 클라이언트 크레이트는 실행마다 다시 컴파일됩니다(의존성은 `build/ci/target-e2e`에 남음). 릴리스 프로필 그대로면 실행 파일과 단위 시험이 각각 3분 반쯤 걸리고, 이렇게 하면 각각 45초쯤입니다. 제품 빌드의 프로필은 그대로입니다.

손으로 돌릴 때는 다음과 같습니다.

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

# 5) 계정 단계 (1.6.0). 시험 키는 scripts/ci.sh e2e가 build/ci/e2e/api에 만들어 둔 것을 씁니다
#    (반복 바이트 seed라 출시 빌드의 공개키 확인이 거부하는 키). 그 공개키를 넣어 3)을 다시 빌드하고,
#    PC 쪽 실행 파일은 /usr 아래에 설치합니다
KEYS=$PWD/build/ci/e2e/api
(cd server/api && cargo build --release --features fake-sso)
(cd upstream/rustdesk && PKG_CONFIG_PATH=$HJV_E2E_WORK/pkgconfig HJV_SERVER=hjv.test \
   HJV_SERVER_KEY=$(cat $HJV_E2E_WORK/srv/id_ed25519.pub) \
   HJV_LOGIN_SEAL_PUBKEY=$(cat $KEYS/seal.pub) HJV_TRUST_PUBKEYS=$(cat $KEYS/trust.pub) \
   cargo build --release --features linux-pkg-config,hjv-e2e)
sudo install -D -m 755 upstream/rustdesk/target/release/rustdesk /usr/local/lib/hjviewer-e2e/rustdesk
HJV_E2E_ACCOUNT=1 HJV_E2E_KEYS=$KEYS PC_CLIENT=/usr/local/lib/hjviewer-e2e/rustdesk \
  HJV_API=$PWD/server/api/target/release/hjv-api HJV_FAKE_SSO=$PWD/server/api/target/release/hjv-fake-sso \
  CADDY=/path/to/caddy tools/e2e/firewall-e2e.sh strict     # open은 S21만 돕니다
sudo rm -rf /usr/local/lib/hjviewer-e2e
#    선택: CLIENT_OLD=<1.5.1 클라이언트>(X2, X3), FAKETIME_LIB=<libfaketime.so.1>(S27). 없으면 그 줄만 건너뜁니다
```

필요한 패키지(Ubuntu 24.04): `iptables xvfb imagemagick`(계정 단계는 `curl python3`도), 그리고 upstream 클라이언트 빌드 의존성(`libgtk-3-dev libvpx-dev libyuv-dev libopus-dev libaom-dev libxdo-dev libpulse-dev libpam0g-dev clang nasm` 등)입니다. 전체 목록은 RustDesk CI를 참고하세요.

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

연세포탈 로그인과 같은 계정 기기끼리의 비밀번호 없는 연결을 위 방화벽 구성에서 실제 클라이언트, 실제 hjv-api, 패치한 hbbs로 확인합니다(설계 3판 7-4). `tools/e2e/firewall-e2e.sh`를 `HJV_E2E_ACCOUNT=1`로 돌리며, `scripts/ci.sh e2e`는 패치에 계정 코드가 있으면 알아서 켭니다. strict에서 아래 시나리오 전부를, open에서 S21을 돌립니다.

**구성**
- 443만 열린 네트워크 네임스페이스 넷: `hjv-pc`(PC), `hjv-ctl`·`hjv-ctl2`(컨트롤러), `hjv-pc2`(1.5.1 PC, 그다음 두 번째 1.6.0 PC). 네임스페이스마다 machine-id를 따로 줍니다. hjv-api가 한 계정의 기기를 앱의 uuid로 구분하기 때문입니다(같으면 한 기기의 다시 로그인으로 보고 앞 키를 폐기함).
- 호스트 루프백에 hbbs·hbbr(패치 0004), Caddy(`deploy/Caddyfile`의 계정 경로), hjv-api(`--listen 127.0.0.1:41120`), 가짜 SSO `hjv-fake-sso`(127.0.0.1:41600). 방화벽 확인에 hbbs 콘솔, hjv-api, 가짜 SSO 포트를 더합니다. 실제 연세 SSO는 부르지 않습니다. 계정 단계의 Caddy는 `debug`로 띄워, hjv-api에 넘긴 요청마다 돌아온 상태 코드가 `caddy.log`에 남습니다(Authorization 값은 Caddy가 가림). S26이 PC의 동기화 응답을 여기서 봅니다.
- 시험 계정은 가짜 SSO의 것입니다: alice `2026000001`(홍길동), bob `2026000002`, 명단에 넣지 않는 `Y9001`.
- hjv-api 시험 설정: `HJV_API_BIND_OS=windows,linux`, `HJV_API_TRUST_REFRESH=5`, `HJV_API_TRUST_TTL=30`, E2E 시험 키. 한 계정이 몇 분 안에 스무 번 넘게 로그인하므로 성공 로그인 한도와 SSO 호출 한도(`HJV_API_LIMIT_OK_ID`, `_OK_IP`, `_SSO`)는 올려 둡니다(운영값이면 한 계정의 분당 다섯 번째 SSO 호출부터 401). 비밀번호 없는 연결 스위치는 운영처럼 **꺼진 채로 시작**합니다.
- 클라이언트는 시험 공개키(`HJV_LOGIN_SEAL_PUBKEY`, `HJV_TRUST_PUBKEYS`)를 넣어 빌드합니다. PC 쪽 실행 파일은 `/usr/local/lib/hjviewer-e2e/rustdesk`에 설치해 그 경로로 실행합니다. 리눅스는 `/usr` 아래의 실행 파일만 설치형으로 보고, hjv-api는 설치된 기기만 계정에 묶기 때문입니다. 리눅스 PC는 Windows의 HKLM 값을 설정 폴더의 `HJViewer_binding`(0600)에 둡니다.
- PC에는 연결 창(CM) 대신 `--hjv-e2e-cm`을 띄워, 서비스가 연결마다 CM에 넘기는 상대 ID와 이름을 받아 적습니다.

**헤드리스 명령**(시험 기능 `hjv-e2e`에만 있음, 클라이언트 패치 0034)
- `--hjv-e2e-login <포탈 ID> <비밀번호>`, `--hjv-e2e-logout`, `--hjv-e2e-devices`, `--hjv-e2e-keys`, `--hjv-e2e-remove <PC ID>`, `--hjv-e2e-revoke-key <ckid>`: 앱이 보내는 요청을 앱과 같은 Rust 경로(`http_request`의 계정 훅: 봉인, 계정 키, 요청 서명, 응답 관찰)로 보냅니다. 그래서 PC에서는 로그인만으로 묶입니다.
- `--hjv-e2e-option <이름> [값 …]`은 실행 중인 서비스의 옵션을 IPC로 읽거나 씁니다(여러 쌍은 한 번의 쓰기). `--hjv-e2e-local-option`은 그 사용자의 LocalConfig, `--hjv-e2e-service-password`는 서비스의 영구 비밀번호, `--hjv-e2e-id`는 기기 ID와 공개키입니다.
- 화면 세션(`--hjv-e2e-screenshot`, `-stream`, `-forward`)에 비밀번호 `""`를 주면 비밀번호 없는 연결입니다. 비밀번호 창이 뜨면 `hjv-e2e: password required <제목>`을 찍고 **종료 코드 3**, 'error' 창(키 바뀜 경고)이면 `hjv-e2e: error <제목>`을 찍고 **종료 코드 4**로 끝납니다. PC가 증명을 거절했을 때의 창은 제목이 `Could not connect without a password`라서 그냥 비밀번호 창(`Password Required`)과 구분됩니다.
- 비밀번호 없는 연결의 데이터 경로는 원격 화면(`--hjv-e2e-screenshot <ID> ""`)으로 봅니다. 포트 포워딩은 증명을 보내지 않으므로 S22와 X7에만 씁니다.

**시나리오와 결과**(2026-10-13, WSL2 Ubuntu 24.04, `scripts/ci.sh e2e`, 실행 순서대로. `""`는 비밀번호 없이 연 원격 화면, T0는 같은 실행의 비밀번호 연결 시간, 상한 B = T0 + 2초)

| # | 단계 | 결과 |
|---|---|---|
| S0 | 기존 단계 전부(PC는 설치 경로의 실행 파일) | 통과, strict는 relay, open은 direct (P2P) |
| S24 | 스위치가 꺼진 채 PC·컨트롤러가 alice로 로그인 → `""` → `admin passwordless on --owner-of <PC ID>` → `""` | PC 상태 `disabled`, 거절 창과 종료 3. 켠 뒤 3.5초 만에 성공(갱신 5초) |
| S1–S3 | PC 로그인, 컨트롤러 로그인, 기기 목록 | `this device: added`, PC 로그 `trust list active, 2 key(s)`, `홍길동`, 목록에 PC가 `active`와 `pk`로, 컨트롤러 자신은 없음 |
| T0 | 로그인하지 않은 기기의 비밀번호 연결 | 3.6초 |
| S4–S6 | `""`, 5초 스트림, PC 로그와 CM | 비밀번호 창 없이 화면(3.4초, RMSE 0.070), 프레임 수신, `same-account login (remote) by Linux PC from <ID>` 한 줄, CM 이름 `Linux PC · same account(홍*동)` |
| S7–S10 | bob의 컨트롤러 | 목록에 PC 없음, 증명 없이 바로 비밀번호 창(종료 3, 2.6초 ≤ B), PC 비밀번호로는 성공(D6), 틀린 비밀번호는 다시 입력(종료 3). bob의 세션에서 PC가 받은 이름은 OS 사용자 이름이고 성명·포탈 ID가 아님(H21) |
| S22 | alice의 포트 포워딩 `""` | 증명 없이 비밀번호 창, 종료 3 |
| S23 | 컨트롤러의 토큰만 다른 기기에서 써서 목록 요청 | 401 |
| X1 | PC `approve-mode=click` | `No Password Access`, PC 쪽에 수락 요청 |
| X5 | PC `verification-method=use-temporary-password` | 거절 창, 종료 3 |
| X8 | PC를 '내 기기만'으로(승인 password, 영구만, 비밀번호 없음) | alice `""` 성공, bob의 예전 비밀번호는 실패 |
| S16 | 다른 묶음 ID의 묶기 요청, 받아 둔 목록 파일 고치기, 허용 파일의 묶음 ID 바꾸기 | 묶음 그대로 / 해시가 달라 다시 받음 / 서버가 확인하지 않는 묶음이라 종료 3 |
| S12, S13 | hjv-api `kill -STOP`, 그다음 종료 | 둘 다 성공(PC가 받아 둔 목록, 컨트롤러는 디스크의 `hjv-my-devices`). 종료 중 계정 경로는 503 |
| S13b | 종료한 채 목록 수명(30초)을 넘김 | PC의 즉시 동기화가 실패해 거절 창, 종료 3(2.7초 ≤ B). hjv-api를 다시 켜면 `active` |
| S14 | hjv-ctl2: 로그아웃 → alice로 로그인(새 키) → 바로 `""` | 성공(PC가 키 일련번호를 보고 즉시 동기화) |
| S15 | PC 서비스 종료 → PC 사용자 토큰 삭제 → 다시 시작 → `""` | 성공(허용 표시와 캐시). 시작 뒤 첫 동기화 34밀리초 |
| S17 | hjv-ctl2(alice)가 컨트롤러의 키를 폐기 | 2.7초 만에 거절 창, 종료 3(갱신 5초) |
| S18 | hjv-ctl2가 PC를 목록에서 뺌 | PC가 서명된 `revoked`를 받고 허용 표시 삭제, 종료 3 |
| S19 | PC에서 로그아웃 | 1초 뒤 `""`가 종료 3, hjv-api도 묶음을 풂 |
| S19b | hjv-api가 꺼진 동안 PC에서 로그아웃 | 그래도 1초 뒤 종료 3. hjv-api를 다시 켜면 목록에 `listed` |
| S16b | S19b 뒤 예전 묶기 요청을 로그인 없이 다시 넣음(로그인 뒤 10분 안) | 묶이지 않음(풀린 묶음 기록), 종료 3 |
| R3 | 로그아웃이 묶기 확인보다 먼저 닿음: 새 묶음의 묶기 요청이 hjv-api에 가 있는 동안 풀기 요청 | hjv-api는 그 묶음을 확인해 주지만 PC는 묶지 않음, 종료 3 |
| S25 | PC `hjv-pwless-off Y` → `""` → Config2를 `N`으로 다시 씀 → `""` → PC에서 다시 로그인 → `""` | 1초 뒤 종료 3, 다시 써도 종료 3(`Off`), 다시 로그인한 뒤에만 성공 |
| S26 | `admin binding revoke <PC ID>`, 다시 묶고 `device remove` | 둘 다 서명된 `revoked`로 허용 표시 삭제, 종료 3. 그 뒤 PC의 빈 묶음 보고에는 서명 없는 `unbound`(동기화 응답이 200, 404 순서)이고, PC는 다시 로그인할 때까지 `revoked`를 보여 줌 |
| X6 | `admin passwordless off` | 6.7초 만에 종료 3 |
| X7 | PC의 기기 키만 지우고 다시 시작(hbbs에 새 키 등록) | 원격 화면과 포트 포워딩 모두 키 바뀜 경고(종료 4), 증명도 비밀번호도 나가지 않음(PC에 닿은 로그인 없음) |
| X2, X3 | 1.5.1 PC에 alice가 `""`, 1.5.1 컨트롤러가 비밀번호로 | 바로 비밀번호 창(종료 3, 2.8초 ≤ B) / 성공 |
| K1 | 계정 키를 읽을 수 없는 동안의 계정 요청(잠긴 아이폰의 대역) | 요청을 보내지 않고 `hjv: account key unavailable`, 로그아웃 없음 |
| K2 | 기기가 믿지 않는 인증서 앞에서 로그인, "insecure TLS fallback"을 켠 채 | 인증서 오류로 실패, SSO 호출 0(H15) |
| S11 | bob의 틀린 비밀번호, 명단 밖 `Y9001` | 같은 실패 문구, 명단 밖 ID는 SSO 호출 0 |
| D1 | 목록 요청이 나가 있는 동안 로그아웃 | 뒤늦게 온 목록이 `hjv-my-devices`를 다시 만들지 않음 |
| S27 | 컨트롤러 시계 +15분(`libfaketime`) | 첫 요청 409 `clock`, 토큰·`hjv-my-devices` 그대로, `hjv-clock-offset` -900. 다음 프로세스부터 목록과 `""` 성공. 로그인도 시계 문구 뒤 다시 하면 성공 |
| X7b | X7 뒤 hjv-api가 hbbs의 새 키를 봄 → 목록 새로 고침 → `""` → PC에서 다시 로그인 → `""` | 65초 뒤 `needs-relogin`, 경고 없이 비밀번호 창(종료 3). 다시 로그인하면 새 키로 `active`, 성공 |
| X4 | 두 번째 1.6.0 PC(alice)가 컨트롤러 | 성공(PC → PC) |
| S20 | 실행 폴더 전체(Caddy의 debug 로그 포함)에서 포탈 비밀번호 4개, 토큰·계정 키 30개, hjv-api의 키 셋(봉인·목록·기록) 찾기 | 각 클라이언트의 LocalConfig 파일 밖에는 없음. hjv-api의 키는 자격 증명 폴더 밖에는 없음 |
| S21 | open 모드에서 `""` | `direct (P2P)`로 성공(2.9초) |

- 걸린 시간: `scripts/ci.sh e2e` 전체 7분 42초~9분 12초(다섯 번, 같은 PC에서 다른 빌드가 도는 정도에 따라). 그중 strict 약 5분 50초(계정 단계 약 5분 30초), open 약 22초이고, 나머지는 새로 받은 트리의 클라이언트 빌드와 계정 단위 시험입니다(각각 42~47초, 다른 빌드와 겹치면 1분~1분 40초). 맨 처음 한 번은 의존성 전체(클라이언트 2분 29초, 단위 시험 1분 49초, hjv-api 42초)와 1.5.1 클라이언트(2분 45초), Caddy와 `libfaketime`을 준비하느라 13분 55초가 걸렸습니다.
- 기다림이 긴 곳은 둘입니다: S13b(목록 수명 30초)와 X7b(아래).

**고침을 되돌렸을 때 실패하는지**: 다섯 군데를 되돌린 클라이언트로 같은 스크립트를 돌려, 해당 시나리오만 실패하는 것을 확인했습니다(커밋하지 않은 확인용 빌드). H21의 이름 훅을 빼면 S9, 로그아웃이 거둔 묶기 요청의 확인을 버리는 처리를 빼면 R3(로그아웃한 PC가 묶이고 `""`가 들어감), 포트 포워딩의 키 비교 순서를 되돌리면 X7(비밀번호가 나가 포워딩이 붙음), 계정 요청의 엄격 TLS(H15)를 빼면 K2, 로그아웃 뒤 목록 확인을 빼면 D1이 실패했습니다.

### 계정 단계로 찾은 것

- **open 모드가 중계로만 붙던 원인**: 프로그램이 아니라 호스트였습니다. Docker가 깔린 뒤로 호스트의 `FORWARD` 정책이 DROP이 되어 피어 사이 UDP가 버려졌고, 스크립트는 P2P인지 확인하지 않아 통과로 보였습니다. 스크립트가 필요한 규칙을 넣고(위 모드 표), S21이 `direct (P2P)`를 확인합니다.
- **기기 키가 바뀐 PC가 `needs-relogin`이 되는 데 걸리는 시간**: 다시 시작한 PC는 hbbs에 등록하기 전에 첫 동기화를 보냅니다. 그때 hjv-api가 hbbs에 물으면 아직 옛 키라 아무것도 바꾸지 못하고, 다시 묻는 것은 1분 뒤(같은 PC 1분에 한 번)의 다음 동기화 때입니다. 다시 시작한 PC는 목록을 받기 전이라 동기화 간격이 기본 120초(±20%)이므로, 운영에서는 다시 켜진 뒤 약 2분(96~144초)에 바뀝니다. E2E는 61초 뒤에 PC의 동기화를 두드려(`hjv-pc-sync`) 서버의 1분만 확인하고(63~65초), 그 1분 동안 X2·X3·K1·K2·S11·D1·S27을 돌립니다.
- **새 키의 첫 연결과 PC의 '10초에 한 번'**: PC는 증명 때문에 하는 즉시 동기화를 10초에 한 번만 합니다(설계 3-5). 그래서 방금 로그인한 기기의 첫 연결이 앞선 그런 동기화 뒤 10초 안이면 한 번 거절됩니다. 설계가 받아들인 한계이고, 스크립트는 S14와 X4 앞에서 그 간격을 띄웁니다.
- **PC 서비스가 시작할 때마다 `HEAD /api/hjv/pc/sync` 한 번**: upstream의 HTTP 클라이언트가 TLS 방식을 고르려고 보내는 탐색 요청이고, hjv-api는 405로 답합니다. 동기화는 그 뒤의 POST이고 첫 동기화의 1.5초 안에 들어 있습니다. 운영의 웹 서버 로그에 PC 서비스 시작마다 405가 한 줄씩 보이는 것은 정상입니다(Caddy의 debug 로그에서 봄).
- 위조 묶음(S16의 셋째)과 풀린 묶음의 예전 요청(S16b)은 서버가 확인하지 않거나 PC의 기록이 막아 아무것도 켜지 못했습니다.

### 계정 단계가 다루지 않는 것

- Windows의 HKLM 값과 DPAPI, 제거 훅, 로그온 화면, 아이폰 Keychain, 안드로이드 백업 제외: 리눅스에서는 파일과 LocalConfig가 그 자리를 대신합니다. 실기기 점검 목록은 설계 7-6에 있습니다.
- Flutter 화면(로그인 창, 알림, 설정 줄, 배너): 헤드리스 명령은 같은 요청을 보낼 뿐 화면을 띄우지 않습니다. 문구와 조건은 Dart 시험이 봅니다.
- 실제 연세 SSO, 운영 한도 값, nginx(운영의 웹 서버, `scripts/ci.sh deploy`가 봄).
- PC의 2FA와 신뢰 기기, 파일 전송·카메라·터미널 종류의 증명(단위 시험이 봄), 원격 켜기(아래 단계).

## 원격 켜기 단계 (1.6.0)

NAS 중계기(hjv-relay)가 서버가 서명한 켜기 명령을 받아 잠든 PC에 매직 패킷을 보내는지 확인합니다(설계 3판 8-10). `tools/e2e/wol-e2e.sh`이고, `scripts/ci.sh wol`이 sudo 없이 돌립니다(서버와 `hjv-probe`는 `scripts/ci.sh server`로 먼저 빌드). 이 스크립트는 아직 없습니다. 원격 켜기 E2E 작업이 쓰고 결과를 이 절에 적습니다.

- **구성**: 사용자 네임스페이스 안에 스위치와 라우터, 랜카드 둘인 NAS, 잠든 PC(IP 없는 랜카드에서 프레임을 포착), 다른 LAN을 둡니다. 바깥에 hjv-api, hbbs, Caddy(시험 CA)를 둡니다. hjv-api 시험 설정은 `HJV_API_BIND_OS=windows,linux`입니다. PC 쪽 클라이언트는 무권한 사용자·마운트 네임스페이스(`unshare -Urm`) 안에서 `/usr/local/lib` 위에 tmpfs를 올리고 `hjviewer-e2e/rustdesk`로 복사해 실행합니다(설치형으로 보이게 하려는 것이고, 바깥의 `/usr`는 바뀌지 않음).
- **흐름**: alice로 로그인해 PC 묶기 → PC가 `--hjv-e2e-wol-report <MAC> <IPv4/접두사> <게이트웨이>`로 켜기 정보를 동기화에 실음 → hjv-relay가 등록 요청 → `admin relay approve <코드> --owner-of <PC ID>` → PC를 "잠재움"(클라이언트 종료, IP 삭제) → 앱처럼 토큰으로 켜기 요청 → 5초 안에 PC 랜카드에서 255.255.255.255와 NAS 랜카드의 브로드캐스트, 포트 9·7, 3번 = 12프레임이 잡히고 다른 LAN에는 0 → `sent` → PC를 "깨움" → 30초 안에 `confirmed`.
- **거부 사례**: 서명 없음, 다른 키, 만료, 다른 중계기 앞 명령, 명령 재사용, MAC 5개, 모르는 네트워크, 분당 11번째 → 모두 프레임 0과 이유. 다른 계정의 켜기 요청 → 404. 중계기가 꺼져 있으면 503.
- 예상 시간: 빌드를 빼고 1~2분. 시험 값은 문서용 주소(RFC 5737)와 MAC(`00:00:5e:00:53:xx`)만 씁니다.
