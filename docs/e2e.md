# 방화벽 E2E 검증

실제 HJViewer 클라이언트 두 대와 서버를 리눅스 한 대에서 띄웁니다. **회사 방화벽처럼 서버로 가는 HTTPS(TCP 443)만 허용한 네트워크**에서 접속, 데이터 전달, 화면 공유가 되는지 확인합니다.

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
