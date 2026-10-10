# HJViewer 아키텍처

TeamViewer처럼 **모바일 앱(iOS/Android)에서 Windows PC 화면을 보고 제어**하는 원격 데스크톱 서비스입니다.
핵심 요구사항은 **어떤 방화벽·NAT 뒤에 있어도 연결되는 것**입니다.
오픈소스 [RustDesk](https://github.com/rustdesk/rustdesk)(클라이언트 1.5.0)와 [rustdesk-server](https://github.com/rustdesk/rustdesk-server)(1.1.17)를 포크하고, 방화벽 통과에 필요한 부분만 패치합니다.

## 1. 방화벽을 넘는 원리

방화벽을 "뚫는" 것이 아니라 **양쪽 모두 밖으로만 연결**합니다. 거의 모든 네트워크가 허용하는 **HTTPS(TCP 443)**만 사용합니다.

```
 [Windows PC 에이전트]                [중계 서버 (클라우드)]                 [iOS / Android 앱]
        │  wss://서버/ws/id  (상시 연결)   ┌──────────────┐   wss://서버/ws/id        │
        ├──────────────────────────────▶ │ Caddy :443   │ ◀────────────────────────┤
        │                                │  ├─ /ws/id    → hbbs (ID·시그널링)       │
        │  wss://서버/ws/relay (필요 시)   │  ├─ /ws/relay → hbbr (중계)              │
        ├──────────────────────────────▶ │  └─ /api/*    → (향후 계정 API)          │
        │                                └──────────────┘                          │
        │                                                                          │
        └──────────── WebRTC P2P (UDP, 네트워크가 허용하면 직접 연결) ─────────────────┘
```

연결은 두 경로를 **동시에** 시도하고 먼저 성공하는 쪽을 씁니다(P2P 우선).

| 네트워크 상황 | 사용 경로 | 비고 |
|---|---|---|
| 가정·일반 NAT | WebRTC P2P (UDP 직접) | 지연 최소, 서버 대역폭 0 |
| UDP 차단, 443만 허용 (일반 회사망) | hbbr 중계 over `wss://…/ws/relay` | P2P 실패 후 약 2.5초 안에 전환 |
| 명시적 HTTP/SOCKS5 프록시만 허용 | 앱 네트워크 설정에 프록시 입력 → CONNECT 터널 위 wss | 패치로 지원 (upstream은 WS 연결이 프록시를 무시함). HTTPS 프록시는 미지원 |
| TLS 검사(복호화) 프록시 | 기기에 회사 루트 CA가 설치돼 있으면 통과 | 미설치 기기는 TLS 실패 → `hjv-probe`로 진단. `allow-insecure-tls-fallback` 옵션으로 허용할 수 있음(서버 인증은 약해지지만 세션은 종단 간 암호화로 보호) |
| 관리자가 원격제어 앱 범주·도메인을 명시적으로 차단 | 연결 안 됨 | 정책 차단은 우회하지 않습니다 (TeamViewer도 동일) |

서버에서 외부에 여는 포트는 **443(과 인증서 발급용 80)뿐**입니다. hbbs/hbbr의 포트(21115–21119)는 내부망에만 둡니다.

## 2. 구성 요소

| 구성 요소 | 역할 | 출처 |
|---|---|---|
| PC 에이전트 (Windows) | 화면 캡처·입력 주입. Windows 서비스로 실행되어 UAC·로그인 화면도 제어 | RustDesk 클라이언트 + `client/patches` |
| 모바일 앱 (Flutter) | 제어 측. Android는 피제어도 가능(접근성 서비스) | 같은 코드베이스 |
| hbbs | 기기 ID·공개키 등록, 온라인 상태, 접속 요청·WebRTC 시그널링 전달 | rustdesk-server + `server/patches` |
| hbbr | P2P 실패 시 두 연결을 짝지어 암호화된 바이트를 그대로 전달 | rustdesk-server |
| Caddy | TLS 종료(Let's Encrypt 자동), 경로별 라우팅 | `deploy/Caddyfile` |

**종단 간 암호화**: 세션 키는 두 기기가 직접 교환합니다. hbbs가 서명한 기기 공개키로 상대를 검증하고, WebRTC에서는 상대가 서명한 DTLS 지문으로 다시 검증합니다. 따라서 hbbs와 hbbr은 화면 내용을 볼 수 없습니다.

## 3. 접속 순서

1. **PC 등록**: PC가 `wss://서버/ws/id`에 접속해 `RegisterPk`(ID, 공개키)를 보내고 연결을 유지합니다. hbbs는 10초마다 빈 프레임(하트비트)을 보내고 PC가 되돌려 보냅니다. 연결이 끊기면 즉시 오프라인 처리됩니다.
2. **접속 요청**: 앱이 `PunchHoleRequest`(대상 ID, WebRTC offer)를 보냅니다.
3. **전달**: hbbs가 PC의 상시 연결로 `PunchHole`(offer, 앱의 주소 키)을 보냅니다.
4. **응답**: PC가 새 wss 연결로 answer를 보내고, 중계 경로(hbbr)도 함께 준비합니다. 두 기기의 ICE 후보는 hbbs를 거쳐 오갑니다.
5. **경합**: 앱은 WebRTC P2P와 중계를 동시에 진행하고 P2P를 우선합니다. 2.5초 안에 P2P가 안 되면 중계를 씁니다.
6. **인증·세션**: 서로의 신원을 검증하고 세션 키를 교환한 뒤 PC 비밀번호 또는 수락 창으로 접속을 허용합니다.

## 4. upstream 대비 변경점

### 서버 (`server/patches`)
오픈소스 hbbs에는 방화벽 환경에 필요한 기능이 **빠져 있었습니다** (소스 분석 결과).

| 문제 | 영향 | 패치 |
|---|---|---|
| PC 등록을 UDP로만 받음 (TCP/WS의 `RegisterPk`에 `NOT_SUPPORT` 응답) | UDP가 막힌 PC는 **영원히 오프라인** | TCP/WS 등록 + 상시 연결 + 하트비트, 해당 연결로 요청 전달 |
| WebRTC offer/answer를 버리고 ICE 후보를 처리하지 않음 | 자체 서버로는 **P2P 불가**, 전부 중계 | offer·answer·`force_relay` 전달, ICE 후보 양방향 라우팅 |
| 응답 후 연결을 맵에서 제거 | answer 이후 ICE 후보 전달 불가 | 연결마다 송신 큐, 연결이 닫힐 때만 정리 |
| WS 클라이언트를 `IP:0`으로 식별 | 같은 회사(NAT) 사용자끼리 응답이 뒤섞임 | 프록시의 `X-Real-Port`로 구분 |
| WS로 온 `OnlineRequest` 무시 | 앱에서 기기 온라인 표시 안 됨 | WS 엔드포인트에서 응답 |
| hbbr이 동시에 도착한 중계 요청 두 개를 짝짓지 못함 (경쟁 상태) | 양쪽이 같은 순간 중계에 붙으면 세션이 30초 후 실패 | 찾기와 대기 등록을 한 락 안에서 처리 |

검증: 통합 테스트 `tests/tcp_signaling.rs`(2개)와 `tests/relay_pairing.rs`(동시 도착 100쌍)를 추가했습니다. 모두 패치 전 실패하고 패치 후 통과합니다.
마지막 hbbr 버그는 아래의 방화벽 E2E 검증에서 실제 클라이언트로 발견했습니다.

알려진 한계 (upstream과 동일):
- **IP별 등록 제한**: 공인 IP 하나당 분당 약 30회, 하루 300개 ID까지 등록할 수 있습니다. PC 수백 대가 공인 IP 하나를 공유하는 대형 사무실에서는 서버 재시작 직후 일부 PC가 최대 1분 늦게 등록되고, 300대를 넘으면 새 ID 등록이 거부됩니다. 기업 고객을 받으려면 이 제한을 설정값으로 바꿔야 합니다.
- **서버 단일 인스턴스**: hbbs는 1대로 운영합니다(SQLite). 확장은 중계(hbbr)를 늘리는 방식입니다.

### 클라이언트 (`client/patches`)
- 빌드할 때 `HJV_SERVER`(도메인)와 `HJV_SERVER_KEY`(hbbs 공개키)를 넣으면 서버 주소·키가 고정되고, 사용자 설정 화면에서 숨겨집니다.
- **WebSocket을 강제**합니다. 신호와 중계는 항상 443을 쓰고, WebRTC는 기본으로 켭니다(upstream은 공식 서버에서만 켭니다).
- upstream 업데이트 확인과 자동 업데이트를 끕니다.
- 앱 이름은 `HJViewer`, Android/iOS 식별자는 `kr.ac.yonsei.hjviewer`입니다. 아이콘은 HJ 모노그램을 띄운 모니터입니다. 원본 SVG는 `client/icons`에 있고, `scripts/make-icons.sh`가 플랫폼별 파일을 모두 `client/overlay`에 다시 만듭니다(같은 원본이면 같은 파일). Windows 실행 파일 정보와 Windows 실행 파일 정보와 정보(About) 화면의 링크는 이 저장소를 가리킵니다.
- (hbb_common) **WebSocket 연결이 프록시 설정을 따릅니다.** upstream은 WS 연결에서 프록시를 무시해서(`to-do: websocket proxy`) 프록시로만 나갈 수 있는 회사망에서는 연결되지 않았습니다. HTTP CONNECT와 SOCKS5 터널 위에서 TLS와 WebSocket 핸드셰이크를 하도록 고쳤습니다. 테스트는 패치 전 실패, 패치 후 통과합니다.

검증: 전체 앱 빌드는 CI에서만 할 수 있습니다. 대신 설정 모듈을 실제 hbb_common/base 크레이트와 함께 별도로 실행해서 확인했습니다(`wss://<서버>/ws/id`와 `/ws/relay` 변환, 서버 고정, WebRTC 기본값). hbb_common 단위 테스트 101개도 모두 통과합니다.

### 방화벽 E2E 검증 ([e2e.md](e2e.md))

실제 HJViewer 클라이언트(Linux 빌드, 같은 Rust 코어) 두 대를 네트워크 네임스페이스에 넣고, 서버로 가는 TCP 443만 허용하는 방화벽을 걸어 검증했습니다.
등록, 비밀번호 로그인, 데이터 전달, **원격 화면 수신**이 모두 동작했습니다. 443만 열려 있으면 중계(wss)로, 피어 간 UDP가 열려 있으면 WebRTC P2P로 연결됩니다.

## 5. 플랫폼 제약

- **iOS**: 다른 앱에 입력을 주입하는 API가 없어서 **iPhone은 원격 제어할 수 없습니다**. 제어 측 전용입니다. 화면 공유(보기)는 ReplayKit 방송으로만 가능하며 추후 과제입니다.
- **Android**: 제어 측과 피제어 측이 모두 가능합니다(MediaProjection + 접근성 서비스). Google Play는 접근성 API 사용 목적을 고지·심사합니다.
- **보이스피싱 악용**: 원격제어 앱은 사기에 악용되는 대표 수단입니다. 국내 금융 앱은 원격제어 앱 실행을 탐지합니다. 스토어 심사와 사용자 신뢰를 위해 접속 수락 UI, 세션 중 표시, 접속 이력 같은 기능을 제품 요구사항에 넣어야 합니다.

## 6. 라이선스·상표

- RustDesk 클라이언트와 서버는 모두 **AGPL-3.0**입니다. 수정한 앱을 배포하거나 수정한 서버를 네트워크로 서비스하면, 해당 사용자에게 **수정본 소스를 제공할 의무**가 있습니다. 저장소가 비공개여도 이 의무는 남습니다.
- "RustDesk" 이름과 로고는 쓸 수 없습니다. 앱 이름, 아이콘, Windows 실행 파일 정보, 정보 화면 링크를 HJViewer로 바꿨습니다. Windows 실행 파일은 `HJViewer.exe`이고, Windows의 URL 스킴은 그에 따라 `hjviewer://`입니다. 모바일의 URL 스킴(`rustdesk://`)은 아직 upstream 그대로입니다.
- 저장소 루트의 `LICENSE`(AGPL-3.0 전문)와 `NOTICE`(upstream 저작권자와 변경 범위)가 고지이고, 모든 빌드 결과물에도 라이선스·소스 위치·서드파티 고지 파일이 함께 들어갑니다.
- RustDesk의 프린터 드라이버 어댑터 DLL은 소스가 공개되지 않아 AGPL 배포 조건을 지킬 수 없으므로 Windows 빌드에서 뺐습니다(원격 인쇄 꺼짐).

## 7. 다음 단계

1. 서버 배포 → `hjv-probe`로 실제 회사망·모바일망에서 검증 ([deploy.md](deploy.md))
2. CI로 Windows/Android/iOS 빌드 ([ci.md](ci.md)) → 실기기 연결 테스트 (P2P와 중계 각각)
3. 브랜딩 마무리: 모바일 URL 스킴(`rustdesk://`)
4. 계정과 기기 관리 API(`/api`): 로그인, 내 기기 목록, 접속 이력. 오픈소스 hbbs에는 없어서 직접 구현해야 합니다.
5. 운영: 중계 트래픽 모니터링(세션당 대략 수 Mbps), hbbr 증설(`hbbs -r relay1,relay2`), 키 파일 `id_ed25519` 백업, ICE 후보 전달 속도 제한
6. 선택: TURN 서버(coturn). UDP는 되지만 P2P가 실패하는 대칭형 NAT에서 WS 중계보다 지연이 적습니다.
