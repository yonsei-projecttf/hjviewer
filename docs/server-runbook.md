# 서버 배포 런북 (서버에서 작업할 에이전트·운영자용)

Ubuntu 22.04/24.04 서버 한 대에 HJViewer 서버(hbbs·hbbr)를 띄우고 HTTPS(443)로 연결되게 하는 절차입니다. 경로는 두 가지입니다.

- **A. 80/443이 비어 있는 서버**: Docker로 Caddy, hbbs, hbbr을 띄웁니다. 인증서는 Caddy가 받습니다.
- **B. 이미 nginx나 Apache가 80/443을 쓰는 서버**: Docker 없이 GitHub Release의 hbbs/hbbr 바이너리를 systemd 서비스로 127.0.0.1에만 띄웁니다. 기존 웹 서버는 `/ws/id`, `/ws/relay`, `/api/`만 넘겨 받습니다.

위에서부터 순서대로 실행하고, 마지막에 **보고** 형식으로 결과를 알려 주세요. 배경 설명은 [deploy.md](deploy.md)에 있습니다. 다만 그 문서의 명령은 실행하지 말고, 이 런북의 명령만 실행하세요.

1.6.0의 계정 API(연세포탈 로그인, 내 기기 목록, 같은 계정 기기끼리 비밀번호 없는 연결, 원격 켜기)는 B 경로를 마친 서버에 [7절](#7-계정-api-추가-160)의 순서로 더합니다. 7절에는 그 절만의 규칙과 보고 형식이 있습니다.

이 런북이 쓰는 것은 모두 개발 PC에서 `scripts/ci.sh`로 시험합니다. 시험 대상은 다음과 같습니다.

- A 경로의 이미지 빌드, compose 파일, Caddy 구성
- B 경로의 Release 패키지: 그 안의 systemd 유닛을 같은 보안 옵션으로 띄웁니다.
- nginx·Apache 설정 파일과 확인 명령

개발 PC는 운영과 같은 Ubuntu 24.04입니다([ci.md](ci.md)).

## 실행 규칙

- **코드 블록 하나는 새 셸 하나에서 통째로 실행합니다**(예: `bash -c '...'`). 블록 안의 `exit`는 그 셸만 끝냅니다. 사람이 SSH 터미널에 붙여 넣을 때는 먼저 `bash`를 입력해 하위 셸을 연 다음 붙여 넣으세요.
- 셸 변수는 다음 블록으로 이어지지 않습니다. 그래서 블록마다 첫 줄에서 `DOMAIN`을 정합니다. 설치한 뒤에는 설정 파일에서 읽습니다(A: `/opt/hjviewer/deploy/.env`, B: `/etc/hjviewer/hjviewer.env`).
- **"멈춤"**이라고 적힌 경우에는 그 자리에서 아무것도 바꾸지 마세요. 그때까지의 출력을 붙여 사용자에게 보고하고 지시를 기다립니다.
- A 경로의 빌드처럼 오래 걸리는 작업은 백그라운드로 돌리고 로그로 확인합니다. 그 밖의 명령은 길어야 몇 분입니다.
- 비밀번호와 개인키는 채팅으로 주고받지 않습니다.

## 입력값

- `DOMAIN`: 앱이 접속할 호스트 이름(예: `hjviewer.example.ac.kr`). 앱에 들어가는 값이라 정한 뒤에는 바꾸지 않습니다. 모르면 사용자에게 물어보세요.
- DOMAIN이 이미 이 서버의 다른 웹사이트 주소라면, 사용자가 **그 사이트와 함께 쓰기로** 정했는지 확인합니다. 정했다면 2단계 4번에서 멈추지 않고 4-3의 "(나) 기존 사이트에 추가"로 갑니다. 그 사이트의 인증서를 그대로 씁니다.
- B 경로: 설치할 **Release 버전** `VERSION`(예: `2026.10.10`). [Releases](https://github.com/yonsei-projecttf/hjviewer/releases)의 `server-<VERSION>`입니다. 사용자가 알려 줍니다.
- 인증서를 새로 받아야 할 때만 필요한 것: Let's Encrypt 알림을 받을 **이메일**과 약관 동의. 필요해지면 사용자에게 묻습니다.

## 하지 말 것

- 이 서버의 **다른 서비스(웹 서버, 컨테이너, DB)를 멈추거나 재시작하거나 교체하지 마세요.** 웹 서버는 설정을 검사한 뒤 reload만 합니다.
- 웹 서버 전역 설정(`nginx.conf`, `apache2.conf`)과 다른 사이트의 설정 파일은 고치지 마세요. HJViewer용 파일만 새로 추가합니다. 사용자가 기존 사이트와 같은 도메인을 쓰기로 정한 경우만 "(나) 기존 사이트에 추가" 절차를 따릅니다.
- B 경로에서는 Docker를 설치하지 마세요. A 경로에서는 Docker 전역 설정(`/etc/docker/daemon.json`)을 바꾸거나, Docker를 재시작하거나 업그레이드하지 마세요. 서버의 모든 컨테이너가 함께 재시작됩니다.
- `docker system prune`, `docker image prune -a`, `docker volume prune`, `docker builder prune`, `docker compose down -v`, `--remove-orphans`를 쓰지 마세요. 다른 서비스의 컨테이너, 이미지, 볼륨이나 Caddy 인증서가 지워집니다.
- `apt upgrade`나 `full-upgrade`를 하지 마세요. 패키지는 이 런북의 명령으로만 설치합니다.
- 80/443 외의 포트를 열지 마세요. 방화벽(ufw, iptables/nftables, firewalld)과 fail2ban 규칙도 바꾸지 마세요. 꺼져 있는 ufw를 켜면 SSH가 끊길 수 있습니다.
- hbbs/hbbr은 접속한 쪽이 보내는 `X-Real-IP`, `X-Real-Port`, `X-Forwarded-For` 헤더를 그대로 믿고, 그 값으로 기기를 구분합니다. 그래서 두 조건을 꼭 지킵니다.
  1. hbbs/hbbr은 127.0.0.1에만 listen합니다. 21115–21119를 외부에 공개하지 마세요.
  2. 웹 서버가 이 세 헤더를 클라이언트가 보낸 값과 상관없이 항상 덮어씁니다. `nginx-hjviewer-locations.conf`와 `apache-hjviewer.conf`의 해당 줄을 바꾸거나 지우지 마세요. 특히 X-Forwarded-For를 `$proxy_add_x_forwarded_for`로 넘기거나 Apache의 `ProxyAddHeaders`를 켜면, 클라이언트가 보낸 값이 남아 주소를 위조할 수 있습니다. `scripts/ci.sh`가 nginx와 Apache 모두에서 위조한 헤더가 덮어써지는지 확인합니다.
- **서버 개인키 `id_ed25519`를 출력(cat, less, base64 등)하거나 복사하거나 전송하지 마세요.** 위치는 A가 `/opt/hjviewer/deploy/data/`, B가 `/var/lib/hjviewer/`입니다. `id_ed25519*` 같은 와일드카드도 쓰지 마세요. 에이전트가 본 출력은 기록에 남습니다. 보고할 것은 `id_ed25519.pub`(공개키)뿐입니다.
- `/opt/hjviewer`나 `/var/lib/hjviewer`가 이미 있으면 지우거나 덮어쓰지 마세요. `git clean -x`, `git reset --hard`, `rm -rf deploy/data`도 쓰지 마세요. 개인키를 잃으면 이미 배포한 앱을 모두 다시 빌드해야 합니다.

## 1. 점검 (읽기만 함)

```bash
DOMAIN=hjviewer.example.ac.kr     # ← 입력값으로 바꿈
S="sudo -n"                       # 비밀번호가 필요하면 묻지 않고 실패하는 sudo
echo "== sudo"; $S true 2>/dev/null && echo SUDO_OK || echo SUDO_NEEDS_PASSWORD
echo "== system"; lsb_release -ds; nproc; grep -E '^(MemTotal|MemAvailable|SwapTotal|SwapFree):' /proc/meminfo; swapon --show
echo "== docker"; command -v docker; $S docker version -f 'server {{.Server.Version}}' 2>&1 | tail -1; $S docker compose version 2>&1 | tail -1
dpkg -l 2>/dev/null | awk '/^ii/ && $2 ~ /^(docker|containerd|podman|runc)/ {print $2, $3}'; ls -d /snap/docker 2>/dev/null   # snap Docker ("snap list"는 snapd가 응답하지 않으면 2분 멈춤)
ROOT=$($S docker info -f '{{.DockerRootDir}}' 2>/dev/null); df -h / /var ${ROOT:+"$ROOT"} 2>/dev/null
echo "== containers, compose projects"; $S docker ps --format '{{.Names}}\t{{.Ports}}' 2>/dev/null; $S docker compose ls -a 2>/dev/null
echo "== ports 80/443 and 21115-21119"; $S ss -ltnp '( sport = :80 or sport = :443 )'; $S ss -ltnup | grep -E ':2111[5-9]\b' || echo "2111x: free"
echo "== web servers (active, enabled)"; for u in nginx apache2 caddy haproxy traefik; do echo "$u $(systemctl is-active $u 2>/dev/null) $(systemctl is-enabled $u 2>/dev/null)"; done
echo "== nginx"; command -v nginx >/dev/null && { $S nginx -t 2>&1 | tail -1; $S nginx -T 2>/dev/null | grep -nE '^# configuration file|^\s*(listen|server_name|ssl_certificate|include)\s'; }
echo "== apache"; command -v apache2ctl >/dev/null && { $S apache2ctl configtest 2>&1 | tail -1; $S apache2ctl -S 2>/dev/null; }
echo "== certbot"; command -v certbot && { $S certbot certificates 2>/dev/null | grep -E 'Certificate Name|Domains|Expiry|Certificate Path'; $S ls /etc/letsencrypt/accounts 2>/dev/null; systemctl is-active certbot.timer snap.certbot.renew.timer 2>/dev/null; }
echo "== DNS of $DOMAIN (public resolver), this server's addresses"
curl -s --max-time 10 "https://dns.google/resolve?name=$DOMAIN&type=A" | grep -o '"data":"[^"]*"' || echo "A: none"
curl -s --max-time 10 "https://dns.google/resolve?name=$DOMAIN&type=AAAA" | grep -o '"data":"[^"]*"' || echo "AAAA: none"
echo "outgoing IPv4: $(curl -4 -s --max-time 10 https://ifconfig.me)  IPv6: $(curl -6 -s --max-time 10 https://ifconfig.me)"
ip -br addr | grep -v '^lo '; grep -nw -- "$DOMAIN" /etc/hosts
echo "== outbound access (000 = blocked)"
for u in https://github.com https://index.crates.io/config.json https://static.crates.io https://deb.debian.org/debian/ https://public.ecr.aws/v2/ https://registry-1.docker.io/v2/ https://acme-v02.api.letsencrypt.org/directory; do echo "$(curl -s -o /dev/null -w '%{http_code}' --max-time 10 "$u") $u"; done
echo "== MTU"; dev=$(ip route get 1.1.1.1 2>/dev/null | grep -o 'dev [^ ]*' | cut -d' ' -f2); echo "$dev $(cat "/sys/class/net/$dev/mtu" 2>/dev/null)"
echo "== firewall (information only)"; $S ufw status 2>/dev/null | head -1; systemctl is-active firewalld nftables 2>/dev/null | paste -sd' '
echo "== bridges (VMs, LXD)"; ip -br link show type bridge
echo "== existing install"; ls -ld /opt/hjviewer /var/lib/hjviewer /etc/hjviewer 2>&1
```

## 2. 판단

1단계 출력을 보고 아래를 차례로 판단합니다.

1. **sudo**: `SUDO_NEEDS_PASSWORD`면 멈춥니다. 사용자에게 두 가지 중 하나를 요청합니다. 이 작업 동안만 비밀번호 없는 sudo를 주거나, sudo 명령을 사용자가 직접 실행하는 것입니다. 비밀번호를 채팅으로 받지 마세요.
2. **경로**
   - **A (Caddy, Docker)**: 80과 443이 ss와 docker ps 어디에도 없고, nginx·apache2·caddy·haproxy·traefik이 하나도 enabled가 아닐 때.
   - **B (기존 웹 서버 뒤, systemd)**: 80/443을 쓰는 프로그램이 이 서버에 직접 설치된 `nginx` 또는 `apache2`일 때(ss의 프로세스 이름).
   - 그 밖의 경우에는 멈춥니다. 다른 컨테이너(`docker-proxy`)나 caddy·haproxy·traefik이 80/443을 쓰는 경우, 둘 중 한 포트만 쓰이는 경우, 웹 서버가 꺼져 있지만 enabled인 경우(재부팅 뒤 Caddy와 포트를 다툼)가 여기에 해당합니다.
   - 21115–21119를 이미 쓰는 프로그램이 있으면 멈춥니다(예: 기존 RustDesk 서버).
3. **DNS**
   - DNS에 A 레코드가 없으면(새 서브도메인) 멈춥니다. 사용자에게 이 서버의 공인 IPv4로 A 레코드를 만들어 달라고 요청합니다.
   - A 값이 `outgoing IPv4`나 `ip -br addr`의 주소와 같으면 진행합니다. 둘 다 다르면 멈춥니다. 그 주소의 80/443이 이 서버로 전달되는지(NAT, 포트 포워딩) 사용자에게 확인합니다.
   - AAAA가 있으면 그 주소가 `ip -br addr`에 있어야 합니다. 없으면 멈춥니다. 사용자가 AAAA를 지우거나 이 서버 주소로 고쳐야 합니다. 그대로 두면 IPv6를 먼저 쓰는 클라이언트와 인증서 발급이 실패합니다.
   - `/etc/hosts`에 DOMAIN이 있으면 서버 안에서 하는 확인이 바깥과 다르게 나올 수 있습니다. 보고에 적습니다.
4. **B 경로의 웹 서버 설정**
   - nginx는 `nginx -t`, Apache는 `apache2ctl configtest`가 지금 이미 실패하면 멈춥니다. 원래 있던 문제를 이 작업이 떠안지 않기 위해서입니다.
   - DOMAIN이 nginx의 `server_name`이나 Apache의 `apache2ctl -S`에 이미 있으면 **기존 사이트와 같은 도메인**입니다. 사용자가 그 사이트와 함께 쓰기로 이미 정했다면 진행합니다(4-3의 "(나) 기존 사이트에 추가"). 정하지 않았다면 멈추고 고르게 합니다. 하나는 새 서브도메인(입력값을 바꿔 1단계부터 다시)이고, 다른 하나는 기존 사이트에 추가입니다. 기존 사이트에 추가하면 DNS와 인증서를 새로 만들 필요가 없습니다. 그 대신 그 사이트 설정 파일에 한 줄이 들어갑니다.
5. **A 경로만: Docker**
   - snap으로 설치한 Docker(`/snap/docker`가 있거나 `command -v docker`가 `/snap/bin/docker`)이거나 podman이면 멈춥니다. snap Docker는 `/opt` 아래 파일을 제대로 쓰지 못합니다.
   - Docker는 있는데 `docker compose version`이 실패하면 멈춥니다. 사용자에게 설치할 패키지를 확인합니다. docker-ce는 `docker-compose-plugin`, Ubuntu의 docker.io는 `docker-compose-v2`입니다. 이때 `docker.io`는 설치하지 마세요. docker-ce가 지워집니다.
   - Docker 서버 버전이 28 미만이면 멈춥니다. 28 미만에서는 같은 네트워크의 다른 기기가 컨테이너 포트(hbbs)에 직접 닿을 수 있습니다. 업그레이드하면 모든 컨테이너가 재시작되므로 사용자가 정합니다.
   - Docker가 없으면 3-1에서 설치합니다. 다만 브리지(`virbr*`, `lxdbr*` 등)가 보이면 먼저 사용자에게 확인합니다. Docker가 FORWARD 정책을 DROP으로 바꿔서 VM이나 LXD 네트워크가 끊길 수 있습니다.
6. **A 경로만: 메모리**. MemAvailable과 SwapFree를 더한 값으로 판단합니다. B 경로는 빌드하지 않으므로 해당하지 않습니다.
   - 3GB 이상이면 그대로 진행합니다.
   - 2–3GB이면 3-3에서 `HJV_BUILD_JOBS=1`을 넣습니다.
   - 2GB 미만이면 3-3에서 `HJV_BUILD_JOBS=1`을 넣고 3-2에서 스왑을 만듭니다. 단, 스왑이 하나도 없고 `/swapfile`도 없을 때만입니다. 그 밖의 경우에는 멈춥니다.
7. **디스크**: A 경로는 Docker 데이터가 있는 파일 시스템(`/var`, 또는 DockerRootDir)에 6GB 이상 비어 있어야 합니다. 빌드 뒤에도 빌드 캐시가 약 4GB 남습니다. B 경로는 50MB면 됩니다. 모자라면 멈춥니다.
8. **외부 접속**
   - A 경로: github.com, index.crates.io, static.crates.io, deb.debian.org, public.ecr.aws 중 하나라도 000이면 멈춥니다. 빌드에 모두 필요합니다. acme-v02.api.letsencrypt.org도 필요합니다. registry-1.docker.io는 기본 설정에서 쓰지 않습니다. public.ecr.aws만 막혀 있고 Docker Hub가 된다면, 3-3에서 `HJV_REGISTRY=docker.io/library`를 넣으면 됩니다.
   - B 경로: github.com만 필요합니다(Release 내려받기). 000이면 멈춥니다.
9. **A 경로만: MTU**. 1500 미만(예: 1450)이면 3-4에서 MTU용 빌드 명령을 씁니다. 다만 Caddy가 인증서를 받다가 멈출 수 있으므로 진행하지 말고 멈춥니다.
10. `/opt/hjviewer`, `/var/lib/hjviewer`, `/etc/hjviewer` 중 하나라도 이미 있으면 멈춥니다.
11. 대학이나 클라우드의 외부 방화벽(보안 그룹)은 서버 안에서 볼 수 없습니다. A 경로는 인바운드 TCP 80/443이, B 경로는 지금 웹 서버가 쓰는 포트가 열려 있어야 합니다. 사용자에게 확인합니다.

A 경로는 3단계로, B 경로는 4단계로 갑니다.

## 3. A 경로: Docker와 Caddy

### 3-1. Docker 설치 (Docker가 없을 때만)

```bash
sudo apt-get -o DPkg::Lock::Timeout=600 update
if sudo apt-get -s install docker.io docker-compose-v2 docker-buildx git | grep '^Remv'; then echo 'STOP: 설치하면 위 패키지가 제거됨'; exit 1; fi
sudo env DEBIAN_FRONTEND=noninteractive NEEDRESTART_SUSPEND=1 apt-get -o DPkg::Lock::Timeout=600 install -y --no-upgrade docker.io docker-compose-v2 docker-buildx git
sudo docker version -f 'server {{.Server.Version}}'; sudo docker compose version
```

`NEEDRESTART_SUSPEND=1`은 설치 뒤 다른 서비스가 자동으로 재시작되지 않게 합니다(Ubuntu 24.04의 needrestart). docker 그룹에 사용자를 추가하지 마세요. 이 런북은 모든 docker 명령을 sudo로 실행합니다.

### 3-2. 스왑 (2단계 6번에 해당할 때만)

```bash
[ -e /swapfile ] && { echo 'STOP: /swapfile이 이미 있음'; exit 1; }
sudo fallocate -l 2G /swapfile && sudo chmod 600 /swapfile && sudo mkswap /swapfile && sudo swapon /swapfile \
  && { grep -q '^/swapfile ' /etc/fstab || echo '/swapfile none swap sw 0 0' | sudo tee -a /etc/fstab; }
swapon --show
```

### 3-3. 코드와 설정

```bash
DOMAIN=hjviewer.example.ac.kr     # ← 입력값
[ -e /opt/hjviewer ] && { echo 'STOP: /opt/hjviewer가 이미 있음'; exit 1; }
sudo env GIT_TERMINAL_PROMPT=0 git clone https://github.com/yonsei-projecttf/hjviewer.git /opt/hjviewer || exit 1
cd /opt/hjviewer/deploy || exit 1
sudo install -d -m 700 data       # 서버 개인키가 생길 곳: root만 읽을 수 있게
sudo cp .env.example .env
sudo sed -i "s/^HJV_DOMAIN=.*/HJV_DOMAIN=$DOMAIN/" .env
# 2단계 6번에 해당하면:  echo 'HJV_BUILD_JOBS=1' | sudo tee -a .env
# 2단계 8번에 해당하면:  echo 'HJV_REGISTRY=docker.io/library' | sudo tee -a .env
grep -v '^#' .env | grep .
sudo docker compose config --services   # caddy hbbs hbbr
sudo git -C /opt/hjviewer log -1 --format='%h %s'
```

프로젝트 이름은 compose 파일에 `hjviewer`로 고정돼 있어서, 이 서버의 다른 compose 프로젝트와 섞이지 않습니다.

### 3-4. 서버 이미지 빌드 (백그라운드)

빌드는 2 vCPU에서 10–20분, 1 vCPU에서는 그보다 오래 걸립니다. 포그라운드로 기다리면 에이전트 도구의 시간 제한에 걸리고, 빌드가 취소되면 처음부터 다시 컴파일해야 합니다. 그래서 세션과 분리해 실행합니다.

```bash
sudo setsid -f sh -c 'cd /opt/hjviewer/deploy && docker compose build > /var/tmp/hjv-build.log 2>&1; echo "exit=$?" >> /var/tmp/hjv-build.log' < /dev/null > /dev/null 2>&1
sleep 10; sudo tail -n 3 /var/tmp/hjv-build.log
```

2단계 9번(MTU 1500 미만)에 해당하면 위 첫 줄 대신 아래를 실행합니다. 빌드가 호스트 네트워크를 쓰게 해서 MTU 문제를 피합니다. 3-3에서 `HJV_BUILD_JOBS`나 `HJV_REGISTRY`를 넣었다면 `--build-arg` 값도 그에 맞춥니다.

```bash
sudo setsid -f sh -c 'cd /opt/hjviewer && docker build --network host --build-arg REGISTRY=public.ecr.aws/docker/library --build-arg BUILD_JOBS=2 -f deploy/Dockerfile -t hjviewer-server:local . > /var/tmp/hjv-build.log 2>&1; echo "exit=$?" >> /var/tmp/hjv-build.log' < /dev/null > /dev/null 2>&1
```

몇 분 간격으로 아래를 실행해 확인합니다. 명령은 바로 끝납니다.

```bash
sudo tail -n 3 /var/tmp/hjv-build.log
```

- 마지막 줄이 `exit=0`이면 3-5로 갑니다.
- `exit=` 뒤가 0이 아니면 `sudo tail -n 40 /var/tmp/hjv-build.log`의 출력을 붙여 보고하고 멈춥니다. 다시 시도하지 않습니다.
- 로그가 30분 넘게 그대로면 멈춥니다.
- `buildx`, `Bake` 관련 경고는 정상입니다. 레지스트리에 로그인하거나 compose 파일을 고치지 마세요.

### 3-5. 기동과 인증서

```bash
cd /opt/hjviewer/deploy || exit 1
sudo docker compose up -d && sudo docker compose ps
timeout 180 sh -c 'until sudo docker compose logs caddy 2>&1 | grep -qE "certificate obtained successfully|could not get certificate"; do sleep 5; done'
sudo docker compose logs caddy 2>&1 | grep -E 'certificate obtained successfully|could not get certificate|challenge failed' | tail -3
```

- `certificate obtained successfully`가 나오면 5단계로 갑니다.
- 실패 줄이 나오거나 3분 동안 아무것도 나오지 않으면 멈춥니다. Caddy를 재시작하지 마세요. Caddy는 스스로 재시도하는데, 재시작을 반복하면 Let's Encrypt 실패 한도(시간당 5회)에 걸립니다. 사용자에게 외부 방화벽의 인바운드 80/443을 확인해 달라고 합니다.
- A 경로의 80/443은 Docker가 ufw와 상관없이 엽니다. ufw는 건드리지 않습니다. 같은 이유로 compose 파일에 포트를 추가하지 마세요. ufw로 막을 수 없습니다.

## 4. B 경로: Release를 systemd 서비스로 (Docker 없음)

hbbs/hbbr은 C 라이브러리 말고는 의존성이 없는 바이너리 두 개입니다(Ubuntu 24.04에서 빌드, glibc 2.39 이상 필요). systemd가 이 둘을 서비스 전용 사용자 `hjviewer`로 실행합니다. 서비스는 127.0.0.1에만 listen하고, 권한(capability)을 모두 버립니다. 쓸 수 있는 곳은 자기 데이터 디렉터리뿐이고, CPU 2코어와 메모리 1GB의 상한이 걸립니다. 설치되는 것은 다음과 같습니다.

| 위치 | 내용 |
|---|---|
| `/opt/hjviewer/bin/hbbs`, `hbbr` | 바이너리 (root 소유) |
| `/opt/hjviewer/share/` | LICENSE, NOTICE, SOURCE(이 빌드의 소스 커밋), THIRD_PARTY_NOTICES, `web/`(nginx·Apache 설정 파일) |
| `/etc/hjviewer/hjviewer.env` | `HJV_DOMAIN`과 중계 대역폭 상한 |
| `/etc/systemd/system/hjviewer-hbbs.service`, `hjviewer-hbbr.service` | 서비스 유닛 |
| `/var/lib/hjviewer/` | 서버 키 쌍과 기기 DB. 디렉터리 700, 파일 600, 소유자 `hjviewer` |

### 4-1. 내려받기, 확인, 설치 (서비스는 아직 시작하지 않음)

```bash
VERSION=2026.10.10                # ← Release 버전 (입력값)
DOMAIN=hjviewer.example.ac.kr     # ← 입력값
for d in /opt/hjviewer /var/lib/hjviewer /etc/hjviewer /etc/systemd/system/hjviewer-hbbs.service; do [ -e "$d" ] && { echo "STOP: $d 이(가) 이미 있음"; exit 1; }; done
T=$(mktemp -d) && cd "$T" || exit 1
base="https://github.com/yonsei-projecttf/hjviewer/releases/download/server-$VERSION"
pkg="hjviewer-server-$VERSION-linux-x86_64"
if ! curl -fsSLO --max-time 120 "$base/$pkg.tar.gz" || ! curl -fsSLO --max-time 30 "$base/SHA256SUMS"; then echo 'STOP: 내려받기 실패'; exit 1; fi
grep " $pkg.tar.gz\$" SHA256SUMS | sha256sum -c - || { echo 'STOP: 체크섬이 다름'; exit 1; }
tar -xzf "$pkg.tar.gz" && cd "$pkg" || exit 1
if ldd bin/hbbs | grep 'not found'; then echo 'STOP: 이 서버에서 실행할 수 없음 (glibc 2.39 이상 필요)'; exit 1; fi
cat SOURCE.txt
getent passwd hjviewer || sudo useradd --system --user-group --home-dir /var/lib/hjviewer --no-create-home --shell /usr/sbin/nologin hjviewer
sudo install -d -m 755 /opt/hjviewer/bin /opt/hjviewer/share/web /etc/hjviewer
sudo install -m 755 bin/hbbs bin/hbbr /opt/hjviewer/bin/
sudo install -m 644 LICENSE.txt NOTICE.txt SOURCE.txt THIRD_PARTY_NOTICES.txt /opt/hjviewer/share/
sudo install -m 644 web/* /opt/hjviewer/share/web/
sed "s/^HJV_DOMAIN=.*/HJV_DOMAIN=$DOMAIN/" systemd/hjviewer.env.example | sudo tee /etc/hjviewer/hjviewer.env > /dev/null
sudo install -m 644 systemd/hjviewer-hbbs.service systemd/hjviewer-hbbr.service /etc/systemd/system/
sudo systemctl daemon-reload
grep -v '^#' /etc/hjviewer/hjviewer.env | grep .
systemd-analyze verify /etc/systemd/system/hjviewer-hbbs.service /etc/systemd/system/hjviewer-hbbr.service && echo UNITS_OK
cd / && rm -rf "$T"
```

`UNITS_OK`가 나오지 않으면 멈춥니다. `hjviewer.env`에는 비밀 값이 없습니다. 중계 대역폭 상한은 6단계에서 정합니다.

### 4-2. 기동과 확인

```bash
sudo systemctl enable --now hjviewer-hbbs.service hjviewer-hbbr.service
sleep 3; systemctl is-active hjviewer-hbbs hjviewer-hbbr   # active active
sudo ss -ltnup | grep -E ':2111[5-9]\b'                     # 127.0.0.1:21115-21119 (UDP 21116 포함)만
sudo ls -la /var/lib/hjviewer/                              # 디렉터리 700, 파일 600, 소유자 hjviewer
echo "pub  $(sudo cat /var/lib/hjviewer/id_ed25519.pub)"
for s in hbbs hbbr; do echo "$s $(sudo journalctl -q -u hjviewer-$s -o cat | grep -o 'Key: [A-Za-z0-9+/=]*' | tail -1 | cut -d' ' -f2)"; done
sudo journalctl -q -u hjviewer-hbbs -u hjviewer-hbbr -o cat | grep -E 'ERROR|panicked|denied' | tail -5
```

- 두 서비스가 `active`이고, 21115–21119가 모두 127.0.0.1이고, 세 키 값(`pub`, `hbbs`, `hbbr`)이 같아야 합니다. 아니면 멈춥니다.
- hbbr은 hbbs가 키를 만든 다음에 시작합니다(유닛의 ExecStartPre). 그래서 키가 다르게 생기지 않습니다.
- 이 공개키는 앱 빌드에 필요합니다. 웹 서버 설정(4-3)을 나중에 하더라도, 여기서 먼저 사용자에게 보고하면 앱 빌드를 시작할 수 있습니다.

### 4-3. 웹 서버 설정

둘 중 하나입니다.

- **(가) 새 사이트**: DOMAIN이 HJViewer 전용입니다. 인증서를 마련하고, 새 사이트 파일을 추가합니다.
- **(나) 기존 사이트에 추가**: DOMAIN을 이 서버의 기존 웹사이트와 함께 씁니다. 인증서는 그 사이트 것을 그대로 쓰고, 그 사이트 설정에 include 한 줄을 넣습니다. 아래 "(나) 기존 사이트에 추가"로 바로 갑니다.

**(가) 인증서**: DOMAIN용 인증서를 다음 순서로 찾거나 받습니다.

1. **이미 DOMAIN을 포함하는 인증서가 있으면** 그것을 씁니다(1단계 certbot 출력, 또는 다른 사이트가 쓰는 와일드카드 인증서). 파일이 DOMAIN을 포함하는지는 `sudo openssl x509 -noout -ext subjectAltName -enddate -in <파일>`로 확인합니다.
2. **certbot이 있고 `/etc/letsencrypt/accounts`가 있으면**(이미 Let's Encrypt를 쓰는 서버) 바로 받습니다.
   ```bash
   DOMAIN=$(sed -n 's/^HJV_DOMAIN=//p' /etc/hjviewer/hjviewer.env); [ -n "$DOMAIN" ] || exit 1
   sudo certbot certonly --nginx -d "$DOMAIN" --non-interactive --deploy-hook 'systemctl reload nginx'
   ```
3. **둘 다 아니면**, 사용자에게 이메일을 받고 [Let's Encrypt 약관](https://letsencrypt.org/repository/) 동의를 확인한 뒤 실행합니다.
   ```bash
   DOMAIN=$(sed -n 's/^HJV_DOMAIN=//p' /etc/hjviewer/hjviewer.env); [ -n "$DOMAIN" ] || exit 1
   EMAIL=admin@example.ac.kr          # ← 사용자가 알려 준 이메일
   sudo env DEBIAN_FRONTEND=noninteractive NEEDRESTART_SUSPEND=1 apt-get -o DPkg::Lock::Timeout=600 install -y --no-upgrade certbot python3-certbot-nginx
   sudo certbot certonly --nginx -d "$DOMAIN" --non-interactive --agree-tos -m "$EMAIL" --deploy-hook 'systemctl reload nginx'
   ```
4. 학교가 인증서를 발급하는 경우처럼 위에 해당하지 않으면 멈춥니다.

Apache는 `python3-certbot-apache`, `--apache`, `--deploy-hook 'systemctl reload apache2'`로 바꿉니다. certbot이 실패하면 다시 시도하지 말고 멈춥니다. `--deploy-hook`은 인증서를 갱신할 때마다 웹 서버를 reload해서 새 인증서를 읽게 합니다.

**(가) 사이트 설정 (nginx)**: [nginx-hjviewer.conf](../deploy/nginx-hjviewer.conf)를 새 사이트로 추가합니다. 이 파일은 HJViewer 경로(`/ws/id`, `/ws/relay`, `/api/`)를 담은 [nginx-hjviewer-locations.conf](../deploy/nginx-hjviewer-locations.conf)를 include합니다. 두 파일 모두 4-1에서 `/opt/hjviewer/share/web/`에 설치했습니다.

```bash
DOMAIN=$(sed -n 's/^HJV_DOMAIN=//p' /etc/hjviewer/hjviewer.env); [ -n "$DOMAIN" ] || exit 1
[ -e /etc/nginx/sites-available/hjviewer ] && { echo 'STOP: 이미 있음'; exit 1; }
[ -e /etc/nginx/snippets/hjviewer-locations.conf ] && { echo 'STOP: 이미 있음'; exit 1; }
sudo nginx -t 2>/dev/null || { echo 'STOP: 기존 설정이 이미 실패'; exit 1; }
sudo install -D -m 644 /opt/hjviewer/share/web/nginx-hjviewer-locations.conf /etc/nginx/snippets/hjviewer-locations.conf
sed "s/DOMAIN/$DOMAIN/g" /opt/hjviewer/share/web/nginx-hjviewer.conf | sudo tee /etc/nginx/sites-available/hjviewer > /dev/null
grep -nE 'listen|server_name|ssl_certificate|include' /etc/nginx/sites-available/hjviewer
```

활성화하기 전에 파일을 다음과 같이 맞춥니다.

- 인증서를 1번(기존 인증서)으로 정했으면 `ssl_certificate` 두 줄을 그 파일 경로로 바꿉니다.
- `listen` 줄은 이 서버의 다른 사이트를 따라 씁니다(1단계 `nginx -T` 출력). 다른 사이트에 `listen [::]:443 ssl;`이 있으면 같은 줄을 추가합니다. 특정 IP로 listen하면(`listen 203.0.113.10:443 ssl;`) 그 형식으로 바꿉니다. 80 쪽도 같습니다. 이렇게 하지 않으면 nginx가 이 사이트를 고르지 않아, `nginx -t`는 통과해도 접속이 다른 사이트로 갑니다.
- nginx.conf가 `sites-enabled`를 include하지 않고 `conf.d`만 include하면, 파일을 `/etc/nginx/conf.d/hjviewer.conf`로 만들고 아래 `ln` 줄은 건너뜁니다.

```bash
sudo ln -s /etc/nginx/sites-available/hjviewer /etc/nginx/sites-enabled/hjviewer
out=$(sudo nginx -t 2>&1); echo "$out"
if echo "$out" | grep -qE 'emerg|test failed|conflicting server name'; then sudo rm /etc/nginx/sites-enabled/hjviewer; echo 'STOP: 설정 오류, 되돌림'; exit 1; fi
sudo systemctl reload nginx
```

**(가) 사이트 설정 (Apache)**: [apache-hjviewer.conf](../deploy/apache-hjviewer.conf)를 같은 방법으로 추가합니다.

```bash
DOMAIN=$(sed -n 's/^HJV_DOMAIN=//p' /etc/hjviewer/hjviewer.env); [ -n "$DOMAIN" ] || exit 1
[ -e /etc/apache2/sites-available/hjviewer.conf ] && { echo 'STOP: 이미 있음'; exit 1; }
sudo apache2ctl configtest 2>/dev/null || { echo 'STOP: 기존 설정이 이미 실패'; exit 1; }
sed "s/DOMAIN/$DOMAIN/g" /opt/hjviewer/share/web/apache-hjviewer.conf | sudo tee /etc/apache2/sites-available/hjviewer.conf > /dev/null
```

인증서 경로와 `<VirtualHost>` 주소를 nginx와 같은 기준으로 맞춘 뒤 활성화합니다. `a2enmod`가 restart하라고 안내해도 reload만 합니다.

```bash
sudo a2enmod ssl proxy proxy_http proxy_wstunnel headers alias && sudo a2ensite hjviewer
if ! sudo apache2ctl configtest; then sudo a2dissite hjviewer; echo 'STOP: 설정 오류, 되돌림'; exit 1; fi
sudo systemctl reload apache2
```

**(나) 기존 사이트에 추가 (nginx)**: 그 사이트의 HTTPS server 블록에 `include` 한 줄만 넣습니다. 사이트의 다른 경로는 그대로이고, `/ws/id`, `/ws/relay`, `/api/`만 HJViewer로 갑니다. 이 구성은 `scripts/ci.sh`에서 시험합니다. 시험 조건은 HTTP/2를 켜고 모든 경로를 로그인 페이지로 보내는 WordPress 같은 사이트입니다.

1. 그 사이트가 이 경로들을 쓰는지 확인합니다.
   ```bash
   DOMAIN=$(sed -n 's/^HJV_DOMAIN=//p' /etc/hjviewer/hjviewer.env); [ -n "$DOMAIN" ] || exit 1
   sudo nginx -T 2>/dev/null | grep -nE '^# configuration file|server_name|location[^{]*/(ws|api)\b|real_ip_header|set_real_ip_from'
   for p in /ws/id /api/heartbeat; do echo "$p $(curl -s -o /dev/null -w '%{http_code} %{redirect_url}' --max-time 10 --resolve "$DOMAIN:443:127.0.0.1" "https://$DOMAIN$p")"; done
   ```
   - DOMAIN 사이트의 server 블록 안에 `/ws`나 `/api`로 시작하는 location이 있으면 멈춥니다.
   - `real_ip_header`나 `set_real_ip_from`이 보이면 nginx가 `$remote_addr`를 요청 헤더에서 가져오는 것입니다(앞단 프록시나 CDN용). `set_real_ip_from`에 그 앞단 프록시 주소만 있는지 확인하고, 넓은 범위(예: 0.0.0.0/0)가 있으면 멈춥니다. 클라이언트가 자기 주소를 정할 수 있게 되기 때문입니다.
   - 두 경로가 404이거나 그 사이트의 로그인 페이지 등으로 넘어가면(302) 사이트가 쓰지 않는 것이니 진행합니다. 200이면 사이트가 그 경로를 쓰는 것이니 멈춥니다. WordPress는 REST API를 `/wp-json/`에 두므로 보통 `/api/`를 쓰지 않습니다.
2. 반영은 그 사이트 사용이 적은 시간에 합니다. cron 등으로 nginx가 자동 reload되는 시각과 겹치지 않게 합니다. 먼저 같은 443 포트를 쓰는 모든 사이트의 지금 응답 코드를 기록해 둡니다(5번에서 비교). 기본 인증이나 IP 제한이 걸린 사이트는 401·403이 정상일 수 있습니다.
   ```bash
   for name in $(sudo nginx -T 2>/dev/null | sed 's/#.*//' | grep -oE '(^|[[:space:];{])server_name[[:space:]][^;]*' | sed -E 's/.*server_name[[:space:]]+//' | tr -s ' \t' '\n' | grep '\.' | grep -vE '^[~*.]' | sort -u); do
     echo "$name $(curl -s -o /dev/null -w '%{http_code}' --max-time 10 --resolve "$name:443:127.0.0.1" "https://$name/")"
   done | tee /tmp/hjv-sites-before.txt
   ```
3. `# configuration file` 줄로 DOMAIN의 443 server 블록이 있는 파일을 찾습니다. sites-enabled의 파일은 보통 sites-available을 가리키는 심볼릭 링크이므로 `readlink -f`로 실제 파일을 구해서 그 파일을 백업하고 고칩니다. 백업은 웹 서버가 읽지 않는 `/root/hjv-backup`에 둡니다. sites-enabled나 conf.d 안에 두면 `include sites-enabled/*` 때문에 같은 server 블록이 두 번 읽힙니다.
   ```bash
   F=$(readlink -f /etc/nginx/sites-enabled/example)   # ← 1번에서 찾은 파일. 심볼릭 링크를 풀어 실제 파일로
   echo "$F"; [ -f "$F" ] || { echo 'STOP: 파일이 아님'; exit 1; }
   sudo install -d -m 700 /root/hjv-backup && sudo cp -p "$F" /root/hjv-backup/
   [ -e /etc/nginx/snippets/hjviewer-locations.conf ] && { echo 'STOP: 이미 있음'; exit 1; }
   sudo install -D -m 644 /opt/hjviewer/share/web/nginx-hjviewer-locations.conf /etc/nginx/snippets/hjviewer-locations.conf
   ```
4. 그 파일(`$F`, 실제 파일)을 편집해, `listen ... 443 ssl`과 `server_name ... DOMAIN`이 있는 server 블록 안의 `server_name` 줄 바로 아래에 다음 한 줄만 넣습니다. 같은 도메인의 443 server 블록이 둘 이상이면 각각 넣습니다. 80 포트 블록에는 넣지 않습니다. nginx-hjviewer.conf의 `location /`나 80 포트 블록은 **넣지 마세요.** 기존 사이트를 덮어씁니다.
   ```nginx
       include snippets/hjviewer-locations.conf;
   ```
5. 검사하고 reload한 뒤, 2번에서 기록한 응답 코드와 비교합니다. 검사가 실패하거나 응답 코드가 달라지면 백업으로 되돌리고 멈춥니다.
   ```bash
   F=$(readlink -f /etc/nginx/sites-enabled/example)   # ← 3번과 같은 파일
   sudo diff -u "/root/hjv-backup/$(basename "$F")" "$F"   # include 한 줄만 보여야 함
   out=$(sudo nginx -t 2>&1); echo "$out"
   if echo "$out" | grep -qE 'emerg|test failed'; then sudo cp -p "/root/hjv-backup/$(basename "$F")" "$F"; echo 'STOP: 설정 오류, 되돌림'; exit 1; fi
   sudo systemctl reload nginx; sleep 2
   while read -r name before; do
     after=$(curl -s -o /dev/null -w '%{http_code}' --max-time 10 --resolve "$name:443:127.0.0.1" "https://$name/")
     echo "$name $before -> $after"; [ "$before" = "$after" ] || echo "  DIFFERENT"
   done < /tmp/hjv-sites-before.txt
   ```
   `DIFFERENT`가 하나라도 나오면 백업을 되돌리고(`sudo cp -p "/root/hjv-backup/$(basename "$F")" "$F" && sudo nginx -t && sudo systemctl reload nginx`) 멈춥니다.

참고할 점은 다음과 같습니다.

- 그 사이트의 listen에 HTTP/2(`http2`)가 켜져 있어도 됩니다. 앱은 HTTP/1.1로 협상하므로 WebSocket이 열리고, 브라우저는 그대로 HTTP/2를 씁니다.
- nginx reload(설정 변경, 인증서 교체, cron 등)가 일어나도 HJViewer의 열린 연결은 끊기지 않습니다.
  - 열린 연결은 옛 worker에 남고, 옛 worker는 그 연결이 닫힐 때 끝납니다. 그동안 `ps -C nginx -o pid,etime,rss,args`에 `worker process is shutting down`이 보입니다.
  - 늘 연결해 두는 PC가 있으면 이런 worker가 며칠씩 남을 수 있습니다(시험에서 각 7MB 정도).
  - nginx.conf에 `worker_shutdown_timeout`을 두면 그 시간 뒤 남은 연결이 끊깁니다. PC는 곧바로 다시 등록되지만, 그때 중계로 진행 중인 원격 세션은 끊깁니다. 이 설정은 서버의 모든 사이트에 적용되므로 서버 운영자가 정합니다.
- 그 server 블록이나 http 블록에 `limit_conn`이 있으면 보고에 적습니다. PC마다 상시 연결을 하나씩 유지하므로, 같은 주소 뒤의 PC가 많으면 제한에 걸릴 수 있습니다.

**(나) 기존 사이트에 추가 (Apache)**: 그 사이트의 443 VirtualHost 안, 다른 `ProxyPass`(특히 `ProxyPass /`)보다 위에 넣습니다. 넣을 것은 `/opt/hjviewer/share/web/apache-hjviewer.conf`의 `ProxyPass` 두 줄과 `<Location "/ws/">` 블록뿐입니다.

- `/api/`를 그 사이트가 쓰지 않으면(위 1번과 같은 확인) `Redirect 404 /api/`도 넣습니다.
- apache-hjviewer.conf의 `Redirect 404 /`와 `ErrorDocument` 줄은 **넣지 마세요.** 기존 사이트 전체가 404가 됩니다.
- 넣기 전에 파일을 백업하고, `apache2ctl configtest`가 실패하면 백업으로 되돌리고 멈춥니다.

## 5. 확인

웹 서버를 거쳐 hbbs/hbbr까지 닿는지 봅니다. A·B 공통입니다.

```bash
DOMAIN=$(sed -n 's/^HJV_DOMAIN=//p' /etc/hjviewer/hjviewer.env /opt/hjviewer/deploy/.env 2>/dev/null | head -1); [ -n "$DOMAIN" ] || exit 1
# WebSocket: 각각 5초 뒤 101이 나오면 정상(curl 종료 코드 28도 정상)
for p in /ws/id /ws/relay; do echo "$p $(curl -s -o /dev/null -w '%{http_code}' --max-time 5 --http1.1 --resolve "$DOMAIN:443:127.0.0.1" -H 'Connection: Upgrade' -H 'Upgrade: websocket' -H 'Sec-WebSocket-Version: 13' -H 'Sec-WebSocket-Key: SGVsbG9Xb3JsZDEyMzQ1Ng==' "https://$DOMAIN$p")"; done
echo "/api/ $(curl -s -o /dev/null -w '%{http_code}' --max-time 5 --resolve "$DOMAIN:443:127.0.0.1" -X POST "https://$DOMAIN/api/heartbeat")"   # 404
echo | openssl s_client -connect 127.0.0.1:443 -servername "$DOMAIN" 2>/dev/null | openssl x509 -noout -subject -issuer -enddate
```

키와 개인키 권한은 경로마다 다릅니다. 세 키 값(`pub`, `hbbs`, `hbbr`)이 같아야 합니다.

```bash
# A 경로: 700 root data, 600 root data/id_ed25519
cd /opt/hjviewer/deploy || exit 1
echo "pub  $(sudo cat data/id_ed25519.pub)"
for s in hbbs hbbr; do echo "$s $(sudo docker compose logs --no-log-prefix $s | grep -o 'Key: [A-Za-z0-9+/=]*' | tail -1 | cut -d' ' -f2)"; done
sudo chmod 600 data/id_ed25519 && sudo stat -c '%a %U %n' data data/id_ed25519
```

```bash
# B 경로: 700 hjviewer /var/lib/hjviewer, 600 hjviewer id_ed25519
echo "pub  $(sudo cat /var/lib/hjviewer/id_ed25519.pub)"
for s in hbbs hbbr; do echo "$s $(sudo journalctl -q -u hjviewer-$s -o cat | grep -o 'Key: [A-Za-z0-9+/=]*' | tail -1 | cut -d' ' -f2)"; done
sudo stat -c '%a %U %n' /var/lib/hjviewer /var/lib/hjviewer/id_ed25519
```

- `--resolve`는 요청을 이 서버의 127.0.0.1:443으로 보냅니다. 학내 NAT가 자기 공인 주소로 되돌아오는 연결(헤어핀)을 막아도 확인할 수 있습니다. 웹 서버가 특정 IP에서만 listen해서 000이 나오면, `127.0.0.1`을 그 IP로 바꿔 다시 실행합니다.
- `/ws/*`가 101이 아니면 멈춥니다.
  - 000은 웹 서버나 TLS 문제입니다.
  - 404는 사이트 설정이 선택되지 않은 것이고, 502는 hbbs/hbbr이 떠 있지 않은 것입니다.
  - (나) 기존 사이트에 추가한 경우, 302(로그인 페이지 등)는 그 사이트가 아직 이 경로를 받는 것입니다. include가 다른 server 블록에 들어갔거나, server 수준의 `return`·`rewrite`가 먼저 실행된 것입니다.
  - 401은 사이트 전체에 걸린 인증(`auth_basic` 등)입니다.
- 세 키 값이 다르면 둘 다 파일의 키를 다시 읽게 하고 다시 확인합니다. A는 `sudo docker compose restart hbbs hbbr`, B는 `sudo systemctl restart hjviewer-hbbs hjviewer-hbbr`입니다. 그래도 다르면 멈춥니다.
- `/api/heartbeat`는 어느 경로에서나 404입니다. 다만 1.6.0의 `Caddyfile`(A 경로)과 `apache-hjviewer.conf`((가) 새 사이트)는 계정 경로(`/api/login`, `/api/peers`, `/api/hjv/…` 등)를 hjv-api로 넘기는데, 1~6단계는 hjv-api를 설치하지 않으므로 그 경로는 503과 JSON으로 답합니다. 고장이 아닙니다(7-14의 "hjv-api가 멈춰 있는 동안").
- 바깥 네트워크에서 하는 확인은 서버 안에서 할 수 없습니다. 보고 뒤에 사용자가 합니다(아래 "보고 뒤 사용자가 할 일").

## 6. 마무리

- **개인키 백업은 사용자가 직접 합니다.** 에이전트는 키를 복사하거나 출력하지 않고, 보고에 "개인키 백업 필요"라고만 적습니다. 방법은 아래 "보고 뒤 사용자가 할 일"에 있습니다.
- **중계 대역폭 상한(선택)**: 단위는 Mb/s입니다.
  - A 경로: `/opt/hjviewer/deploy/.env`에 `HJV_TOTAL_BANDWIDTH=100`(전체)과 `HJV_SINGLE_BANDWIDTH=20`(세션당)을 넣고 `sudo docker compose up -d`를 실행합니다. compose 파일은 고치지 마세요. 그러면 `git pull`이 실패합니다.
  - B 경로: `/etc/hjviewer/hjviewer.env`의 `TOTAL_BANDWIDTH`와 `SINGLE_BANDWIDTH`를 채우고 `sudo systemctl restart hjviewer-hbbr`을 실행합니다.
  - 적용됐는지는 hbbr 로그의 `TOTAL_BANDWIDTH`·`SINGLE_BANDWIDTH` 줄로 확인합니다.
- **A 경로의 빌드 캐시**: 약 4GB가 남습니다. 지워야 하면 먼저 사용자에게 묻습니다. `docker builder prune`은 다른 프로젝트의 캐시도 지웁니다.
- **A 경로 업데이트**(나중에)
  1. `sudo git -C /opt/hjviewer pull --ff-only`
  2. 3-4처럼 백그라운드로 빌드하고 `exit=0`을 확인합니다.
  3. `cd /opt/hjviewer/deploy && sudo docker compose up -d`
- **B 경로 업데이트**(나중에, 사용이 적은 시간에)
  1. 새 `VERSION`으로 4-1처럼 내려받아 체크섬을 확인합니다(그 블록의 앞부분까지만).
  2. 그 디렉터리에서 다음을 실행합니다.
     ```bash
     sudo install -m 755 bin/hbbs bin/hbbr /opt/hjviewer/bin/
     sudo install -m 644 systemd/hjviewer-hbbs.service systemd/hjviewer-hbbr.service /etc/systemd/system/
     sudo install -m 644 web/* /opt/hjviewer/share/web/
     sudo install -m 644 LICENSE.txt NOTICE.txt SOURCE.txt THIRD_PARTY_NOTICES.txt /opt/hjviewer/share/
     sudo systemctl daemon-reload
     sudo systemctl restart hjviewer-hbbs hjviewer-hbbr
     ```
  3. 재시작하면 PC는 몇 초 안에 다시 등록되지만, 진행 중인 원격 세션은 끊깁니다.
  4. 1.6.0부터 묶음에는 hjv-api도 들어 있습니다. hjv-api가 설치된 서버에서는 위 명령에 더해, 7-14의 "업그레이드"대로 스냅숏, 설정 파일, 옛 실행 파일을 남긴 뒤 같은 디렉터리에서 다음을 실행합니다. 유닛 파일을 바꾼 뒤 `daemon-reload`를 하지 않으면 systemd는 옛 유닛으로 다시 시작합니다.
     ```bash
     [ -x bin/hjv-api ] || { echo 'STOP: 묶음에 hjv-api가 없음'; exit 1; }
     diff -q systemd/hjviewer-api.socket /etc/systemd/system/hjviewer-api.socket
     sudo install -m 755 bin/hjv-api /opt/hjviewer/bin/
     for u in systemd/hjviewer-api*.socket systemd/hjviewer-api*.service systemd/hjviewer-api*.timer; do [ -e "$u" ] && sudo install -m 644 "$u" /etc/systemd/system/; done
     sudo systemctl daemon-reload
     sudo systemctl restart hjviewer-api
     ```
     `diff` 줄이 `differ`를 보였으면(소켓 유닛이 바뀜) 마지막 줄 대신 `sudo systemctl stop hjviewer-api.service hjviewer-api.socket && sudo systemctl start hjviewer-api.socket hjviewer-api.service`를 합니다. 서비스만 다시 시작하면 systemd가 쥐고 있던 옛 소켓을 그대로 넘겨받습니다.

     질문 10의 답이 '안 됨'이어서 hjv-api를 비공개 묶음으로 설치한 서버(7-1의 3번, 7-3 끝)에서는 공개 묶음에 `bin/hjv-api`가 없어 이 블록이 `STOP`으로 멈춥니다. 그때는 사용자가 올린 새 비공개 hjv-api 묶음의 디렉터리에서 이 블록을 실행하고, 그 묶음의 `web/*`도 `/opt/hjviewer/share/web/`에 설치한 뒤(`sudo install -m 644 web/* /opt/hjviewer/share/web/`) 아래의 nginx 비교를 합니다. 그 묶음의 고지는 7-3 끝처럼 `/opt/hjviewer/share/hjv-api/`에 둡니다.
- **nginx 경로 파일**: 업데이트 뒤에는 묶음의 nginx 파일이 설치된 것과 다른지 봅니다. A 경로는 `/opt/hjviewer/deploy/nginx-hjviewer-locations.conf`와 `/etc/nginx/snippets/hjviewer-locations.conf`를 비교합니다. B 경로는 다음을 실행합니다. 7절의 nginx 전환(7-8)을 한 서버의 경로 파일에는 `location ^~ /api/` 블록 대신 `include snippets/hjviewer-api.conf;`가 있으므로, 묶음 파일에 7-8과 같은 변환을 한 결과와 비교하고 계정 API의 두 파일도 비교합니다. 묶음 파일을 그대로 다시 설치하면 7-8의 전환이 되돌아가 로그인과 PC 동기화가 모두 404가 됩니다.
  ```bash
  W=/opt/hjviewer/share/web; L=/etc/nginx/snippets/hjviewer-locations.conf; N=$(mktemp)
  if grep -qx 'include snippets/hjviewer-api.conf;' "$L"; then   # 7-8의 전환을 한 서버
    awk '/^location \^~ \/api\/ \{/ { print "include snippets/hjviewer-api.conf;"; skip = 1; next }
      skip { if ($0 ~ /^\}/) skip = 0; next } { print }' "$W/nginx-hjviewer-locations.conf" > "$N"
    diff -q "$W/nginx-hjviewer-api.conf" /etc/nginx/snippets/hjviewer-api.conf
    diff -q "$W/nginx-hjviewer-api-zones.conf" /etc/nginx/conf.d/hjviewer-api-zones.conf
  else
    cp "$W/nginx-hjviewer-locations.conf" "$N"
  fi
  diff -q "$N" "$L"
  ```
  아무것도 나오지 않으면 바꿀 것이 없습니다. `differ`(또는 `No such file`)가 나온 파일만 아래처럼 바꾸고, 끝나면 `rm -f "$N"`을 합니다. 조용한 시간에 하고 cron의 nginx 재적재 시각은 피합니다.
  1. 바꿀 파일마다 지금 파일을 `/root/hjv-backup/`(`sudo install -d -m 700`)에 `sudo cp -p`로 남깁니다.
  2. 설치합니다. 경로 파일은 `sudo install -m 644 "$N" "$L"`, 계정 API의 두 파일은 `sudo install -m 644 "$W/nginx-hjviewer-api.conf" /etc/nginx/snippets/hjviewer-api.conf`와 `sudo install -m 644 "$W/nginx-hjviewer-api-zones.conf" /etc/nginx/conf.d/hjviewer-api-zones.conf`입니다.
  3. `sudo nginx -t`가 통과하면 `sudo systemctl reload nginx`를 하고, 5절의 첫 블록(7-8을 한 서버는 7-8의 3번도)으로 확인합니다. 실패하면 1번에서 남긴 파일을 되돌리고 멈춥니다.
- **B 경로 되돌리기**(사용자가 요청할 때만): 아래 순서를 지킵니다. include 줄을 남긴 채 경로 파일부터 지우면 `nginx -t`가 실패합니다.
  1. nginx의 include 줄을 지우거나 백업을 되돌리고, `sudo nginx -t && sudo systemctl reload nginx`를 실행합니다. (가) 새 사이트라면 sites-enabled의 링크를 지우고 reload합니다.
  2. 그다음 `sudo rm /etc/nginx/snippets/hjviewer-locations.conf`를 실행합니다.
  3. 서비스를 지웁니다.
     ```bash
     sudo systemctl disable --now hjviewer-hbbr hjviewer-hbbs
     sudo rm /etc/systemd/system/hjviewer-hbbs.service /etc/systemd/system/hjviewer-hbbr.service && sudo systemctl daemon-reload
     sudo rm -r /opt/hjviewer /etc/hjviewer
     ```
  4. 서버 키와 기기 DB(`/var/lib/hjviewer`), 사용자 `hjviewer`는 사용자가 개인키 백업을 확인한 뒤에만 지웁니다(`sudo rm -r /var/lib/hjviewer && sudo userdel hjviewer`).

## 보고

아래 형식으로 알려 주세요. 앱 빌드에 앞의 두 값이 필요합니다. B 경로는 4-2가 끝났을 때 이 두 값만 먼저 보고해도 됩니다.

```
HJV_SERVER=<DOMAIN>
HJV_SERVER_KEY=<A: sudo cat /opt/hjviewer/deploy/data/id_ed25519.pub | B: sudo cat /var/lib/hjviewer/id_ed25519.pub 의 출력>
키 일치: .pub = hbbs = hbbr (예/아니오)
버전: <A: sudo git -C /opt/hjviewer log -1 --format=%h | B: server-<VERSION>, SOURCE.txt의 커밋>
경로: A(Caddy) | B(systemd, nginx|apache) (가) 새 사이트 | (나) 기존 사이트에 추가, 추가·수정한 설정 파일: <경로>
인증서: <발급자>, 만료 <날짜>, 갱신: Caddy 자동 | certbot.timer + deploy-hook | 기존 사이트와 같음 | 기타(<설명>)
확인(서버 안): /ws/id <코드>, /ws/relay <코드>, /api/ <코드>, 다른 사이트 응답 코드 변화 없음(예/아니오)
서버: <Ubuntu 버전>, <vCPU>, <메모리 전체/가용>, <디스크 여유>, Docker <A: 서버 버전 | B: 없음>
바꾼 것: <설치한 패키지, 만든 사용자, 추가·수정한 설정 파일, 스왑, 설정 파일에 넣은 값>
남은 일: 개인키 백업(사용자), 바깥에서 확인(사용자), 첫 PC 등록 뒤 헤더 확인
문제: <있으면>
```

## 보고 뒤 사용자가 할 일

1. **바깥 네트워크에서 확인**: 학교 밖처럼 서버와 다른 네트워크의 Windows PC에서 PowerShell로 실행합니다. 101이 나오면 정상입니다.
   ```powershell
   curl.exe -s -o NUL -w "%{http_code}`n" --max-time 5 --http1.1 -H "Connection: Upgrade" -H "Upgrade: websocket" -H "Sec-WebSocket-Version: 13" -H "Sec-WebSocket-Key: SGVsbG9Xb3JsZDEyMzQ1Ng==" https://DOMAIN/ws/id
   ```
   단계별로 더 자세히 보려면 Rust가 있는 PC에서 `tools/hjv-probe`를 실행합니다([deploy.md](deploy.md)의 "확인").
2. **개인키 백업**: 서버에서 개인키의 복사본을 만듭니다.
   - A 경로: `sudo install -m 600 -o "$USER" /opt/hjviewer/deploy/data/id_ed25519 ~/hjviewer-id_ed25519`
   - B 경로: `sudo install -m 600 -o "$USER" /var/lib/hjviewer/id_ed25519 ~/hjviewer-id_ed25519`

   자기 PC에서 `scp <서버>:hjviewer-id_ed25519 .`로 받아 비밀번호 관리자 같은 안전한 곳에 보관합니다. 그다음 서버에서 `shred -u ~/hjviewer-id_ed25519`로 복사본을 지웁니다.
3. **첫 PC 등록 뒤 헤더 확인**: PC 앱을 하나 설치해 ID가 나오면, 서버에서 다음을 실행합니다.
   ```bash
   # A 경로
   cd /opt/hjviewer/deploy && sudo docker compose logs --no-log-prefix hbbs | grep update_pk | tail -3
   # B 경로
   sudo journalctl -q -u hjviewer-hbbs -o cat | grep update_pk | tail -3
   ```
   주소가 그 PC의 공인 IP와 포트여야 합니다(포트는 0이 아님). `127.0.0.1`이나 `172.x`, `:0`이 보이면 웹 서버가 `X-Real-IP`/`X-Real-Port`를 넘기지 않는 것입니다. 이 상태에서는 같은 NAT 뒤의 PC들이 서로의 응답을 받게 됩니다.

## 7. 계정 API 추가 (1.6.0)

1.6.0의 연세포탈 로그인, 내 기기 목록, 같은 계정 기기끼리 비밀번호 없는 연결, 원격 켜기를 위해 계정 API **hjv-api**를 더하고 hbbs를 패치 0004로 바꾸는 절차입니다. 1~6단계로 B 경로(systemd, nginx)를 마친 서버가 대상입니다. Apache와 A 경로(Caddy)는 다루지 않습니다. 그런 서버가 1.6.0의 설정 파일로 계정 경로에 503을 답하는 것은 고장이 아니며, 7-14의 "hjv-api가 멈춰 있는 동안"에 설명과 할 일이 있습니다.

- 동작 원리: [architecture.md](architecture.md) 5절. hjv-api의 설정, 관리 명령, 형식: [server/api/README.md](../server/api/README.md). 개인정보: [privacy.md](privacy.md). NAS 중계기 설치: [relay-dsm.md](relay-dsm.md).
- 아래 표의 순서대로 합니다. 소절 번호는 표의 단계를 따릅니다. 서버에서 하는 것은 3~7단계, 9단계, 12단계, 15단계의 승인 명령입니다.

| 단계 | 할 일 | 누가 | 소절 |
|---|---|---|---|
| 0 | 원격 켜기 현장 시험(NAS의 DSM 작업. 8단계의 출시 태그 전까지), 남은 결정과 질문의 답 | 사용자 | 7-1 |
| 1 | 개발 PC에서 모든 작업을 합치고 `scripts/ci.sh`의 모든 단계(`leak` 포함) 통과, 문서 갱신 | 개발 PC | 7-1 |
| 2 | push(출시 태그 없이), 확인 빌드, 서버 묶음 `server-<VERSION>` | 개발 PC | 7-1 |
| 3 | 읽기 점검 | 운영 에이전트 | 7-2 |
| 4 | hjv-api 설치. 비밀번호 없는 연결 스위치는 꺼진 채로, nginx는 그대로 | 운영 에이전트(사용자 승인) | 7-3 |
| 5 | 키와 자격 증명 만들기 | **사용자만**(운영 에이전트는 멈춤) | 7-4 |
| 6 | 공개키 대조값, 시작, selfcheck, 명단 | 운영 에이전트 | 7-5 |
| 7 | hbbs 0004로 교체 | 운영 에이전트, 조용한 시간 | 7-6 |
| 8 | 앱과 중계기 빌드, 지문 대조, 출시 태그 | 개발 PC, 사용자 확인 | 7-7 |
| 9 | nginx 전환 | 운영 에이전트(사용자 승인), 조용한 저녁 | 7-8 |
| 10~13 | 배포, 운영자 PC부터 켜기, 다른 사용자 | 개발 PC·사용자·운영 에이전트 | 7-9 |
| 14 | 최소 버전 강제 | 사용자 승인 | 7-10 |
| 15 | NAS 중계기 승인과 켜기 | 사용자, 운영 에이전트(승인 명령) | 7-11 |

그 뒤의 소절은 되돌리기(7-12), 사고 대응(7-13), 감시와 백업(7-14), 일상 운영(7-15), 보고(7-16)입니다.

### 7-0. 이 절의 규칙

- 운영 서버 작업은 운영 에이전트(서버에서 작업하는 AI 에이전트)가 **사용자 승인을 받은 뒤** 합니다. 비밀값(SSO 자격 증명, 키)은 사용자만 다룹니다.
- 서비스를 다시 시작하거나 nginx를 바꾸는 단계(7, 9)는 조용한 저녁에 합니다. cron이 nginx를 자동으로 다시 읽는 시각(매일 20:00 등)은 피합니다. 같은 서버의 다른 서비스 일정과 겹치는지는 사용자가 정합니다.
- 최소 버전 작업이 함께 나가면 그 작업의 단계를 같은 번호에 넣습니다. hbbs 재시작, nginx reload, 빌드 변수 설정은 각각 **한 번**만 합니다.
- **운영 에이전트의 권한(결정 O1)**: 지금처럼 둡니다. 작업 동안 비밀번호 없는 sudo를 받거나(2단계 1번), 사용자가 sudo 명령을 직접 실행합니다. 1.6.0부터 이 서버의 목록 키로 서명한 목록은 그 계정에 묶인 모든 PC를 비밀번호 없이 엽니다. root를 가진 동안 이것을 막는 것은 아래 규칙뿐이므로 반드시 지킵니다.
- **하지 말 것** (앞의 "하지 말 것"에 더해):
  - 자격 증명(`/etc/credstore.encrypted/*`, `/var/lib/systemd/credential.secret`)을 출력·복호화·복사하는 일. `systemd-creds decrypt`, 서비스의 `$CREDENTIALS_DIRECTORY`나 `/proc/*/environ` 읽기.
  - hjv-api의 DB(`/var/lib/hjviewer-api/`)와 그 백업을 직접 여는 일(`sqlite3`, `cat`, 복사). DB는 관리 명령(`hjv-api admin`)으로만 다룹니다.
  - `hjv-api keygen`과 `hjv-api admin --reveal`. 키 만들기와 원래 값 보기는 사람만 합니다.
  - 포탈 ID, 성명, IP 주소, 기기 이름을 보고나 대화에 옮기는 일. 명단 파일은 열어 보지 않고 가져오기만 합니다. PC는 사용자가 알려 준 PC ID로 가리킵니다(`--owner-of`).
  - 저널 원문을 대화에 붙이는 일. 경고·오류는 줄 수만 보고합니다.
  - 같은 서버의 다른 사이트(WordPress 등)의 DB나 설정에서 비밀을 찾는 일.
  - 실제 계정으로 로그인해 보는 일. 로그인 확인은 사용자가 앱으로 합니다.
  - `passwordless off` 뒤 24시간이 지나기 전에 hjv-api를 멈추거나 nginx를 되돌리는 일(7-12의 규칙).

### 7-1. 시작하기 전 (0~2단계, 사용자와 개발 PC)

서버에서 하는 일이 아닙니다. 3단계를 시작하기 전에 1번의 결정·질문의 답과 2~4번이 끝났는지 사용자에게 확인합니다. 1번의 현장 시험과 5번은 push 뒤에 해도 되지만, 5번에 적은 때까지는 끝나야 합니다.

1. 원격 켜기의 현장 시험(NAS의 DSM 작업으로 PC를 깨워 봄, [relay-dsm.md](relay-dsm.md)의 현장 시험 장과 [부록](relay-dsm.md#부록-현장-시험-보충))과 남은 결정·질문의 답. 현장 시험은 깨울 PC인 개발 PC가 작업을 쉬는 동안에만 할 수 있어서, push를 막지 않고 출시 태그를 막습니다(5번).
2. 개발 PC: 모든 작업 묶음을 합친 커밋에서 `scripts/ci.sh server`, `client`(Rust 1.75 check 포함), `deploy`, `e2e`, `wol`, `leak`이 통과하고, privacy.md, architecture.md, 이 런북, README의 "현재 상태" 표가 그 커밋의 내용으로 고쳐져 있음([ci.md](ci.md)).
   - Windows·iOS 전용 코드를 작업 브랜치로 미리 빌드해 보지 않습니다(결정 O2). 그 코드는 4번의 push 바로 뒤에 돌리는 **확인 빌드**(`check_only=true`, [ci.md](ci.md)의 "실행")에서 처음 컴파일됩니다. 확인 빌드는 공개키 없이 돌고, 산출물을 나눠 주지 않으며, 태그를 붙이지 않습니다. 실패하면 고친 뒤 한 번 더 push합니다. 구현이 모두 끝나면 바로 앱 빌드를 시작한다는 사용자 결정(2026-10-13)에 따른 순서입니다.
   - 통과는 마지막 줄의 "CI passed"만 보고 판단하지 않습니다. `scripts/ci.sh`는 무엇을 건너뛰었어도 마지막에 "CI passed"를 찍기 때문입니다. 모든 묶음을 합친 커밋의 기록에는 `skipped:`로 시작하는 줄이 없어야 하고, `PASS` 줄의 수가 지금까지의 통과 기록과 같아야 합니다(2026-10-13 기준 `e2e` 62줄, `wol` 20줄. `wol`은 [e2e.md](e2e.md#원격-켜기-단계-160) 표의 19단계와 마지막 `PASS (wol)` 줄입니다). 특히 `wol`은 클라이언트에 켜기 정보 보고의 시험 명령(`--hjv-e2e-wol-report`, 클라이언트 패치 0035)이 없으면 E2E를 돌리지 않고도 통과로 끝납니다.
3. **push 전 관문 둘**
   - `scripts/ci.sh leak` 통과: 운영 도메인, 학교 공인 주소 대역, NAS·PC의 주소와 MAC, 부서 공유 폴더 이름, 실제 포탈 ID가 추적 파일, 패치, push할 커밋에 없음. 값 목록 `build/leak-patterns.txt`는 사용자가 만듭니다([ci.md](ci.md)의 "push 전 검사").
     - 목록에는 공개되면 안 되는 값만 넣습니다. PC와 NAS의 네트워크 기록에서 값을 옮길 때 브로드캐스트 주소 `255.255.255.255` 같은 공개 값이 섞이면, 그 값을 쓰는 중계기 코드와 시험, [relay/PROTOCOL.md](../relay/PROTOCOL.md), 원격 켜기 E2E가 모두 걸립니다.
   - **연세 SSO 연동 코드를 공개해도 되는지에 대한 답**(질문 10). `server/api`(연세 SSO password grant를 하는 `sso.rs` 포함)는 이 push로 처음 공개됩니다. 비밀값은 저장소에 없습니다.
     - '됨'이면 그대로 push합니다.
     - '안 됨'이면 push 전에 아직 push하지 않은 커밋을 다시 짜서 `server/api`와 그 배치 파일(`deploy/systemd/hjviewer-api*`, `deploy/nginx-hjviewer-api*.conf`)을 공개 역사에서 빼고, 사용자가 따로 만드는 비공개 저장소에 둡니다. 코드를 두지 않는 빌드 전용 저장소(`hjviewer-private`)는 쓰지 않습니다. 서버 묶음의 hjv-api는 그 비공개 저장소에서 만들고, 공개 저장소의 `scripts/package-server.sh`는 `server/api`가 없으면 hjv-api를 빼고 묶습니다. 앱의 봉인·요청 서명 코드(클라이언트 패치)와 hbbs 0004는 배포하거나 네트워크로 제공하는 AGPL 코드의 수정이라 그대로 공개합니다. `GET /api/hjv/source`는 공개된 부분만 가리킵니다.
       - 이때 hjv-api 묶음은 그 비공개 저장소의 Release에 둡니다. 배치는 공개 묶음과 같게 합니다(`bin/hjv-api`, `systemd/hjviewer-api*`, `web/nginx-hjviewer-api*.conf`, env 예시, 저널 보관 설정, 고지, 그 묶음만의 `SHA256SUMS`). 서버는 토큰 없이 비공개 Release를 받을 수 없으므로, 사용자가 받아 서버에 올리고 7-3을 그 묶음으로 합니다(7-3 끝, 고지는 따로 둠). hbbs, hbbr과 그 고지(7-6)는 공개 묶음에서 받습니다.
       - push 전에 README, NOTICE, architecture.md 7절에서 hjv-api를 이 저장소의 AGPL 프로그램이라고 적은 문장과 `server/api/README.md`를 가리키는 링크(README, architecture.md, 이 런북)도 고칩니다.
4. push(출시 태그는 아직 붙이지 않음)와 확인 빌드. 그 공개 커밋으로 서버 묶음 `server-<VERSION>`을 만들어 Release에 올립니다([ci.md](ci.md)의 "서버 Release 만들기"). 묶음에는 hbbs(패치 0004, 최소 버전 작업이 있으면 0005도), hbbr, hjv-api, 유닛(소켓, 서비스, 감시·백업 타이머), nginx 파일과 zones 파일, env 예시, 저널 보관 설정, 고지가 들어 있고, `SHA256SUMS`에는 그 묶음 한 줄만 있습니다.
5. **개발 PC의 CI가 보지 못해 push 뒤에 남는 것**. 하나씩 사용자와 확인하고, 적힌 때까지 끝냅니다.
   - **현장 시험 결과(출시 태그 전)**: 사용자가 1번의 현장 시험을 마치면 [wol-pc-setup.md](wol-pc-setup.md) 2장 "현장 시험 결과"의 표, 시험한 HJViewer 판, 상태 줄을 채우고, 그 절의 주석에 있는 대조 목록(그 문서의 일반 안내와 [relay-dsm.md](relay-dsm.md) 부록의 결과 읽기 표)을 처리한 뒤 `scripts/ci.sh leak`을 다시 돌려 push합니다. 표에 '시험 전'이 남아 있는 동안에는 출시 태그 `client-1.6.0`을 붙이지 않습니다. 확인 빌드를 위한 첫 push는 표를 '현장 시험 전'이라고 사실대로 둔 채 나가도 됩니다.
   - **TCP 펀치 시험(출시 태그 전)**: 클라이언트 lib 시험 `rendezvous_mediator::tests::a_punch_that_meets_the_peers_syn_connects_both_ends`(루프백에서 TCP 동시 열기)는 WSL2인 개발 PC에서 통과할 수 없고(1.5.1의 트리에서도 실패), 클라이언트 패치 0031이 그 파일 `src/rendezvous_mediator.rs`를 건드렸습니다. WSL이 아닌 리눅스(CPU 2개 이상. 하나뿐이면 이 시험은 아무것도 확인하지 않고 통과함)에서 패치를 적용한 트리로 한 번 돌려 통과를 확인합니다([e2e.md](e2e.md) "실행"의 3)과 같은 빌드 환경에서 `cargo test --locked --features linux-pkg-config --lib a_punch_that_meets_the_peers_syn_connects_both_ends`). 그럴 수 없으면 방화벽 E2E가 그 경로를 덮는다고 보는 이유를 [e2e.md](e2e.md)에 적어 둡니다.
   - **열린 질문(8단계의 출시 빌드 전에 사용자가 정함)**: 기기 키가 바뀐 PC(설정까지 지우고 다시 설치한 PC 등)가 `needs-relogin`이 되기까지 설계는 1분 안이라고 했지만, 지금 구현은 다시 켜진 뒤 약 2분(96~144초)입니다. 다시 켜진 PC가 hbbs에 등록하기 전에 첫 동기화를 보내 거절되고, 다음 동기화가 약 120초 뒤이기 때문입니다. 그동안 컨트롤러에는 비밀번호 창 대신 키 바뀜 경고가 뜹니다(안전한 쪽). 그대로 두고 문서를 '약 2분'으로 맞출지, 거절된 PC가 60초쯤 뒤에 한 번 더 묻게 앱을 고칠지 정합니다.
   - **열린 질문(사설 주소 망의 PC에 중계기를 승인하기 전에 사용자가 정함)**: PC의 망이 사설 주소(RFC 1918, 100.64.0.0/10)이면 hjv-api는 PC와 NAS의 나가는 주소가 같을 때만 같은 곳으로 봅니다. 사설 망은 곳마다 되풀이되기 때문입니다. 공유기 뒤의 집과 사무실은 둘이 같은 주소로 나가므로 괜찮지만, NAT 없이 서버에 닿는 사내망처럼 서버가 둘의 사설 주소를 그대로 보는 곳에서는 늘 다른 곳이 됩니다. 그러면 승인 도구가 `no (a private network of another site: the egress addresses differ)`로 멈추고, `--force`로 승인해도 켜기 요청이 409 `no_relay`(`different_egress`)를 받으며 앱에 "모든 중계기로 시도"도 나오지 않아 그 PC를 켤 수 없습니다([e2e.md](e2e.md#원격-켜기-단계로-찾은-것)). 그런 PC가 있으면 hjv-api의 판정(`server/api/src/wolinfo.rs`)을 넓힐지 정합니다.
   - **사용자만 할 수 있는 실기기 시험**. Windows·iOS 전용 코드는 컴파일만 확인했고 실행해 본 적이 없으며, 실제 DSM과 실제 랜카드도 써 보지 못했습니다. 결과가 기대와 다르면 고친 뒤 다시 빌드합니다.
     - Windows 설치와 비밀번호 없는 로그인(11단계, 7-9): 1.6.0을 1.5.1 위에 설치하고 로그인해 알림과 설정 → 계정의 줄을 보고, 관리자가 스위치를 켠 뒤(12단계) 휴대폰에서 비밀번호 없이 연결합니다. [끄기], [이 PC 빼기], 로그아웃이 그 PC에서 바로 듣는지, 재부팅 뒤 로그온 화면, 다른 Windows 사용자, 다시 설치와 제거 뒤도 봅니다.
     - 아이폰 로그인(11단계, 7-9): TestFlight의 1.6.0에서 로그인해 "액세스 가능한 장치"의 내 PC에 비밀번호 없이 연결하고, 새 기기 알림([내 기기 맞음], [아님]), '로그인한 기기' 목록, 잠긴 동안의 "보안 키를 지금 쓸 수 없습니다", 백업에서 복원한 뒤의 "로그인이 풀렸습니다"를 봅니다.
     - NAS 중계기 설치(15단계, 7-11): [relay-dsm.md](relay-dsm.md)대로 DSM에 설치하고(부서의 허락을 받은 뒤, 질문 14) 로그의 등록 코드, 승인 뒤의 `등록이 승인되어 켜기 명령을 기다립니다.`, `status` 작업의 결과를 봅니다.
     - NAS에서만 볼 수 있는 중계기 동작(15단계, 7-11): 개발 PC의 시험은 중계기를 root가 아닌 사용자로 돌려 권한 내리기를 거치지 않고, DSM 부트업 스크립트도 개발 PC의 `sh`로만 돌려 봤습니다. Synology 공유 폴더의 권한(ACL) 아래에서 멈춤 표지를 본 적도 없습니다. 그래서 [relay-dsm.md](relay-dsm.md) 5장의 첫 로그에 `권한을 uid 65534 gid 65534로 낮췄습니다.`가 있는지, 6장 8번의 멈춤 확인(10장의 두 줄)이 되는지, NAS가 다음에 재부팅된 뒤 로그에 시작 줄이 생기는지(11장 끝)를 봅니다.
     - 휴대폰의 켜기 메뉴(15단계 뒤): 꺼진 내 PC의 카드 메뉴 "켜기"와 카드를 누를 때의 권유, 진행 문구의 순서, 켜진 뒤 연결("연결합니다…"가 30초 안인지), 진행 창을 닫았다가 다시 뜨는 것, NAS를 껐을 때와 5분 안에 켜지지 않을 때의 문구를 봅니다. PC 쪽에서는 묶은 뒤 1~3분 안에 서비스 로그의 `hjv: wake-on-LAN report: 1 wired card(s)`, 깨운 뒤 아무도 연결하지 않아도 10분 넘게 깨어 있는지와 `hjv: resumed: staying awake for ten minutes`를 봅니다.

### 7-2. 읽기 점검 (3단계)

아무것도 바꾸지 않습니다. 마지막 `systemd-run` 줄만 임시 유닛 하나를 띄웠다가 지웁니다.

```bash
S="sudo -n"
echo "== sudo"; $S true 2>/dev/null && echo SUDO_OK || echo SUDO_NEEDS_PASSWORD
echo "== already there?"; ls -ld /run/hjviewer-api.sock /etc/systemd/system/hjviewer-api* /etc/hjviewer/hjviewer-api.env /var/lib/hjviewer-api /var/log/hjviewer-api /etc/nginx/conf.d/hjviewer-api-zones.conf /etc/nginx/snippets/hjviewer-api.conf 2>&1
echo "== hbbs, hbbr"; systemctl is-active hjviewer-hbbs hjviewer-hbbr; ls -l /opt/hjviewer/bin/
echo "== nginx"; $S nginx -t 2>&1 | tail -1
$S nginx -T 2>/dev/null | grep -nE '^# configuration file|hjviewer|zone=hjv_|real_ip_header|set_real_ip_from|proxy_intercept_errors|auth_basic|limit_req|limit_conn' | head -60
grep -n 'conf.d/\*.conf' /etc/nginx/nginx.conf
echo "== nginx workers"; ps -o user=,group= -C nginx | sort | uniq -c
echo "== php-fpm pools"; $S grep -hE '^\s*(user|group|listen\.group)\s*=' /etc/php/*/fpm/pool.d/*.conf 2>/dev/null | sort | uniq -c
echo "== DNS"; grep -E '^nameserver' /etc/resolv.conf
echo "== journald, credentials"; systemctl --version | head -1; ls /etc/systemd/journald*.conf* 2>/dev/null; systemd-creds has-tpm2 2>&1 | head -1; $S ls -l /var/lib/systemd/credential.secret 2>&1
echo "== disk"; df -h / /var
echo "== timers and cron (names only)"; systemctl list-timers --all --no-legend | awk '{print $(NF-1)}' | sort -u; ls /etc/cron.d; $S grep -lE 'nginx|backup|rsync|restic|borg' /etc/crontab /etc/cron.d/* 2>/dev/null
echo "== IP filter: without, then with IPAddressDeny (the second should be 000)"
curl -sk -o /dev/null -w '%{http_code}\n' --max-time 5 https://127.0.0.1/
$S systemd-run --wait --pipe --quiet -p IPAddressDeny=127.0.0.1 curl -sk -o /dev/null -w '%{http_code}\n' --max-time 5 https://127.0.0.1/
```

판단:
- `SUDO_NEEDS_PASSWORD`면 2단계 1번처럼 멈춥니다.
- `ls -ld`가 하나라도 찾으면(이미 설치를 시작한 서버) 멈추고 어디까지 했는지 보고합니다.
- `nginx -t`가 이미 실패하면 멈춥니다.
- `real_ip_header`나 `set_real_ip_from`이 HJViewer 경로가 있는 server 블록이나 http 블록에 있고 넓은 범위(예: 0.0.0.0/0)를 믿으면 멈춥니다. hjv-api는 nginx가 `$remote_addr`로 채우는 `X-Real-IP`로 로그인 시도를 셉니다.
- `zone=hjv_` 이름을 다른 설정이 이미 쓰거나, nginx.conf가 http 블록에서 `conf.d/*.conf`를 읽지 않으면 멈춥니다(zones 파일을 둘 곳).
- nginx 작업 프로세스의 그룹이 `www-data`가 아니면 보고합니다. 7-3에서 소켓 그룹을 드롭인으로 바꿉니다.
- PHP-FPM이 `www-data`로 돌면 보고합니다. 같은 서버의 PHP도 hjv-api 소켓을 부를 수 있다는 뜻입니다(키, 자격 증명, DB는 다른 사용자라 읽지 못함).
- IP 필터의 두 줄: 첫 줄이 000이면 웹 서버가 127.0.0.1에서 받지 않는 것이니 판단하지 않고 보고합니다. 첫 줄은 000이 아닌데 둘째 줄도 000이 아니면, 이 커널에서는 systemd의 IP 필터가 효과가 없으므로 보고하고 7-3의 나가는 연결 제한(선택)을 넣지 않습니다.
- 백업이나 동기화 작업이 `/var/lib`나 `/etc`를 서버 밖으로 보내면 보고합니다. hjv-api의 DB와 자격 증명은 서버 밖 사본을 두지 않기로 했습니다(7-14).

보고: 위 판단의 결과와 개수만. 설정 파일 내용, 주소, 이름은 옮기지 않습니다.

### 7-3. hjv-api 설치 (4단계)

hjv-api의 실행 파일, 유닛, 설정 파일과 묶음의 고지·웹 서버 설정 사본(`/opt/hjviewer/share/`)만 설치합니다. 서비스는 키가 생기는 5단계 뒤에 시작하고, nginx는 9단계까지 그대로이며, hbbs는 7단계에서 바꿉니다. 이 묶음의 `bin/hbbs`, `bin/hbbr`은 여기서 설치하지 않습니다.

```bash
VERSION=2026.10.20                # ← hjv-api가 든 서버 묶음의 Release 버전 (입력값)
for f in /etc/systemd/system/hjviewer-api.service /etc/systemd/system/hjviewer-api.socket /etc/hjviewer/hjviewer-api.env /var/lib/hjviewer-api /var/log/hjviewer-api; do [ -e "$f" ] && { echo "STOP: $f 이(가) 이미 있음"; exit 1; }; done
T=$(mktemp -d) && cd "$T" || exit 1
base="https://github.com/yonsei-projecttf/hjviewer/releases/download/server-$VERSION"
pkg="hjviewer-server-$VERSION-linux-x86_64"
if ! curl -fsSLO --max-time 120 "$base/$pkg.tar.gz" || ! curl -fsSLO --max-time 30 "$base/SHA256SUMS"; then echo 'STOP: 내려받기 실패'; exit 1; fi
grep " $pkg.tar.gz\$" SHA256SUMS | sha256sum -c - || { echo 'STOP: 체크섬이 다름'; exit 1; }
tar -xzf "$pkg.tar.gz" && cd "$pkg" || exit 1
[ -x bin/hjv-api ] || { echo 'STOP: 묶음에 hjv-api가 없음'; exit 1; }
if ldd bin/hjv-api | grep 'not found'; then echo 'STOP: 이 서버에서 실행할 수 없음'; exit 1; fi
bin/hjv-api version; head -3 SOURCE.txt
getent passwd hjviewer-api || sudo useradd --system --user-group --home-dir /var/lib/hjviewer-api --no-create-home --shell /usr/sbin/nologin hjviewer-api
sudo install -m 755 bin/hjv-api /opt/hjviewer/bin/
sudo install -m 644 LICENSE.txt NOTICE.txt SOURCE.txt THIRD_PARTY_NOTICES.txt /opt/hjviewer/share/
sudo install -m 644 web/* /opt/hjviewer/share/web/    # 계정 API의 nginx 파일 둘, 그리고 새 저장소 주소의 사본
sudo install -m 644 systemd/hjviewer-api.env.example /etc/hjviewer/hjviewer-api.env
units=()
for u in systemd/hjviewer-api*.socket systemd/hjviewer-api*.service systemd/hjviewer-api*.timer; do
  [ -e "$u" ] || continue
  sudo install -m 644 "$u" /etc/systemd/system/
  case $u in *@.*) units+=("$(basename "${u/@./@check.}")") ;; *) units+=("/etc/systemd/system/$(basename "$u")") ;; esac
done
[ -f systemd/journald@hjviewer.conf ] && sudo install -m 644 systemd/journald@hjviewer.conf /etc/systemd/
sudo systemctl daemon-reload
systemd-analyze verify "${units[@]}" && echo UNITS_OK
cd / && rm -rf "$T"
```

`UNITS_OK`가 나오지 않으면 멈춥니다. 감시·백업 유닛(`hjviewer-api-*`)은 설치만 하고 6단계에서 켭니다.

질문 10의 답이 '안 됨'이었으면(7-1의 3번) 공개 묶음에는 hjv-api가 없어 `STOP: 묶음에 hjv-api가 없음`에서 멈춥니다. 그때는 사용자가 비공개 저장소의 Release에서 받은 hjv-api 묶음(`.tar.gz`와 `SHA256SUMS`)을 서버의 빈 폴더에 올립니다. 위 블록을 `T=$(mktemp -d)` 대신 `T=<그 폴더>`로, `pkg=`를 그 묶음의 이름(`.tar.gz` 앞부분)으로 두고, `base=`와 `curl` 줄을 건너뛰어 그대로 실행합니다. 다만 고지 파일 줄은 `/opt/hjviewer/share/` 대신 `/opt/hjviewer/share/hjv-api/`(먼저 `sudo install -d -m 755`)에 설치합니다. `/opt/hjviewer/share/`의 고지는 7-6에서 공개 묶음의 hbbs·hbbr 것으로 바뀝니다.

설정 파일(비밀 아님)을 채웁니다. hbbs 공개키는 이 서버의 것을 넣고, SSO 주소와 client-id, 학교 주소 대역은 사용자에게 받습니다. SSO 값은 같은 서버의 WordPress SSO 로그인 플러그인(projecttf-auth)이 쓰는 것과 같은 등록입니다(결정 D2).

```bash
TOKEN_URL='https://<SSO 호스트>/<토큰 경로>'          # ← 사용자에게 받음
USERINFO_URL='https://<SSO 호스트>/<사용자 정보 경로>'  # ← 사용자에게 받음
CLIENT_ID='<client-id>'                                # ← 사용자에게 받음
CAMPUS_NETS='<학교 주소 대역, 쉼표로>'                   # ← 사용자에게 받음(비우면 학내·학외를 가리지 않음)
E=/etc/hjviewer/hjviewer-api.env
sudo sed -i -e "s|^HJV_API_SSO_TOKEN_URL=.*|HJV_API_SSO_TOKEN_URL=$TOKEN_URL|" \
  -e "s|^HJV_API_SSO_USERINFO_URL=.*|HJV_API_SSO_USERINFO_URL=$USERINFO_URL|" \
  -e "s|^HJV_API_SSO_CLIENT_ID=.*|HJV_API_SSO_CLIENT_ID=$CLIENT_ID|" \
  -e "s|^HJV_API_CAMPUS_NETS=.*|HJV_API_CAMPUS_NETS=$CAMPUS_NETS|" \
  -e "s|^HJV_API_HBBS_PUBKEY=.*|HJV_API_HBBS_PUBKEY=$(sudo cat /var/lib/hjviewer/id_ed25519.pub)|" "$E"
sudo grep -cE '^HJV_API_(SSO_TOKEN_URL|SSO_USERINFO_URL|SSO_CLIENT_ID|HBBS_PUBKEY)=.+' "$E"   # 4
sudo grep -E '^#?HJV_API_(BIND_OS|SECOND_FACTOR|TRUST_REFRESH|TRUST_TTL|PASSWORDLESS_KINDS)=' "$E"
```

- 마지막 줄의 기본값을 바꾸지 않습니다: `HJV_API_BIND_OS=windows`, `HJV_API_SECOND_FACTOR=off`(결정 U1), `HJV_API_TRUST_REFRESH=120`(60~300), `HJV_API_TRUST_TTL=86400`(U4, 72시간 이하), `HJV_API_PASSWORDLESS_KINDS=remote,file,camera`(U3). 개발 PC의 `scripts/ci.sh deploy`는 이 가운데 `BIND_OS`, `SECOND_FACTOR`, `TRUST_REFRESH`, `TRUST_TTL` 넷을 묶음의 예시에서 검사하고, 이 서버의 설정 파일 사본(비밀 없음)으로도 검사할 수 있습니다(`HJV_API_ENV_FILE`, [ci.md](ci.md)). `PASSWORDLESS_KINDS`는 검사하지 않으므로 이 출력으로 확인합니다.
- nginx 작업 프로세스의 그룹이 `www-data`가 아니면(7-2) 소켓 그룹을 드롭인으로 바꿉니다: `sudo systemctl edit hjviewer-api.socket`에 `[Socket]`과 `SocketGroup=<그 그룹>`.
- (선택, 사용자 승인 뒤) 나가는 연결 제한: 7-2의 IP 필터 시험에서 둘째 줄만 000이었으면 `/etc/systemd/system/hjviewer-api.service.d/egress.conf`에 `[Service]`, `IPAddressDeny=any`, `IPAddressAllow=localhost <SSO 서버 주소> <DNS 서버 주소, 127.0.0.53이 아닐 때>`를 둡니다. SSO 서버의 주소가 바뀌면 로그인이 멈추므로, 넣었다는 것을 보고에 적습니다.

### 7-4. 키와 자격 증명 만들기 (5단계, 사용자만)

**멈춤.** 운영 에이전트는 이 단계를 하지 않고, 사용자가 끝났다고 알려 줄 때까지 기다립니다. 사용자는 에이전트 세션의 터미널이 아닌 곳에서 합니다. 개인키는 화면에 나오지 않고, 화면에는 공개키, 키 번호, 지문만 나옵니다.

**(가) 오프라인 키**: 봉인 키, 목록 키 2번, 명령 키 2번. 서버가 없어져도 되살리거나 교체할 수 있게 서버 밖에 둡니다.

1. 에이전트가 돌지 않는 리눅스 환경(오프라인 PC나 Ubuntu 24.04 라이브 USB)에서, 7-3과 같은 서버 묶음(체크섬을 확인한 것)의 `bin/hjv-api`로 이동식 매체에 바로 만듭니다. AI 에이전트가 도는 개발 PC(WSL 포함)에서는 만들지 않습니다.
   ```bash
   M='/media/<매체>'                                                 # ← 이동식 매체
   ./hjv-api keygen seal --out "$M/hjviewer-seal.key"                # 화면: 봉인 키 ID와 공개키(HJV_LOGIN_SEAL_PUBKEY)
   ./hjv-api keygen list --kid 2 --out "$M/hjviewer-list-2.key"      # 화면: 2:<base64>와 지문
   ./hjv-api keygen command --kid 2 --out "$M/hjviewer-command-2.key" # 원격 켜기용. 화면: 2:<base64>와 지문
   ```
2. 매체를 하나 더 만들어(파일 복사) 서로 다른 곳에 둡니다. 프로젝트 폴더, NAS의 부서 공유 폴더, 그 폴더로 가는 백업 대상에는 두지 않습니다.
3. 서버에는 지금 봉인 키만 넣습니다. 목록 키 2번과 명령 키 2번은 교체할 때 넣습니다(7-15). 서버 디스크에 평문이 남지 않게 표준 입력으로 넣습니다. 그 계정의 sudo가 비밀번호를 묻지 않으면 리눅스·macOS 셸에서 한 줄로 됩니다(PowerShell에는 입력 `<`가 없음).
   ```bash
   ssh <서버> 'sudo install -d -m 700 /etc/credstore.encrypted && sudo systemd-creds encrypt --name=hjviewer-api-login-key - /etc/credstore.encrypted/hjviewer-api-login-key' < /media/<매체>/hjviewer-seal.key
   ```
   sudo가 비밀번호를 묻는 계정이면 위 줄은 터미널이 없어 실패합니다. 그때는 서버의 메모리 파일 시스템 `/dev/shm`을 잠깐 거칩니다. 디스크의 파일로는 남지 않지만 메모리가 모자라면 스왑에 쓰일 수 있으므로, 넣자마자 지웁니다. 아래 세 줄은 **Windows PowerShell이나 리눅스·macOS 셸에서만** 합니다(Windows에서는 `scp`의 원본만 `E:\hjviewer-seal.key`처럼 바꿈). 명령 프롬프트(cmd)는 작은따옴표를 따옴표로 보지 않고 `&&`와 `>`를 자기가 처리하므로, 첫 줄과 셋째 줄은 서버에서 실패하고 `scp`만 성공해 평문 키가 `/dev/shm`에 남습니다.
   ```bash
   ssh <서버> 'rm -f /dev/shm/hjv-key; umask 077; : > /dev/shm/hjv-key; stat -c %a:%U /dev/shm/hjv-key'
   scp /media/<매체>/hjviewer-seal.key <서버>:/dev/shm/hjv-key
   ssh -t <서버> 'sudo install -d -m 700 /etc/credstore.encrypted && sudo systemd-creds encrypt --name=hjviewer-api-login-key /dev/shm/hjv-key /etc/credstore.encrypted/hjviewer-api-login-key; rm -f /dev/shm/hjv-key'
   ```
   - 첫 줄은 남아 있던 파일을 지우고 이 계정만 읽는 빈 파일을 새로 만듭니다. 출력이 `600:<그 계정 이름>`이 아니면 둘째 줄로 가지 않습니다. `scp`는 이미 있는 파일의 권한을 바꾸지 않고 내용만 씁니다.
   - 어느 줄이든 실패하거나 중간에 끊겼으면 `ssh <서버> 'rm -f /dev/shm/hjv-key'`로 지웁니다.

**(나) 서버에서 바로 만드는 것**: SSO 자격 증명, 기록 키, 목록 키 1번, 명령 키 1번. 사용자가 서버에 SSH로 들어가 합니다.

```bash
sudo -v
sudo install -d -m 700 /etc/credstore.encrypted
# SSO 자격 증명: WordPress SSO 로그인 플러그인(projecttf-auth)과 같은 값(D2)
systemd-ask-password -n "SSO client secret:" | sudo systemd-creds encrypt --name=hjviewer-sso-client-secret - /etc/credstore.encrypted/hjviewer-sso-client-secret
systemd-ask-password -n "SSO api uuid:" | sudo systemd-creds encrypt --name=hjviewer-sso-api-uuid - /etc/credstore.encrypted/hjviewer-sso-api-uuid
# 서버에만 있는 키(오프라인 사본 없음)
sudo /opt/hjviewer/bin/hjv-api keygen log-key --encrypt-to /etc/credstore.encrypted/hjviewer-api-log-key
sudo /opt/hjviewer/bin/hjv-api keygen list --kid 1 --encrypt-to /etc/credstore.encrypted/hjviewer-api-list-key        # 화면: 1:<base64>와 지문
sudo /opt/hjviewer/bin/hjv-api keygen command --kid 1 --encrypt-to /etc/credstore.encrypted/hjviewer-api-command-key  # 원격 켜기용. 화면: 1:<base64>와 지문
sudo ls -l /etc/credstore.encrypted/
```

- SSO client secret은 토큰 요청의 `Authorization: Basic ` 뒤에 오는 값 그대로입니다. `-n`은 끝에 줄바꿈을 붙이지 않습니다.
- 여섯 자격 증명(SSO 둘, 봉인 키, 기록 키, 목록 키, 명령 키)이 모두 있어야 6단계에서 서비스가 시작합니다. 유닛이 명령 키(`hjviewer-api-command-key`)도 요구합니다.
- `keygen list`와 `keygen command`는 화면에 공개키(`N:<base64>`)와 지문(`fingerprint: N:<SHA-256 16진수>`)을 냅니다. `keygen command`의 첫 줄은 `N:<base64>`뿐이라 `HJV_COMMAND_PUBKEY`의 항목으로 그대로 씁니다. 지문은 적어 두었다가 6단계의 `selfcheck`, 8단계의 빌드 요약과 대조합니다.
- 기록 키를 잃으면 새로 만듭니다(옛 기록의 꼬리표와 이어지지 않을 뿐). 목록 키 1번과 명령 키 1번을 잃으면 오프라인의 2번으로 바꿉니다(7-15).
- 최소 버전 작업이 함께 나가면 그 작업의 업데이트 키도 이때 만듭니다(그 작업의 문서).

**(다) 알려 줄 것**: 공개값만 운영 에이전트와 개발 PC에 알립니다. 모두 비밀이 아닙니다.

- `HJV_LOGIN_SEAL_PUBKEY`: 봉인 키의 공개키(base64 한 개)와 키 ID
- `HJV_TRUST_PUBKEYS`: `1:<base64>,2:<base64>`와 두 지문
- `HJV_COMMAND_PUBKEY`: `1:<base64>,2:<base64>`와 두 지문

### 7-5. 공개키 대조값, 시작, selfcheck, 명단 (6단계)

```bash
TRUST='1:<base64>,2:<base64>'      # ← 사용자가 알려 준 HJV_TRUST_PUBKEYS
COMMAND='1:<base64>,2:<base64>'    # ← 사용자가 알려 준 HJV_COMMAND_PUBKEY
E=/etc/hjviewer/hjviewer-api.env
sudo sed -i -e "s|^#\?HJV_API_TRUST_PUBKEYS=.*|HJV_API_TRUST_PUBKEYS=$TRUST|" \
  -e "s|^#\?HJV_API_COMMAND_PUBKEYS=.*|HJV_API_COMMAND_PUBKEYS=$COMMAND|" "$E"
[ "$(sudo grep -cE '^HJV_API_(TRUST|COMMAND)_PUBKEYS=.+' "$E")" = 2 ] || { echo 'STOP: 공개키 대조값 두 줄이 설정 파일에 없음'; exit 1; }
sudo grep -E '^HJV_API_(TRUST|COMMAND)_PUBKEYS=' "$E"
sudo systemctl enable --now hjviewer-api.socket hjviewer-api.service
sleep 3; systemctl is-active hjviewer-api.socket hjviewer-api.service      # active active
sudo stat -c '%a %U:%G %n' /run/hjviewer-api.sock /var/lib/hjviewer-api /var/lib/hjviewer-api/hjv-api.sqlite3 /var/log/hjviewer-api /var/log/hjviewer-api/operator.log
sudo chattr +a /var/log/hjviewer-api/operator.log && sudo lsattr /var/log/hjviewer-api/operator.log
A="sudo /opt/hjviewer/bin/hjv-api admin"
$A passwordless status        # 첫 줄에 off
$A selfcheck
sudo curl -s --unix-socket /run/hjviewer-api.sock http://localhost/api/hjv/source; echo
sudo journalctl --namespace=hjviewer -u hjviewer-api -p warning --since -10min -o cat | wc -l
```

- 소켓은 `660 root:www-data`(또는 7-3에서 바꾼 그룹), 상태 디렉터리와 로그 디렉터리는 `700 hjviewer-api`, DB와 `operator.log`는 `600`이어야 합니다. 로그 디렉터리를 root로 만들거나 나중에 `chown`·`chmod`하지 않습니다. 추가 전용 속성이 걸린 파일 때문에 systemd가 소유자를 고치다 실패해 서비스가 시작하지 않습니다([server/api/README.md](../server/api/README.md)의 "The operator record").
- `STOP: 공개키 대조값 …`이면 아무것도 시작하지 않은 채 멈춥니다. 설정 파일에 `HJV_API_TRUST_PUBKEYS=`나 `HJV_API_COMMAND_PUBKEYS=` 줄이 없는 것이니 묶음의 env 예시와 비교합니다.
- `selfcheck`는 SSO 주소의 DNS·TCP·TLS, hbbs 콘솔, 목록 키와 명령 키를 봅니다. 키마다 두 줄입니다: 고정한 공개키의 지문(`list keys the apps pin …`, `command keys the relays pin …`)과 쓰는 키가 고정한 그 번호의 키인지(`list key in use (HJV_API_LIST_KID 1): the pinned key 1, …`, `command key in use (HJV_API_COMMAND_KID 1): the pinned key 1, …`). 7단계 전에는 `hbbs console` 줄 하나만 `[FAIL]`(패치 0004 전)이고 나머지는 `[ OK ]`여야 합니다.
- 목록 키나 명령 키 줄이 `[FAIL]`이면 멈춥니다. 고정한 목록 키가 다르면 모든 PC가 모든 목록을 거절하고, 명령 키가 다르면 모든 중계기가 켜기 명령을 거절합니다. `HJV_API_COMMAND_PUBKEYS`가 비어 있어도 쓰는 명령 키를 대조할 수 없어 `[FAIL]`입니다.
- 지문 줄(공개값)은 사용자가 5단계에서 키를 만들 때 본 지문과 같아야 하고, 8단계의 대조에 쓰므로 목록 키와 명령 키 것을 모두 보고에 옮깁니다.
- 서비스가 시작하지 않으면 `sudo journalctl --namespace=hjviewer -u hjviewer-api -n 20 -o cat`의 오류 줄을 사용자에게만 보여 줍니다. 설정 파일의 모르는 `HJV_API_` 이름, 빠진 SSO 값, 없는 자격 증명(명령 키 포함, 7-4)이 흔한 원인입니다.

명단을 넣습니다. 사용자가 포탈 ID를 한 줄에 하나씩 적은 파일을 서버에 두고(예: `/root/hjviewer-allow.txt`, 600) 경로만 알려 줍니다. 운영 에이전트는 파일을 열지 않고 가져오기만 합니다. 출력은 개수뿐입니다.

```bash
sudo /opt/hjviewer/bin/hjv-api admin allow import /root/hjviewer-allow.txt
sudo /opt/hjviewer/bin/hjv-api admin report | head -3
```

가져온 뒤 파일은 사용자가 지웁니다. 명단은 DB에 있습니다.

감시와 백업 타이머를 켭니다(7-14).

```bash
for t in /etc/systemd/system/hjviewer-api-*.timer; do [ -e "$t" ] && sudo systemctl enable --now "$(basename "$t")"; done
systemctl list-timers 'hjviewer-api-*' --no-legend
```

### 7-6. hbbs 0004로 교체 (7단계)

조용한 시간에 합니다. hbbs만 다시 시작하므로 PC는 몇 초 안에 다시 등록되지만, 그때 진행 중인 원격 세션은 끊깁니다. 최소 버전 작업의 hbbs 0005가 함께 나가면 같은 실행 파일에 들어 있으므로 다시 시작은 한 번입니다.

```bash
VERSION=2026.10.20                # ← 7-3과 같은 Release 버전
T=$(mktemp -d) && cd "$T" || exit 1
base="https://github.com/yonsei-projecttf/hjviewer/releases/download/server-$VERSION"
pkg="hjviewer-server-$VERSION-linux-x86_64"
if ! curl -fsSLO --max-time 120 "$base/$pkg.tar.gz" || ! curl -fsSLO --max-time 30 "$base/SHA256SUMS"; then echo 'STOP: 내려받기 실패'; exit 1; fi
grep " $pkg.tar.gz\$" SHA256SUMS | sha256sum -c - || { echo 'STOP: 체크섬이 다름'; exit 1; }
tar -xzf "$pkg.tar.gz" && cd "$pkg" || exit 1
if ldd bin/hbbs | grep 'not found'; then echo 'STOP: 이 서버에서 실행할 수 없음'; exit 1; fi
for s in hbbs hbbr; do   # 저장소 주소(Documentation= 줄)만 다른 것은 바뀐 것으로 보지 않음
  diff <(grep -v '^Documentation=' "systemd/hjviewer-$s.service") <(grep -v '^Documentation=' "/etc/systemd/system/hjviewer-$s.service") || { echo "STOP: $s 유닛도 바뀜"; exit 1; }
done
sudo install -d -m 700 /root/hjv-backup
sudo cp -p /opt/hjviewer/bin/hbbs "/root/hjv-backup/hbbs-before-$VERSION"
sudo install -m 755 bin/hbbs /opt/hjviewer/bin/hbbs
sudo install -m 644 LICENSE.txt NOTICE.txt SOURCE.txt THIRD_PARTY_NOTICES.txt /opt/hjviewer/share/   # 이 hbbs의 고지(7-3과 같은 묶음이면 같은 파일)
sudo install -m 644 systemd/hjviewer-hbbs.service systemd/hjviewer-hbbr.service /etc/systemd/system/
sudo systemctl daemon-reload
sudo systemctl restart hjviewer-hbbs
sleep 3; systemctl is-active hjviewer-hbbs hjviewer-hbbr
sudo journalctl -q -u hjviewer-hbbs --since -2min -o cat | grep -E 'ERROR|panicked' | tail -5
sudo /opt/hjviewer/bin/hjv-api admin selfcheck | grep 'hbbs console'
cd / && rm -rf "$T"
```

- 지금 운영 중인 서버의 유닛은 `server-2026.10.10` 묶음의 것이고, 1.6.0 묶음의 유닛과는 `Documentation=` 줄(저장소를 옮기기 전의 주소라 지금은 404)만 다릅니다. 그래서 비교에서 그 줄을 빼고, 같으면 두 유닛을 새것으로 바꿔 링크도 고칩니다. hbbr은 다시 시작하지 않습니다. 바뀐 것이 설명 줄뿐이라 `daemon-reload`로 충분합니다.
- `STOP: hbbs 유닛도 바뀜`(또는 `hbbr`)이면 아무것도 바꾸지 않은 채 멈추고, `diff`가 보인 차이를 보고합니다. 그 차이는 "B 경로 업데이트"처럼 사용자 승인을 받아 따로 반영합니다.
- `hbbs console: answers hjv-pk (patch 0004)`가 나와야 합니다. 5절의 첫 블록을 다시 실행해 `/ws/id`, `/ws/relay`가 101인지도 봅니다. 사용자는 바깥에서 `hjv-probe`로 확인합니다.
- 되돌리기: 보관한 `/root/hjv-backup/hbbs-before-<VERSION>`을 같은 방법으로 넣고 hbbs만 다시 시작합니다(7-12).

### 7-7. 앱과 중계기 빌드, 지문 대조 (8단계, 개발 PC)

서버에서 할 일은 `sudo /opt/hjviewer/bin/hjv-api admin selfcheck`의 지문 줄을 다시 보고하는 것뿐입니다. 나머지는 개발 PC와 사용자가 합니다([ci.md](ci.md)의 "계정 기능의 공개키", "hjv-relay 빌드").

1. 비공개 빌드 저장소의 시크릿에 `HJV_LOGIN_SEAL_PUBKEY`, `HJV_TRUST_PUBKEYS`, `HJV_COMMAND_PUBKEY`(그리고 최소 버전 작업의 `HJV_UPDATE_PUBKEY`)를 넣습니다. 공개값입니다.
2. 2단계의 커밋으로 Windows(`HJViewer.exe`), Android, iOS, hjv-relay를 **`testflight=false`**로 빌드합니다. Windows·iOS 전용 코드는 2단계의 확인 빌드에서 이미 컴파일해 본 것이고(7-1), 여기서는 공개키를 넣은 출시 빌드를 만듭니다. 실패하면 개발 PC에서 고치고 다시 push한 커밋으로 빌드합니다. 서버 코드가 바뀌었으면 서버 묶음과 3~7단계의 해당 부분도 다시 합니다.
3. 빌드 요약의 고정 공개키 지문이 `selfcheck`의 지문, 그리고 5단계에서 사용자가 본 지문과 같은지 사람이 대조합니다. 봉인 키 ID는 지문의 앞 16자리입니다.
4. 같고, 7-1의 5번에서 출시 태그 전에 끝내기로 한 것(현장 시험 결과, TCP 펀치 시험)이 끝났으면 그 커밋에 출시 태그 `client-1.6.0`, `relay-1.6.0`을 붙입니다. 아직 나눠 주지 않습니다(TestFlight 업로드는 9단계 뒤).

### 7-8. nginx 전환 (9단계)

**관문**: 다음이 모두 끝났는지 사용자에게 확인합니다. 하나라도 아니면 멈춥니다.
- 질문 7의 답(개인정보 처리자와 연락처, 정보보호팀의 회신: 공공기관 개인정보파일 등록, AI 운영 에이전트가 처리위탁·국외 이전에 해당하는지)을 반영한 [privacy.md](privacy.md)가 공개 저장소에 있음. 2단계의 push에 이미 들어갔으면 됩니다. 답이 그 뒤에 왔으면 privacy.md의 처리 근거, 처리자, 연락처(7, 9, 10절, 두 언어)를 고쳐 문서만 한 번 더 push합니다(`scripts/ci.sh leak` 통과 뒤. 앱과 서버 묶음은 그대로).
- 사용자 약 10명에게 무엇이 저장되는지 미리 알림.
- 날짜: 같은 서버의 다른 서비스 일정과 겹치지 않는 조용한 저녁(질문 9). cron의 nginx 재적재 시각은 피함.

절차:

1. 같은 443 포트의 모든 사이트의 지금 응답 코드를 기록합니다. 4-3 (나)의 2번 블록과 같습니다(`/tmp/hjv-sites-before.txt`).
2. 설치하고 검사합니다. 실패하면 되돌리고 멈춥니다.
   ```bash
   L=/etc/nginx/snippets/hjviewer-locations.conf
   [ "$(grep -c '^location \^~ /api/ {' "$L")" = 1 ] || { echo 'STOP: 경로 파일의 /api/ 블록이 예상과 다름'; exit 1; }
   sudo install -d -m 700 /root/hjv-backup && sudo cp -p "$L" /root/hjv-backup/hjviewer-locations.conf.before-api
   sudo install -m 644 /opt/hjviewer/share/web/nginx-hjviewer-api-zones.conf /etc/nginx/conf.d/hjviewer-api-zones.conf
   sudo install -m 644 /opt/hjviewer/share/web/nginx-hjviewer-api.conf /etc/nginx/snippets/hjviewer-api.conf
   N=$(mktemp)
   awk '/^location \^~ \/api\/ \{/ { print "include snippets/hjviewer-api.conf;"; skip = 1; next }
     skip { if ($0 ~ /^\}/) skip = 0; next } { print }' "$L" > "$N"
   sudo install -m 644 "$N" "$L" && rm -f "$N"
   sudo diff -u /root/hjv-backup/hjviewer-locations.conf.before-api "$L"   # /api/ 블록이 include 한 줄로 바뀐 것만
   out=$(sudo nginx -t 2>&1); echo "$out"
   if echo "$out" | grep -qE 'emerg|test failed'; then
     sudo cp -p /root/hjv-backup/hjviewer-locations.conf.before-api "$L"
     sudo rm -f /etc/nginx/conf.d/hjviewer-api-zones.conf /etc/nginx/snippets/hjviewer-api.conf
     echo 'STOP: 설정 오류, 되돌림'; exit 1
   fi
   sudo systemctl reload nginx
   ```
3. 확인합니다. 하나라도 다르면 2번의 되돌리기(백업 복원, 두 파일 삭제, `nginx -t`, reload)를 하고 멈춥니다.
   ```bash
   DOMAIN=$(sed -n 's/^HJV_DOMAIN=//p' /etc/hjviewer/hjviewer.env); [ -n "$DOMAIN" ] || exit 1
   c() { curl -s --max-time 10 --resolve "$DOMAIN:443:127.0.0.1" -o /tmp/hjv-body -w '%{http_code}' "$@"; echo " $(head -c 80 /tmp/hjv-body)"; }
   echo "source         $(c "https://$DOMAIN/api/hjv/source")"          # 200 {"license":"AGPL-3.0-only",…}
   echo "login-options  $(c "https://$DOMAIN/api/login-options")"       # 200 []
   echo "heartbeat      $(c -X POST "https://$DOMAIN/api/heartbeat")"   # 404 {"error":"not found"}
   echo "x.php          $(c "https://$DOMAIN/api/x.php")"               # 404 JSON (사이트의 PHP로 가지 않음)
   echo "peers          $(c "https://$DOMAIN/api/peers")"               # 401 (토큰 없음)
   while read -r name before; do
     after=$(curl -s -o /dev/null -w '%{http_code}' --max-time 10 --resolve "$name:443:127.0.0.1" "https://$name/")
     echo "$name $before -> $after"; [ "$before" = "$after" ] || echo "  DIFFERENT"
   done < /tmp/hjv-sites-before.txt
   ```
   5절의 첫 블록으로 `/ws/id`, `/ws/relay`가 101인지도 봅니다.
4. 1.5.1 앱은 이 전환의 영향을 받지 않습니다. 로그인하지 않으므로 계정 경로를 부르지 않습니다.
5. 최소 버전 작업이 함께 나가면 그 작업의 nginx 위치도 2번에서 같은 스니펫과 함께 넣고, reload는 한 번만 합니다(그 작업의 문서).

### 7-9. 배포와 켜기 (10~13단계)

1. **배포(10단계, 개발 PC)**: 7-7의 커밋으로 iOS 잡을 `testflight=true`로 다시 돌려 TestFlight 내부 그룹에 올립니다(빌드 번호만 바뀜). Windows·Android 산출물을 나눠 줍니다(Google Play 내부 테스트는 그 작업의 문서대로). nginx 전환 뒤에 올리는 이유는 1.6.0 아이폰이 전환 전의 `/api` 404를 받지 않게 하려는 것입니다.
2. **운영자 PC(11단계, 사용자)**: 운영자의 Windows PC에 1.6.0을 1.5.1 위에 설치하고 로그인합니다. 알림에 "관리자가 이 기능을 켜면 바로 쓸 수 있습니다"가 나오고, 설정 → 계정에 "관리자가 일시 중지함"이 보여야 합니다. PC 비밀번호(영구 비밀번호)가 정해져 있는지 보고, 아이폰에서도 로그인합니다.
3. **운영자 계정부터 켜기(12단계)**: 사용자가 알려 준 운영자 PC의 ID로 그 계정만 켭니다.
   ```bash
   A="sudo /opt/hjviewer/bin/hjv-api admin"
   $A passwordless on --owner-of 123456789     # ← 운영자 PC의 ID
   $A passwordless status
   ```
   5분 안에 그 PC의 설정 줄이 "비밀번호 없이 연결 켜짐"으로 바뀌어야 합니다. 일주일 동안 실기기 점검(설계 3판 7-6: 재부팅 뒤 로그온 화면, 다른 Windows 사용자, 로그아웃, 다시 설치와 제거, 업데이트 뒤 로그인 유지, 아이폰의 Keychain과 복원 안내 등)을 하며 쓴 뒤, 사용자 승인을 받아 모두에게 켭니다.
   ```bash
   sudo /opt/hjviewer/bin/hjv-api admin passwordless on
   sudo /opt/hjviewer/bin/hjv-api admin report | head -6
   ```
4. **다른 사용자(13단계)**: 한 사람씩 명단에 넣고(7-15), 각자 PC에 설치하고 로그인합니다. 아이폰 사용자는 먼저 App Store Connect 사용자로 초대해 TestFlight 내부 그룹에 넣어야 합니다([distribution.md](distribution.md)의 "아이폰 사용자 추가").

### 7-10. 최소 버전 강제 (14단계)

최소 버전 작업의 문서를 따릅니다. 세 플랫폼의 1.6.0이 나가고 PC들이 1.6.0으로 올라간 뒤에만 합니다.

### 7-11. NAS 중계기 승인과 켜기 (15단계)

nginx는 9단계에서 이미 준비됐습니다(중계기 등록 한도의 zone 포함).

1. 사용자가 NAS에 hjv-relay를 설치합니다([relay-dsm.md](relay-dsm.md): File Station으로 올리고, 작업 화면에 SHA-256을 적음). NAS의 관리자 계정은 에이전트에게 주지 않습니다.
2. 사용자가 로그의 **등록 코드**와, 그 NAS로 켤 PC 하나의 ID를 알려 줍니다.
3. 승인하기 전에, 그 PC의 주인에게 이미 아는 길(대면, 전화, 알려진 메일)로 확인했는지 사용자에게 묻습니다. PC ID는 비밀이 아니므로 ID만으로는 주인을 확인할 수 없습니다.
4. 사용자 승인을 받고 실행합니다(질문 17).
   ```bash
   A="sudo /opt/hjviewer/bin/hjv-api admin"
   $A relay list
   $A relay approve KZDV-VJ2U-MNDU-YAUF --owner-of 123456789 --name '연구실 NAS'   # ← 등록 코드, PC ID, 앱에 보일 이름
   $A relay list     # 승인한 뒤: 그 계정의 PC마다 판정을 다시 봄
   ```
   - 승인 도구의 기본 출력은 대기 중인 중계기와 그 PC가 보고한 네트워크를 비교한 **판정**만 보입니다: 같은 망인지(`on the PC's network: yes (…)`), 나가는 주소가 같은지(`the same egress address as the PC: yes`), 중계기의 첫 요청과 PC의 보고가 얼마나 됐는지. 주소와 망의 원래 값은 사람이 `--reveal`로만 봅니다.
   - 판정이 같은 망(`on the PC's network: yes`)이 아니면 승인 도구가 아무것도 바꾸지 않고 스스로 멈춥니다(`nothing changed: …`). 그 PC의 네트워크에 닿지 못하는 엉뚱한 중계기일 수 있으므로 그대로 보고하고 멈춥니다. PC 주인이 코드를 확인했는데도 판정이 나오지 않을 때만(PC가 아직 켜기 정보를 보내지 않아 `unknown`일 때 등) 사용자 승인 뒤 같은 명령에 `--force`를 붙입니다. 판정이 `no (a private network of another site: the egress addresses differ)`이면 `--force`로 승인해도 그 PC를 켤 수 없으므로 붙이지 않습니다(7-1의 5번의 열린 질문).
   - 같은 망 판정은 중계기가 스스로 알린 랜카드 정보를 PC의 서명된 보고와 비교한 것이고, hjv-api가 직접 본 것은 나가는 주소뿐입니다(출력의 괄호 줄 `on the PC's network: by the relay's own report …`). 틀린 코드나 다른 NAS는 걸러 내지만, 일부러 그 PC의 네트워크를 알리는 중계기는 막지 못합니다. 승인의 근거는 3번의 주인 확인이므로, 판정이 `yes`여도 `--force`를 쓸 때도 3번을 건너뛰지 않습니다.
   - `--name`은 앱이 켜기 진행 창에 그대로 보이는 중계기 이름입니다(1~32자). "연구실 NAS"처럼 혼자서 읽히는 이름이 좋고, 정하지 않으면 `NAS`와 코드 첫 묶음(`NAS KZDV`)입니다. 이미 승인한 중계기에 같은 명령을 `--name`과 함께 다시 하면 이름만 바뀝니다.
   - 중계기가 아직 등록을 요청하지 못했으면 같은 명령으로 코드를 미리 승인할 수 있습니다(`approved ahead: …`). 7일 안에 그 키로 요청이 오면 바로 승인됩니다. 이때는 판정 없이 승인되므로, 중계기가 붙은 뒤 `relay list`의 판정이 같은 망이 아니면 폐기합니다.
5. 1분 안에 NAS 로그에 `등록이 승인되어 켜기 명령을 기다립니다.`가 찍힙니다. 사용자가 앱에서 꺼진 PC의 "켜기"로 확인합니다. 켜지지 않으면 PC 쪽 준비([wol-pc-setup.md](wol-pc-setup.md))부터 봅니다.
6. 폐기: `sudo /opt/hjviewer/bin/hjv-api admin relay revoke <중계기 ID>`(등록 코드로도 됨). 중계기는 명령을 받지 않고 1시간마다만 묻습니다. 폐기한 중계기의 키는 다시 승인되지 않습니다(7-12의 4번).

### 7-12. 되돌리기

1. **비밀번호 없는 연결만 멈출 때**: `sudo /opt/hjviewer/bin/hjv-api admin passwordless off`. 그 뒤 **hjv-api를 24시간 동안, 또는 `report`의 `PCs holding their binding that have not synced since the last switch change`가 0이 될 때까지** 켜 둡니다. 그러면 모든 PC가 PC 비밀번호로만 연결을 받습니다. 그 전에 hjv-api를 멈추거나 nginx를 되돌리면, 그동안 잠들어 있던 PC는 꺼짐을 받지 못하고 받아 둔 목록을 받은 때부터 24시간까지 씁니다.
2. **계정 기능 전체**: 먼저 1번을 하고 그 조건까지 기다립니다. 그다음 7-8의 백업(`/root/hjv-backup/hjviewer-locations.conf.before-api`)으로 옛 `location ^~ /api/` 블록을 되돌리고 `sudo nginx -t && sudo systemctl reload nginx`. zones 파일과 `snippets/hjviewer-api.conf`는 남겨도 해가 없습니다. 로그인한 앱은 회수할 수 없으므로 로그인 창과 "액세스 가능한 장치" 탭에 오류가 남습니다. hjv-api를 멈추면 PC들의 목록은 24시간 안에 만료되어 저절로 비밀번호로 돌아갑니다. hjv-api를 일부러 멈춰 둘 때는 점검 타이머와 백업 타이머를 먼저 멈춥니다(`sudo systemctl disable --now hjviewer-api-check.timer hjviewer-api-backup.timer`). 그대로 두면 점검과 백업이 실패할 때마다 경보가 납니다. 다시 켤 때 타이머도 켭니다(7-5의 마지막 블록).
3. **hbbs**: 보관한 옛 실행 파일(`/root/hjv-backup/hbbs-before-<VERSION>`)을 넣고 조용한 시간에 hbbs만 다시 시작합니다. 로그인은 되지만 새 PC 묶기와 켜짐 확인은 안 되고, 0005가 있었다면 최소 버전 거절도 멈춥니다.
4. **중계기**: `hjv-api admin relay revoke <중계기 ID>`. NAS에서는 사용자가 공유 폴더에 `hjv-relay.stop`을 두고 작업을 끕니다(재부팅 없이, [relay-dsm.md](relay-dsm.md) 10장). 폐기한 중계기의 키는 다시 승인되지 않습니다(`relay approve`가 거절). 잘못 폐기했으면 사용자가 NAS에서 중계기의 상태 폴더를 지워(같은 장의 "제거") 새 키와 새 등록 코드를 만든 뒤 7-11처럼 다시 승인받습니다.
5. **hjv-api 새 판**: 설치 전에 남긴 스냅숏과 설정 파일을 옛 실행 파일과 함께 되돌립니다(7-14의 업그레이드).

### 7-13. 사고 대응

키나 계정이 샜거나 hjv-api 서버가 장악됐을 수 있을 때입니다.

1. `passwordless off`. 7-12의 1번 규칙대로 hjv-api를 켜 둡니다.
2. 필요하면 `strict on`(연결마다 확인). 서버에 닿는 PC는 증명이 올 때마다 목록을 새로 받습니다.
3. 목록 키를 2번으로 바꿉니다(7-15). 서버에 닿는 PC는 늦어도 5분 안에 2번 목록을 받고, 그 뒤로 1번 목록을 받지 않습니다.
4. `user revoke-all --all`로 모든 계정 키와 토큰을 폐기합니다. 모두 기기마다 다시 로그인합니다. PC의 묶음은 남습니다.
5. 개인정보가 샜을 가능성이 있으면 사용자와 학교 정보보호팀에 바로 알립니다. 2026-09-11부터 시행된 개정 개인정보 보호법은 유출 가능성도 통지하게 합니다.

| 샌 것 | 할 수 있는 것 | 되돌리기 |
|---|---|---|
| 봉인 키 | 가로챈 로그인의 비밀번호 열기(TLS 가로채기도 필요) | 새 봉인 키로 앱을 다시 빌드해 나눠 주고, 자격 증명에서 옛 키를 뺌(그 뒤 옛 앱의 로그인은 실패) |
| 목록 키 | PC의 요청에 답할 수 있을 때만 거짓 목록 | 2번으로 교체 |
| 명령 키 | 중계기의 네트워크에서 성가신 켜기 | 2번으로 교체 |
| 중계기 키 | 거짓 결과, PC들의 MAC과 네트워크 정보 보기 | `relay revoke` |
| 계정 키(+토큰) | 그 계정의 묶인 PC | `key revoke <ckid>`(보통 2분, 늦어도 5분) |
| 기기 키 | 그 PC로서 동기화(목록 보기), 목록 올리기 선점 | 지금과 같은 경계 |

### 7-14. 감시와 백업

hjv-api가 멈추거나 동기화만 실패해도, 비밀번호 없는 연결은 PC가 받아 둔 목록으로 하루 동안 계속되므로 아무도 모릅니다. 24시간 뒤 모든 PC가 한꺼번에 비밀번호 창으로 돌아갑니다. 그래서 감시를 둡니다. 동작 원리와 명령은 [server/api/README.md](../server/api/README.md)의 "Monitoring"과 "Backup and restore"에 있고, 시험 서버에서 한 연습의 기록은 [hjv-api-drill.md](hjv-api-drill.md)에 있습니다.

- **감시**: `hjviewer-api.service`, 점검 유닛, 백업 유닛이 실패하면 `OnFailure=`로 경보 유닛(`hjviewer-api-alert@.service`)이 돕니다. 5분 타이머(`hjviewer-api-check.timer`)의 점검(`hjv-api check`)은 `report`의 줄이 아니라 소켓(`/run/hjviewer-api.sock`)의 `GET /api/hjv/source`와 관리 소켓의 `health`를 봅니다. 경보가 되는 것은 다음입니다.
  - 소켓이나 관리 소켓이 답하지 않음
  - 묶음을 가진 PC 가운데 켜진 PC가 있는데 그중 어느 것도 10분 안에 동기화하지 않음. 켜진 PC는 hbbs가 저장된 키로 90초 안에 등록한 것으로 본 PC이고, 지난 4분 동안의 점검마다 그렇게 보인 것만 셉니다(막 깨어난 PC가 첫 동기화를 할 시간)
  - hbbs 콘솔이 답하지 않음
  - SSO 호출 중지(지금 또는 지난 10분 안). `invalid_client`(7-15의 "SSO 자격 증명이 바뀔 때")와 주소 거절을 구분해 알립니다
  - 1시간 넘게 묻지 않은 활성 NAS 중계기. NAS를 밤에 끄거나 재우는 곳에서는 그때마다 이 경보가 나므로, 그런 일정이 있으면 사용자에게 알려 기준을 정합니다(질문 15)

  꺼진 PC는 아무것도 말하지 않으므로 모든 PC가 잠든 밤은 경보가 아닙니다. 새 키로 다시 설치되어 그 PC에서 다시 로그인하기를 기다리는 PC(`report`의 needs-relogin)도 빼고 봅니다. 결과가 있으면 줄마다 저널에 오류를 쓰고 `/run/hjviewer-api-check/findings`에 남긴 뒤 유닛이 실패합니다. 이상이 없으면 저널에 한 줄입니다.
- **경보 경로(질문 12)**: 경보 유닛은 언제나 저널(이름 공간 `hjviewer`)에 오류 한 줄을 씁니다. 텔레그램을 쓰지 않으면 이 줄이 경보이고, 운영 에이전트의 매일 보고가 그 수를 셉니다(개수만): `sudo journalctl --namespace=hjviewer -p warning --since yesterday -o cat | wc -l`과 `sudo /opt/hjviewer/bin/hjv-api admin report`. 텔레그램으로 받기로 하면 사용자가 봇을 서버에 직접 넣습니다(에이전트 없이). 경보 유닛은 이 자격 증명이 있을 때만 가져옵니다(`ImportCredential=`).
  ```bash
  { printf 'TELEGRAM_BOT_TOKEN='; systemd-ask-password -n "bot token:"; printf '\nTELEGRAM_CHAT_ID=%s\n' '<채팅 번호>'; } |
    sudo systemd-creds encrypt --name=hjviewer-api-telegram - /etc/credstore.encrypted/hjviewer-api-telegram
  sudo systemctl start hjviewer-api-alert@test.service   # 시험 메시지
  ```
  같은 유닛이 같은 방식으로 끝나고(systemd가 넘기는 `MONITOR_SERVICE_RESULT`, `MONITOR_EXIT_CODE`, `MONITOR_EXIT_STATUS`) 결과도 같으면 6시간에 한 번만 보내고, 그 밖에는 바로 보냅니다. 비정상 종료와 그 뒤의 시작 실패는 서로 다른 경보입니다. 메시지에는 유닛, 끝난 방식, 호스트 이름, 시각, 결과(개수와 상태)만 있고 사람, 기기, 주소, 비밀은 없습니다. 보내지 않으려면 그 자격 증명 파일을 지웁니다. 텔레그램을 쓰기로 정하면 [privacy.md](privacy.md) 9절에 경보 문구(위의 내용)가 텔레그램을 거친다는 한 줄을 두 언어로 더합니다.
- **hjv-api가 멈춰 있는 동안**: 원격 세션과 ID·비밀번호 연결은 그대로이고, 비밀번호 없는 연결도 PC가 받아 둔 목록으로 하루 동안 계속됩니다. 달라지는 것은 웹 서버가 hjv-api로 넘기는 계정 경로(`/api/login`, `/api/currentUser`, `/api/logout`, `/api/users`, `/api/peers`, `/api/hjv/…`)의 답입니다. 세 웹 서버 모두 그 경로에 503과 JSON `{"error":"HJViewer 서버를 지금 쓸 수 없습니다. 비밀번호 없는 연결은 PC가 받아 둔 목록으로 하루 동안 계속됩니다. 잠시 뒤 다시 시도해 주세요."}`로 답하고, 로그인한 앱은 "HJViewer 서버를 지금 쓸 수 없습니다" 안내를 보입니다. 그 밖의 `/api/`(`/api/heartbeat` 등)는 전처럼 404입니다.
  - **nginx**(7-8의 전환을 한 서버): `/api/login-options`와 `/api/device-group/accessible`은 nginx가 직접 답하므로 그대로입니다. 비정상 종료는 systemd가 5초 뒤 스스로 다시 시작하고 경보가 갑니다. 그래도 503이 이어지면 다음을 보고 사용자에게 알립니다.
    ```bash
    systemctl is-active hjviewer-api.socket hjviewer-api.service
    sudo journalctl --namespace=hjviewer -u hjviewer-api -p warning --since -1h -o cat | wc -l   # 개수만
    ```
    멈춰 있으면(`inactive`, `failed`) 사용자 승인 뒤 `sudo systemctl start hjviewer-api.socket hjviewer-api.service`를 하고 `sudo /opt/hjviewer/bin/hjv-api admin selfcheck`로 확인합니다. 시작하지 않으면 7-5의 마지막 항목처럼 오류 줄을 사용자에게만 보여 주고 멈춥니다. 하루 안에 되살리지 못하면 모든 PC가 비밀번호 창으로 돌아간다는 것도 알립니다.
  - **Caddy(A 경로)와 Apache**: 1.6.0의 `deploy/Caddyfile`과 `deploy/apache-hjviewer.conf`도 같은 경로를 hjv-api의 소켓 `/run/hjviewer-api.sock`으로 넘기고, 닿지 않으면 위의 503 JSON으로 답합니다(1.5.1까지의 파일은 404). 이 런북에는 그 두 웹 서버 뒤에 hjv-api를 두는 절차가 없으므로, 이 런북대로 설치한 A 경로 서버와 Apache 서버에서 이 503은 hjv-api가 없는 서버의 정상 응답입니다.
    - 고칠 것이 없습니다. `Caddyfile`이나 Apache 사이트 파일을 고쳐 404로 되돌리지 않습니다. 5절의 확인(`/ws/id`·`/ws/relay` 101, `/api/heartbeat` 404)이 그대로이면 됩니다.
    - 이런 서버에 맞는 앱은 계정 공개키(`HJV_LOGIN_SEAL_PUBKEY`, `HJV_TRUST_PUBKEYS`)를 넣지 않은 빌드입니다. 그런 앱에는 로그인이 없어 계정 경로를 부르지 않습니다([architecture.md](architecture.md) 5-9). 계정 공개키를 넣은 앱을 이런 서버에 붙이면 로그인할 때 위 문구가 나옵니다. 워크플로를 직접 실행한 빌드는 공개키가 없어도 되지만, 비공개 빌드 저장소를 거친 1.6.0 출시 빌드는 계정 공개키가 있어야 통과합니다([ci.md](ci.md)의 "계정 기능의 공개키").
    - Caddy는 `/api/login-options`와 `/api/device-group/accessible`에 직접 답하고, Apache는 이 둘도 hjv-api로 넘기므로 함께 503입니다. nginx와 달리 두 웹 서버에는 로그인과 중계기 등록의 주소별 한도가 없고 hjv-api 자신의 한도만 있습니다.
    - 이 런북 밖에서 hjv-api를 직접 붙여 둔 서버라면 nginx와 같은 순서(상태, 시작, `selfcheck`)로 봅니다. 소켓 유닛의 그룹이 그 웹 서버의 작업 프로세스 그룹이어야 합니다(7-3의 드롭인).
- **백업**: 매일 타이머(`hjviewer-api-backup.timer`)가 `hjv-api admin backup --keep-days 14 /var/lib/hjviewer-api/backup`(SQLite 온라인 백업)으로 그 폴더에 root만 읽는 0600 파일(`hjv-api-<UTC 시각>.sqlite3`)을 만들고 14일이 지난 백업을 지웁니다. 아래의 복원 전 사본과 업그레이드 스냅숏도 이 폴더에 두므로 14일 뒤 함께 지워집니다. 디스크 손상이나 잘못된 업그레이드에서 되돌리는 용도입니다. 자격 증명, `/var/lib/systemd/credential.secret`, 운영자 기록은 넣지 않습니다. 서버 밖 사본은 1.6.0에서 만들지 않습니다(질문 8). 디스크를 통째로 잃으면 사용자 약 10명이 기기마다 한 번 다시 로그인합니다.
- **복원**: 백업은 그 뒤에 폐기한 키와 토큰, 스위치 변경, 묶음 해제, 중계기 폐기를 되돌리므로, 복원은 그 자체로 사고 대응입니다. 복원한 백업은 비밀번호 없는 연결이 모든 계정에서 꺼진 채로 시작합니다. hjv-api가 백업 파일을 처음 열 때 첫 응답 전에 끄고 저널에 남깁니다. PC들은 2분마다 동기화하므로, 그렇지 않으면 시작과 다음 명령 사이에 백업 때의 스위치와 그 뒤 폐기한 키가 담긴 서명된 목록을 받을 수 있습니다. DB 파일을 바꾸는 일은 7-0의 "하지 말 것"의 예외이므로, 사용자가 직접 하거나 사용자가 이 복원을 승인한 뒤에만 합니다. 파일 내용은 출력하지 않습니다.
  ```bash
  B='/var/lib/hjviewer-api/backup/<백업 파일>'   # ← 사용자가 고른 백업
  A="sudo /opt/hjviewer/bin/hjv-api admin"
  D=/var/lib/hjviewer-api/hjv-api.sqlite3
  sudo test -s "$B" || { echo 'STOP: 백업 파일이 없음'; exit 1; }
  $A passwordless off
  sudo systemctl stop hjviewer-api-check.timer   # 서비스를 멈춘 동안 점검이 경보를 내지 않게
  # 지금 DB를 백업 폴더에 남김(다른 백업과 함께 14일 뒤 지워짐)
  $A backup "/var/lib/hjviewer-api/backup/before-restore-$(date -u +%Y%m%d%H%M).sqlite3" || { echo 'STOP: 지금 DB를 백업하지 못함'; exit 1; }
  sudo systemctl stop hjviewer-api.socket hjviewer-api.service   # 소켓도 멈춤: 그대로 두면 요청이 서비스를 다시 띄움
  sudo rm -f "$D" "$D-wal" "$D-shm"
  sudo install -o hjviewer-api -g hjviewer-api -m 600 "$B" "$D"
  sudo systemctl start hjviewer-api.socket hjviewer-api.service   # 모든 계정이 꺼진 채로 시작
  $A passwordless off             # 확인: "(no change)"여야 함
  $A user revoke-all --all        # 모든 계정 키와 토큰을 폐기
  $A passwordless status; $A allow list | head -1; $A report | head -5
  $A relay list                   # 백업 뒤에 폐기한 중계기가 다시 활성이면 relay revoke
  ```
  - 두 번째 `passwordless off`가 `(no change)`가 아니면(복원한 DB가 꺼진 채로 시작하지 않았다는 뜻) 그대로 보고합니다.
  - `STOP: 지금 DB를 백업하지 못함`이면 사용자에게 보고하고 점검 타이머를 다시 켭니다(`sudo systemctl start hjviewer-api-check.timer`). DB가 망가져 백업이 되지 않는 경우에만, 사용자 승인 뒤 `rm -f` 대신 그 파일들을 `/root/hjv-backup/before-restore-<시각>/`(700)로 옮기고, 그 사본은 14일 안에 지웁니다(개인정보 처리방침의 백업 14일).
  - 중계기도 백업 때의 상태로 돌아옵니다. 백업 뒤에 폐기한 중계기는 다시 활성이 되고, hjv-api는 복원한 백업에 활성 중계기가 있으면 시작할 때 저널에 그 수를 적습니다. 무엇을 폐기했었는지는 운영자 기록에만 있습니다: `sudo grep -a 'cmd="relay revoke" targets=' /var/log/hjviewer-api/operator.log | awk '{print $1, $6}'`(시각과 중계기 ID). 백업 파일 이름의 시각(UTC)보다 뒤에 폐기한 것을 사용자 승인 뒤 다시 `relay revoke`합니다. 되살아난 중계기가 받는 것은 그 계정의 PC를 주인이 켤 때의 켜기 명령뿐이고, PC에 연결할 권한은 없습니다.

  명단과 계정별 스위치를 확인하고 사용자 승인을 받아 `passwordless on`과 `sudo systemctl start hjviewer-api-check.timer`. 모두 휴대폰과 PC에서 한 번 다시 로그인합니다. PC의 묶음은 남으므로, 그 PC로 들어오는 연결을 위해 PC에서 다시 로그인할 필요는 없습니다. 백업 뒤에 풀렸던 묶음이 서버에 되살아나도, 허용 표시가 이미 지워진 PC는 증명을 받지 않습니다(`listed`).
- **업그레이드**: hjv-api의 저장 이전은 한 방향입니다(옛 실행 파일은 새 스키마를 거부). 새 판을 설치하기 전에 `sudo /opt/hjviewer/bin/hjv-api admin backup /var/lib/hjviewer-api/backup/before-<VERSION>.sqlite3`로 스냅숏을 남기고, 옛 실행 파일과 설정 파일도 `/root/hjv-backup/`에 복사합니다(`sudo cp -p /opt/hjviewer/bin/hjv-api /root/hjv-backup/hjv-api-before-<VERSION>`, 설정 파일은 `hjviewer-api.env-before-<VERSION>`). 되돌릴 때는 옛 실행 파일, 스냅숏, 설정 파일을 함께 되돌립니다. 옛 실행 파일과 설정 파일을 제자리에 넣고, 위의 복원 블록을 `B`=스냅숏으로 합니다. 새 판이 돌지 않아 `backup` 줄이 실패하면, 사용자 승인 뒤 그 줄 대신 지금 DB 파일들을 `/root/hjv-backup/before-restore-<시각>/`(700)로 옮기고 14일 안에 지웁니다. 스냅숏도 복원한 백업이므로 모든 계정이 꺼진 채로 시작하고, 확인과 `user revoke-all --all` 뒤 사용자 승인을 받아 켭니다. 스냅숏은 다른 백업처럼 14일 뒤 지워지므로 되돌리기는 그 안에 정합니다.
- **호스트 재구축**: 봉인 키는 오프라인 사본에서 되살립니다(7-4의 (가) 3번). 목록 키 1번과 명령 키 1번은 서버에만 있었으므로 오프라인의 2번을 넣고 `HJV_API_LIST_KID=2`, `HJV_API_COMMAND_KID=2`로 바꿉니다(7-15). 앱과 hjv-relay 빌드가 1번과 2번을 함께 고정하므로 앱과 NAS는 건드리지 않습니다. 기록 키는 새로 만듭니다. 백업이 남아 있으면 위의 복원처럼 넣어 꺼진 채로 시작하고, `user revoke-all --all` 뒤 사용자 승인을 받아 켭니다. 백업도 없으면 빈 DB로 시작하므로 명단을 다시 넣고(7-5), 모두 PC에서도 다시 로그인하며, 중계기는 다시 등록을 요청해 다시 승인받습니다(7-11). 이 절차들은 시험 서버에서 연습했습니다([hjv-api-drill.md](hjv-api-drill.md)).

### 7-15. 일상 운영

관리 명령은 `sudo /opt/hjviewer/bin/hjv-api admin <명령>`이고, 출력은 가려져 있습니다(포탈 ID는 앞 4자, 이름·기기 이름·주소는 없음, 대신 꼬리표). 모든 명령은 운영자 기록에 남습니다(5분 점검이 읽는 `health`만 예외: 개수만 돌려주고 아무것도 바꾸지 않음). 전체 목록은 `hjv-api admin help`와 [server/api/README.md](../server/api/README.md)의 "Admin"에 있습니다.

- **명단**: 넣기 `allow import <파일>`, 빼기 `allow remove <파일>`(사용자가 쓴 파일, 7-5처럼). 빼면 그 계정의 토큰과 계정 키가 바로 폐기됩니다. 그 계정에 묶인 PC는 남고 키 없는 목록을 받습니다. 그 PC를 다른 사람에게 넘길 때는 아래처럼 풉니다.
- **PC를 계정에서 풀기**: `binding list`로 보고 `binding revoke <PC ID>`로 풉니다. `device remove <PC ID>`는 같은 명령의 옛 이름입니다. 어느 이름이든 그 PC의 묶음 ID를 180일 동안 무덤으로 남기므로, 그 PC는 다음 동기화에서 서명된 `revoked`를 받아 허용 표시를 지웁니다. 그 뒤 그 PC의 빈 보고에는 서명 없는 `unbound`가 갑니다.
- **PC를 넘기거나 버릴 때**(사용자 안내): 그 PC에서 **온라인일 때** 로그아웃한 뒤 넘기거나 제거합니다. 제거만 해도 그 PC의 허용 표시는 지워지지만 서버의 묶음은 남아, 이전 주인의 카드에 "마지막 확인 n일 전"이 보이고 새 사용자의 로그인은 "다른 계정에 등록된 PC"가 됩니다. 그때는 이전 주인이 다른 기기에서 "내 목록에서 빼기"를 하거나 관리자가 `binding revoke <PC ID>`를 합니다.
- **PC의 기기 키가 바뀌었을 때**: 설정을 잃은 PC(설정까지 지우고 다시 설치 등)는 같은 ID로 새 키를 등록합니다. 서버는 그 PC의 첫 동기화에서 hbbs에 키를 묻고, 진짜 hbbs가 새 키를 가졌으면 그 PC를 `needs-relogin`으로 둡니다(`report`의 `PCs that need a login on them`). 목록을 새로 고친 컨트롤러는 그 PC에 비밀번호 창을 띄웁니다. **그 PC에서 같은 계정으로 다시 로그인**하면 새 키가 저장되고 묶음 ID는 그대로라 비밀번호 없는 연결이 돌아옵니다. PC가 새 ID를 받았으면 새 ID는 그 PC에서 다시 로그인해야 목록에 오르고, 옛 ID의 카드는 주인이 빼거나 180일 정리가 지웁니다.
- **계정 키**: `key list [<포탈 ID|tag:T>]`(키마다 PC별 마지막 비밀번호 없는 연결과 30일 횟수), `key revoke <ckid>`(잃어버린 기기를 사용자가 직접 로그아웃시킬 수 없을 때), `user revoke-all <포탈 ID|tag:T>`.
- **계정 삭제 요청**: 명단에서 빼고(`allow remove <파일>`, 사용자가 쓴 파일) `user delete <포탈 ID|tag:T>`. 명단은 `user delete`로 지워지지 않습니다. `user delete`는 그 계정의 PC를 먼저 풀고(무덤) 그 계정의 중계기를 폐기한 뒤, 계정과 그 키·토큰·사용 기록·켜기 기록을 지웁니다. 남는 것은 묶음 무덤(180일), 폐기된 중계기의 ID·키·시각, 꼬리표로 남은 로그인·기기·키 기록(6개월), 운영자 기록이고, 백업은 14일 뒤 사라진다고 요청한 사람에게 알립니다.
- **SSO 자격 증명이 바뀔 때**: hjv-api는 같은 서버의 WordPress SSO 로그인 플러그인(projecttf-auth)과 같은 SSO 등록을 씁니다. 그 플러그인의 client secret이나 api uuid가 바뀌면 같은 날 사용자가 7-4의 (나)처럼 `hjviewer-sso-client-secret`(또는 `hjviewer-sso-api-uuid`)를 다시 만들고 `sudo systemctl restart hjviewer-api`를 합니다. 그대로 두면 SSO가 `invalid_client`로 거절해 모든 로그인이 "연세 인증 서버에 지금 연결할 수 없습니다"가 되고, 감시가 경보를 냅니다.
- **목록 키 바꾸기**(목록 키 1번이 샜거나 잃었을 때):
  1. 사용자가 오프라인 매체의 목록 키 2번(`hjviewer-list-2.key`)을 7-4의 (가) 3번처럼 표준 입력으로 넣습니다. 자격 증명 이름은 그대로 `hjviewer-api-list-key`이고(명령의 `--name=`과 파일 이름을 그것으로 바꿈), 그 안의 키가 2번으로 바뀝니다.
  2. 설정 파일에 `HJV_API_LIST_KID=2`를 넣고 `sudo systemctl restart hjviewer-api`, `admin selfcheck`로 확인합니다(`HJV_API_TRUST_PUBKEYS`는 그대로 `1:…,2:…`).
  3. 모든 1.6.x 앱은 이미 2번을 믿습니다. PC는 2번 목록을 한 번 받으면 1번 목록을 다시 받지 않고, 서버에 닿는 PC는 늦어도 5분 안에 받습니다.
  4. 다음 앱 빌드는 2번과 새 3번을 고정하고, 그 빌드가 나가면 서버의 `HJV_API_TRUST_PUBKEYS`도 `2:…,3:…`로 바꿉니다. `report`의 `PCs by app version`으로 아직 옛 빌드인 PC를 봅니다.
- **명령 키 바꾸기**(명령 키 1번이 샜거나 잃었을 때): 같은 방법으로 `hjviewer-command-2.key`를 `hjviewer-api-command-key`에 넣고 `HJV_API_COMMAND_KID=2`, 다시 시작한 뒤 `admin selfcheck`의 명령 키 두 줄을 봅니다([relay/PROTOCOL.md](../relay/PROTOCOL.md) 3.2). hjv-relay 빌드가 1번과 2번을 함께 고정하므로 NAS는 건드리지 않고, 승인한 중계기는 다시 승인받지 않습니다. 다음 hjv-relay 빌드가 2번과 새 3번을 고정합니다.
- **봉인 키 바꾸기**: 자격 증명 `hjviewer-api-login-key`는 여러 줄을 받고 첫 줄이 지금 키입니다. 사용자가 새 키 파일과 옛 키 파일(오프라인 매체)을 이 순서로 이어(`cat 새 옛`) 7-4의 (가) 3번처럼 넣고 `sudo systemctl restart hjviewer-api`를 합니다. 새 공개키로 빌드한 앱이 모두 나간 뒤 새 키만 다시 넣습니다.

### 7-16. 보고 (7절)

아래 형식으로 알려 주세요. 포탈 ID, 성명, IP 주소, 기기 이름은 적지 않습니다.

```
단계: <끝낸 단계 번호>
hjv-api: <hjv-api version의 판과 커밋>, 소켓 <권한 소유자>, DB 600(예/아니오), operator.log 추가 전용(예/아니오)
selfcheck: [ OK ] <n>줄, [FAIL] <n>줄(<어느 항목>), 고정 목록 키 지문 <1:…, 2:…>, 고정 명령 키 지문 <1:…, 2:…>(공개값)
스위치: passwordless <off | 운영자 계정만 | 모두>, strict <off | on>
명단: <개수>
nginx: /api/hjv/source <코드>, /api/login-options <코드와 본문>, /api/heartbeat <코드>, /api/x.php <코드>, 다른 사이트 응답 코드 변화 없음(예/아니오), /ws/id·/ws/relay 101(예/아니오)
report: 계정 <n>, 활성 키 <n>, 묶음을 가진 PC <n>, 그중 10분 안에 동기화 <n>, 스위치 뒤 아직 동기화하지 않은 PC <n>, needs-relogin <n>
감시·백업: 타이머 활성(예/아니오), 마지막 백업 <sudo ls -t /var/lib/hjviewer-api/backup | head -1 의 파일 이름에 있는 UTC 시각>, 경보 경로 <텔레그램 | 저널>
중계기: <사용 중 n, 대기 n, 폐기 n, 미리 승인한 코드 n>(relay list 첫 줄)
바꾼 것: <설치한 파일, 만든 사용자, 고친 설정 파일, 드롭인>
문제: <있으면>
```
