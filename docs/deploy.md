# 서버 배포

공개 포트는 **443(HTTPS)과 80(인증서 발급)**뿐입니다. TLS 인증서는 Caddy가 Let's Encrypt에서 자동으로 발급하고 갱신합니다.

## 준비물

- 리눅스 VM 1대: 2 vCPU / 2 GB면 PoC에 충분합니다. 중계 트래픽은 대역폭을 쓰므로 트래픽 요금을 확인하세요. 사용자가 한국에 있으면 서울 리전을 권장합니다.
- Docker와 Docker Compose v2
- 도메인 하나: 예를 들어 `rd.example.com`의 A 레코드를 VM의 공인 IP로 지정합니다.
- 클라우드 보안 그룹에서 인바운드 **TCP 80, 443**을 엽니다. 다른 포트는 열지 않습니다.

## 실행

80/443이 비어 있는 전용 VM이라면 다음과 같습니다(런북의 A 경로). 이미 nginx나 Apache가 80/443을 쓰는 서버는 [server-runbook.md](server-runbook.md)의 B 경로를 따르세요. B 경로는 Docker 없이 GitHub Release의 hbbs/hbbr을 systemd 서비스로 127.0.0.1에만 띄우고, 기존 웹 서버에 HJViewer 경로를 include합니다. 런북은 에이전트가 그대로 실행할 수 있게 쓴 전체 절차입니다.

