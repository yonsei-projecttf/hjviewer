# 방화벽 E2E 검증

실제 HJViewer 클라이언트 두 대와 서버를 리눅스 한 대에서 띄웁니다. **회사 방화벽처럼 서버로 가는 HTTPS(TCP 443)만 허용한 네트워크**에서 접속, 데이터 전달, 화면 공유가 되는지 확인합니다. 1.6.0부터는 같은 구성에 계정 API를 더해 연세포탈 로그인과 같은 계정 기기끼리의 비밀번호 없는 연결을 확인하는 [계정 단계](#계정-단계-160), hbbs와 PC가 낮은 버전의 컨트롤러를 거절하는지 확인하는 [최소 버전 단계](#최소-버전-단계-160), 무권한 네트워크 네임스페이스에서 NAS 중계기의 켜기를 확인하는 [원격 켜기 단계](#원격-켜기-단계-160)가 있습니다.

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
- 패치에 계정 코드가 있으면 [계정 단계](#계정-단계-160)도 돕니다. 클라이언트의 HJViewer 코드 단위 시험(`--lib hjv`), hjv-api와 가짜 SSO 빌드, 태그 `client-1.5.1`로 만드는 1.5.1 비교용 클라이언트(태그와 서버 키가 같으면 한 번만 빌드해 `build/ci/e2e-old/rustdesk`에 둠), Ubuntu의 `libfaketime` 패키지 풀기(`build/ci/faketime`, 설치하지 않음)가 더해집니다.
- hbbs에 서버 패치 0005가 있으면(`--hjv-min-controller-file`) [최소 버전 단계](#최소-버전-단계-160)의 V1-V4를 돌립니다. `server/patches`에 0005가 있는데 hbbs에 그 옵션이 없으면(전에 빌드한 hbbs) 실패합니다. 패치에 업데이트 코드(`src/hjviewer_update`)가 있으면 클라이언트를 업데이트 시험 키로 빌드하고 단위 시험(`--lib hjv`)도 그 빌드로 돌립니다. 업데이트 코드의 단위 시험(`--lib hjviewer_update`)은 키만 뺀 빌드로 한 번 더 돌립니다. 키 없는 빌드가 1.5.1처럼 아무것도 하지 않는지 보는 시험은 키가 든 빌드에서는 바로 끝나기 때문입니다. 이 저장소에 `tools/hjv-release`가 있으면 그것을 빌드해(`build/ci/target-hjv-release`) 시험 정책에 서명하고 V5-V9를 돌립니다. 셋 중 없는 것은 건너뛴 것으로 셉니다.
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

# 6) 최소 버전 단계 (1.6.0). hbbs는 서버 패치 0005가 든 것, 클라이언트는 패치 0037·0038이 든 것을
#    업데이트 시험 키로 빌드한 것이어야 합니다(키는 `hjv-release test-keys --env`가 출력함)
(cd tools/hjv-release && cargo build --release)
(cd upstream/rustdesk && env PKG_CONFIG_PATH=$HJV_E2E_WORK/pkgconfig HJV_SERVER=hjv.test \
   HJV_SERVER_KEY=$(cat $HJV_E2E_WORK/srv/id_ed25519.pub) \
   $(../../tools/hjv-release/target/release/hjv-release test-keys --env) \
   cargo build --release --features linux-pkg-config,hjv-e2e)
HJV_E2E_MIN_CONTROLLER=1 HJV_E2E_UPDATE_KEYS=1 HJV_RELEASE=$PWD/tools/hjv-release/target/release/hjv-release \
  CADDY=/path/to/caddy tools/e2e/firewall-e2e.sh strict
#    계정 단계와 함께 돌리려면(최소 버전 단계는 계정 단계 뒤에 돔) 6)의 빌드를 쓰지 않습니다. 그 빌드에는 계정
#    공개키가 없고 /usr 아래에 설치하지도 않습니다. 5)의 계정 공개키와 6)의 업데이트 키를 빌드 하나에 함께 넣고,
#    그것을 5)처럼 설치해 PC_CLIENT로 준 뒤 5)와 6)의 값을 모두 줍니다. scripts/ci.sh e2e가 이렇게 합니다
(cd upstream/rustdesk && env PKG_CONFIG_PATH=$HJV_E2E_WORK/pkgconfig HJV_SERVER=hjv.test \
   HJV_SERVER_KEY=$(cat $HJV_E2E_WORK/srv/id_ed25519.pub) \
   HJV_LOGIN_SEAL_PUBKEY=$(cat $KEYS/seal.pub) HJV_TRUST_PUBKEYS=$(cat $KEYS/trust.pub) \
   $(../../tools/hjv-release/target/release/hjv-release test-keys --env) \
   cargo build --release --features linux-pkg-config,hjv-e2e)
sudo install -D -m 755 upstream/rustdesk/target/release/rustdesk /usr/local/lib/hjviewer-e2e/rustdesk
HJV_E2E_ACCOUNT=1 HJV_E2E_KEYS=$KEYS PC_CLIENT=/usr/local/lib/hjviewer-e2e/rustdesk \
  HJV_API=$PWD/server/api/target/release/hjv-api HJV_FAKE_SSO=$PWD/server/api/target/release/hjv-fake-sso \
  HJV_E2E_MIN_CONTROLLER=1 HJV_E2E_UPDATE_KEYS=1 HJV_RELEASE=$PWD/tools/hjv-release/target/release/hjv-release \
  CADDY=/path/to/caddy tools/e2e/firewall-e2e.sh strict
sudo rm -rf /usr/local/lib/hjviewer-e2e
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
- 시험 계정은 가짜 SSO의 것으로, HJViewer의 ID 규칙(대문자로 바꿔 Z나 Y로 시작, ZA 제외)에 맞는 가짜 ID입니다: alice `ZZTEST01`(홍길동), bob `ZZTEST02`, 미리 넣지 않는 carol `YZTEST03`(Y 코드. 가짜 SSO가 그 pcode를 소문자로 주고 hjv-api가 대문자로 둠). 가짜 SSO는 행정허브 SSO 등록의 모양(`success`, `access_token`, `data[0]`의 `pcode`·`name`·`email`·`deptNm`)으로 답하고, 대문자로 바꾸지 않은 사용자 이름이나 계약과 다른 양식은 거절하므로 모든 로그인 시나리오가 그 계약을 지납니다. hjv-api는 운영 기본값처럼 열린 로그인(`HJV_API_LOGIN=open`)으로 시작하고, alice와 bob만 미리 가져옵니다. carol은 처음 로그인할 때 명단에 오릅니다(S28).
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
| S16 | 다른 묶음 ID의 묶기 요청, 받아 둔 목록 파일 고치기, 허용 파일의 묶음 ID 바꾸기 | 묶음 그대로 / 들어오고 목록이 해시와 다시 맞음(해시가 달라 다시 받았는지, 5초 주기 동기화가 먼저 덮었는지는 단위 시험이 봄) / 서버가 확인하지 않는 묶음이라 종료 3 |
| S12, S13 | hjv-api `kill -STOP`, 그다음 종료 | 둘 다 성공(PC가 받아 둔 목록, 컨트롤러는 디스크의 `hjv-my-devices`). 종료 중 계정 경로는 503 |
| S13b | 종료한 채 목록 수명(30초)을 넘김 | PC의 즉시 동기화가 실패해 거절 창, 종료 3(2.7초 ≤ B). hjv-api를 다시 켜면 `active` |
| S14 | hjv-ctl2: 로그아웃 → alice로 로그인(새 키) → 바로 `""` | 성공: PC의 목록에 새 키가 있음(키 일련번호를 보고 한 즉시 동기화인지 주기 동기화인지는 단위 시험이 봄) |
| S15 | PC 서비스 종료 → PC 사용자 토큰 삭제 → 다시 시작 → `""` | 성공(허용 표시와 캐시). 시작 뒤 첫 동기화 34밀리초 |
| S17 | hjv-ctl2(alice)가 컨트롤러의 키를 폐기 | 2.7초 만에 거절 창, 종료 3(갱신 5초) |
| S18 | hjv-ctl2가 PC를 목록에서 뺌 | PC가 서명된 `revoked`를 받고 허용 표시 삭제, 종료 3 |
| S19 | PC에서 로그아웃 | PC가 1.5초 안에 묶음을 지움(걸린 ms를 PASS 줄에. 1초는 단위 시험이 봄), `""`가 종료 3, hjv-api도 묶음을 풂 |
| S19b | hjv-api가 꺼진 동안 PC에서 로그아웃 | 그래도 1.5초 안에 묶음을 지우고 종료 3. hjv-api를 다시 켜면 목록에 `listed` |
| S16b | S19b 뒤 예전 묶기 요청을 로그인 없이 다시 넣음(로그인 뒤 10분 안) | 묶이지 않음(풀린 묶음 기록), 종료 3 |
| R3 | 로그아웃이 묶기 확인보다 먼저 닿음: 새 묶음의 묶기 요청이 hjv-api에 가 있는 동안 풀기 요청 | hjv-api는 그 묶음을 확인해 주지만 PC는 묶지 않음, 종료 3 |
| S25 | PC `hjv-pwless-off Y` → `""` → Config2를 `N`으로 다시 씀 → `""` → PC에서 다시 로그인 → `""` | 1.5초 안에 `Off`(걸린 ms를 PASS 줄에), 종료 3, 다시 써도 종료 3(`Off`), 다시 로그인한 뒤에만 성공 |
| S26 | `admin binding revoke <PC ID>`, 다시 묶고 `device remove` | 둘 다 서명된 `revoked`로 허용 표시 삭제, 종료 3. 그 뒤 PC의 빈 묶음 보고에는 서명 없는 `unbound`(동기화 응답이 200, 404 순서)이고, PC는 다시 로그인할 때까지 `revoked`를 보여 줌 |
| X6 | `admin passwordless off` | 6.7초 만에 종료 3 |
| X7 | PC의 기기 키만 지우고 다시 시작(hbbs에 새 키 등록) | 원격 화면과 포트 포워딩 모두 키 바뀜 경고(종료 4), 증명도 비밀번호도 나가지 않음(PC에 닿은 로그인 없음) |
| X2, X3 | 1.5.1 PC에 alice가 `""`, 1.5.1 컨트롤러가 비밀번호로 | 바로 비밀번호 창(종료 3, 2.8초 ≤ B) / 성공 |
| K1 | 계정 키를 읽을 수 없는 동안의 계정 요청(잠긴 아이폰의 대역) | 요청을 보내지 않고 `hjv: account key unavailable`, 로그아웃 없음 |
| K2 | 기기가 믿지 않는 인증서 앞에서 로그인, "insecure TLS fallback"을 켠 채 | 인증서 오류로 실패, SSO 호출 0(H15) |
| S11 | bob의 틀린 비밀번호, 미리 넣지 않은 carol의 틀린 비밀번호 | 같은 실패 문구, SSO 호출 각 1번(열린 로그인), 실패한 로그인은 명단을 바꾸지 않음(명단 2개 그대로) |
| D1 | 목록 요청이 나가 있는 동안 로그아웃 | 목록 요청이 200으로 답을 받았고(그래야 키 확인을 지남), 뒤늦게 온 목록이 `hjv-my-devices`를 다시 만들지 않음 |
| S27 | 컨트롤러 시계 +15분(`libfaketime`) | 첫 요청 409 `clock`, 토큰·`hjv-my-devices` 그대로, `hjv-clock-offset` -900. 다음 프로세스부터 목록과 `""` 성공. 로그인도 시계 문구 뒤 다시 하면 성공 |
| X7b | X7 뒤 hjv-api가 hbbs의 새 키를 봄 → 목록 새로 고침 → `""` → PC에서 다시 로그인 → `""` | 65초 뒤 `needs-relogin`, 경고 없이 비밀번호 창(종료 3). 다시 로그인하면 새 키로 `active`, 성공 |
| X4 | 두 번째 1.6.0 PC(alice)가 컨트롤러 | 두 번째 PC의 목록에 자기 자신은 없음, 성공(PC → PC) |
| S28 | hjv-ctl2(D1 뒤로 비어 있음)에서 미리 넣지 않은 carol이 로그인 | 성공(SSO 호출 1번). `allow list` 첫 줄 `allowlist: 3 IDs, 3 registered (1 by a login), 0 blocked; login open`, carol의 항목은 `from login`. hjv-api 로그에 `… was registered (acct=<꼬리표>)` 한 줄, 로그와 가린 명단에 carol의 ID 없음(대소문자 무시: hjv-api는 ID를 소문자로 둠) |
| S29 | `admin block YZTEST03` | 출력 `blocked: 1 blocked …, 1 tokens and 1 account keys revoked; …`. carol의 기기 목록 요청이 바로 401. 틀린 비밀번호는 bob과 같은 실패 문구, 맞는 비밀번호는 SSO를 거친 뒤 403 `이 계정은 관리자가 HJViewer 사용을 막았습니다. 다시 쓰려면 관리자에게 문의하세요.`(SSO 호출 각 1번) |
| S30 | `admin unblock YZTEST03` | carol이 다시 로그인하고 기기 목록 성공 |
| S31 | `admin user delete YZTEST03`(명단 항목도 함께 지워짐) → hjv-api를 `HJV_API_LOGIN=list`로 다시 시작 | `report` 둘째 줄 `login: list (…)`. 명단에 없는 carol은 맞는 비밀번호로도 틀린 비밀번호와 같은 실패 문구, SSO 호출 0. 미리 넣은 bob은 로그인 성공. 그 뒤 기본값(open)으로 다시 시작 |
| S32 | HJViewer의 ID 규칙: 학번 꼴 `1234567890`, `zatest01`, `BZTEST01`로 로그인. 이어서 가짜 SSO의 `ZZSSO-DENIED`(사용자 정보 호출이 거절)와 `ZZSSO-ZA`(pcode가 ZA로 시작) | 앞의 셋은 SSO 호출 0으로 403 `연세대학교 교직원의 연세포탈 ID로만 로그인할 수 있습니다. 입력한 ID를 확인해 주세요.`, 뒤의 둘은 SSO 호출 각 1번 뒤 403 `이 계정으로는 HJViewer를 쓸 수 없습니다. 연세대학교 교직원만 쓸 수 있으며, 교직원인데도 이 문구가 나오면 관리자에게 문의하세요.`. `report`에 `login id-rule`과 사유별 `login denied-user-refused`, `login denied-pcode-rule`, hjv-api 로그에 입력한 ID 없음, hjv-ctl2의 bob 로그인은 그대로 |
| S20 | 실행 폴더 전체(Caddy의 debug 로그 포함)에서 포탈 비밀번호 4개, 토큰·계정 키 30개, hjv-api의 키 셋(봉인·목록·기록) 찾기 | 각 클라이언트의 LocalConfig 파일 밖에는 없음. hjv-api의 키는 자격 증명 폴더 밖에는 없음 |
| S21 | open 모드에서 `""` | `direct (P2P)`로 성공(2.9초) |

- 걸린 시간: `scripts/ci.sh e2e` 전체 7분 42초~9분 12초(다섯 번, 같은 PC에서 다른 빌드가 도는 정도에 따라). 그중 strict 약 5분 50초(계정 단계 약 5분 30초), open 약 22초이고, 나머지는 새로 받은 트리의 클라이언트 빌드와 계정 단위 시험입니다(각각 42~47초, 다른 빌드와 겹치면 1분~1분 40초). 맨 처음 한 번은 의존성 전체(클라이언트 2분 29초, 단위 시험 1분 49초, hjv-api 42초)와 1.5.1 클라이언트(2분 45초), Caddy와 `libfaketime`을 준비하느라 13분 55초가 걸렸습니다.
- 기다림이 긴 곳은 둘입니다: S13b(목록 수명 30초)와 X7b(아래).
- S32는 행정허브 SSO 계약과 HJViewer의 ID 규칙(2026-10-14 사용자 결정)을 넣으며 더한 시나리오입니다.
- S11과 S28~S31은 열린 로그인(2026-10-14 사용자 결정)을 넣으며 바꾸거나 더한 시나리오입니다. 명단 방식으로 다시 시작하는 S31은 X7b의 1분 확인을 건드리지 않도록 X4 뒤에 둡니다(hjv-api를 다시 시작하면 "같은 PC는 1분에 한 번"의 기억이 사라져, X7b가 기다리는 다시 묻기가 1분을 채우지 않고 일어남).

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

## 최소 버전 단계 (1.6.0)

최소 버전 설계(설계 2-2, 2-4, 2-5)의 두 강제를 실제 프로그램으로 확인합니다. 하나는 hbbs가 낮은 버전의 컨트롤러를 punch hole 단계에서 거절하는 것(서버 패치 0005)입니다. 다른 하나는 PC가 서명된 버전 정책을 스스로 받아, 낮은 버전 컨트롤러의 로그인을 거절하는 것(클라이언트 패치 0037·0038)입니다. strict 모드에서 계정 단계 다음에 돕니다. `scripts/ci.sh e2e`는 hbbs와 패치에 그 기능이 있으면 알아서 켭니다([실행](#실행)).

**구성**
- 버전: E2E 클라이언트는 버전을 찍지 않은 빌드라서 upstream 체크아웃의 버전(1.5.0)을 보냅니다. 스크립트가 `--version`으로 읽습니다. hbbs의 `MIN`은 그다음 패치 버전(1.5.1), 정책의 최소 버전은 그다음 부 버전(1.6.0)입니다.
- hbbs: `HJV_MIN_CONTROLLER_FILE=$RUN/min-controller.conf`로 띄웁니다. 처음에는 파일이 없습니다(최소 버전 없음). V1과 V4가 운영자처럼 옆에 쓴 뒤 `mv`로 바꿉니다. hbbs는 10초마다 다시 읽습니다. 문구는 설계 2-5의 S1에 시험 서버와 지어낸 문 2 코드를 넣은 것입니다.
- 업데이트 키: 클라이언트와 PC 실행 파일을 이 저장소의 시험 키로 빌드합니다(`hjv-release test-keys --env`: 서명 키 1·2, 루트 키 101, 하한 `1:0`). `scripts/ci.sh`는 같은 seed(40, 80, a0에서 시작하는 연속 32바이트)에서 공개키를 계산합니다. `hjv-e2e` 기능이 있는 빌드는 서버가 `hjv.test`이고 시험 키일 때만 정책을 따릅니다(설계 1-7). 이 빌드는 리눅스에서 `windows` 항목을 따르고, PC는 리눅스 컨트롤러를 Windows 컨트롤러로 봅니다.
- 정책: `tools/hjv-release test-sign`이 시험 키 1로 서명합니다(세대 1). V9의 첫 정책(seq 3)만 시험 키 3으로 서명합니다. E2E 빌드가 믿지 않는 키라서 도구는 `--raw`일 때만 서명합니다. V9의 둘째 정책(seq 4)은 시험 키 1로 서명한 뒤, 봉투의 서명 값을 seq 2 봉투의 것으로 바꿔 내줍니다. 서명한 봉투는 실행 폴더의 `policy/envelope-<seq>.json`에 남습니다. Caddy가 `https://hjv.test/api/hjv/version`으로 내줍니다. 운영에서는 nginx가 내주고 공개 `deploy/Caddyfile`에는 정책이 없습니다. 그래서 실행 때 만드는 Caddy 설정에 그 경로만의 사이트 블록을 `deploy/Caddyfile`의 사이트 앞에 둡니다. 정책 파일은 서버처럼 한 번에 바꿉니다(`mv`). 패키지 값은 지어낸 것입니다. 리눅스에서는 아무것도 받지 않습니다.
- 첫 정책(seq 1)은 처음부터 내줍니다: `windows`의 최소 1.6.0, 기한은 내일입니다(신뢰 규칙의 유예 쌍: `grace_until`과 `grace_min` 0.0.0. 기한 전에는 아무것도 막지 않습니다). PC 서비스는 시작 30초 뒤 스스로 받습니다. 그래서 앞 단계 전체가 이 정책 아래서 돕니다. 기한 전이라 아무도 막히지 않습니다.
- 컨트롤러는 컨트롤러 네트워크(`hjv-ctl`)에서 로그인한 적 없는 별도의 쪽으로 돕니다. `ver`는 이 빌드, `old`는 1.5.1 클라이언트(`CLIENT_OLD`, 없으면 V3와 V8만 건너뜀)입니다. 클라이언트 호출마다 30초 제한이 있고, 넘으면 실패입니다. 거절은 종료 코드와 출력 문구를 글자 그대로 맞춰 봅니다.
- 평범한 입력만 씁니다: 첫 정책(최소를 올리고 기한은 내일), 그 올림이 이미 강제되는 정책(기한이 지남), 비운 `MIN`, 그리고 빌드가 믿지 않는 키로 서명한 정책과 다른 정책의 서명 값을 단 정책(V9). 모두 세대 1이고 신뢰 규칙(2026-10-13)의 유예 쌍을 가집니다. 새 세대의 선언과 키 폐기는 쓰지 않습니다. 선언 하나를 쓰는 단계(신뢰 규칙 7-5)는 아직 없습니다.

**시나리오와 결과**(2026-10-14, WSL2 Ubuntu 24.04, `scripts/ci.sh e2e`, 실행 순서대로)

| # | 단계 | 결과 |
|---|---|---|
| V1 | hbbs 파일을 `MIN=1.5.1`, `FROM=2026-01-01T00:00:00Z`, `TEXT=<S1>`로 바꿈 | 재시작 없이 다음 읽기에 `min-controller 1.5.1 from 2026-01-01T00:00:00Z`(10초마다 읽으므로 0~10초 뒤. 로그가 늦게 써지는 것까지 13초를 허용) |
| V2 | 이 빌드(1.5.0, 업데이트 키 있음)로 연결 | 종료 1, 0.2초. 표준 오류에는 R1 한 줄뿐이고 hbbs 문구와 그 링크는 없음. hbbs 로그에 그 시도의 거절이 한 줄(`version "1.5.0"`) |
| V3 | 1.5.1 클라이언트로 연결 | hbbs 문구가 그대로 옴(`%MIN%` 자리에 1.5.1, 문 2 링크 포함) |
| V4 | `MIN=`(비움) | 다음 읽기에 `min-controller off: MIN is empty in …`. 같은 컨트롤러가 PC 화면을 받음 |
| V5 | PC 서비스가 첫 정책을 스스로 받았는지 | PC 로그에 `version policy: now at 1:1`(서비스 시작 30초 뒤). 정책을 버린 줄(`refused (…)`)이나 `off in this build`가 없음 |
| V6 | 기한 전(내일)에 1.5.0 컨트롤러 | PC 화면을 받음 |
| V7 | 기한이 지난 정책(seq 2, 같은 최소 1.6.0, 기한 1시간 전)을 내주고 PC 서비스를 다시 시작 | 33~34초 뒤 PC 로그에 `now at 1:2`. 컨트롤러는 종료 4: PC가 보낸 L1(`이 PC는 HJViewer 1.6.0 이상에서만 연결을 받습니다. … (지금 버전 1.5.0). (version not allowed)`), 그다음 R1이 'Login Error' 창으로. PC 로그에 `version policy: refused the login of a controller ("Linux", version "1.5.0")`. 계정 단계가 띄운 연결 창(CM)에는 로그인이 닿지 않음 |
| V8 | 1.5.1 클라이언트로 같은 PC에 | PC의 L1이 그대로 'Login Error' 창으로 옴 |
| V9 | 빌드가 믿지 않는 시험 키 3으로 서명한 정책(seq 3, 기한까지 seq 2와 같음)을 내주고 PC 서비스를 다시 시작. 서비스가 서버에 묻기 전(시작 30초 안)에 연결 | 시작 6초 뒤 로그인이 V7처럼 거절됨(종료 4, L1, R1): 저장해 둔 1:2로 거절함. 그 거절이 깨운 확인에서 PC 로그에 `version policy: refused (UnknownKey)`가 남고 `now at 1:3`은 없음. 다시 연결해도 같은 거절. 연결 창(CM)에는 닿지 않음 |
| V9(이어서) | 시험 키 1로 서명했지만 서명 값은 seq 2의 것인 정책(seq 4, 나머지는 seq 2와 같음)을 내주고 PC 서비스를 한 번 더 다시 시작. 다시 등록하자마자 연결 | 시작 6초 뒤 로그인이 V7처럼 거절됨(종료 4, L1, R1): 시험 키 3의 정책을 거부한 뒤에도 저장해 둔 1:2가 그대로임. 그 거절이 깨운 확인에서 PC 로그에 `version policy: refused (BadSignature)`가 남고 `now at 1:4`는 없음. 연결 창(CM)에는 닿지 않음 |

- 다시 연결하지 않는지: 헤드리스 클라이언트에는 화면 창이 없어서 앱의 자동 재연결이 없습니다. 그래서 두 가지를 봅니다. 거절이 재시도 없이 바로 끝나는지(V2는 0.2초, hbbs 로그의 거절이 시도당 한 줄)와, 앱의 자동 재연결(`check_if_retry`)을 막는 조건을 지키는지입니다. 자동 재연결은 제목이 'Connection Error'인 창에서 문구에 `not allowed` 같은 말이 없을 때만 합니다. hbbs의 거절 문구와 R1은 `(version not allowed)`로 끝나고, PC의 거절은 제목이 'Login Error'입니다.
- 거절된 로그인은 PC의 정책 확인을 하나 깨웁니다(60초에 한 번 제한 안에서). 그래서 정책 요청 수는 세지 않습니다.
- PC는 정책을 시작 30초 뒤와 그 뒤 6시간마다 받습니다. 그래서 V7은 서비스를 다시 시작해 새 정책을 받게 합니다.
- V9는 다시 시작한 서비스가 hbbs에 다시 등록하자마자(`keep_alive` 줄) 연결합니다. 로그인이 서비스 시작 30초 안에 끝나지 않으면 실패로 봅니다. 그 전에는 서비스가 서버에 묻지 않으므로, 그 거절은 서비스가 저장해 둔 정책에서 나온 것입니다. 거절된 로그인은 바로 확인을 깨우고, 서비스는 시험 키 3의 정책을 거부합니다. 시험 키 3의 정책은 기한도 seq 2의 것을 그대로 둡니다. 더 늦은 기한은 완화라서, 아무 키나 믿는 PC도 그 정책을 다른 이유(`Relaxation`)로 거부하기 때문입니다. 그래서 아무 키나 믿는 PC는 그 정책을 받아(`now at 1:3`) 바로 실패하고, 다른 이유로 거부해도 실패합니다. 시작할 때 저장된 정책을 읽지 않는 PC는 첫 연결이 들어가서, 거부한 정책 때문에 가진 정책을 버리는 PC는 두 번째 연결이 들어가서 실패합니다.
- 시험 키 3의 정책은 키 번호에서 끝납니다. PC는 봉투의 키 번호로 빌드에 고정된 키를 먼저 찾고, 없으면 서명을 보기 전에 거부합니다(`UnknownKey`). 그래서 이 정책은 PC가 고정된 키만 믿는지를 볼 뿐, 서명 값 자체를 확인하는지는 보지 못합니다. 그것은 V9의 둘째 정책(seq 4)이 봅니다. 고정된 키 1의 서명이지만 다른 글(seq 2)의 서명이라, 서명을 확인하는 PC만 거부합니다(`BadSignature`). 나머지는 기한까지 seq 2와 같아서, 서명을 확인하지 않는 PC는 거부할 다른 이유가 없어 그 정책을 받고(`now at 1:4`) 바로 실패합니다. 정책을 거부한 서비스는 30분 동안 다시 묻지 않고(거절된 로그인이 있어도), 거부한 정책은 30분에 한 줄만 로그에 남깁니다. 그래서 서비스를 한 번 더 다시 시작합니다. 다시 시작한 서비스는 저장해 둔 1:2로 로그인을 거절하고, 그 거절이 깨운 확인에서 seq 4를 거부합니다. 서명 값의 확인은 같은 단계의 단위 시험도 봅니다. `trust.rs`의 바이트 변조 시험과 `mod.rs`의 변조된 봉투(`BadSignature`)이고, 키가 든 빌드와 키를 뺀 빌드에서 모두 돕니다.
- 걸린 시간: 이 단계는 1분 20초쯤입니다. 대부분 V1·V4에서 hbbs가 파일을 다시 읽기를 기다리는 것(각 10초 안)과, V7에서 다시 시작한 서비스가 정책을 받기까지(33~34초)입니다. V9는 17초쯤입니다(서비스를 두 번 다시 시작하고 각각 다시 등록까지 3초쯤, 연결 세 번). 클라이언트 단위 시험은 업데이트 코드의 시험까지 `--lib hjv` 224개가 됩니다(무시 2개: 도구 벡터의 선택 시험과 `write_json_copy`). 업데이트 키를 뺀 빌드의 `--lib hjviewer_update`는 141개입니다(무시 1개: 도구 벡터의 선택 시험). `scripts/ci.sh e2e` 전체는 V9의 첫 부분까지일 때 10분 0초였고(새로 받은 트리의 클라이언트와 두 단위 시험 빌드 포함), V9의 둘째 부분이 10초쯤 더합니다.

**검사가 실패하는지**: 일부러 틀리게 한 입력이나 클라이언트로 같은 스크립트를 돌렸고, 매번 그 자리에서 실패했습니다(커밋하지 않은 확인). seq 2의 기한을 1시간 뒤로 두면 V7에서 컨트롤러가 PC 화면을 받아 실패합니다(종료 0). 첫 정책을 빌드가 믿지 않는 서명 키 3으로 서명하면(`test-sign --raw`) PC가 `version policy: refused (UnknownKey)`를 남기고 V5에서 실패합니다. V9는 세 가지로 봤습니다. seq 3을 빌드가 믿는 키 1로 서명하면 아무 키나 믿는 PC처럼 PC가 그 정책을 받고(`now at 1:3`) V9가 바로 실패합니다. 서비스를 다시 시작하기 전에 PC가 저장한 정책 파일(`HJViewer_update.json`)을 지우면 시작할 때 저장된 정책을 읽지 않는 PC처럼 첫 연결이 PC 화면을 받고(종료 0) V9가 실패합니다. `trust.rs`의 `signed_by()`가 늘 참을 돌려주게 바꿔 빌드한 클라이언트, 곧 서명 값을 확인하지 않는 PC로 돌리면 V1-V8과 V9의 첫 부분(시험 키 3, `UnknownKey`)은 통과합니다. 둘째 정책에서 PC가 `now at 1:4`를 남기고 V9가 'the PC took the policy 1:4'로 실패합니다.

### 최소 버전 단계가 다루지 않는 것

- 앱의 화면(업데이트 화면, 배너, 링크 없는 거절 창): Dart 시험과 기기 시험.
- 서명 키 정책의 완화 거부, 루트 키의 선언, 새 세대, 키 폐기: 단위 시험과 `tools/hjv-release`의 벡터가 봅니다. 신뢰 규칙을 다시 구현한 뒤 통합(WP-I1)에서 이 단계에 선언 하나(완화, 그다음 그 완화를 놓친 PC가 받는 서명 키 정책의 올림)를 더합니다.
- 문 1(로그인한 앱의 패키지 받기), 짧은 링크, 문 2: hjv-api의 업데이트 경로가 아직 이 트리에 없습니다(WP-A1, 통합 WP-I1). 헤드리스 명령에도 문 1 명령이 없습니다.
- 컨트롤러의 정책 받기: 거절을 받으면 바로 확인을 시작하지만, 헤드리스 명령은 그 전에 끝납니다.
- Windows의 무인 설치와 화면 바꾸기 경로의 거절, 실제 1.5.1 앱의 창: 기기 시험.

## 원격 켜기 단계 (1.6.0)

꺼진 PC를 앱에서 켜는 길 전체를 실제 프로그램으로 확인합니다(설계 3판 8-10). 앱의 켜기 요청 → hjv-api가 서명한 명령 → NAS의 hjv-relay → PC 랜카드에 닿는 매직 패킷 → 깨어난 PC의 확인까지입니다. `tools/e2e/wol-e2e.sh`이고 `scripts/ci.sh wol`이 돌립니다(서버와 `hjv-probe`는 `scripts/ci.sh server`로 먼저 빌드).

**sudo를 쓰지 않습니다**(`sudo`를 실패하는 가짜로 바꿔 놓고 돌려, 한 번도 부르지 않는 것을 확인함). 스크립트는 이 사용자의 사용자·네트워크·마운트·PID 네임스페이스 안에서 자기 자신을 다시 실행합니다. 그 안에서 띄운 프로세스, 네트워크, 마운트는 스크립트가 끝나면 함께 사라지고, 스크립트의 PID나 그 프로세스 그룹에 SIGTERM·SIGINT·SIGKILL을 보내도 1초 안에 모두 사라집니다(`setpriv --pdeathsig`과 `unshare --kill-child`). 호스트의 `/usr`, `/etc`, iptables는 바뀌지 않습니다. 2분쯤 걸리는 실행이 10분이 되도록 끝나지 않으면 스스로 끝냅니다. `--inside`는 스크립트의 둘째 절반이라 네임스페이스의 첫 프로세스로만 돕니다. 화면도 쓰지 않습니다(Xvfb, ImageMagick 불필요).

```
                    인터넷 203.0.113.0/24
   hjv.test .10 : Caddy 443 → hbbs · hbbr · hjv-api(→ 가짜 SSO)        alice의 휴대폰 .50
                  Caddy 8443 → 서버 행세를 하는 시험 서버               bob의 휴대폰 .51
        ────────────────┬───────────────────────────────────┬────────────────
                        │      라우터 (스크립트가 도는 네임스페이스)      │
          LAN A 192.0.2.0/24 (.1)                    LAN B 198.51.100.0/24 (.1)
        ┌───────────────┴───────────────┐          ┌─────────┴─────────┐
   PC .67                        NAS eth0 .20   NAS eth1 .20       다른 PC .30
   MAC 00:00:5e:00:53:67         (PC와 같은 게이트웨이)  (NAS의 기본 경로)   (프레임이 오면 안 됨)
```

**구성**
- 스크립트가 도는 네임스페이스가 세 네트워크의 스위치(브리지)이자 그 사이의 라우터이고, 서버도 여기서 돕니다. 이 안에서는 사용자가 root라서 Caddy가 443을 그대로 엽니다. PC, NAS, 다른 LAN의 PC, 휴대폰 둘은 각각 네트워크 네임스페이스입니다. 주소는 문서용(RFC 5737), PC의 MAC도 문서용입니다.
- NAS는 랜카드가 둘입니다. 기본 경로는 LAN B 쪽이라(메트릭이 낮음), 랜카드를 골라 보내지 않은 패킷은 다른 LAN으로 나갑니다. LAN A 쪽 랜카드에는 PC와 같은 게이트웨이를 줍니다. 서버가 "같은 망"으로 보는 조건입니다.
- 서버는 패치한 hbbs·hbbr, hjv-api, 가짜 SSO, `deploy/Caddyfile`의 Caddy입니다. hjv-api에서 시험에 맞춘 설정은 가짜 SSO와 hbbs 콘솔의 주소, E2E 시험 키(명령 키 포함), `HJV_API_BIND_OS=windows,linux`뿐이고, 한도와 주기는 운영 기본값입니다. `hjv.test`는 네임스페이스 안에서만 바꿔 끼운 `/etc/hosts`로 풉니다. Caddy는 계정 단계처럼 `debug`로 띄워, PC의 동기화가 언제 왔는지를 그 로그에서 봅니다.
- 클라이언트와 중계기는 root가 아니라 이 사용자로 돕니다(안쪽에 사용자 네임스페이스를 하나 더 만들어 uid를 맞추고 권한을 없앰). 쪽마다 HOME, `/tmp`(앱의 IPC), machine-id, 호스트 이름이 따로입니다.
- **PC를 설치형으로**: 리눅스는 `/usr` 아래의 실행 파일만 설치형으로 보고, hjv-api는 설치된 기기만 계정에 묶습니다. 계정 단계는 `sudo install`을 쓰지만, 여기서는 PC 쪽 명령의 마운트 네임스페이스에서만 시험 폴더를 `/usr/local/lib`에 bind mount합니다. 그 안에서 실행 파일은 `/usr/local/lib/hjviewer-e2e/rustdesk`이고 호스트의 `/usr/local/lib`는 그대로입니다.
- hjv-relay는 DSM 부트업 작업처럼 `run --state … --log …`로 NAS에서 띄우고, 상태 파일 자리(`--status-file`)와 시험 CA(`--ca`)만 더합니다. 부트업 작업의 `--watch`와 `--stop`(새 실행 파일, 멈춤 표지)은 쓰지 않습니다. root가 아니므로 권한 내리기도 여기서 돌지 않습니다. 둘 다 `relay/`의 시험이 봅니다(권한 내리기는 `tests/netns.rs`를 root로 돌릴 때).
- **잠든 PC**는 서비스가 없고 랜카드에 주소가 없지만 링크는 살아 있습니다. `tools/e2e/capture.py`가 그 랜카드와 다른 LAN의 PC에서 packet socket으로 프레임을 받아, 그 PC의 매직 패킷(ff 여섯 개와 MAC 열여섯 번)이 든 것만 적습니다. PC는 두 가지로 재웁니다. "종료"는 서비스를 끝내고 깨울 때 새로 시작합니다(설계의 방식). "절전"은 서비스 프로세스를 멈춰 두었다가(SIGSTOP) 깨울 때 이어서 돌립니다. 이어서 도는 서비스는 시계가 뛴 것을 보고 재개로 판단합니다.
- **앱의 켜기 요청**은 `tools/e2e/wol-helper.py request`가 보냅니다. 켜기 요청은 앱의 Dart 코드가 보내는 것이라 헤드리스 클라이언트에는 명령이 없습니다. 그래서 헤드리스 클라이언트로 로그인한 뒤, 그 LocalConfig의 토큰과 계정 키(리눅스는 `hjv-ctl-key`)로 `X-HJV-Proof`를 계산해 붙입니다. 서명은 표준 라이브러리만으로 한 Ed25519이고, 시작할 때 문서의 시험 벡터(server/api/README.md의 `X-HJV-Proof`, relay/PROTOCOL.md 13장의 명령)와 같은지 확인합니다.
- **서버 행세를 하는 시험 서버**(`wol-helper.py impostor`): 중계기는 서버 이름으로 답하는 쪽이 누구든, 명령 키의 서명이 없거나 정해진 범위를 벗어난 명령으로는 패킷을 보내지 않아야 합니다(PROTOCOL.md 14장). 둘째 중계기를 `--server hjv.test:8443`으로 띄워 이 서버 앞에 두고, hjv-api라면 서명하지 않을 명령을 받게 합니다.

**헤드리스 명령**: 계정 단계의 명령에 `--hjv-e2e-wol-report <MAC> <IPv4/접두사> <게이트웨이>`(클라이언트 패치 0035)가 더해집니다. 묶인 PC가 그 값의 유선 랜카드 하나짜리 켜기 정보를 서비스와 같은 서명으로 동기화 한 번에 실어 보내고, hjv-api가 받으면 `hjv-e2e: wol report taken`을 찍습니다. Windows의 실제 수집(레지스트리, 전원 설정)은 리눅스에 없어 이 명령이 대신합니다.

**시나리오와 결과**(2026-10-13, WSL2 Ubuntu 24.04, `scripts/ci.sh wol`, 실행 순서대로)

| # | 단계 | 결과 |
|---|---|---|
| W1 | PC(설치 경로의 실행 파일)의 서비스가 등록 → PC에서 alice로 로그인 | `this device: added`, 서비스 상태 `active`(묶임) |
| W2 | alice와 bob이 각자 휴대폰에서 로그인 → PC가 `--hjv-e2e-wol-report` | 보고 전 판정 `unknown` → `wol report taken` → `ok`, 안내 없음. 앱이 받는 답에 MAC 없음. 승인된 중계기가 없어 목록에 "켜기"가 없음 |
| W3 | 맨 처음 띄워 둔 hjv-relay의 hello | `pending`. `admin relay list`의 등록 코드가 중계기 로그의 코드와 같음 |
| W4 | PC가 켜져 있을 때 켜기 요청 | 409 `online` |
| W5 | bob이 alice의 PC에 켜기 요청, 판정 조회 | 둘 다 404 |
| | PC 종료(서비스 끝, 랜카드 주소 삭제). hjv-api는 그 뒤 30초 동안 켜져 있다고 봅니다. 그 사이에 F0~F2 | |
| F0 | 둘째 중계기를 서버 행세를 하는 시험 서버 앞에 띄움 | 그 서버가 active라고 하면 active(상태는 서버 이름을 믿음. 명령은 아님) |
| F1 | 위조 명령 11개: 서명 없음, 엉터리 서명, 다른 키의 서명, 모르는 키 번호, 서명한 뒤 MAC을 고침, 만료, 다른 중계기 앞, MAC 5개, 브로드캐스트 MAC, 모르는 망, 포트 80 | 차례로 `bad_command`, `bad_signature`, `bad_signature`, `unknown_key`, `bad_signature`, `expired`, `wrong_relay`, `bad_macs`, `bad_macs`, `no_iface`, `bad_params`. 이유가 결과와 중계기 로그에 남고, 두 LAN 모두 어느 MAC의 매직 패킷도 없음(포착이 `any`) |
| F2 | 올바른 명령 11개를 한 번에, 그리고 첫 명령을 한 번 더 | 10개는 4프레임씩 나감(PC 랜카드 40, 다른 LAN 0). 11번째는 `rate_limited`, 다시 온 명령은 `replayed`, 둘 다 프레임 0 |
| W6 | PC가 꺼진 지 30초 뒤, 승인된 중계기가 없는 계정의 켜기 요청 | 409 `no_relay`(`no_relays`) |
| W7 | `admin relay approve <코드> --owner-of <PC ID>` | 승인, 판정 `on the PC's network: yes (the same network and gateway)`. 기본 출력에 주소와 MAC 없음. `--reveal`에는 중계기 랜카드, PC 랜카드(MAC은 끝 두 바이트만), 양쪽의 나가는 주소. 요청이 서버에 붙잡혀 있던 중계기는 1초 안에 승인을 앎(쉬는 사이에 승인되면 스크립트가 중계기의 다음 요청을 기다림). 앱의 판정에 중계기 `NAS XXXX`, `exact`, 연결됨. 목록의 둘째 줄 끝에 " · 켜기 가능" |
| W8 | alice의 휴대폰이 켜기 요청 | 202 `queued`(`exact`) → PC 랜카드에 12프레임: 255.255.255.255와 192.0.2.255, 포트 9와 7, 0.2초 간격으로 세 번. 모두 이더넷 브로드캐스트, NAS의 LAN A 주소에서, 내용은 매직 패킷 102바이트뿐. 다른 LAN은 0 → `sent`(packets 12). 중계기 로그에 승인과 보낸 패킷 한 줄씩(MAC은 가림), `hjv-relay status`에 그 작업과 패킷 12 |
| | NAS의 중계기를 끔. hjv-api는 그 뒤 1분 동안 연결된 것으로 봅니다 | |
| W9 | 1분 안의 두 번째 켜기 요청, bob의 작업 조회 | 같은 작업(202, `sent`) / 404 |
| W10 | PC를 깨움(주소 복구, 서비스 시작) | 깨운 지 4~7초 뒤 `confirmed`(요청 뒤 7~10초). 깨어난 뒤 PC의 동기화가 있었고 hbbs에 다시 온라인 |
| | PC 절전(서비스를 멈춰 둠, 랜카드 주소 삭제) | |
| W11 | 끝난 작업의 1분 안에 다시 켜기 요청 | 429 `rate_limited`, `retry_after` 18~22 |
| W12 | 중계기를 끈 지 1분 뒤 켜기 요청 | 503 `relay_offline`, `last_seen`은 끄기 직전(0~1초 전) |
| W13 | NAS의 랜카드를 /25로 바꾸고(PC는 /24) 중계기를 다시 시작 → 켜기 요청 | 같은 키라 새 승인 없이 바로 active. 202 `queued`(`overlap`) → 12프레임이 255.255.255.255와 NAS 자신의 브로드캐스트 192.0.2.127로 가고, PC 쪽 브로드캐스트 192.0.2.255로는 0. 다른 LAN 0 → `sent` |
| W14 | PC 재개(주소 복구, 멈춘 서비스를 이어 돌림) | 1초 안에 PC의 동기화(서비스가 시계가 뛴 것을 봄), 4초 안에 둘째 작업 `confirmed`, hbbs에 다시 온라인 |
| W15 | `admin relay revoke <중계기 ID>` | 중계기가 바로 알고 로그에 적음. 판정에 중계기 없음, 목록에 "켜기" 없음, `relay list`에 revoked 1 |
| L1 | 로그와 운영 보고 | hjv-api 로그와 운영자 기록에 MAC, LAN 주소, PC ID 없음. 중계기 로그의 MAC은 가려짐. 포탈 비밀번호 2개, 토큰·계정 키 6개는 각 클라이언트의 LocalConfig 밖에, hjv-api의 키 4개는 자격 증명 폴더 밖에 없음. `admin report`에 켜기 정보를 보고한 PC 1, `confirmed` 2 |

- 걸린 시간: 스크립트만 1분 47초~1분 54초(열다섯 번). `scripts/ci.sh wol` 전체는 네 번 3분 33초~3분 50초였습니다(새로 받은 트리의 클라이언트 빌드 45~51초, 켜기 정보 보고의 단위 시험 6개와 그 빌드 51~60초, 나머지가 스크립트. hjv-api와 hjv-relay는 그대로면 다시 빌드하지 않음). `E2E_CLIENT`로 빌드해 둔 클라이언트를 주면 1분 53초이고, 의존성까지 처음 빌드한 실행은 8분 16초였습니다.
- 그 시간의 대부분은 hjv-api의 규칙 세 가지를 기다리는 것입니다. PC는 마지막으로 본 뒤 30초 동안 켜져 있다고 보고, 중계기는 마지막 요청 뒤 1분 동안 연결된 것으로 보고, 같은 PC는 1분에 한 번만 켭니다. 값은 설정이 아니라 코드에 있어 줄일 수 없습니다. 첫 30초에는 F0~F2를 돌립니다. PC 등록과 로그인(W1~W5)이 20초 넘게 걸리는 느린 실행에서는 승인이 중계기가 쉬는 사이에 떨어져 30초까지 더 걸립니다(아래 "승인 직후의 중계기").
- W11은 첫 작업의 1분 안에, PC가 다시 잠든 지 30초가 지난 때에만 볼 수 있습니다. 아주 느린 실행에서 그 틈이 없으면 "skipped" 한 줄을 찍고 건너뜁니다. 지금까지의 실행에서는 요청 뒤 38~42초에 확인했고(여유 15초 이상), 다른 프로세스 40개가 CPU를 다 쓰는 동안 돌린 실행(1분 54초)에서도 건너뛰지 않았습니다.

**검사가 실패하는지**: 다섯 가지를 일부러 틀리게 해서 같은 스크립트를 돌렸고, 각각 그 자리에서만 실패했습니다(커밋하지 않은 확인).

| 틀리게 한 것 | 실패한 곳 |
|---|---|
| PC가 다른 MAC을 보고(그 MAC을 보는 확인 둘은 빼고) | W8: 작업은 `sent`, 패킷 12라고 하지만 PC 랜카드에 그 PC의 프레임이 0 |
| NAS의 LAN A 랜카드에 게이트웨이가 없음 | W7: 승인 도구가 "같은 망"이 아니라며 승인하지 않음 |
| 다른 명령 키를 믿게 빌드한 중계기 | F1: "다른 키의 서명" 명령이 패킷 4개로 나감 |
| PC의 실행 파일이 설치 경로가 아님 | W1: `this device: added`가 없음 |
| 토큰을 실행 폴더의 다른 파일에 적어 둠 | L1: 그 파일 이름을 대며 실패(값은 찍지 않음) |

`scripts/ci.sh wol`을 한 번 돌린 뒤에는 빌드 없이 스크립트만 다시 돌릴 수 있습니다.

```bash
CI=$PWD/build/ci
HJV_E2E_WORK=$CI/e2e HJV_E2E_KEYS=$CI/e2e/api CADDY=$(ls -d $CI/caddy-*/caddy) \
  HBBS=$CI/target/release/hbbs HBBR=$CI/target/release/hbbr CLIENT=$CI/target-e2e/release/rustdesk \
  HJV_API=$CI/target-api-e2e/release/hjv-api HJV_FAKE_SSO=$CI/target-api-e2e/release/hjv-fake-sso \
  HJV_RELAY=$CI/target-relay/release/hjv-relay tools/e2e/wol-e2e.sh
# 기록은 $HJV_E2E_WORK/run-wol에 남습니다: 서버·중계기·PC의 로그, 받은 프레임(capture-*.txt), 관리 명령의 출력
```

손으로 빌드할 때는 계정 단계의 5)에 중계기를 더합니다: `(cd relay && HJV_SERVER=hjv.test HJV_COMMAND_PUBKEY=$(cat $KEYS/command.pub) cargo build --release)`. 클라이언트에는 패치 0035가 있어야 합니다.

필요한 것은 무권한 사용자 네임스페이스(Ubuntu 24.04가 AppArmor로 막아 둔 곳은 `sudo sysctl kernel.apparmor_restrict_unprivileged_userns=0`)와 `unshare`(util-linux), `ip`(iproute2), `python3`, `curl`입니다. 작업 폴더와 프로그램은 `/run` 아래만 아니면 됩니다(스크립트가 네임스페이스 안에서 `/run`을 자기 것으로 바꿈).

`capture.py`는 따로도 쓸 수 있습니다. PC와 같은 망의 리눅스 기기에서 `sudo python3 -I tools/e2e/capture.py eth0 <PC의 MAC>`을 띄워 두면 NAS가 보낸 매직 패킷이 그 망에 도는지 보입니다.

### 원격 켜기 단계로 찾은 것

- **승인 직후의 중계기**: 승인을 기다리는 중계기는 30초마다 묻고, hjv-api는 그 요청을 20초까지 붙잡습니다. 붙잡힌 동안 승인하면 바로 답이 가서 로그에 `등록이 승인되어 켜기 명령을 기다립니다.`가 찍힙니다(W7). 쉬는 30초 사이에 승인하면 두 가지가 다릅니다.
  - 로그: 중계기는 다음 요청 때 알게 되는데, 그 요청은 이미 승인된 중계기의 것이라 켜기 명령이 없으면 20초를 다 채우고 답이 옵니다. 그래서 로그에는 승인 뒤 길게는 50초쯤에 찍힙니다(처음 판에서 봄). 런북의 "1분 안"에 듭니다.
  - 바로 누른 켜기: hjv-api는 승인 전의 마지막 요청으로도 중계기를 1분 동안 연결된 것으로 보므로 켜기 요청을 202로 받습니다. 하지만 명령은 중계기가 다음에 물을 때(길게는 30초 뒤) 전달됩니다. 앱에는 그동안 "전달하는 중"이 보이고, 명령은 60초 동안 유효하므로 켜지기는 합니다. 스크립트는 5초 안의 `sent`를 보려고, 승인된 중계기가 처음 묻기를 기다린 뒤에 켜기를 요청합니다. 승인을 일부러 그 사이로 늦춘 실행에서는 24초를 기다렸고 전체가 2분 33초였습니다.
- **다시 시작한 직후의 중계기**: 중계기는 hello를 보낸 2초 뒤에 첫 요청을 보냅니다(요청 사이의 최소 간격). hjv-api는 그 요청이 와야 연결된 것으로 보므로, 그 사이의 켜기 요청은 503 `relay_offline`을 받습니다(처음 판의 W13이 여기서 실패). 스크립트는 앱의 판정(`relay.online`)이 참이 될 때까지 기다린 뒤 요청합니다. NAS가 막 다시 켜진 2초 동안의 일이라 제품은 그대로 둡니다.
- **절전에서 깨어난 PC**: 멈췄다 이어서 도는 서비스는 시계가 30초 넘게 뛴 것을 보고 바로 동기화합니다. hjv-api는 그 동기화로 작업을 `confirmed`로 바꾸고(W14, 다음 5초 점검 때), hbbs 연결도 곧 다시 맺어집니다. 종료했다 켜진 PC(W10)는 리눅스에서 서비스가 시작할 때 설정 동기화를 3초 기다린 뒤라 그만큼 늦습니다(upstream의 리눅스·macOS 전용 동작).
- **나가는 주소가 다른 NAS**: 기본 경로가 다른 랜카드에 있는 NAS는 PC와 나가는 주소가 다릅니다. 승인 출력의 `the same egress address as the PC`는 `no`이지만, 공인 주소 망에서는 망과 게이트웨이만 보므로 `exact`로 잡힙니다.
- **NAT 없이 서버에 닿는 사설 망**(고치지 않음, 서버 쪽 규칙): hjv-api는 사설 주소 망(RFC 1918, 100.64.0.0/10)이면 PC와 중계기의 나가는 주소가 같아야 같은 곳으로 봅니다. 공유기 뒤의 집이나 사무실은 그렇습니다. 그런데 서버가 같은 사내망에 있어 사설 주소를 그대로 보는 망에서는 PC와 NAS의 주소가 서로 다르므로 늘 "다른 곳"이 됩니다. 이 구성에서 LAN A만 192.168.77.0/24로 바꿔 돌려 보면 W7의 승인이 `no (a private network of another site: the egress addresses differ)`로 멈춥니다. `--force`로 승인해도 켜기 요청은 409 `no_relay`(`different_egress`)입니다. 요청에 `{"any_relay":true}`를 넣으면 받아 주지만(202, `any`), 앱은 이 경우 "모든 중계기로 시도"를 숨기므로 사용자가 켤 길이 없습니다. 그런 망의 PC를 켜려면 hjv-api의 규칙(`wolinfo.rs`의 판정)을 넓혀야 합니다.
- 시험 구성에서 고친 것: Caddy는 CA를 만든 조금 뒤에 서버 인증서를 받습니다. 그 사이에 뜬 중계기는 TLS 오류로 5초쯤 물러났다가 다시 붙습니다. 스크립트는 443과 8443이 TLS로 답할 때까지 기다린 뒤 중계기를 띄웁니다.

### 원격 켜기 단계가 다루지 않는 것

- 실제 랜카드와 메인보드가 매직 패킷으로 켜지는지(절전, 최대 절전, 종료, BIOS 설정): 현장 시험([relay-dsm.md](relay-dsm.md) 부록, [wol-pc-setup.md](wol-pc-setup.md)).
- Windows의 켜기 정보 수집(레지스트리, 전원 설정)과 깨어난 뒤 10분 동안 잠들지 않게 하는 것: 단위 시험과 실기기 점검(설계 7-6). 여기서는 PC의 보고를 헤드리스 명령이 대신합니다.
- 중계기의 권한 내리기, DSM 부트업 작업의 감시 루프, musl 정적 실행 파일: `relay/`의 시험과 `scripts/package-relay.sh`. 여기서는 개발 PC에서 빌드한 실행 파일을 이 사용자로 돌립니다.
- 사설 망에서 나가는 주소 비교, "모든 중계기로 시도", 전에 켠 적 있는 망(history), 서버 쪽 한도(계정 시간당 20번, 중계기 분당 10번): hjv-api의 시험(`tests/wake.rs`, `tests/relay.rs`).
- 앱의 "켜기" 메뉴와 진행 창: Dart 시험. 켠 뒤의 연결은 계정 단계의 비밀번호 없는 연결 그대로입니다.
- nginx(운영의 웹 서버)의 중계기 등록 한도: `scripts/ci.sh deploy`.
