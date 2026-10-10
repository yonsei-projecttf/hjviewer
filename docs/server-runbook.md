# 서버 배포 런북 (서버에서 작업할 에이전트·운영자용)

Ubuntu 서버 한 대에 HJViewer 서버(hbbs·hbbr, TLS 443)를 Docker로 띄우는 절차입니다. 위에서부터 순서대로 실행하고, 마지막에 **보고** 형식으로 결과를 알려 주세요. 배경 설명은 [deploy.md](deploy.md)에 있습니다.

## 입력값

- `DOMAIN`: 이 서버를 가리키는 도메인(예: `hjviewer.example.ac.kr`). 모르면 사용자에게 물어보세요.

## 하지 말 것

- 이 서버에서 돌고 있는 **다른 서비스(웹 서버, 컨테이너)를 멈추거나 교체하지 마세요.** 80/443을 이미 쓰는 프로그램이 있으면 B 경로로 갑니다.
- Docker 전역 설정(`/etc/docker/daemon.json`)을 바꾸거나 Docker를 재시작하지 마세요. 다른 컨테이너가 함께 재시작됩니다. 필요한 설정은 이 저장소의 compose 파일에 들어 있습니다.
- 80/443 외의 포트를 열지 마세요. 특히 hbbs/hbbr 포트(21115–21119)는 외부에 공개하면 안 됩니다. hbbs는 프록시가 보내는 `X-Real-IP` 헤더를 그대로 믿습니다.
- `deploy/data/id_ed25519`(서버 **개인키**)를 채팅, 이슈, 저장소에 올리지 마세요. 보고할 것은 `.pub` 파일(공개키)뿐입니다.
- 꺼져 있는 방화벽(ufw)을 임의로 켜지 마세요. SSH가 끊길 수 있습니다.

## 1. 점검 (읽기만 함)

```bash
DOMAIN=hjviewer.example.ac.kr                          # 입력값
lsb_release -ds; nproc; free -h; df -h /
getent ahostsv4 "$DOMAIN" | head -1                    # 도메인이 가리키는 IP
curl -s https://ifconfig.me; echo                      # 이 서버의 공인 IP (둘이 같아야 함)
sudo ss -ltnp '( sport = :80 or sport = :443 )'        # 80/443을 쓰는 프로그램
docker --version; docker compose version
sudo ufw status
```

판단 기준은 다음과 같습니다.

- **80/443이 비어 있으면 A 경로**(Caddy가 TLS 인증서를 직접 받음), 이미 nginx·Apache 등이 쓰고 있으면 **B 경로**(기존 웹 서버가 프록시)입니다.
- 도메인 IP가 이 서버의 공인 IP와 다르면 진행하지 말고 보고하세요. A 경로는 Let's Encrypt가 80/443으로 이 서버에 들어와야 인증서를 발급합니다.
- 메모리가 2GB 미만이고 스왑도 없으면 2단계 전에 스왑을 만드세요. Docker 이미지를 만들 때 Rust 컴파일에 메모리가 필요합니다.
  ```bash
  sudo fallocate -l 2G /swapfile && sudo chmod 600 /swapfile && sudo mkswap /swapfile && sudo swapon /swapfile
  echo '/swapfile none swap sw 0 0' | sudo tee -a /etc/fstab
  ```
- Docker가 없으면 Ubuntu 패키지로 설치합니다(22.04/24.04).
  ```bash
  sudo apt-get update && sudo apt-get install -y docker.io docker-compose-v2 git
  ```
- 대학이나 클라우드의 외부 방화벽(보안 그룹)은 서버 안에서 확인할 수 없습니다. 인바운드 TCP 80/443이 열려 있는지 사용자에게 확인하세요.

## 2. 코드 받기

```bash
sudo git clone https://github.com/ysfaculty/hjviewer.git /opt/hjviewer
cd /opt/hjviewer/deploy
sudo cp .env.example .env
sudo sed -i "s/^HJV_DOMAIN=.*/HJV_DOMAIN=$DOMAIN/" .env
```

## 3-A. 80/443이 비어 있을 때: Caddy 포함

```bash
cd /opt/hjviewer/deploy
sudo docker compose up -d --build        # 처음에는 hbbs/hbbr 컴파일로 수 분이 걸립니다
sudo docker compose ps
sudo docker compose logs caddy | grep -i -m3 'certificate obtained\|error'
```

ufw가 **이미 켜져 있다면** `sudo ufw allow 80,443/tcp`만 추가합니다.

## 3-B. 80/443을 이미 다른 웹 서버가 쓸 때: 기존 웹 서버가 프록시

```bash
cd /opt/hjviewer/deploy
sudo docker compose -f docker-compose.behind-proxy.yml up -d --build
sudo ss -ltnp | grep -E ':2111[89]'      # 127.0.0.1:21118, 127.0.0.1:21119 만 보여야 함
```

`DOMAIN`의 HTTPS 설정(이미 있는 server 블록 또는 VirtualHost)에 아래 내용을 추가합니다. 설정을 검사한 뒤 reload합니다. restart가 아니라 reload입니다. 아래 두 설정은 2026-10-10에 nginx 1.24와 Apache 2.4.58로 실제 hbbs/hbbr 앞에 두고 시험했습니다(PC 등록과 하트비트, 중계 전송, 클라이언트 포트 전달).

**nginx** (`sudo nginx -t && sudo systemctl reload nginx`)