```bash
git clone https://github.com/yonsei-projecttf/hjviewer.git && cd hjviewer/deploy
sudo install -d -m 700 data       # 서버 개인키가 생길 곳: root만 읽을 수 있게
cp .env.example .env              # HJV_DOMAIN=rd.example.com 으로 수정
sudo docker compose build         # 첫 빌드는 hbbs/hbbr 컴파일로 10분 안팎
sudo docker compose up -d
sudo docker compose logs hbbs hbbr | grep 'Key:'   # 두 값이 같아야 함
sudo cat data/id_ed25519.pub      # 이 값이 클라이언트 빌드의 HJV_SERVER_KEY
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
| `deploy/docker-compose.yml` | A 경로: caddy(80/443 공개), hbbs, hbbr(내부 전용) |
| `deploy/Caddyfile` | `/ws/id`는 hbbs:21118, `/ws/relay`는 hbbr:21119로 전달, `X-Real-IP`/`X-Real-Port` 전달, `/api/*`는 404 |
| `deploy/systemd/` | B 경로: hbbs, hbbr의 systemd 유닛과 설정 예시(`hjviewer.env.example`). 서비스 전용 사용자, 127.0.0.1에만 listen, 권한 없음, CPU·메모리 상한 |
| `scripts/package-server.sh` | B 경로의 GitHub Release 패키지(바이너리, 유닛, 웹 서버 설정, 라이선스 고지)를 만듭니다 |
| `deploy/docker-compose.behind-proxy.yml` | 이미 Docker를 쓰는 서버에서 B 경로 대신 쓸 수 있는 compose 파일: 기존 웹 서버 뒤에서 hbbs, hbbr만 실행. 호스트 네트워크의 127.0.0.1에만 listen |
| `deploy/nginx-hjviewer.conf`, `deploy/apache-hjviewer.conf` | 기존 웹 서버의 사이트 설정. 위 Caddyfile과 같은 일을 합니다 |
| `deploy/nginx-hjviewer-locations.conf` | nginx에 넣을 HJViewer 경로(`/ws/id`, `/ws/relay`, `/api/`). 새 사이트 파일이 include하고, 이미 운영 중인 사이트와 도메인을 함께 쓸 때는 그 사이트의 server 블록에 include 한 줄로 넣습니다 |
| `deploy/.env.example` | compose용 `HJV_DOMAIN`과 선택 설정: `COMPOSE_FILE`(behind-proxy), 이미지 레지스트리, 빌드 병렬 수, 중계 대역폭 상한 |

두 compose 파일은 프로젝트 이름을 `hjviewer`로 고정해, 같은 서버의 다른 compose 프로젝트와 섞이지 않게 합니다. 기반 이미지(rust, debian, caddy)는 기본으로 Docker 공식 이미지의 ECR 미러(public.ecr.aws)에서 받습니다. Docker Hub는 익명 다운로드를 주소마다 제한해서, 학내 NAT처럼 주소를 함께 쓰는 곳에서 막힐 수 있기 때문입니다. hbbs와 hbbr 컨테이너는 Linux 권한(capability)을 모두 버리고 실행합니다.

- **hbbs의 WS 포트(21118)를 외부에 공개하면 안 됩니다.** hbbs는 `X-Real-IP`/`X-Real-Port` 헤더를 그대로 믿기 때문에, 직접 접속할 수 있으면 주소를 위조할 수 있습니다.
- B 경로(systemd)와 behind-proxy compose는 이 때문에 hbbs/hbbr을 127.0.0.1에 묶습니다. 포트를 127.0.0.1에 게시(publish)하는 Docker 방식은 Docker 28 미만에서 같은 LAN의 다른 기기가 닿을 수 있어서 쓰지 않았습니다. 다만 그 서버에 로그인한 다른 사용자는 127.0.0.1에 접속할 수 있습니다.
- 웹 서버는 `X-Real-IP`, `X-Real-Port`, `X-Forwarded-For`를 클라이언트가 보낸 값과 상관없이 항상 덮어써야 합니다. 저장소의 nginx·Apache 설정이 그렇게 하고, `scripts/ci.sh`가 확인합니다.
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
- 운영에는 **1 vCPU / 1GB RAM**이면 충분합니다. 다만 서버에서 이미지를 직접 빌드(`docker compose build`)하는 동안에는 더 필요합니다. 가용 메모리와 스왑을 합쳐 3GB(병렬 2개 기준, `HJV_BUILD_JOBS=1`이면 2GB 정도)와 Docker 디스크 여유 6GB가 있어야 합니다. 빌드 캐시가 약 4GB 남습니다. 모자라면 스왑을 잠시 늘리거나, 다른 PC에서 이미지를 만들어 옮기세요.
- 실제 사용량은 hbbr 로그(`docker compose logs hbbr`)와 서버의 네트워크 사용량으로 확인합니다. 서버에서 `hjv-load relay`로 직접 측정해 볼 수도 있습니다.

## 소규모 비공개 운영 (사용자 10명 이내)

이 저장소는 공개입니다. 그래서 서버 주소와 공개키를 숨기는 것만으로는 다른 사람을 막을 수 없습니다. 소스로 앱을 빌드하면 누구나 서버를 쓸 수 있으므로, 다음 순서로 노출을 줄이세요.

1. **실제 서버 값으로 빌드한 앱을 공개하지 않습니다.** 공개 저장소의 CI는 예시 값으로만 돌립니다. 실제 앱은 비공개 빌드 저장소의 CI에서 빌드해서 사용자 10명에게만 직접 나눠 줍니다([ci.md](ci.md)의 "비공개 빌드 저장소"). 비공개 저장소에서 빌드할 때도 `HJV_SERVER`/`HJV_SERVER_KEY`는 시크릿으로 넣으면 로그에 남지 않습니다.
   - AGPL 때문에 **나눠 주는 빌드는 이 공개 저장소에 올라간 커밋으로만** 만듭니다. 빌드 저장소는 코드를 따로 갖지 않고 공개 저장소의 커밋을 받아 빌드합니다. CI의 `scripts/package-notices.sh`는 빌드한 커밋이 공개 저장소에 없으면 실패합니다.
   - 앱과 함께 결과물에 들어 있는 `LICENSE.txt`, `NOTICE.txt`, `SOURCE.txt`, `THIRD_PARTY_NOTICES.txt`도 전달하세요. 로컬에서 빌드했다면 `scripts/package-notices.sh`로 만들 수 있습니다.
2. **접속은 비밀번호로 보호됩니다.** 서버를 쓸 수 있는 것과 남의 PC에 들어가는 것은 다른 문제입니다. 각 PC의 영구 비밀번호를 길게 설정하세요. 서버 운영자도 세션 내용을 볼 수 없습니다(종단 간 암호화).
3. **낯선 기기를 확인합니다.** 등록된 기기 목록은 `data/db_v2.sqlite3`의 `peer` 테이블에 있습니다. Ubuntu에 기본으로 있는 python3로 읽습니다.
   ```bash
   sudo python3 -c 'import sqlite3; [print(*r) for r in sqlite3.connect("data/db_v2.sqlite3").execute("select id, created_at, info from peer")]'
   ```
4. **중계 대역폭에 상한을 둡니다.** `.env`에 `HJV_TOTAL_BANDWIDTH=100`(Mb/s, 전체)과 `HJV_SINGLE_BANDWIDTH=20`(Mb/s, 세션당)을 넣고 `sudo docker compose up -d`를 실행합니다. 비워 두면 hbbr 기본값(전체 1024, 세션당 128)입니다. compose 파일을 직접 고치면 나중에 `git pull`이 충돌합니다.
5. **서버 방화벽**에서는 80/443 외 인바운드를 모두 닫습니다. compose는 hbbs/hbbr 포트를 외부에 공개하지 않습니다.

## 대안: 공인 IP 없이 Cloudflare Tunnel로 띄우기

사내망 서버나 개발 PC처럼 **인바운드 포트를 열 수 없는 곳**에서도 서버를 운영할 수 있습니다. `cloudflared`가 Cloudflare로 나가는 연결을 열고, 그 연결로 `https://<도메인>`이 들어옵니다. TLS는 Cloudflare가 처리하므로 Caddy는 HTTP로만 받습니다.

1. Caddy 사이트 주소를 `http://<도메인>:8080`처럼 HTTP로 바꾸고, compose에서 80/443 대신 내부 포트만 둡니다.
2. 도메인이 Cloudflare에 있다면 named tunnel(`cloudflared tunnel create`, ingress `service: http://caddy:8080`)로 고정 주소를 씁니다.
3. 잠깐 시험만 할 때는 계정 없이 `cloudflared tunnel --url http://localhost:8080`도 됩니다. 이 경우 `https://<임의>.trycloudflare.com` 주소가 나오는데 **재시작할 때마다 바뀝니다**. 앱에는 빌드 시점의 주소가 박히므로, 주소가 바뀌면 다시 빌드해야 합니다.

주의: 이 구성에서 hbbs가 보는 클라이언트 IP는 모두 cloudflared(127.0.0.1)입니다. 연결 구분은 `X-Real-Port`로 되지만, IP별 등록 제한(분당 약 30회)을 모든 기기가 나눠 씁니다. 시험용이나 소규모용으로만 쓰세요. 실제 IP를 쓰려면 Caddy에서 `X-Real-IP`를 `{http.request.header.CF-Connecting-IP}`로 넘기면 됩니다.

## 업데이트

패치나 upstream 고정 커밋(`scripts/upstream.env`)이 바뀌면 compose로 띄운 서버는 다음을 실행합니다. behind-proxy도 `.env`의 `COMPOSE_FILE` 덕분에 명령이 같습니다. systemd(B 경로)는 새 Release를 받아 바이너리를 바꿉니다([server-runbook.md](server-runbook.md) 6단계).

```bash
git pull --ff-only && sudo docker compose build && sudo docker compose up -d
```
