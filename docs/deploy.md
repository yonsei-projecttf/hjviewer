# 서버 배포

공개 포트는 **443(HTTPS)과 80(인증서 발급)**뿐입니다. TLS 인증서는 Caddy가 Let's Encrypt에서 자동으로 발급하고 갱신합니다.

## 준비물

- 리눅스 VM 1대: 2 vCPU / 2 GB면 PoC에 충분합니다. 중계 트래픽은 대역폭을 쓰므로 트래픽 요금을 확인하세요. 사용자가 한국에 있으면 서울 리전을 권장합니다.
- Docker와 Docker Compose v2
- 도메인 하나: 예를 들어 `rd.example.com`의 A 레코드를 VM의 공인 IP로 지정합니다.
- 클라우드 보안 그룹에서 인바운드 **TCP 80, 443**을 엽니다. 다른 포트는 열지 않습니다.

## 실행

```bash
git clone https://github.com/ysfaculty/hjviewer.git && cd hjviewer/deploy
cp .env.example .env          # HJV_DOMAIN=rd.example.com 으로 수정
docker compose up -d --build  # 첫 빌드는 hbbs/hbbr 컴파일 때문에 몇 분 걸림
docker compose logs -f        # hbbs/hbbr 로그에 같은 "Key: ..." 가 찍히는지 확인
cat data/id_ed25519.pub       # 이 값이 클라이언트 빌드의 HJV_SERVER_KEY
```

`data/id_ed25519`는 서버의 개인키입니다. 잃어버리면 이미 배포한 모든 앱이 서버를 신뢰하지 못하게 되니 **반드시 백업**하세요. 외부에 노출되면 안 됩니다.

## 확인

어느 PC에서든 실행합니다. 고객사 네트워크처럼 방화벽이 있는 환경에서 실행하는 것이 가장 의미 있습니다.

```bash
cd tools/hjv-probe && cargo build --release
./target/release/hjv-probe rd.example.com --peer <PC의 ID>
```

```
[ OK ] TCP connect (12 ms) 203.0.113.10:443
[ OK ] TLS + WebSocket /ws/id (85 ms)
[ OK ] rendezvous round trip (hbbs) (9 ms) 123456789=online
[ OK ] TLS + WebSocket /ws/relay (hbbr) (80 ms)
all checks passed: apps on this network can reach the server over 443
```

실패한 단계에는 원인 후보가 함께 출력됩니다. 예를 들어 TLS 실패는 TLS 검사 프록시, WebSocket 실패는 Upgrade 헤더를 제거하는 프록시를 뜻합니다.

## 구성 설명

| 파일 | 내용 |
|---|---|
| `deploy/Dockerfile` | 고정된 upstream 커밋 + `server/patches`로 hbbs/hbbr 빌드 |
| `deploy/docker-compose.yml` | caddy(80/443 공개), hbbs, hbbr(내부 전용) |
| `deploy/Caddyfile` | `/ws/id`는 hbbs:21118, `/ws/relay`는 hbbr:21119로 전달, `X-Real-IP`/`X-Real-Port` 전달, `/api/*`는 404 |

- **hbbs의 WS 포트(21118)를 외부에 공개하면 안 됩니다.** hbbs는 `X-Real-IP`/`X-Real-Port` 헤더를 그대로 믿기 때문에, 직접 접속할 수 있으면 주소를 위조할 수 있습니다.
- `hbbr -k _`는 hbbs와 같은 **공개키**를 요구합니다. 공개키는 앱에 들어 있으므로, 이 설정은 다른 서버용으로 빌드된 RustDesk 앱을 막는 정도입니다. 이 서버용 앱을 가진 사람은 누구나 중계를 쓸 수 있습니다. 소수만 쓰는 서버는 아래 "소규모 비공개 운영"을 참고하세요.
- Caddy 앞에 클라우드 로드밸런서를 두면 `{remote_host}` 대신 클라이언트 IP 헤더를 써야 합니다. Caddy의 `trusted_proxies`와 `{client_ip}`를 참고하세요.

## 용량 산정 (실측 기반)

2026-10-10에 Intel i7-14700 PC에서 이 저장소의 서버 구성(Caddy → hbbs/hbbr)을 띄우고 측정했습니다. `tools/hjv-load`로 부하를 걸었고, 세션 비트레이트는 실제 클라이언트로 쟀습니다(`HJV_E2E_BITRATE=1 tools/e2e/firewall-e2e.sh strict`).

| 항목 | 측정값 |
|---|---|
| 대기 중 메모리 | Caddy 50MB + hbbs 11MB + hbbr 5MB = **약 70MB** |
| PC 25대 상시 등록 (10초마다 하트비트) | CPU 0.1% 미만, 메모리 변화 거의 없음 |
| 중계 5Mb/s | CPU 한 코어의 1.2% (Caddy TLS 0.9% + hbbr 0.3%) |
| 중계 25Mb/s (5세션) | 한 코어의 5% |
| 중계 100Mb/s (10세션) | 한 코어의 **17%** (Caddy 12.6% + hbbr 4.5%), 메모리 변화 없음 |

원격 세션 하나의 영상 비트레이트(1280×720, VP9, 기본 화질):

| 화면 | 비트레이트 |
|---|---|
| 변화 없음 | 약 0 (화면이 바뀔 때만 전송) |
| 문서 스크롤 | 0.3Mb/s |
| 화면 전체가 계속 바뀜 (최악) | 30Mb/s |

실제 업무 화면(웹, 문서, 가끔 이미지)은 1080p 기준 대략 0.5~2Mb/s, 동영상 재생은 수 Mb/s로 보면 됩니다. 이 값은 측정이 아닌 추정입니다.

