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
- `hbbr -k _`로 실행하므로 서버 키를 가진 클라이언트만 중계 서버를 쓸 수 있습니다.
- Caddy 앞에 클라우드 로드밸런서를 두면 `{remote_host}` 대신 클라이언트 IP 헤더를 써야 합니다. Caddy의 `trusted_proxies`와 `{client_ip}`를 참고하세요.

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