- `DOMAIN`용 `server` 블록이 이미 있으면(`grep -rl "server_name.*$DOMAIN" /etc/nginx/`), 그 `listen 443 ssl` 블록 안에 아래 세 `location`을 넣습니다.
- `DOMAIN`이 이 용도로 새로 만든 서브도메인이라 설정이 없으면 다음 순서로 진행합니다.
  1. 인증서가 없으면 사용자에게 확인을 받고 먼저 발급합니다. `sudo apt-get install -y certbot python3-certbot-nginx && sudo certbot certonly --nginx -d "$DOMAIN"`을 실행하면 이메일과 약관 동의를 묻습니다.
  2. `/etc/nginx/sites-available/hjviewer`를 아래처럼 만들고 `sudo ln -s /etc/nginx/sites-available/hjviewer /etc/nginx/sites-enabled/`로 켭니다.
  ```nginx
  server {
      listen 443 ssl;
      server_name DOMAIN;                 # 실제 도메인으로
      ssl_certificate     /etc/letsencrypt/live/DOMAIN/fullchain.pem;
      ssl_certificate_key /etc/letsencrypt/live/DOMAIN/privkey.pem;
      # 아래 세 location을 여기에
      location / { return 200 "HJViewer server (GNU AGPL-3.0). Source: https://github.com/ysfaculty/hjviewer\n"; }
  }
  ```

```nginx
location = /ws/id {
    proxy_pass http://127.0.0.1:21118;
    proxy_http_version 1.1;
    proxy_set_header Upgrade $http_upgrade;
    proxy_set_header Connection "upgrade";
    proxy_set_header X-Real-IP $remote_addr;
    proxy_set_header X-Real-Port $remote_port;
    proxy_read_timeout 120s;
}
location = /ws/relay {
    proxy_pass http://127.0.0.1:21119;
    proxy_http_version 1.1;
    proxy_set_header Upgrade $http_upgrade;
    proxy_set_header Connection "upgrade";
    proxy_set_header X-Real-IP $remote_addr;
    proxy_set_header X-Real-Port $remote_port;
    proxy_read_timeout 120s;
}
location ^~ /api/ {
    return 404;
}
```

**Apache** (`sudo a2enmod proxy proxy_http proxy_wstunnel headers`, `sudo apachectl configtest && sudo systemctl reload apache2`)

```apache
RequestHeader set X-Real-IP "expr=%{REMOTE_ADDR}"
RequestHeader set X-Real-Port "expr=%{REMOTE_PORT}"
ProxyPass        /ws/id    ws://127.0.0.1:21118
ProxyPass        /ws/relay ws://127.0.0.1:21119
ProxyTimeout 120
<Location "/api/">
    Require all denied
</Location>
```

`proxy_read_timeout 120s`(nginx)나 `ProxyTimeout 120`(Apache)은 꼭 넣어야 합니다. 기본 60초로 두면 PC의 상시 연결이 끊겼다 다시 붙기를 반복합니다.

## 4. 확인

```bash
ws() { curl -s -o /dev/null -w '%{http_code}\n' --max-time 5 --http1.1 \
  -H 'Connection: Upgrade' -H 'Upgrade: websocket' -H 'Sec-WebSocket-Version: 13' \
  -H 'Sec-WebSocket-Key: SGVsbG9Xb3JsZDEyMzQ1Ng==' "https://$DOMAIN$1"; }
ws /ws/id        # 101 이어야 함
ws /ws/relay     # 101 이어야 함
cd /opt/hjviewer/deploy && sudo docker compose logs hbbs hbbr 2>/dev/null | grep 'Key:'   # B 경로는 -f docker-compose.behind-proxy.yml 추가
```

hbbs와 hbbr의 `Key:` 값이 같아야 합니다. 서버 밖의 네트워크(사용자 PC나 회사망)에서 `tools/hjv-probe`로 확인하면 더 확실합니다.

```bash
cd tools/hjv-probe && cargo build --release && ./target/release/hjv-probe "$DOMAIN"
```

## 5. 마무리

- **개인키 백업**: `deploy/data/id_ed25519`를 안전한 곳에 따로 보관합니다(권한 600). 이 키를 잃으면 이미 배포한 모든 앱을 다시 빌드해야 합니다.
- **선택: 중계 대역폭 상한**. compose 파일의 hbbr 서비스에 다음을 넣고 `up -d`를 다시 실행합니다.
  ```yaml
      environment:
        TOTAL_BANDWIDTH: "100"   # Mb/s, 전체
        SINGLE_BANDWIDTH: "20"   # Mb/s, 세션당
  ```
- **업데이트**: `cd /opt/hjviewer && sudo git pull && cd deploy && sudo docker compose up -d --build`. B 경로는 `-f docker-compose.behind-proxy.yml`을 붙입니다.

## 보고

아래 형식으로 알려 주세요. 앱 빌드에 이 값들이 필요합니다.

```
HJV_SERVER=<DOMAIN>
HJV_SERVER_KEY=<deploy/data/id_ed25519.pub 내용>
경로: A(Caddy) | B(nginx/apache 프록시)
서버: <Ubuntu 버전>, <vCPU>, <RAM>, <디스크 여유>
확인: /ws/id <코드>, /ws/relay <코드>, hbbs·hbbr Key 일치 여부
바꾼 것: <설치한 패키지, 수정한 웹 서버 설정 파일, 방화벽 규칙, 스왑>
문제: <있으면>
```