**산정 방법**: 서버가 쓰는 대역폭은 **중계되는 세션에만** 생깁니다. P2P로 직접 연결된 세션은 서버 대역폭을 쓰지 않습니다. 중계 세션은 받은 만큼 내보내므로, 서버의 수신과 송신이 각각 세션 비트레이트만큼 늘어납니다.

- 최대 대역폭 ≈ 동시 중계 세션 수 × 세션 비트레이트 (수신·송신 각각)
- 월 트래픽(송신) ≈ 사용자 수 × 하루 사용 시간 × 평균 비트레이트 × 중계 비율 × 30일
- CPU ≈ 중계 100Mb/s당 빠른 코어 하나의 17% (클라우드 vCPU는 2배 정도로 잡기)

**사용자 10명 이내라면** 다음과 같습니다.

- 동시 세션 3~5개, 세션당 평균 1~2Mb/s, 모두 중계되는 최악의 경우로 잡습니다. 최대 약 10~20Mb/s(동영상까지 포함하면 50Mb/s), CPU 한 코어의 수 %입니다.
- 월 트래픽은 10명 × 하루 1시간 × 1Mb/s × 30일 ≈ **월 135GB 송신**(전부 중계일 때)입니다. P2P로 붙는 만큼 줄어듭니다.
- 권장 사양은 **1 vCPU / 1GB RAM / 디스크 10GB**이면 충분합니다. 다만 서버에서 `docker compose up --build`로 hbbs를 직접 컴파일하려면 2GB 이상의 RAM(또는 스왑)이 필요합니다. 메모리가 작으면 다른 PC에서 이미지를 만들어 옮기세요.
- 실제 사용량은 hbbr 로그(`docker compose logs hbbr`)와 서버의 네트워크 사용량으로 확인합니다. 서버에서 `hjv-load relay`로 직접 측정해 볼 수도 있습니다.

## 소규모 비공개 운영 (사용자 10명 이내)

이 저장소는 공개입니다. 그래서 서버 주소와 공개키를 숨기는 것만으로는 다른 사람을 막을 수 없습니다. 소스로 앱을 빌드하면 누구나 서버를 쓸 수 있으므로, 다음 순서로 노출을 줄이세요.

1. **실제 서버 값으로 빌드한 앱을 공개하지 않습니다.** 공개 저장소의 CI는 예시 값으로만 돌립니다. 실제 앱은 비공개 미러 저장소의 CI나 로컬에서 빌드해서 사용자 10명에게만 직접 나눠 줍니다. 비공개 저장소에서 빌드할 때도 `HJV_SERVER`/`HJV_SERVER_KEY`는 시크릿으로 넣으면 로그에 남지 않습니다.
2. **접속은 비밀번호로 보호됩니다.** 서버를 쓸 수 있는 것과 남의 PC에 들어가는 것은 다른 문제입니다. 각 PC의 영구 비밀번호를 길게 설정하세요. 서버 운영자도 세션 내용을 볼 수 없습니다(종단 간 암호화).
3. **낯선 기기를 확인합니다.** 등록된 기기 목록은 `data/db_v2.sqlite3`의 `peer` 테이블에 있습니다.
   ```bash
   docker compose exec hbbs sh -c 'apt-get -qq update >/dev/null; apt-get -qq install -y sqlite3 >/dev/null; sqlite3 /data/db_v2.sqlite3 "select id, created_at, info from peer"'
   ```
4. **중계 대역폭에 상한을 둡니다.** hbbr은 환경 변수로 중계 속도를 제한합니다. 예: `TOTAL_BANDWIDTH=100`(Mb/s, 전체), `SINGLE_BANDWIDTH=20`(Mb/s, 세션당). docker-compose의 hbbr에 `environment:`로 넣으세요.
5. **서버 방화벽**에서는 80/443 외 인바운드를 모두 닫습니다. compose는 hbbs/hbbr 포트를 외부에 공개하지 않습니다.

## 대안: 공인 IP 없이 Cloudflare Tunnel로 띄우기

사내망 서버나 개발 PC처럼 **인바운드 포트를 열 수 없는 곳**에서도 서버를 운영할 수 있습니다. `cloudflared`가 Cloudflare로 나가는 연결을 열고, 그 연결로 `https://<도메인>`이 들어옵니다. TLS는 Cloudflare가 처리하므로 Caddy는 HTTP로만 받습니다.

1. Caddy 사이트 주소를 `http://<도메인>:8080`처럼 HTTP로 바꾸고, compose에서 80/443 대신 내부 포트만 둡니다.
2. 도메인이 Cloudflare에 있다면 named tunnel(`cloudflared tunnel create`, ingress `service: http://caddy:8080`)로 고정 주소를 씁니다.
3. 잠깐 시험만 할 때는 계정 없이 `cloudflared tunnel --url http://localhost:8080`도 됩니다. 이 경우 `https://<임의>.trycloudflare.com` 주소가 나오는데 **재시작할 때마다 바뀝니다**. 앱에는 빌드 시점의 주소가 박히므로, 주소가 바뀌면 다시 빌드해야 합니다.

주의: 이 구성에서 hbbs가 보는 클라이언트 IP는 모두 cloudflared(127.0.0.1)입니다. 연결 구분은 `X-Real-Port`로 되지만, IP별 등록 제한(분당 약 30회)을 모든 기기가 나눠 씁니다. 시험용이나 소규모용으로만 쓰세요. 실제 IP를 쓰려면 Caddy에서 `X-Real-IP`를 `{http.request.header.CF-Connecting-IP}`로 넘기면 됩니다.

## 업데이트

패치나 upstream 고정 커밋(`scripts/upstream.env`)이 바뀌면 다음을 실행합니다.

```bash
git pull && docker compose up -d --build
```
