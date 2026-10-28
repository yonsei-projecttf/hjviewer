# 서버 배포 런북 (서버에서 작업할 에이전트·운영자용)

Ubuntu 22.04/24.04 서버 한 대에 HJViewer 서버(hbbs·hbbr)를 띄우고 HTTPS(443)로 연결되게 하는 절차입니다. 경로는 두 가지입니다.

- **A. 80/443이 비어 있는 서버**: Docker로 Caddy, hbbs, hbbr을 띄웁니다. 인증서는 Caddy가 받습니다.
- **B. 이미 nginx나 Apache가 80/443을 쓰는 서버**: Docker 없이 GitHub Release의 hbbs/hbbr 바이너리를 systemd 서비스로 127.0.0.1에만 띄웁니다. 기존 웹 서버는 `/ws/id`, `/ws/relay`, `/api/`만 넘겨 받습니다.

위에서부터 순서대로 실행하고, 마지막에 **보고** 형식으로 결과를 알려 주세요. 배경 설명은 [deploy.md](deploy.md)에 있습니다. 다만 그 문서의 명령은 실행하지 말고, 이 런북의 명령만 실행하세요.

1.6.0의 계정 API(연세포탈 로그인, 내 기기 목록, 같은 계정 기기끼리 비밀번호 없는 연결, 원격 켜기)는 B 경로를 마친 서버에 [7절](#7-계정-api-추가-160)의 순서로 더합니다. 7절에는 그 절만의 규칙과 보고 형식이 있습니다. 최소 버전과 업데이트 배포(서명된 버전 정책, 설치 파일, hbbs의 최소 컨트롤러 버전, 문 2)는 [8절](#8-최소-버전과-업데이트-배포-160), 1.8.0의 교직원 전용 출입 제한(출입증 키, hbbs의 `access.conf`, 이용 비밀번호, 출시일 순서)은 [9절](#9-교직원-전용-출입-제한-180)입니다.

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
for u in https://github.com https://release-assets.githubusercontent.com https://objects.githubusercontent.com https://index.crates.io/config.json https://static.crates.io https://deb.debian.org/debian/ https://public.ecr.aws/v2/ https://registry-1.docker.io/v2/ https://acme-v02.api.letsencrypt.org/directory; do echo "$(curl -s -o /dev/null -w '%{http_code}' --max-time 10 "$u") $u"; done
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
   - B 경로: github.com과 release-assets.githubusercontent.com이 필요합니다(Release 내려받기. github.com이 파일을 그 주소로 넘김, 예전 주소는 objects.githubusercontent.com). github.com이 000이거나, release-assets와 objects가 둘 다 000이면 멈춥니다.
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
     [ -f bin/hjv-update-verify.sh ] && sudo install -m 755 bin/hjv-update-verify.sh /opt/hjviewer/bin/   # 8절의 서버 확인 스크립트
     sudo install -m 644 systemd/hjviewer-hbbs.service systemd/hjviewer-hbbr.service /etc/systemd/system/
     sudo install -m 644 web/* /opt/hjviewer/share/web/
     sudo install -m 644 LICENSE.txt NOTICE.txt SOURCE.txt THIRD_PARTY_NOTICES.txt /opt/hjviewer/share/
     sudo systemctl daemon-reload
     sudo systemctl restart hjviewer-hbbs hjviewer-hbbr
     ```
  3. 재시작하면 PC는 몇 초 안에 다시 등록되지만, 진행 중인 원격 세션은 끊깁니다. `/etc/hjviewer/min-controller.conf`가 있는 서버(8-3)에서는 12초 뒤 hbbs의 `min-controller` 줄을 봅니다. `min-controller <MIN> from …`(또는 `min-controller off: …`)여야 하고, ERROR(`… no minimum is in force`)면 멈추고 보고합니다.
     ```bash
     if sudo test -e /etc/hjviewer/min-controller.conf; then sleep 12; sudo journalctl -q -u hjviewer-hbbs --since -1min -o cat | grep min-controller | grep -v 'min-controller refused' | tail -2; fi
     ```
     출입 확인을 넣은 서버(`hjviewer.env`에 `HJV_ACCESS_FILE` 줄이 있음, 9절)에서는 같은 때 9-7의 "hbbs를 다시 시작한 뒤" 블록으로 `access` 줄도 봅니다.
  4. 1.6.0부터 묶음에는 hjv-api도 들어 있습니다. hjv-api가 설치된 서버에서는 위 명령에 더해, 7-14의 "업그레이드"대로 스냅숏, 설정 파일, 옛 실행 파일을 남긴 뒤 같은 디렉터리에서 다음을 실행합니다. 유닛 파일을 바꾼 뒤 `daemon-reload`를 하지 않으면 systemd는 옛 유닛으로 다시 시작합니다.
     ```bash
     [ -x bin/hjv-api ] || { echo 'STOP: 묶음에 hjv-api가 없음'; exit 1; }
     for c in $(sed -n 's/^LoadCredentialEncrypted=//p' systemd/hjviewer-api.service); do sudo test -s "/etc/credstore.encrypted/$c" || { echo "STOP: 새 유닛이 요구하는 자격 증명 $c 이(가) 없음"; exit 1; }; done
     diff -q systemd/hjviewer-api.socket /etc/systemd/system/hjviewer-api.socket
     sudo install -m 755 bin/hjv-api /opt/hjviewer/bin/
     for u in systemd/hjviewer-api*.socket systemd/hjviewer-api*.service systemd/hjviewer-api*.timer; do [ -e "$u" ] && sudo install -m 644 "$u" /etc/systemd/system/; done
     sudo systemctl daemon-reload
     sudo systemctl restart hjviewer-api
     ```
     `diff` 줄이 `differ`를 보였으면(소켓 유닛이 바뀜) 마지막 줄 대신 `sudo systemctl stop hjviewer-api.service hjviewer-api.socket && sudo systemctl start hjviewer-api.socket hjviewer-api.service`를 합니다. 서비스만 다시 시작하면 systemd가 쥐고 있던 옛 소켓을 그대로 넘겨받습니다.

     `STOP: 새 유닛이 요구하는 자격 증명 …`이면 아무것도 설치하지 않은 채 멈추고 사용자에게 알립니다. 그 자격 증명 없이 새 유닛을 넣으면 hjv-api가 시작하지 않아 로그인, PC 동기화, 문 1, 켜기가 모두 멈춥니다. 교직원 전용 출입증([access-tickets.md](access-tickets.md))이 든 묶음부터 유닛이 출입증 키(`hjviewer-api-ticket-key`)를 요구합니다. 그때는 사용자가 7-4 (나)의 출입증 키 줄을, `/opt/hjviewer/bin/hjv-api` 자리에 이 묶음의 실행 파일 경로(운영 에이전트가 `echo "$PWD/bin/hjv-api"`로 알려 줌)를 넣어 합니다. 설치된 옛 판은 `keygen ticket`을 모릅니다. 사용자가 화면의 첫 줄(`1:<base64>`)을 알려 주면 운영 에이전트는 이 블록을 처음부터 다시 하고, 이어서 그 값을 설정 파일에 넣어 다시 시작한 뒤 `selfcheck`의 출입증 키 두 줄을 봅니다(7-5와 같은 줄). 옛 실행 파일은 이 이름을 몰라 시작하지 않으므로, 이 줄은 새 실행 파일을 넣은 뒤에만 넣습니다.
     ```bash
     TICKET='1:<base64>'    # ← 사용자가 알려 준 출입증 키의 공개키
     E=/etc/hjviewer/hjviewer-api.env
     if sudo grep -q '^#\?HJV_API_TICKET_PUBKEYS=' "$E"; then sudo sed -i "s|^#\?HJV_API_TICKET_PUBKEYS=.*|HJV_API_TICKET_PUBKEYS=$TICKET|" "$E"; else echo "HJV_API_TICKET_PUBKEYS=$TICKET" | sudo tee -a "$E" > /dev/null; fi
     sudo grep -c '^HJV_API_TICKET_PUBKEYS=.' "$E"    # 1
     sudo systemctl restart hjviewer-api
     sudo /opt/hjviewer/bin/hjv-api admin selfcheck | grep 'ticket key'
     ```
     두 줄 모두 `[ OK ]`이고 지문이 사용자가 키를 만들 때 본 것과 같아야 합니다. hbbs의 `access.conf`(서버 패치 0006)의 `KEY=` 값도 이 값입니다([access-tickets.md](access-tickets.md) 6-1).

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
- 아래 표의 순서대로 합니다. 소절 번호는 표의 단계를 따릅니다. 서버에서 하는 것은 3~7단계, 9단계, 12단계, 14단계(8절), 15단계의 승인 명령입니다.

| 단계 | 할 일 | 누가 | 소절 |
|---|---|---|---|
| 0 | 원격 켜기 현장 시험(NAS의 DSM 작업. 10단계의 배포 전까지. 출시 태그는 사용자 결정으로 2026-10-14에 먼저 붙임), 남은 결정과 질문의 답 | 사용자 | 7-1 |
| 1 | 개발 PC에서 모든 작업을 합치고 `scripts/ci.sh`의 모든 단계(`leak` 포함) 통과, 문서 갱신 | 개발 PC | 7-1 |
| 2 | push(출시 태그 없이), 확인 빌드, 서버 묶음 `server-<VERSION>` | 개발 PC | 7-1 |
| 3 | 읽기 점검 | 운영 에이전트 | 7-2 |
| 4 | hjv-api 설치. 비밀번호 없는 연결 스위치는 꺼진 채로, nginx는 그대로 | 운영 에이전트(사용자 승인) | 7-3 |
| 5 | 키와 자격 증명 만들기 | **사용자만**(운영 에이전트는 멈춤) | 7-4 |
| 6 | 공개키 대조값, 시작, selfcheck, 로그인 방식 | 운영 에이전트 | 7-5 |
| 7 | hbbs 0004로 교체 | 운영 에이전트, 조용한 시간 | 7-6 |
| 8 | 앱과 중계기 빌드, 지문 대조, 출시 태그 | 개발 PC, 사용자 확인 | 7-7 |
| 9 | nginx 전환 | 운영 에이전트(사용자 승인), 조용한 저녁 | 7-8 |
| 10~13 | 배포, 운영자 PC부터 켜기, 다른 사용자 | 개발 PC·사용자·운영 에이전트 | 7-9 |
| 14 | 업데이트 공개키 파일(8-1의 3번), 최소 버전 강제 | 사용자 승인 | 7-10 |
| 15 | NAS 중계기 승인과 켜기 | 사용자, 운영 에이전트(승인 명령) | 7-11 |

그 뒤의 소절은 되돌리기(7-12), 사고 대응(7-13), 감시와 백업(7-14), 일상 운영(7-15), 보고(7-16), App Review 데모 계정(7-17)입니다.

### 7-0. 이 절의 규칙

- 운영 서버 작업은 운영 에이전트(서버에서 작업하는 AI 에이전트)가 **사용자 승인을 받은 뒤** 합니다. 비밀값(SSO 자격 증명, 키)은 사용자만 다룹니다.
- 서비스를 다시 시작하거나 nginx를 바꾸는 단계(7, 9)는 조용한 저녁에 합니다. cron이 nginx를 자동으로 다시 읽는 시각(매일 20:00 등)은 피합니다. 같은 서버의 다른 서비스 일정과 겹치는지는 사용자가 정합니다.
- 최소 버전 작업이 함께 나가면 그 작업의 단계를 같은 번호에 넣습니다. hbbs 재시작, nginx reload, 빌드 변수 설정은 각각 **한 번**만 합니다.
- **운영 에이전트의 권한(결정 O1)**: 지금처럼 둡니다. 작업 동안 비밀번호 없는 sudo를 받거나(2단계 1번), 사용자가 sudo 명령을 직접 실행합니다. 1.6.0부터 이 서버의 목록 키로 서명한 목록은 그 계정에 묶인 모든 PC를 비밀번호 없이 엽니다. root를 가진 동안 이것을 막는 것은 아래 규칙뿐이므로 반드시 지킵니다.
- **사람에 관한 출력과 일**([privacy.md](privacy.md) 9절의 AI 도우미): 운영 에이전트가 본 출력은 그 에이전트를 실행하는 외부 AI 서비스(미국의 Anthropic)로 갑니다. 그래서 처리방침 9절은 운영 에이전트가 개수와, 사람이나 사람의 기기를 가리키지 않는 출력만 보고, 사람이나 사람의 기기에 관한 일은 사람만 한다고 밝힙니다. 아래 "하지 말 것"의 첫 두 항목이 그 규칙이고, 이 런북의 다른 절차(8절과 9절도)보다 앞섭니다. 다른 곳이 그런 일을 '사용자 승인 뒤' 한다고 적었어도(7-9, 7-11, 7-13, 7-14의 복원 뒤, 7-15, 9-4, 9-7, 9-9 등) 사용자가 서버에서 직접 하고, 운영 에이전트는 그 앞뒤의 개수만 봅니다. 예외는 사람이 아닌 App Review 계정(7-17)뿐입니다. 운영 에이전트가 7-17대로 그 꼬리표(`T`)의 줄을 보고, 그 꼬리표로 명령합니다(`unlock`, `user delete`, `user revoke-all`, 그 ID를 파일에서 바로 넘기는 `allow import`). 그 계정의 데모 PC는 ID와 꼬리표 없이 상태와 개수(묶음의 상태, `known`/`unknown`, `online`/`offline`)만 봅니다.
- **하지 말 것** (앞의 "하지 말 것"에 더해):
  - 사람이나 사람의 기기를 가리키는 값(포탈 ID와 가린 ID, 성명, 꼬리표 `acct=`·`dev=`·`ip=`·`tag:`, PC ID, 기기 이름, 주소)이 나올 수 있는 출력을 보는 일. 사람별 관리 명령(`users`, `devices`, `allow list`, `binding list`, `key list`, `relay list`, `operators`, 계정별 줄이 있는 `passwordless status`)의 출력과 그런 값이 든 저널 줄(hjv-api의 `login:`·`update:`·`guest:`·`support code:` 줄 대부분, hbbs의 `access refused`·`access closed reg`·`min-controller refused` 줄)이 그렇습니다. 그런 명령과 저널은 `grep -c`, 첫 줄이 개수인 명령의 첫 줄(`| head -1`. `passwordless status`는 계정별 줄 앞의 두 줄, `| head -2`), 꼬리표를 지우는 `sed`(9-9)처럼 개수나 사람을 가리키지 않는 글만 남기는 꼴로만 돌립니다. `report`, `health`, `selfcheck`, `ticket status`는 개수와 상태뿐이라 그대로 봅니다.
  - 사람이나 사람의 기기를 가리키는 명령: `block`, `unblock`, `allow remove`, 사람의 ID를 넣는 `allow import`, `user delete`, `user revoke-all tag:`, `key revoke`, `binding revoke`, `relay revoke`, 사람 계정이나 주소의 `unlock`, `--owner-of <PC ID>`를 쓰는 명령(`passwordless`, `relay approve`). 사용자가 합니다.
  - 자격 증명(`/etc/credstore.encrypted/*`, `/var/lib/systemd/credential.secret`)을 출력·복호화·복사하는 일. `systemd-creds decrypt`, 서비스의 `$CREDENTIALS_DIRECTORY`나 `/proc/*/environ` 읽기.
  - hjv-api의 DB(`/var/lib/hjviewer-api/`)와 그 백업을 직접 여는 일(`sqlite3`, python3의 sqlite3 모듈, `cat`, 복사). DB는 관리 명령(`hjv-api admin`)으로만 다룹니다.
  - `hjv-api keygen`과 `hjv-api admin --reveal`. 키 만들기와 원래 값 보기는 사람만 합니다.
  - 포탈 ID, 성명, IP 주소, 기기 이름을 보고나 대화에 옮기는 일. 명단 파일은 열어 보지 않습니다(사람의 ID를 넣는 가져오기와 PC를 가리키는 명령은 위처럼 사용자가 함).
  - 저널 원문을 대화에 붙이는 일. 경고·오류는 줄 수만 보고합니다.
  - 같은 서버의 다른 사이트(WordPress 등)의 DB나 설정에서 비밀을 찾는 일.
  - 실제 계정으로 로그인해 보는 일. 로그인 확인은 사용자가 앱으로 합니다.
  - `passwordless off` 뒤 24시간이 지나기 전에 hjv-api를 멈추거나 nginx를 되돌리는 일(7-12의 규칙).

### 7-1. 시작하기 전 (0~2단계, 사용자와 개발 PC)

서버에서 하는 일이 아닙니다. 3단계를 시작하기 전에 1번의 결정·질문의 답과 2~4번이 끝났는지 사용자에게 확인합니다. 1번의 현장 시험과 5번은 push 뒤에 해도 되지만, 5번에 적은 때까지는 끝나야 합니다. 6번은 2026-10-14 통합의 점검표이고, 줄마다 적은 사람과 때를 따릅니다.

1. 원격 켜기의 현장 시험(NAS의 DSM 작업으로 PC를 깨워 봄, [relay-dsm.md](relay-dsm.md)의 현장 시험 장과 [부록](relay-dsm.md#부록-현장-시험-보충))과 남은 결정·질문의 답. 현장 시험은 깨울 PC인 개발 PC가 작업을 쉬는 동안에만 할 수 있어서, push와 출시 태그를 막지 않고 배포(10단계, 7-9)를 막습니다(5번). 현장 시험의 hjv-relay는 빌드 저장소의 Release 'hjv-relay 1.6.0'(태그 `relay-1.6.0`, 8622975에서 빌드, 7-11)의 묶음입니다. 묶음 없이 하면 부록의 DSM 내장 명령을 씁니다. 1.6.1 실행의 중계기 아티팩트는 쓰지 않습니다(7-7).
2. 개발 PC: 모든 작업 묶음을 합친 커밋에서 `scripts/ci.sh server`, `client`(Rust 1.75 check 포함), `deploy`, `e2e`, `wol`, `leak`이 통과하고, privacy.md, architecture.md, 이 런북, README의 "현재 상태" 표가 그 커밋의 내용으로 고쳐져 있음([ci.md](ci.md)).
   - Windows·iOS 전용 코드를 작업 브랜치로 미리 빌드해 보지 않습니다(결정 O2). 그 코드는 4번의 push 바로 뒤에 돌리는 **확인 빌드**(`check_only=true`, [ci.md](ci.md)의 "실행")에서 처음 컴파일됩니다. 확인 빌드는 공개키 없이 돌고, 산출물을 나눠 주지 않으며, 태그를 붙이지 않습니다. 실패하면 고친 뒤 한 번 더 push합니다. 구현이 모두 끝나면 바로 앱 빌드를 시작한다는 사용자 결정(2026-10-13)에 따른 순서입니다.
   - 이 확인은 `scripts/ci.sh prepush`로 합니다([ci.md](ci.md)의 "push 전 전체 검사"). 깨끗한 트리에서 여섯 단계를 엄격 모드로 차례로 돌리므로, 건너뛴 것이 하나라도 있으면 실패합니다. 엄격 모드가 아닌 실행은 건너뛴 것이 있어도 `CI passed: <단계> (<n> skipped)`로 끝나므로, 그 통과는 push 전의 통과가 아닙니다. 모든 묶음을 합친 커밋의 기록에는 `skipped:`로 시작하는 줄이 없어야 합니다. `PASS` 줄의 수와 클라이언트 단위 시험의 통과 수가 `scripts/ci.sh`의 하한(`E2E_LEAST_*`, `WOL_LEAST`, `UNIT_LEAST`, `VECTORS_LEAST`. 2026-10-14의 수: `e2e` strict 76줄과 open 2줄, `wol` 20줄, `--lib hjv` 224개, 업데이트 키를 뺀 빌드의 `--lib hjviewer_update` 141개, 클라이언트 코어가 돈 공유 벡터 141건)보다 적으면 단계가 실패하고, 기록의 수도 지금까지의 통과 기록과 견줍니다(`wol`은 [e2e.md](e2e.md#원격-켜기-단계-160) 표의 19단계와 마지막 `PASS (wol)` 줄입니다). `wol`은 클라이언트 패치에 켜기 정보 보고(0035)가 있는데 클라이언트에 그 시험 명령(`--hjv-e2e-wol-report`)이 없으면 실패합니다. 패치에 보고가 없을 때만 E2E를 돌리지 않고 `skipped:` 줄과 함께 통과로 끝납니다.
   - **클라이언트 시리즈의 1.6.0 작업(통합 담당)**: Android 제어 전용 패치(Google Play 작업의 결정 D1), 최소 버전 작업, 최고운영자 화면, 만든 곳과 시범 안내, 스플래시의 클라이언트 커밋은 2026-10-14의 통합(WP-I1)이 설계 3판 12-1의 절차로 0036 뒤에 넣어 패치 0037–0045로 내보냈습니다(스플래시의 덮어쓰기 파일과 같은 커밋). push 전 마지막 검증이 찾은 Windows 패커의 `taskkill`은 0046으로 고쳤습니다. 그 트리로 클라이언트 확인 묶음(Rust 1.75 check, 플랫폼 하니스, Dart 시험)과 클라이언트 전체 lib 시험이 통과했습니다. `scripts/ci.sh prepush`는 26935d0에서 여섯 단계와 빠른 모드 없는 연습이 모두 통과했고, 0046과 문서를 고친 2923dda에서 server·client·e2e·wol·leak을, 운영 서버 사전 점검으로 이 런북과 시험을 고친 545c242에서 server·deploy·leak과 빠른 모드 없는 연습을 다시 돌려 모두 통과했습니다(건너뛴 것 0). 여러 문서가 이미 "Android 앱은 다른 기기를 제어하기만 함"이라고 적고, privacy.md는 Google Play에 내는 개인정보처리방침입니다: [privacy.md](privacy.md) 6절(영어와 한국어), README의 설치 줄, [architecture.md](architecture.md) 2절의 표, 4절의 1.6.0 줄과 6절, [distribution.md](distribution.md)의 결론 아래 문단("모든 Android 빌드는 제어 전용"), "1.5.1에서 1.6.0으로"의 Android 줄과 사용자에게 보내는 안내의 Android 줄, "기기마다 할 일"의 Android 절과 "제어받던 Android 기기", Google Play 내부 테스트 절의 결정 줄. 제어 전용 패치(0041) 없이 push해야 하면 그 문장들을 먼저 되돌립니다.
   - **hbbs 패치 0004의 남은 두 가지(메인 세션이 push 전에 고칠지 정함)**: 2026-10-14의 통합은 0005만 더했고 0004는 그대로입니다. 고치면 0004를 0005와 함께 다시 내보내고 `scripts/ci.sh server`를 다시 돌립니다. `hjv-pk`가 32바이트가 아닌 키도 서명해 돌려주므로 32바이트 키만 아는 키로 답하게 하고, WebSocket으로 33바이트 키를 등록해 `- -` 답을 보는 시험을 더합니다(hjv-api는 그런 답 하나에 매일 확인이 멈추지 않게 이미 고침). 0004 시험이 포트를 고른 뒤 hbbs가 `SO_REUSEPORT`로 여는 사이의 경쟁(`tcp_signaling`과 같은 방식, 한 번 실패가 보고됨)도 고칩니다.
   - **업데이트 키(최소 버전 작업, 사용자만)**: 순서는 [distribution.md](distribution.md)의 "순서"를 따릅니다. 4번의 push와 서버 묶음 `server-<버전>`이 먼저입니다(이 push는 키 없이 나가고, 확인 빌드는 키 없이 통과합니다). 그 뒤 사용자가 서명 키 1·2와 루트 키 101·102를 만들고, 네 공개 줄을 `client/update-keys.env`에 넣어 출시 빌드(8단계) 전에 한 번 더 push합니다. 루트 키는 깨끗한 컴퓨터에서만 만들고 엽니다. 그 컴퓨터는 도구를 공개 저장소의 릴리스 태그(4번의 `server-<버전>`)로 빌드하므로, 루트 키는 첫 push 전에 만들 수 없습니다(`tools/hjv-release/README.md` 1장과 3장의 5-0). 앱 빌드는 그 파일에서만 키를 읽고, 키 줄이 빈 파일로는 1.6.0 출시 빌드가 실패합니다([ci.md](ci.md)). 서버에는 출시 태그의 그 파일을 둡니다(8-1의 3번).
   - **운영 연습 전체(통합 담당)**: 마지막 `server/api` 커밋(최소 버전 작업의 hjv-api, 열린 로그인, 최고운영자 화면을 합친 뒤)에서 `tools/e2e/hjv-api-drill.sh`를 빠른 모드 없이 한 번 돌립니다(약 13분, [hjv-api-drill.md](hjv-api-drill.md)). 빠른 연습(`HJV_DRILL_QUICK=1`)은 동기화가 10분 묵은 상태(점검·복원 뒤)를 보지 않는데, 2026-10-14까지는 빠른 연습만 돌았습니다.
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
   - **현장 시험 결과(배포 전, 7-9)**: 사용자가 1번의 현장 시험을 마치면 [wol-pc-setup.md](wol-pc-setup.md) 2장 "현장 시험 결과"의 표, 시험한 HJViewer 판, 상태 줄을 채우고, 그 절의 주석에 있는 대조 목록(그 문서의 일반 안내와 [relay-dsm.md](relay-dsm.md) 부록의 결과 읽기 표)을 처리한 뒤 `scripts/ci.sh leak`을 다시 돌려 push합니다. 표에 '시험 전'이 남아 있는 동안에는 나눠 주지 않습니다(7-9의 관문). 처음에는 출시 태그 전에 끝낼 것이었지만, 사용자 결정("지금 태그, WOL은 뒤에")으로 출시 태그 `client-1.6.1`을 2026-10-14에 이 시험보다 먼저 붙였습니다. 확인 빌드를 위한 첫 push는 표를 '현장 시험 전'이라고 사실대로 둔 채 나가도 됩니다. 2026-10-15: 현장 시험을 했고 표를 채웠습니다. 운영자 PC는 켜지지 않았습니다. 앱의 켜기부터 NAS의 신호 12개가 PC의 랜카드에 닿는 것까지는 확인했지만(PC를 켜 둔 채 `pktmon`으로 받음, [relay-dsm.md](relay-dsm.md) 부록의 "신호가 PC에 닿는지 보기"), 그 PC의 메인보드가 잠들거나 꺼진 동안 랜카드 전원을 끊습니다(랜 포트 불빛이 꺼짐, BIOS의 PME를 켜도 종료 중 꺼짐). 사용자 결정(2026-10-15): 원격 켜기는 그대로 두고 그 PC로는 더 시험하지 않습니다. 7-9의 관문에서 현장 시험은 끝난 것으로 봅니다.
   - **TCP 펀치 시험(배포 전, 7-9)**: 위 현장 시험과 같은 관문입니다. 클라이언트 lib 시험 `rendezvous_mediator::tests::a_punch_that_meets_the_peers_syn_connects_both_ends`(루프백에서 TCP 동시 열기)는 WSL2인 개발 PC에서 통과할 수 없고(1.5.1의 트리에서도 실패), 클라이언트 패치 0031이 그 파일 `src/rendezvous_mediator.rs`를 건드렸습니다. WSL이 아닌 리눅스(CPU 2개 이상. 하나뿐이면 이 시험은 아무것도 확인하지 않고 통과함)에서 패치를 적용한 트리로 한 번 돌려 통과를 확인합니다([e2e.md](e2e.md) "실행"의 3)과 같은 빌드 환경에서 `cargo test --locked --features linux-pkg-config --lib a_punch_that_meets_the_peers_syn_connects_both_ends`). 그럴 수 없으면 방화벽 E2E가 그 경로를 덮는다고 보는 이유를 [e2e.md](e2e.md)에 적어 둡니다. 2026-10-15: 근거를 [e2e.md](e2e.md)의 "TCP 펀치 시험"에 적었습니다(패치는 그 파일에서 작업을 시작하는 두 줄뿐이고, 펀치 코드와 시험은 upstream 그대로). 사용자가 2026-10-15에 이 근거로 갈음하기로 정했습니다(7-9의 관문에서 TCP 펀치 시험은 끝난 것으로 봄). 나중에 실제 리눅스에서 돌리면 결과를 e2e.md에 더합니다.
   - **열린 질문(8단계의 출시 빌드 전에 사용자가 정함. 2026-10-14 결정: 그대로 두고 문서를 "약 2분"으로 맞춤)**: 기기 키가 바뀐 PC(설정까지 지우고 다시 설치한 PC 등)가 `needs-relogin`이 되기까지 설계는 1분 안이라고 했지만, 지금 구현은 다시 켜진 뒤 약 2분(96~144초)입니다. 다시 켜진 PC가 hbbs에 등록하기 전에 첫 동기화를 보내 거절되고, 다음 동기화가 약 120초 뒤이기 때문입니다. 그동안 컨트롤러에는 비밀번호 창 대신 키 바뀜 경고가 뜹니다(안전한 쪽). 그대로 두고 문서를 '약 2분'으로 맞출지, 거절된 PC가 60초쯤 뒤에 한 번 더 묻게 앱을 고칠지 정합니다.
   - **열린 질문(사설 주소 망의 PC에 중계기를 승인하기 전에 사용자가 정함)**: PC의 망이 사설 주소(RFC 1918, 100.64.0.0/10)이면 hjv-api는 PC와 NAS의 나가는 주소가 같을 때만 같은 곳으로 봅니다. 사설 망은 곳마다 되풀이되기 때문입니다. 공유기 뒤의 집과 사무실은 둘이 같은 주소로 나가므로 괜찮지만, NAT 없이 서버에 닿는 사내망처럼 서버가 둘의 사설 주소를 그대로 보는 곳에서는 늘 다른 곳이 됩니다. 그러면 승인 도구가 `no (a private network of another site: the egress addresses differ)`로 멈추고, `--force`로 승인해도 켜기 요청이 409 `no_relay`(`different_egress`)를 받으며 앱에 "모든 중계기로 시도"도 나오지 않아 그 PC를 켤 수 없습니다([e2e.md](e2e.md#원격-켜기-단계로-찾은-것)). 그런 PC가 있으면 hjv-api의 판정(`server/api/src/wolinfo.rs`)을 넓힐지 정합니다.
   - **열린 질문(8단계의 출시 빌드 전에 사용자가 정함, 보안. 2026-10-14 결정: 1.6.0은 그대로 두고 패치 0032의 Known limits로 남기며 1.6.x에서 다시 봄)**: 키 교환에 실패한 연결에서 사용자가 "종단 간 암호화를 확인할 수 없습니다" 창의 계속을 고르면, 내 목록의 PC에도 1.5.1처럼 기억한 비밀번호가 키 확인 없이 갑니다. hbbs 키를 훔치고 중계를 가로챈 쪽은 이 길로 키 바뀜 경고를 피할 수 있습니다. 그대로 둘지, 내 목록에서 비밀번호 없는 연결이 켜진 PC에는 그런 연결을 키 바뀜 경고로 끊을지 정합니다. 끊으면 hbbs가 PC 키를 잃었을 때 비밀번호로 들어가는 길도 막히므로, 그때 되살리는 방법(그 PC에서 다시 로그인)도 함께 정합니다. 고칠 곳은 클라이언트의 `src/hjviewer_account/ctl.rs` `decide()`이고, 클라이언트 패치 0032의 Known limits에 이 한계가 적혀 있습니다.
   - **열린 질문(8단계 전. 2026-10-14 결정: 알려진 한계로 문서에만 남김)**: Windows의 MachineGuid가 같은 두 로그인(한 PC의 두 Windows 사용자, sysprep 없이 복제한 PC)이 같은 계정이면, 뒤의 로그인이 앞의 계정 키를 같은 기기의 옛 키로 보고 폐기해 서로 로그아웃시킵니다. 그때마다 다른 내 기기에 새 기기 알림이 뜹니다. 복제한 PC만이라도 따로 볼지 정합니다(hjv-api `add_key`의 폐기 조건에 기기 ID를 더함. 한 PC의 두 Windows 사용자는 서버가 구분할 값이 없음).
   - **열린 질문(9단계의 nginx 전환 전. 2026-10-14 결정: 지금처럼 두고 7-14의 nginx 한도 줄 수로 지켜봄)**: nginx의 주소별 한도(로그인과 중계기 등록, 각각 1분에 10번)는 같은 공인 주소(학내 NAT, 공용 Wi-Fi) 뒤의 누구나 채울 수 있습니다. 그러면 hjv-api가 둔 "묶인 PC의 로그인은 주소 한도에서 뺌"이 운영에서는 듣지 않고, 다시 시작한 NAS 중계기의 등록도 막힙니다. `/api/login`의 `limit_req`를 빼고 `limit_conn`만 둘지(hjv-api가 주소·ID별로 직접 셈. `scripts/ci.sh deploy`의 한도 점검도 함께 고침), 지금처럼 두고 7-14의 nginx 한도 줄 수로 지켜볼지 정합니다.
   - **사용자만 할 수 있는 실기기 시험**. Windows·iOS 전용 코드는 컴파일만 확인했고 실행해 본 적이 없으며, 실제 DSM과 실제 랜카드도 써 보지 못했습니다. 결과가 기대와 다르면 고친 뒤 다시 빌드합니다.
     - Windows 설치와 비밀번호 없는 로그인(11단계, 7-9): 1.6.1(`HJViewer-1.6.1-install.exe`)을 1.5.1 위에 설치하고 로그인해 알림과 설정 → 계정의 줄을 보고, 관리자가 스위치를 켠 뒤(12단계) 휴대폰에서 비밀번호 없이 연결합니다. [끄기], [이 PC 빼기], 로그아웃이 그 PC에서 바로 듣는지, 재부팅 뒤 로그온 화면, 다른 Windows 사용자, 다시 설치와 제거 뒤도 봅니다.
       - 제거: 시작 메뉴의 Uninstall로 제거한 뒤 레지스트리에 `HKLM\SOFTWARE\HJViewer\Account`가 없는지 보고, 다시 설치한 그 PC에 예전 주인의 휴대폰이 비밀번호 없이 들어가지 못하는지(비밀번호 창이 뜸) 봅니다. 제거 때 이 키를 지우는 일(클라이언트 패치 0031)은 지금까지 명령 문자열의 시험만 거쳤습니다.
       - 문구: 장치 관리자의 랜카드 속성 '전원 관리' 탭에 있는 항목 이름을 [wol-pc-setup.md](wol-pc-setup.md) 2장의 세 항목과 앱의 켜기 안내(`server/api/src/wolinfo.rs`의 `HINT_NOT_ARMED`, "…대기 모드를 종료할 수 있음")와 견줍니다. 다르면 둘을 실제 문구로 고칩니다(`server/api`를 바꾸면 `scripts/ci.sh`의 server, deploy, e2e, wol 단계를 다시 돌림).
       - Windows 전용 단위 시험: 키보드의 한/영·한자 시험(`src/keyboard.rs`)은 어느 CI 단계도, 확인 빌드도 돌리지 않습니다. 패치를 적용한 트리로 Windows에서 `cargo test --lib keyboard`를 한 번 돌립니다(확인 빌드의 Windows 잡에 단계를 더할지, Windows PC에서 돌릴지는 사용자가 정함).
     - 실제 연세 SSO의 답(11단계, 사용자와 운영 에이전트): 사용자가 앱에서 특수 문자가 든 틀린 비밀번호와 256바이트가 넘는 비밀번호로 한 번씩 로그인해 보고(잠긴 계정이 있으면 그것도), 운영 에이전트는 그동안의 저널에서 `login: SSO error:` 줄의 개수만 봅니다. 그런 상류 오류는 ID별 한도에 세지 않는데, 5번 이어지면 SSO 차단기가 모든 로그인을 2분 멈춥니다. 줄이 생기는 입력이 있으면 아무 포탈 ID 하나로(명단 방식이면 명단의 ID로) 이 정지를 계속 열 수 있으므로, hjv-api가 상류 오류도 ID별로 세게 고칩니다(`server/api/src/login.rs`의 `Failure::Upstream` 갈래).
     - 아이폰 로그인(11단계, 7-9): TestFlight의 1.6.1에서 로그인해 "액세스 가능한 장치"의 내 PC에 비밀번호 없이 연결하고, 새 기기 알림([내 기기 맞음], [아님]), '로그인한 기기' 목록, 잠긴 동안의 "보안 키를 지금 쓸 수 없습니다", 백업을 다른 아이폰에 복원한 뒤의 "로그인이 풀렸습니다"를 봅니다.
     - NAS 중계기 설치(15단계, 7-11): [relay-dsm.md](relay-dsm.md)대로 DSM에 설치하고 로그의 등록 코드, 승인 뒤의 `등록이 승인되어 켜기 명령을 기다립니다.`, `status` 작업의 결과를 봅니다. 부서의 허락(질문 14)은 정해졌습니다: 그 NAS는 사용자 자신의 부서 것입니다(사용자, 2026-10-15). 2026-10-15 오전에 사용자가 설치했고 운영 에이전트가 사용자 승인으로 승인했습니다(7-11의 기록). 그 뒤 켜기 명령을 받아 신호를 보냈으므로 등록은 쓰이고 있습니다. 로그의 승인 줄과 `status` 작업의 결과는 보지 않은 채 종결했습니다(사용자 결정 2026-10-15, 아래 줄).
     - NAS에서만 볼 수 있는 중계기 동작(15단계, 7-11): 개발 PC의 시험은 중계기를 root가 아닌 사용자로 돌려 권한 내리기를 거치지 않고, DSM 부트업 스크립트도 개발 PC의 `sh`로만 돌려 봤습니다. Synology 공유 폴더의 권한(ACL) 아래에서 멈춤 표지를 본 적도 없습니다. 그래서 [relay-dsm.md](relay-dsm.md) 5장의 첫 로그에 `권한을 uid 65534 gid 65534로 낮췄습니다.`가 있는지, 6장 8번의 멈춤 확인(10장의 두 줄)이 되는지, NAS가 다음에 재부팅된 뒤 로그에 시작 줄이 생기는지(11장 끝)를 봅니다. 2026-10-15: 신호를 보낼 때 `SO_BINDTODEVICE를 쓸 수 없어(Operation not permitted …) 주소 bind만으로 랜카드를 고릅니다.`가 남았습니다. 그 NAS의 커널이 권한 없는 `SO_BINDTODEVICE`(Linux 5.7부터)를 허락하지 않는 것이고(relay-dsm.md 11장), 랜 포트가 하나라 보내는 데는 지장이 없었습니다. 권한 내리기 줄, 멈춤 확인, 재부팅 뒤 시작 줄은 보지 않은 채, 사용자 결정(2026-10-15)으로 NAS에 관한 남은 확인을 종결했습니다. 중계기는 켜기 명령을 받아 신호를 보내므로 동작은 확인됐고, 그 셋은 다음에 NAS를 만질 때 봅니다.
     - 휴대폰의 켜기 메뉴(15단계 뒤): 꺼진 내 PC의 카드 메뉴 "켜기"와 카드를 누를 때의 권유, 진행 문구의 순서, 켜진 뒤 연결("연결합니다…"가 30초 안인지), 진행 창을 닫았다가 다시 뜨는 것, NAS를 껐을 때와 5분 안에 켜지지 않을 때의 문구를 봅니다. PC 쪽에서는 묶은 뒤 1~3분 안에 서비스 로그의 `hjv: wake-on-LAN report: 1 wired card(s)`, 깨운 뒤 아무도 연결하지 않아도 10분 넘게 깨어 있는지와 `hjv: resumed: staying awake for ten minutes`를 봅니다. 2026-10-15: 아이폰의 "액세스 가능한 장치" 카드에서 켜기를 눌러 서버의 켜기 작업과 NAS의 신호까지 갔습니다. 운영자 PC가 켜지지 않아 켜진 뒤의 문구, 연결, 10분 깨어 있기는 보지 못했습니다. 처음에는 최고운영자 화면에서 켜기를 찾아 요청이 나가지 않았습니다(그 화면에는 켜기가 없음).
6. **2026-10-14 통합(WP-I1)의 점검표: 1.6.0의 최소 버전과 업데이트, Google Play와 Android 제어 전용, 최고운영자 화면, 열린 로그인, 만든 곳과 시범 안내, 스플래시**. 작업 보고서에만 있던 기기 시험, 미룬 일, 조건부 통과, 남은 작은 문제를 한 줄씩 옮겼습니다. 줄 머리의 굵은 글씨가 할 사람(과 때)입니다. 위 1~5번에 이미 있는 것은 다시 적지 않습니다. 끝낸 줄에는 결과를 적거나 줄을 지웁니다. 시험은 운영 서버가 아니라 시험 서버(`hjv.test`나 `rd.example.com` 역할)와 시험 키로 만든 빌드로 합니다.
   - **(가) push 전, 개발 PC**
     - **메인 세션(push 전 전체 검사 전)**: 개발 PC에 남은 시험 프로세스와 폴더를 정리합니다. 10/12~13의 `binary.rs` 시험이 남긴 `hjv-api serve` 3개와 `/tmp/hjv-binary-*`, scratchpad의 `flutter_tester`(쓰는 작업이 없을 때), `build/e2e/w5-int`(약 13GB)입니다.
     - **메인 세션(push 전 전체 검사)**: 기록의 수를 통합 1부(같은 클라이언트 트리)의 수와 견줍니다. 그때는 `--lib hjv` 244개(`linux-pkg-config`만, 무시 2; ci.sh 하한 224), `--lib hjviewer_update` 159개(무시 1; 키 없는 빌드의 하한 141), 공유 벡터 141건, Dart의 업데이트·계정·최고운영자·Android 순수 시험 107개(웹 대역도 같음), 위젯 시험 63개, 시험 폴더 전체 330개였습니다. 늘어난 만큼 하한(`UNIT_LEAST` 등)을 올릴지 정합니다.
     - **메인 세션(push 전 전체 검사)**: client 단계의 `test_play_upload.py`와 `test_release_info.py`가 `skipped=` 없이 끝났는지 기록에서 봅니다. 이 Python 시험의 건너뜀은 `scripts/ci.sh`가 세지 않습니다. 패치 0041·0042가 들어와 Play 매니페스트의 짝 시험도 이제 돕니다.
     - **메인 세션(push 전 전체 검사)**: `deploy` 단계의 'nginx logged errors' 검사는 `/ws/id` 프록시의 `Connection reset by peer` 한 줄로 가끔 실패합니다(10/13, 10/14 기록). 그 줄 때문이면 다시 돌리고, 되풀이되면 검사를 고칠지 정합니다.
     - **메인 세션(leak 단계)**: 실제 목록으로 돌린 `scripts/ci.sh leak`이 '연세대학교'나 '프로젝트TF'(패치 0044·0045, README, 문서)에서 걸리면 지우지 말고 사용자에게 묻습니다. 사용자가 정한 문구입니다. 정한 결과는 글자를 윤곽선으로 담은 두 파일(`client/icons/splash-text.svg`, overlay의 `hjv_splash_branding.xml`)에도 적용합니다. 이 두 파일의 글자는 검사에 걸리지 않습니다.
   - **(나) 출시 빌드 전에 정할 것**
     - **사용자(보안)**: 명단에 없는 ID의 로그인 한도와 남는 위험을 받아들일지 정합니다. 한도는 `HJV_API_LIMIT_NEW_ID` 2/86400과 `HJV_API_LIMIT_SSO_NEW` 10/3600이고, ID와 주소(IPv6는 /64)가 각각 그 4분의 1까지 씁니다. 남는 위험은 아무도 로그인하지 못한 SSO 호출이 하루 많아야 240번, 남의 연세 계정이 잠길 수 있고 연세 쪽에는 이 서버 주소가 원인으로 보임, 주소 다섯 개면 시간당 틀린 비밀번호 10번으로 처음 로그인을 막을 수 있음입니다([architecture.md](architecture.md) 5-7, 이 런북 7-13). 정보화처에 잠금 기준을 물을지도 정합니다. 바꾸면 env 두 줄입니다.
     - **사용자·메인 세션(보안)**: 업데이트 기록 저장의 경합(클라이언트 패치 0037의 Known limits)을 받아들일지 정합니다. 한 기기의 두 프로세스가 같은 순간 저장하면 낮은 순위의 기록이 남아, 다음 확인(시작 30초 뒤, 그 뒤 6시간마다)까지 늦은 선언의 폐기를 모를 수 있습니다. 고치려면 `src/hjviewer_update/store.rs`의 `save()`와 `write_private()`를 잠금 파일로 감쌉니다(Rust 1.75에는 std 파일 잠금이 없음).
     - **메인 세션**: 계정에 묶이지 않은 설치 PC도 설치 시각이 지나면 6시간마다 문 1에 기기 서명으로 물어 403을 받습니다. 한 공인 주소 뒤에 그런 PC가 8대 넘게 있으면 hjv-api의 주소 실패 한도(`HJV_API_LIMIT_UPDATE_FAIL_IP` 30/86400)가 차서, 그 주소의 실패 요청이 모두 429가 되고 앱에는 '로그인 필요' 대신 '서버가 바쁨'이 보입니다. 고칠지 정합니다(`src/hjviewer_update/install.rs`의 `look()`에서 묶이지 않은 PC는 사람이 누를 때나 하루 한 번만 묻게 함).
     - **메인 세션**: 코어 상태 JSON에 `auto_install`이 없어서, W1 첫 문장과 배너의 '쓰지 않을 때 자동으로 설치됩니다'는 required일 때만 보입니다. 넣으려면 새 상태가 필요한 설계 변경입니다.
     - **메인 세션**: 정책 확인이 실패한 뒤 버전 거절 문구를 고정 문구 R3로 바꾸고 상태에 `check_error`를 두는 일은 하지 않았습니다(새 상태가 필요한 설계 변경). 지금은 R1·R2입니다.
     - **메인 세션**: 패커 `HJViewer.exe`(`libs/portable`의 winres)에는 CompanyName이 없습니다. 설치된 앱의 `HJViewer.exe`(`Runner.rc`)는 '프로젝트TF'입니다. 넣으려면 `libs/portable/Cargo.toml`에 `CompanyName = "프로젝트TF"` 한 줄입니다.
     - **메인 세션**: Play 빌드를 설치 출처(`installerStore`) 대신 빌드 값(`--dart-define=HJV_ANDROID_CHANNEL=play`)으로 알아볼지 정합니다. 바꾸면 client-build.yml 한 줄, Dart, `PlayBundleListsTest`를 함께 고칩니다.
     - **사용자**: E4 화면(받은 APK가 이 앱의 새 버전이 아님)에서 [다시 시도]를 뺄지 정합니다. 빼면 정책이 고쳐져 와도 그 실행에서는 다시 받을 길이 없습니다.
     - **메인 세션**: 설치하지 않고 실행 중인 1.5.1 Windows PC에 맞는 컨트롤러 카드 C1a와 짧은 링크 안내 U1 문구가 없습니다(설계 2-5에 없는 새 문구가 필요). 1.6.0 이상 실행본 PC의 카드 C1b는 이미 나눴습니다. C1a와 U1도 나눌지 정합니다.
     - **메인 세션**: 업데이트 키가 있는 설치 PC에서는 대화 상자를 쓰는 어느 프로세스(연결 관리 창 포함)든 업데이트 상태를 처음 읽을 때 Windows의 감시 스레드가 시작됩니다. 주 창에서만 시작하도록 좁힐지 검토합니다.
     - **메인 세션**: E2E에 신뢰 규칙의 '선언 하나' 시나리오(루트 선언 뒤, 그것을 놓친 PC가 받는 서명 키 정책)가 없습니다. 클라이언트 단위 시험과 공유 벡터가 그 길을 봅니다. 더할지 정합니다.
     - **메인 세션**: E2E에 최고운영자 화면의 시나리오가 없습니다(E2E의 hjv-api에 `HJV_API_OPERATORS` 없음). hjv-api 시험과 앱의 위젯 시험이 봅니다. 더할지 정합니다.
   - **(다) push 뒤의 확인 빌드와 첫 출시**
     - **메인 세션(push 바로 뒤의 확인 빌드)**: 처음 컴파일되는 것을 봅니다. Windows는 `src/hjviewer_update/win.rs`(version.dll의 `GetFileVersionInfoW` 등), `src/platform/windows.rs`의 업데이트 줄, 패커의 `hjv_guard.rs`, `Runner.rc`의 한글 CompanyName·LegalCopyright(`#pragma code_page(65001)`)입니다. Android는 `HjvUpdate.kt`, 매니페스트 병합(lintVital), 스플래시 리소스 링크, Play 번들 단계(표지 `PLAY_OVERLAY`는 AAB에만)입니다. APK의 권한은 ACCESS_NETWORK_STATE, CAMERA(1.9.1부터 없음), INTERNET, POST_NOTIFICATIONS, RECORD_AUDIO, WAKE_LOCK, `kr.ac.yonsei.hjviewer.DYNAMIC_RECEIVER_NOT_EXPORTED_PERMISSION`, REQUEST_INSTALL_PACKAGES뿐이고 `HjvUpdateFileProvider`가 있어야 하며, AAB에는 뒤의 둘이 없어야 합니다(PLAY_FORBIDDEN 단계). iOS는 `LaunchScreen.storyboard`의 컴파일입니다. 업데이트 키 단계의 'no update keys to pin'과 'pinned HJV_POLICY_FLOOR=1:0', 산출물(`-check`)의 `SHA256SUMS`(`sha256sum -c`)와 `release.json` 없음도 봅니다. 그 밖은 [ci.md](ci.md)의 "아직 러너에서 돌려 보지 못한 것"입니다.
     - **메인 세션(확인 빌드 뒤)**: Windows 잡의 두 연기 시험('Update smoke test', 'Packer guard smoke test')이 몇 번 통과하면 그 단계의 `continue-on-error`를 지웁니다.
     - **사용자(첫 Play 업로드)**: 첫 AAB는 Play Console 내부 테스트에 손으로 올립니다. 개인정보처리방침 URL(공개 main의 privacy.md, 카메라·마이크 절 포함)을 먼저 등록하고, 접근성·포그라운드 서비스 선언 요구가 나오지 않는지 봅니다. 손으로 올린 버전에는 그 뒤 검증 실행(`play=true`, `play_release=validate`)만 합니다. 실제 업로드와 순서 검사(먼저 시작한 실행의 play 잡을 다시 돌리면 거부)는 다음 버전의 출시 실행과 함께 봅니다([ci.md](ci.md)의 "Google Play 업로드", "처음 한 번 할 일" 9단계).
     - **사용자(첫 실제 정책, 출시 빌드 뒤 7일 안)**: 출시 빌드의 산출물을 `gh run download`로 `build/release-<버전>`에 받고, `hjv-release policy … --release-dir build/release-<버전> --allow-input check_only`가 windows와 android 모두 '확인함'과 '패키지와 release.json이 실행 …의 산출물과 같음'을 내는지 봅니다. 결과(2026-10-14, 첫 정책을 만들 때, 7-10): 실행 38568720786의 windows와 android가 모두 '확인함'이었습니다. 도구가 `check_only` 입력을 아직 모르므로 `--allow-input check_only`가 필요합니다(도구에 넣을지는 메인 세션). `--attest`는 `gh attestation`이 있는 새 gh와 비공개 저장소의 출처 증명 요금제가 있을 때만 됩니다(요금제는 사용자가 확인).
     - **사용자(키를 만든 뒤)**: 공개 줄을 넣은 `client/update-keys.env`로 `hjv-release keys --production`과 `CALLED=1 python3 -I scripts/check-build-keys.py --pin-update-keys`가 둘 다 통과하는지, 출시 빌드 요약에 키 지문과 하한이 나오는지 봅니다.
     - **사용자(첫 출시 빌드)**: Android 출시 산출물의 `release.json`에 있는 build가 그 APK의 versionCode인지, 키스토어가 없는 실행의 `-debugsigned` 산출물에는 `release.json`이 없는지 봅니다.
     - **사용자(깨끗한 컴퓨터, 운영 서버)**: 도구 README 3장 5-14의 리허설을 두 번 합니다. 8-2로 첫 정책을 넣은 뒤(2026-10-14), 운영자의 기기만 1.6.1을 쓰는 동안 운영 서버의 정책으로 합니다(도구 README 5-14, [distribution.md](distribution.md)의 "순서" 5단계). 시험 서버용 선언으로 하지 않습니다. 이 줄은 6번 머리의 '시험 서버' 규칙에 들지 않습니다. 도구는 깨끗한 컴퓨터에서 공개 저장소의 릴리스 태그로 빌드하고(커밋을 박는 명령은 도구 README 3장 5-0의 1번. 개발 PC의 `build/signer/hjv-release`는 쓰지 않음), 실제 루트 키와 서명 키를 씁니다. 선언은 8-2로 넣습니다(`--relaxes` 없음). 다만 2026-10-14 23:43부터는 거절된 1.5.x 컨트롤러가 문 2 링크로 1.6.1을 스스로 받을 수 있어서, '운영자의 기기만 1.6.1' 조건을 운영자가 지킬 수 없습니다. 리허설을 곧 하고, 하기 전에 `hjv-api admin report`의 앱 버전별 PC를 봅니다. 다른 1.6.1 기기도 세대 2와 3의 선언을 받게 되는 것은 받아들입니다(기준만 다시 잡는 선언이라 막는 것은 바뀌지 않음). 2026-10-15: 사용자 결정으로 보류합니다(다시 정할 때까지 하지 않음).
     - **사용자(Windows Terminal과 VS Code의 WSL 터미널)**: 시험 키로 `hjv-release keygen`(버릴 키, 끝나면 지움)과 `sign`을 돌립니다. 한글이 깨지지 않는지, 비밀번호가 보이지 않는지, Ctrl-C 뒤 입력 표시가 돌아오는지, Ctrl-Z로 멈추지 않는지, 다른 터미널에서 `kill -TSTP` 뒤 `fg`해도 친 글자가 보이지 않는지 봅니다.
     - **사용자(TestFlight)**: App Store Connect 프로파일로 서명한 빌드를 TestFlight로 설치해, 업데이트 화면의 [TestFlight 열기]가 TestFlight 앱을 여는지, TestFlight 앱이 없으면 App Store의 TestFlight 페이지가 열리는지 봅니다. Ad Hoc 빌드에는 I3 안내가 나와야 합니다. 1.9.1부터 App Store에서 설치한 앱은 [TestFlight 열기] 대신 [App Store 열기]가 나오고 App Store의 HJViewer 페이지가 열려야 합니다. 이것은 App Store에 출시된 뒤에만 볼 수 있습니다(심사하는 앱과 TestFlight 설치본은 TestFlight 길). TestFlight 설치본의 길은 1.9.1에서도 같습니다.
   - **(라) 실기기 시험 (사용자. 시험 서버와 시험 키로 만든 빌드, 운영 서버 아님. 결과가 기대와 다르면 고친 뒤 다시 빌드)**
     - **사용자(만든 곳과 시범 안내, 0044)**: 설정 → 정보에 '만든 곳: 프로젝트TF'와 시범 안내 두 문장(영어 UI면 'Made by 프로젝트TF'와 영어 문장. 1.9.1 앱부터는 시범 안내가 없음), Windows 정보 화면의 'HJViewer modifications © 2026 프로젝트TF', 로그인 창 제목 아래의 회색 안내 한 줄을 봅니다. 데스크톱 홈과 휴대폰 설정에 'RustDesk 제공'(Powered by RustDesk) 링크가 없어야 합니다. Windows에서 `HJViewer.exe`의 속성 → 자세히의 저작권과 PowerShell `(Get-Item <설치 폴더>\HJViewer.exe).VersionInfo.CompanyName`이 프로젝트TF인지 봅니다.
     - **사용자(hbbs의 거절 창, 1.5.1 Windows·Android·iPhone)**: 시험 hbbs의 `min-controller.conf`에 `MIN=1.6.0`, `FROM=`(빈 값), `TEXT=<S1 문구>`를 넣고, 10초쯤 뒤 hbbs 로그의 'min-controller 1.6.0 from now'를 확인합니다. '연결 오류' 창에 문구가 그대로(%MIN% 자리에 1.6.0) 나오고, 단추는 [확인]뿐이고, 다시 연결 카운트다운이 없고, 링크를 누르면 브라우저가 열리고, 시도마다 거절 로그가 한 줄이어야 합니다. `MIN=`으로 바꾸면 10초쯤 뒤 다시 연결됩니다. 업데이트 키가 든 1.6.0 앱은 R1만 보이고 링크가 없으며, 곧 정책을 다시 받아 업데이트 화면이나 R2로 갑니다.
     - **사용자(PC의 컨트롤러 거절, 0038)**: 기한이 지난 min 1.6.0 정책을 가진 설치된 1.6.0 PC(서비스 로그 'version policy: now at …', hbbs의 MIN은 끔)에 1.5.1 Windows·Android·iPhone으로 연결합니다. 비밀번호 창보다 먼저 '로그인 오류'와 L1(Android는 Play 문장이 붙음, iPhone은 L2)이 뜨고 다시 연결하지 않아야 합니다. 직접 IP 연결도 같습니다. 기한 전에는 grace_min 아래만 거절되고 grace_min과 min 사이는 연결되며, Android는 두 항목이 모두 막는 버전만 거절됩니다. 서비스를 다시 시작한 직후 30초 안에도 저장된 정책으로 거절하는지, 거절 뒤 1분쯤 정책을 다시 확인하는지, PC 로그의 거절 줄이 1분에 한 줄 이하인지, 설치하지 않은 1.6.0 실행본 PC도 같은 거절을 하는지(어느 프로세스가 연결을 받는지, 설계 2-2의 [불확실]) 봅니다.
     - **사용자(화면 바꾸기의 거절, 0038, 필수: 7-9의 관문)**: 정책의 min만 강제한 상태에서 1.6.0 PC A가 1.5.1 PC B를 제어하다가 [화면 바꾸기]를 누르면, B에 '로그인 오류'와 L1이 뜨고 A는 제어되지 않아야 합니다. B가 1.6.0이면 평소처럼 바뀝니다. 이 줄은 Flutter 데스크톱 빌드에서만 컴파일되어 어떤 자동 시험도 닿지 않습니다. 2026-10-15: 사용자가 이 시험을 완료로 정했습니다(7-9의 기록).
     - **사용자(Windows 무인 설치, 0039)**: (1) 더 높은 패키지를 정책에 넣으면 설치 시각 뒤 서비스가 설치합니다(로그 'update: fetching', 'installing <v> (try 1)', 재시작 뒤 '<v> is installed'). (2) 세션 중에는 설치하지 않고, 닫으면 약 1분 안에 시작합니다(70분 넘는 세션 뒤에도). (3) 최소 버전 아래, 유예 끝, 24시간 넘는 세션이면 다음 02:00~05:00에 W5 창(로그온 화면에서도)이 뜨고 10분 뒤 설치합니다. (4) 해시가 다른 패키지는 실행하지 않고 error=verify, 폴더 삭제, 24시간 대기입니다. (5) 묶이지 않은 PC는 credential=unbound(W2)이고, 로그인하면 1분 안에 device로 바뀌어 다시 받습니다(403일 때. 401은 6시간 뒤). (6) systemprofile의 `hjviewer-update`에 junction을 두면 error=file입니다. (7) ProgramData 스테이징 폴더의 `custom.txt`는 설치에 영향이 없습니다. (8) FILE_SHARE_READ 핸들을 쥔 채 `CreateProcessAsUserW`가 되는지, 보호 DACL(SYSTEM과 Administrators만, 상속 없음)과 소유자를 봅니다. (9) 설치가 3번 실패하면 W6과 [다시 시도]입니다. (10) 한국어 Windows의 `--update` 알림이 '업데이트했습니다.'와 '업데이트하지 못했습니다.'입니다. 2026-10-16: 1.7.0의 정책 seq 3이 무인 설치 시각(2026-10-17 22:13(KST))을 처음 넣었습니다. 그때 계정에 묶인 1.6.1 PC는 운영자 PC 하나였고, 사용자 결정으로 그 PC를 08:38에 [지금 업데이트]로 올렸으므로(1.6.1+5 → 1.7.0+8, 서비스는 자동 시작으로 실행 중) 이번에 무인 설치가 실제로 도는 PC는 없습니다. 이 시험(무인 경로)은 다음 버전에서 합니다.
     - **사용자(Windows 무인 설치와 정책의 변화, 0039, 신뢰 규칙)**: 내려받기나 W5 예고 중에 `--auto-after windows=none`이나 `--withdraw-package windows` 정책을 게시하면 로그 'the policy no longer installs <v> now; it is not started'가 남고 파일이 지워집니다. 1.5.x(최소 버전 아래)는 롤아웃 정지만으로는 계속 설치합니다. 재질문 중에 `hjv.test`의 443만 막으면 'is not started: the server did not confirm the policy just now'이고, 재질문 중에 세션을 열면 'a session started; <v> waits'입니다. latest 1.6.2 라벨에 1.6.1 패커를 넣으면 error=version과 '내려받은 파일이 이 앱의 새 버전이 아니어서…'가 나옵니다. min 1.6.1, grace_min 1.6.0이면 1.6.0 PC는 기한과 퍼짐 뒤에, 1.5.x PC는 발행과 퍼짐 뒤에 설치합니다.
     - **사용자(설치하지 않은 Windows 실행본, 0039, 0040)**: 로그인한 1.6.0 실행본에서 [업데이트]를 누르면 '다운로드' 폴더에 `HJViewer-<v>.exe`가 생기고, [지금 실행]으로 창이 닫힌 뒤 새 버전이 열립니다. 실행 전에 그 파일을 바꾸면 error=verify, 버전이 다른 파일은 error=version입니다. 로그인하지 않았으면 '새 버전을 받으려면 로그인하세요.'입니다. 그 PC에 연결한 컨트롤러의 카드는 '[업데이트]… [지금 실행]' 문구이고, 설치본 PC는 '[지금 업데이트]' 문구입니다.
     - **사용자(패커의 K1, 0039)**: 1.5.1을 켠 채 새 `HJViewer.exe`나 `HJViewer-install.exe`를 열면 K1 창만 뜨고 아무것도 실행되지 않습니다. `start /wait HJViewer.exe --update` 뒤 `echo %ERRORLEVEL%`은 창 없이 32입니다. 실행본의 [지금 실행]은 `HJV_WAIT_PID` 대기 뒤 K1 없이 풀리고 `--cm` 창이 끝납니다. K1과 문 2 페이지가 말하는 '작업 표시줄 오른쪽 아래 아이콘의 [끝내기]'가 설치하지 않은 실행본에도 있는지 봅니다(트레이는 설치본 서비스에만 뜰 수 있음). 없으면 메인 세션이 두 문구를 고칩니다.
     - **사용자(패커의 taskkill, 0046, 보안)**: 설치하지 않은 1.6.0 실행본 PC의 '다운로드' 폴더에 `C:\Windows\notepad.exe`를 `taskkill.exe`라는 이름으로 복사해 둡니다. [업데이트] → [지금 실행]으로 새 버전이 열릴 때와, 그 폴더에 받은 더 새 `HJViewer.exe`를 처음 열 때 메모장이 뜨지 않고 새 버전만 열려야 합니다. 패커는 새 버전을 풀 때 `taskkill`을 Windows 시스템 폴더에서 실행합니다(같은 버전을 다시 열면 실행하지 않음). 끝나면 그 `taskkill.exe`를 지웁니다.
     - **사용자(업데이트 화면, 0040)**: Windows 설치 PC의 required 화면(제목, 버전 줄, W1, 들어오는 연결은 받는다는 줄. 왼쪽 ID·비밀번호와 설정은 그대로), [지금 업데이트] → W5 → 단계 문구 → 새 버전으로 다시 열림, 묶이지 않은 PC의 W2와 [로그인], 설치 3번 실패의 W6을 봅니다. available·grace 배너(×로 닫은 available은 다시 켜도 안 뜨고 grace는 다시 뜸, 마감은 현지 시각), required에서 `--connect`와 `hjviewer://` 링크는 B1 창이고 열린 세션 창의 새 연결도 B1이며 진행 중인 세션은 그대로인지 봅니다. 컨트롤러 카드 C1a와 [이 PC용 설치 링크]의 답(다른 세션 창에서도 옴), 로그인하지 않은 컨트롤러의 U2, 1.6.0 이상 PC의 C1b, Android 기기에는 카드가 없음을 봅니다. iPhone은 링크를 크게 보이고 복사 안내가 없으며(U1), TestFlight 안내는 I1·I2·I3입니다. Android Play 설치본은 [Play 스토어 열기]만, Android 7 미만은 브라우저 내려받기, `move_to_play`면 M1과 [방법 보기]입니다. 설정 → 정보의 정책 줄과 [업데이트 확인], 업데이트 키가 있는 고정 서버 빌드의 '연결 오류' 창에 누를 수 있는 링크가 없는지(키 없는 빌드는 그대로), 다크 모드와 360 폭에서 넘침이 없는지, 휴대폰을 1시간 넘게 뒤에 두었다가 앞으로 가져오면 마지막 확인 시각이 바뀌는지 봅니다.
     - **사용자(Android 앱 안 설치, 0042, 필수: 7-9의 관문)**: 새 APK를 설치한 직후 앱이 바로 켜지는지 먼저 봅니다(FileProvider 설정이 틀리면 시작 때 죽음). 같은 키의 더 높은 versionCode APK는 [새 버전 내려받기] → 진행률 → (허용이 꺼져 있으면 A4와 [설정 열기]) → 설치 화면 → 새 버전이 열리고 `cache/updates`가 빕니다. E4 네 가지를 꼭 봅니다: 다른 패키지 이름, 다른 키, 깨진 서명은 E4와 파일 삭제이고, Android 9에서 같은 키 APK는 통과합니다. Android 11에서 '이 출처 허용' 뒤 앱이 다시 시작되면 다시 내려받은 뒤 설치 화면이 열리는지, 내려받는 동안 다른 앱으로 갔다 돌아오면 설치 화면이 열리는지, 설치 화면의 [취소] 뒤 [설치 화면 열기], Android 13 이상의 '제한된 설정', 갤럭시 One UI 6 이상의 자동 차단(막히면 메인 세션이 [distribution.md](distribution.md)에 한 줄), APK를 여는 다른 앱이 있을 때의 선택 창, Android 7.x의 content URI, 실패마다 `adb logcat -s hjvUpdate`의 경고 줄을 봅니다. 2026-10-15: 사용자가 이 시험을 완료로 정했습니다(7-9의 기록).
     - **사용자(Android 제어 전용, 0041)**: 새로 설치(Android 11 이상, 가능하면 14 이상)하면 아래 탭은 '연결'과 '설정'뿐이고, 홈으로 나갔다 오기와 회전에 죽지 않습니다(`FloatingWindowService … not found` 경고는 정상). 1.5.1(화면 공유, 접근성, 부팅 시 시작을 켰던 휴대폰) 위에 덮어 설치하면 접근성에 HJViewer 항목이 없고 'Service is running' 알림이 없으며 재부팅 뒤 토스트와 충돌이 없습니다. 나가는 연결(비밀번호와 비밀번호 없는 연결, 화면, 터치, 한글, 클립보드, 파일 전송, 터미널), 설정 화면(Deploy, 2FA, 화면 공유, Enhancements가 없고 모든 항목을 눌러도 오류 없음), 음성 통화(마이크 권한을 묻고, 거부하면 PC에 요청이 가지 않음, '이번만 허용'이 백그라운드에서 만료된 경우의 문구)를 봅니다. QR 읽기의 카메라 권한은 1.9.1에서 QR 화면과 그 플러그인이 빠져 보지 않습니다(카메라 권한을 묻는 일이 없어야 함).
     - **사용자(최고운영자 화면, 0043)**: 시험 서버의 `HJV_API_OPERATORS`에 시험 ID를 넣고 다시 시작한 뒤 `hjv-api admin operators`의 첫 줄이 '1 configured, 1 with an account'인지 봅니다. 진입 이름 '최고운영자: 사용자와 PC', PC의 OS 칸 'Windows · HJViewer 1.6.0', 표가 `admin users --reveal`·`binding list --reveal`과 맞는지, IP가 그 PC의 마지막 동기화 주소인지, 볼 때마다 `operator.log`에 `cmd="app overview"` 한 줄이 늘고 `oplog-verify`가 통과하는지 봅니다. 일반 계정에는 항목이 없고 404입니다. 휴대폰 가로+키보드와 큰 글자에서 넘침이 없는지, 다른 기기에서 로그아웃시킨 뒤 [새로 고침]이나 앞으로 가져오기에 '로그인이 풀려서 화면을 닫았습니다.'가 한 번 뜨는지, ID를 뺀 뒤 답 전에 닫아도 항목이 사라지는지, Windows 대화 상자가 Esc와 [닫기]로 닫히고 검색 칸의 Enter로는 닫히지 않는지 봅니다.
     - **사용자(스플래시, 0045)**: Android 8(라이트), 11(라이트·다크, 3버튼·제스처), 12~15(시스템 스플래시의 원과 모니터가 런처 아이콘과 같고 아래 글자가 보이며 흰 화면이 번쩍이지 않음, 삼성 One UI 포함), 앱 설정의 테마가 시스템과 다를 때, 5.1(바탕색만), iPhone(홈 표시줄, 홈 버튼)과 iPad 가로·세로(업데이트 직후 옛 화면이면 재부팅 뒤 다시)를 봅니다. Windows는 바뀐 것이 없어야 합니다.
     - **사용자(업데이트 확인과 설정, 0037, 0039)**: 업데이트 키가 있는 고정 빌드를 설치하면 '자동 업데이트'가 켜진 채 잠겨 있고, 하루 동안 `api.rustdesk.com` 조회가 없습니다(DNS 기록이나 hosts로 봄). TLS 검사 프록시가 있는 망에서는 로그에 'version policy: no answer (certificate)'가 남고 연결 화면이 막히지 않습니다. Play 설치본과 APK 설치본의 `channel`이 `android_play`와 `android`로 나옵니다. 설치된 Windows PC의 서비스가 30초 뒤 정책을 받아 LocalService 프로필의 config 폴더에 `HJViewer_update.json`을 씁니다.
     - **사용자(문 1의 로그인 경로, 0037)**: 로그인한 1.6.0 앱에서 [새 버전 내려받기]를 누르면 hjv-api 저널에 update token 기록이 남고, job.phase가 downloading → ready로 가고, 받은 파일의 크기와 SHA-256이 정책과 같습니다(25 MB에 약 7초). 로그아웃 상태에서는 job.error=login입니다. 요청 서명(AccountSigner. 증명과 함께 200, 없이 401)은 이 시험으로만 확인됩니다(0037의 Known limits).
     - **사용자(1.5.1에서 1.6.0으로 이행, [distribution.md](distribution.md))**: 설치된 1.5.1 PC가 문 2 페이지의 `HJViewer-1.6.1-install.exe`(SmartScreen [추가 정보] → [실행], UAC [예])로 올라가고 ID, 영구 비밀번호, 설정이 남는지 봅니다. 사람 없는 1.5.1 설치 PC를 iPhone에서 C1a 카드의 링크로 올립니다(PC의 Edge 주소창에 손으로 침, 소문자나 '-' 없이도 됨, 네 번째 내려받기와 10분 뒤는 안 됨). UAC [예]를 원격으로 누를 수 있는지, 약 1분 뒤 다시 연결되는지, 파일 전송으로도 한 번 봅니다. Android 1.5.1이 연결 오류 창의 주소로 APK를 받는지(앱 안 브라우저, 안 되면 Chrome), 1.5.1 Windows 컨트롤러의 '연결 오류' 창 주소가 기본 브라우저로 문 2 페이지를 여는지, hbbs 파일이 있을 때 1.6.1 컨트롤러가 1.5.1 PC에서 화면 바꾸기를 누르면 PC에 S1이 뜨는지(문서의 추론) 봅니다.
   - **(마) 배포 뒤 확인 (운영 에이전트는 사용자 승인 뒤, 시험 서버에서 먼저. 포탈 ID, 주소, 코드는 보고하지 않고 줄 수만 보고)**
     - **운영 에이전트(8-1)**: 실제 서버에서 처음 돌릴 때 묶음의 `bin/hjv-update-verify.sh` 설치와 `--self-test`, 업데이트 구역 권한(0750, 0640, 0700), `update-keys.env`, `hjviewer.env`의 `HJV_MIN_CONTROLLER_FILE` 줄을 봅니다. nginx reload 뒤 `/api/hjv/version`이 정책 전에는 404 JSON이고 첫 정책 뒤에는 200과 같은 SHA-256이며, WordPress 경로(`/`, 로그인 페이지)는 그대로인지 봅니다.
     - **운영 에이전트(8-4)**: 열기·닫기 블록이 `door 2: 200`과 `door 2: 404`를 내고, 그동안 `ps -eo args`에 문 2 코드가 보이지 않는지 봅니다.
     - **운영 에이전트(8-3)**: 일부러 틀린 `min-controller.conf`를 넣어 ERROR와 되돌리기 블록을 보고, `systemctl restart hjviewer-hbbs` 뒤의 확인 블록이 'min-controller <MIN> from …'을 내는지 봅니다.
     - **운영 에이전트(7-12의 2번)**: 블록 뒤 `/api/hjv/version`이 200 또는 404 JSON이고 `/api/peers`가 404인지 봅니다.
     - **운영 에이전트(8-5의 (라), 시험 서버)**: 시험 키로 도둑 정책(보관소에 없는 e1-sN)을 건 뒤 (라)의 2번을 하면 저널에 went back ERROR가 한 줄 나오고, 이어서 선언이 8-2로 들어가는지 봅니다.
     - **운영 에이전트(8-8)**: nginx·php-fpm 사용자의 READABLE 줄이 0인지, `auto_block_ips.sh`가 무엇을 세는지, 사이트에 `error_page 404`가 있는지 봅니다.
     - **운영 에이전트(Docker를 쓰는 서버만)**: hbbs가 Docker이면 디렉터리를 마운트했을 때 `min-controller.conf` 교체(`mv`)가 10초 안에 반영되는지 봅니다.
     - **운영 에이전트(문 1, 문 2, 패키지)**: `sudo test -e /srv/hjviewer-updates/pkg/<패키지> || echo MISSING; sudo runuser -u www-data -- test -r /srv/hjviewer-updates/pkg/<패키지> && echo READABLE`이 아무것도 내지 않고(8-8의 1번처럼 존재 확인을 앞에 두고 `runuser`로. `sudo -u`는 root로만 실행을 허락하는 sudo에서 거절되어 통과처럼 보이고, `test -r`만으로는 패키지 경로를 잘못 써도 통과처럼 보임), 8-4처럼 주소를 표준 입력으로 준 `curl -K -`(코드가 명령줄에 남지 않음)로 문 2의 `HJViewer-<v>-install.exe` HEAD가 200과 정책의 크기이고, `/api/hjv/u/0000-0000`이 링크 오류 JSON이고, nginx 기록에 문 2 코드가 없고(코드를 명령줄에 쓰지 않음. 예: `sudo sh -c "sed -n 's/^CODE=//p' /etc/hjviewer/door2.conf | grep -c -F -f - /var/log/nginx/*.log"`가 모두 0), 실제 회선의 내려받기가 약 3.8 MB/s이고, 다음 날 `auto_block_ips.sh`가 이 404를 세지 않는지 봅니다. 설치된 Windows PC가 무인 설치한 뒤 저널의 'update: windows <v> to a PC'와 처음 주소 notice, `operator.log`의 `cmd="update device-new-address"`, `admin report`의 update device-windows를 봅니다.
     - **사용자(열린 로그인, 실제 연세 SSO)**: 명단에 없는 교직원 ID로 1.6.1 앱에서 로그인하면 성공하고 `hjv-api admin allow list`에 'from login'으로 오릅니다(운영 에이전트는 '(n by a login)' 수와 'was registered' 줄 수만 봄). `admin block` 뒤에는 그 기기의 다음 계정 요청과 새 로그인이 거절되고 앱의 로그인 창에 '이 계정은 관리자가 HJViewer 사용을 막았습니다. …'가 그대로 보이며, `admin unblock` 뒤 다시 됩니다. 포탈 ID로 로그인한 뒤 저널에 'login: the portal ID typed and the pcode the SSO names differ' 줄(입력한 ID와 SSO가 알려 준 pcode가 다를 때 생김)이 있는지 보고 결과를 문서에 적습니다. 계정의 열쇠는 pcode가 아니라 로그인할 때 입력한 ID(앞뒤 공백을 지우고 소문자로 바꾼 것)입니다. pcode는 그 사람의 고정 ID로 계정에 함께 두고, 막기, 그 사람의 다른 계정 확인, 최고운영자 화면이 그것을 씁니다. 그래서 이 줄이 있으면 SSO가 같은 pcode로 알려 주는 다른 ID로 같은 사람이 로그인할 때 계정이 둘이 되어 기기와 PC 목록이 나뉩니다(어느 ID를 막아도 그 사람은 막힘). 운영 에이전트는 이 줄의 개수만, 사용자는 두 가지 ID로 로그인했는지를 메인 세션에 알립니다. 계정의 열쇠를 pcode로 바꿀지는 사용자가 정합니다. 맞는 비밀번호인데 앱에 '이 계정으로는 HJViewer를 쓸 수 없습니다. …'가 나오면, 운영 에이전트는 저널의 'login: the SSO accepted the password, but the person may not use HJViewer (…)' 줄의 괄호 안 사유(`user call: refused (access denied)`, `user call: a pcode outside HJViewer's ID rule`, 또는 `user call: data[0].name has a control character`처럼 `data[0].pcode`나 `data[0].name`의 값을 말하는 것)만 메인 세션에 알립니다. 차례로 행정허브 등록의 사용자 정보 호출이 거절한 것, 그 사람의 pcode가 Z·Y로 시작하지 않거나 ZA로 시작한 것, 그 사람의 SSO 자료(pcode나 성명)에 제어 문자가 있거나 너무 길거나 비어 있는 것입니다. 마지막 것은 그 사람만의 자료라 SSO 차단기에 세지 않습니다. `admin report`의 `login denied-user-refused`, `login denied-pcode-rule`, `login denied-bad-value`가 사유별 수입니다. 서로 다른 교직원이 모두 `user call: refused (access denied)`를 받으면 사람이 아니라 이 서버의 설정 문제이므로 7-5의 "모두가 거절되면"대로 합니다. 앱이 '로그인할 수 없습니다. …'만 내고 저널에 `login: SSO error: user call: …`(예: `data[0].pcode is not a string`) 줄이 있으면 답이 행정허브 계약과 다른 것이므로 그 줄만 알립니다. 교직원 포탈 ID가 아닌 형태(예: `test`)를 넣으면 SSO를 부르지 않고 바로 '연세대학교 교직원의 연세포탈 ID로만 로그인할 수 있습니다. …'가 보입니다(운영 에이전트는 `admin report`의 `login id-rule` 수만 봄).
     - **사용자(문 2 페이지)**: 운영 에이전트가 `door2.conf`를 둔 뒤(운영 서버는 2026-10-14 23:24, 8-4) `/api/hjv/get/<코드>/`를 휴대폰과 PC 브라우저로 열면 맨 위에 시범 서비스 안내 두 문장이 보여야 합니다(1.9.1의 hjv-api부터는 정식 서비스라 이 안내가 없습니다). iPhone Safari에서 'TestFlight 열기'(`itms-beta://`)가 열리는지, Android의 앱 안 브라우저(Custom Tab)에서 APK가 받아지는지(설계의 [불확실]) 봅니다.
     - **사용자(최고운영자)**: 배포 때 사용자가 서버의 `HJV_API_OPERATORS`에 자기 ID를 넣고(7-15. 저장소 파일에는 넣지 않음) 조용한 시간에 hjv-api를 다시 시작합니다. `admin operators`에서 그 줄의 account와 allowlist가 yes이고, 앱의 '최고운영자: 사용자와 PC'에 막히지 않은 사용자와 계정에 든 PC가 보이며, `admin block`한 계정은 사용자 목록에서 빠지고 그 계정의 PC는 주인의 포탈 ID·성명과 함께 PC 목록에 남는지 봅니다([privacy.md](privacy.md) 3절).
     - **운영 에이전트(hbbs 0005, 조용한 시간, 사용자가 8-3을 정한 뒤)**: 사용자의 `hjv-release hbbs-conf`가 쓴 `/etc/hjviewer/min-controller.conf`를 넣으면 hbbs 저널에 'min-controller 1.6.1 from <FROM>' 한 줄이 나옵니다. 첫 정책으로 만든 파일은 `FROM`이 이미 지났으므로, 넣는 즉시 1.5.1과 1.6.0은 TEXT 문구로 거절되고 1.6.1은 연결됩니다. 기록(2026-10-14): 사용자 결정으로 23:43(KST)에 넣었고, 10분 뒤 거절은 0이었습니다(8-3).
     - **운영 에이전트와 사용자(첫 실제 정책)**: 사용자가 2026-10-14에 실제 키로 서명한 첫 정책(세대 1, seq 1. 모든 항목의 최소 버전 1.6.1, 유예 없음, 7-10)을 8-2대로 `deploy/hjv-update-verify.sh`로 확인해 넣습니다. 같은 확인은 2026-10-14에 게시 디렉터리에서 이미 통과했습니다. 체크섬이나 SHA-256이 다르면 전송을 `incoming/refused-<버전>-<위치>-<UTC 시각>`으로 옮기고 1단계부터 다시 합니다. 넣은 뒤 사용자가 봅니다: 1.6.1 앱의 설정 → 정보에 "세대 1 · 순번 1"이 보이고 업데이트 안내가 없음, 1.6.0 앱(시험 설치본)은 업데이트 화면이고 내 기기 목록의 설치된 1.6.0 PC는 1.6.1을 강제로 설치함, 정책을 받은 1.6.1 PC는 1.6.1 미만의 컨트롤러(1.5.1, 1.6.0)를 거절함. 기록: 2026-10-14 23:30쯤(KST) 넣었고, `/api/hjv/version`이 그 파일을 내줍니다(SHA-256 `91f2f142…5be8db`, 7-10).
   - **(바) 저장소 밖의 설계 문서와 메모리 (메인 세션)**
     - **메인 세션**: 최소 버전 설계 8-1 표의 1단계(업데이트 키를 마지막 push 전에 만든다고 적음)를 런북과 distribution.md의 순서(첫 push와 서버 묶음, 깨끗한 컴퓨터에서 루트 키, 키 파일 push, 출시 빌드)로 고칩니다(보안).
     - **메인 세션**: 신뢰 규칙 명세의 R11과 3-4 끝의 'Not changed by these rules' 문장을 구현에 맞춥니다. 구현은 내려받은 exe의 FileVersion 앞 세 숫자를 정책의 latest와 비교합니다(패커는 x.y.z.0, package.build는 x.y.z.<실행 번호>). 명세 3-4에는 설치 직전 재질문의 시간 조건(답은 40초까지, 그 밖에는 30분 안에 확인된 정책)을 적습니다.
     - **메인 세션**: 최소 버전 설계 4-5: 출처 확인은 그 실행의 산출물을 직접 받아 패키지와 release.json을 대조하고, 초안은 산출물 보관 7일 안에 만든다고 적습니다.
     - **메인 세션**: 최소 버전 설계 4-4: 문 2 코드는 서버 파일 둘 말고 개발 PC의 `~/hjviewer-secrets/door2/` 사본에도 있다고 적습니다.
     - **메인 세션**: 최소 버전 설계 490행과 decisions.json WP-A1 범위: '코드는 SHA-256 앞 8자'를 '로그 키 HMAC 꼬리표의 앞 8자'로 고칩니다.
     - **메인 세션**: 최소 버전 설계 2-5, 2-6, 5-4의 Android 행을 결정 B'(모든 Android 빌드가 제어 전용)에 맞추고, 620행의 'Play 이전을 강제하지 않는 이유(원격 제어를 받는 Android는 사이드로드본)'를 고칩니다.
     - **메인 세션**: decisions.json WP-R1 범위의 '되돌리기(이전 정책 파일, MIN 비우기)'를 '언제나 seq가 더 높은 새 정책'으로 고칩니다.
     - **메인 세션**: Play 설계 2-11에 덮어쓰기 계약(`client/play/AndroidManifest.xml`과 워크플로의 PLAY_FORBIDDEN이 같은 항목)을 적습니다.
     - **메인 세션**: 신뢰 규칙의 남은 위험에 루트 키 의식의 도구를 공개 저장소에서 빌드하는 위험, 깨끗한 컴퓨터의 시계(5-0은 확인하라고만 함), 위 (나)의 저장 경합을 정한 결과를 적습니다.
     - **메인 세션**: 설계 3판 결정 JSON의 WP-W6b status('시험 전이면 push하지 않음')와 설계 11장(출시 태그만 막음)을 2026-10-14 결정(시험 전이면 배포만 막음, 7-9)에 맞춥니다.
     - **메인 세션**: 설계 3판 11장: 이미 고친 watchdog 줄을 지우고, '런북 7-1의 2번'을 push 관문인 3번으로 고칩니다.
     - **메인 세션**: 설계 3판 13-2: 열린 질문 INT-C 2, 같은 MachineGuid, nginx 주소 한도를 더합니다(지금은 이 런북 5번에만 있음).
     - **메인 세션**: 설계 3판 docs-late 목록: 4-2의 재시도 한 줄, 4-5의 `expired <시각>`과 bind-store, 7-6의 3분을 10분으로, 9-1의 암호화되지 않은 채널, 6-2·6-3·H16·8-5, X7b 문구.
     - **메인 세션**: 메모리 'hjviewer app versioning'의 'now 1.5.1'을 1.6.0으로 고칩니다.
   - **(사) 알려진 한계와 남긴 작은 문제 (기록. 고칠지는 메인 세션이 정하고, 사용자에게 알릴 것은 출시 안내에 넣음)**
     - **메인 세션(사용자에게 알림)**: 클라이언트 패치 0030, 0037, 0039~0043의 Known limits를 출시 안내와 함께 알립니다. 예를 들어 401을 받은 PC는 다시 로그인해도 6시간 기다림(0039), 정책을 기다리는 2초 동안 같은 PC로 가는 다른 연결은 버려짐(0040), 음성 통화는 앱이 화면에 있을 때만 녹음(0041), APK 서명 키를 바꾸려면 그 전 릴리스에서 확인을 고쳐야 하고 캐시가 지워지면 앱을 다시 켤 때까지 설치 화면을 열지 못함(0042), 열린 최고운영자 화면은 다음 계정 요청 때까지 그대로(0043)입니다.
     - **메인 세션(시험 공백)**: 문 1의 다시 보내기 고리(`src/hjviewer_update/door.rs`의 `send_by_login`)에 단위 시험이 없습니다.
     - **메인 세션(시험 공백)**: 업데이트 화면의 생존 변이 15, 17~20, 23(휴대폰 정보 타일 누름, 링크 두 번 요청, 닫힌 카드의 setState, 카드 겹침, 오래된 확인, 휴대폰의 resume)은 시험이 잡지 못합니다. 코드는 맞고, 23은 위 (라)의 기기 시험이 봅니다.
     - **메인 세션(CI)**: `deploy` 단계의 hjv-relay 묶음 시험(`relay/tests/package.rs --ignored`, 2개)과 `server` 단계의 hjv-api 합계에는 통과 수 하한이 없습니다.
     - **메인 세션(CI)**: `deploy`의 `check_site`는 짧은 링크 경로(`/api/hjv/u/…`)가 hjv-api의 404 JSON이고 nginx 기록에 코드가 남지 않는지는 보지 않습니다.
     - **메인 세션(E2E)**: 계정 단계의 `run` 도우미에는 호출별 시간 제한이 없어 멈춘 클라이언트가 e2e 단계를 붙잡을 수 있습니다(V1~V9는 30초 제한이 있음).
     - **메인 세션(문서)**: 8-4의 닫기 조건이 보라는 `hjv-api admin report`의 앱 버전별 PC는 계정에 묶인 PC만 세므로 1.5.x PC는 나오지 않습니다. 판단은 운영자의 PC 목록으로 합니다. distribution.md 9단계와 도구 README처럼 문장을 고칠지 정합니다.
     - **메인 세션(문서)**: 7-4의 `ssh <서버> …`, `scp … <서버>:…` 블록은 그대로 붙여 넣으면 셸이 `<서버>`를 입력 리디렉션으로 읽습니다(shellcheck SC2261). 자리표시를 바꾸고 붙여 넣는다는 줄을 둘지 정합니다.
     - **메인 세션(문서)**: 8-0과 8-2의 'went back' 줄은 hjv-api의 문구를 따릅니다. 그 문구가 바뀌면 함께 고칩니다.
     - **메인 세션(문서)**: `deploy/hjv-update-verify.sh`는 서명 키와 루트 키를 각각 둘까지 읽습니다. 키 수 규칙이 바뀌면 `read_keys`도 고칩니다.
     - **메인 세션(문서, 선택)**: [architecture.md](architecture.md) 2절의 hjv-api 행과 5절에 최고운영자 화면(`GET /api/hjv/admin/overview`)이 없습니다.
     - **메인 세션(운영 단계)**: 문 2 페이지의 iPhone 문구(TestFlight 초대 안내)는 바꾸지 않았습니다. 실제 초대 방식이 정해지면 `server/api/src/update.rs`의 페이지 문구를 맞춥니다.
     - **메인 세션(v3 클라이언트의 문구)**: 서비스 상태가 비었는데 서버 목록에는 있는 로그인된 PC의 설정 화면에 안내가 없습니다(fallback-pc-line-missing-when-listed). 시리즈를 다시 쓰지 않으려고 남겼습니다.
     - **메인 세션(v3 클라이언트의 문구)**: hjv-api가 하루 넘게 멈추면 거절 창과 배너의 문구('하루 동안 계속됩니다')가 사실과 달라집니다(fallback-outage-messages-after-24h). 같은 문장이 nginx·Caddy·Apache의 503 본문과 런북에도 있습니다.
     - **메인 세션(v3 클라이언트의 문구)**: 포탈 비밀번호를 바꿔도 이미 등록된 계정 키와 토큰이 남는다는 안내가 앱에 없습니다.
     - **메인 세션(v3 클라이언트의 문구)**: 새 기기 알림(newDeviceNote)의 문구를 다듬지 않았습니다.
     - **메인 세션(v3 클라이언트)**: 묶음을 풀 때 HKLM의 Use 값을 지우지 않습니다.
     - **메인 세션(v3의 시험 공백)**: 공유 벡터에 revoked·disabled 목록의 경우가 없습니다. 두 저장소의 벡터 JSON과 고정 SHA를 함께 바꿔야 합니다.
     - **메인 세션(v3의 시험 공백)**: 목록 규칙의 가장자리 단언 여섯을 더하지 않았습니다.
     - **메인 세션(v3의 시험 공백)**: `ctl.rs`의 SEEN_FOR 단언은 켠 지 1시간이 안 된 기계에서 건너뜁니다. 시계 주입이 필요합니다.
     - **메인 세션(v3의 시험 공백, 선택)**: 같은 모델의 둘째 랜카드와 상한 4·4·64 시험이 없습니다.
     - **메인 세션(v3의 시험 공백, 선택)**: 실제 대화 상자를 지나는 켜기 위젯 시험이 없습니다.
     - **메인 세션(v3의 시험 공백, 선택)**: hjv-api의 `parse_reply`를 실제 hbbs 답에 대는 교차 시험이 없습니다(벡터 드리프트 검사만 있음).
     - **메인 세션(v3의 경합)**: 관리자 터미널 표지의 경합은 시리즈의 코드를 바꿔야 해서 남겼습니다.
     - **메인 세션(사용자에게 알림, v3)**: 다른 계정으로 로그인한 뒤 도착한 옛 기기 목록은 그 계정의 목록이 올 때까지 남습니다(0030의 Known limits).

### 7-2. 읽기 점검 (3단계)

아무것도 바꾸지 않습니다. 마지막 `systemd-run` 줄만 임시 유닛 하나를 띄웠다가 지웁니다.

```bash
S="sudo -n"
echo "== sudo"; $S true 2>/dev/null && echo SUDO_OK || echo SUDO_NEEDS_PASSWORD
echo "sudo rules for every command: $($S -l 2>/dev/null | grep -cE '^[[:space:]]*\([^)]*\) ([A-Z_]+: )*NOPASSWD: ([A-Z_]+: )*ALL$')"
echo "== already there?"; ls -ld /run/hjviewer-api.sock /etc/systemd/system/hjviewer-api* /etc/hjviewer/hjviewer-api.env /var/lib/hjviewer-api /var/log/hjviewer-api /etc/nginx/conf.d/hjviewer-api-zones.conf /etc/nginx/snippets/hjviewer-api.conf 2>&1
echo "== hbbs, hbbr"; systemctl is-active hjviewer-hbbs hjviewer-hbbr; ls -l /opt/hjviewer/bin/
echo "== nginx"; $S nginx -t 2>&1 | tail -1
$S nginx -T 2>/dev/null | grep -nE '^# configuration file|hjviewer|zone=hjv_|real_ip_header|set_real_ip_from|proxy_intercept_errors|auth_basic|limit_req|limit_conn|limit_rate|modsecurity|^\s*(if|return|rewrite)\b'
grep -nE '^\s*include\s+(/etc/nginx/)?conf\.d/\*\.conf\s*;' /etc/nginx/nginx.conf
echo "== nginx workers"; ps -o user=,group= -C nginx | sort | uniq -c
echo "== php-fpm pools"; $S grep -hE '^\s*(user|group|listen\.group)\s*=' /etc/php/*/fpm/pool.d/*.conf 2>/dev/null | sort | uniq -c
echo "== www-data group members"; getent group www-data | cut -d: -f4 | tr , '\n' | grep -c .
echo "== DNS"; grep -E '^nameserver' /etc/resolv.conf
echo "== outbound without a proxy (000 = blocked)"; for u in https://github.com https://release-assets.githubusercontent.com https://objects.githubusercontent.com https://raw.githubusercontent.com https://api.telegram.org; do echo "$(curl --noproxy '*' -s -o /dev/null -w '%{http_code}' --max-time 10 "$u") $u"; done; echo "proxy variables: $(env | grep -ciE '^(https?|all)_proxy=')"
echo "== journald, credentials"; systemctl --version | head -1; ls /etc/systemd/journald*.conf* 2>/dev/null; systemd-creds has-tpm2 2>&1 | head -1; ls -d /dev/tpm* 2>/dev/null; $S ls -l /var/lib/systemd/credential.secret 2>&1
echo "== for the TPM choice (step 5)"; echo "virtualization $(systemd-detect-virt), firmware $([ -d /sys/firmware/efi ] && echo uefi || echo bios), $(mokutil --sb-state 2>/dev/null | head -1 | grep . || echo 'SecureBoot unknown'), Automatic-Reboot $(apt-config dump | sed -n 's/^Unattended-Upgrade::Automatic-Reboot "\(.*\)";$/\1/p' | tail -1 | grep . || echo unset)"
echo "== disk"; df -h / /var
echo "== timers and cron (names only)"; systemctl list-timers --all --no-legend | awk '{print $(NF-1)}' | sort -u; ls /etc/cron.d; $S sh -c 'grep -lE "nginx|auto_block|backup|rsync|restic|borg" /etc/crontab /etc/cron.d/* /etc/cron.daily/* /etc/cron.weekly/* /var/spool/cron/crontabs/* 2>/dev/null' | sed 's|^/var/spool/cron/crontabs/.*|a user crontab|'; command -v etckeeper; $S test -d /etc/.git && echo ETC_IS_GIT
echo "== IP filter: without, then with IPAddressDeny (the second should be 000)"
curl -sk -o /dev/null -w '%{http_code}\n' --max-time 5 https://127.0.0.1/
$S systemd-run --wait --pipe --quiet -p IPAddressDeny=127.0.0.1 curl -sk -o /dev/null -w '%{http_code}\n' --max-time 5 https://127.0.0.1/
```

판단:
- `SUDO_NEEDS_PASSWORD`면 2단계 1번처럼 멈춥니다.
- `sudo rules for every command`가 0이면 멈추고 사용자에게 알립니다(작업 동안 비밀번호 없는 전체 sudo를 받거나, 사용자가 sudo 명령을 직접 실행). 명령별 규칙만 있으면 `SUDO_OK`가 나와도 7-3, 8-1, 8-2의 `sudo install`, `sudo sh -c`, `sudo tee`가 거절될 수 있습니다.
- `ls -ld`가 하나라도 찾으면(이미 설치를 시작한 서버) 멈추고 어디까지 했는지 보고합니다.
- `nginx -t`가 이미 실패하면 멈춥니다.
- `real_ip_header`나 `set_real_ip_from`이 HJViewer 경로가 있는 server 블록이나 http 블록에 있고 넓은 범위(예: 0.0.0.0/0)를 믿으면 멈춥니다. hjv-api는 nginx가 `$remote_addr`로 채우는 `X-Real-IP`로 로그인 시도를 셉니다.
- `zone=hjv_` 이름을 다른 설정이 이미 쓰거나, nginx.conf가 http 블록에서 `conf.d/*.conf`를 읽지 않으면 멈춥니다(zones 파일을 둘 곳).
- nginx.conf가 `conf.d/*.conf`를 http 밖(main, stream, mail)에서도 읽으면 7-8 전에 보고합니다. 7-8의 zones 파일이 그 자리에서 `nginx -t`를 깹니다.
- HJViewer 경로가 든 server 블록이나 http 블록에 바로(location 밖에) `limit_req`·`limit_conn`·`limit_rate`·`modsecurity`가 있거나, 그 server 블록에 바로 `if`·`return`·`rewrite`가 있으면 보고합니다. 7-8 뒤 계정 경로에도 걸립니다. grep 줄만으로는 그 줄이 location 안인지 알 수 없습니다(WordPress 사이트에는 location 안의 `if`·`return`·`rewrite`가 흔함). 걸린 줄마다 `sudo nginx -T` 출력에서 그 줄 번호의 앞을 보고, location 밖인 줄만 셉니다.
- nginx 작업 프로세스의 그룹이 `www-data`가 아니면 보고합니다. 7-3에서 소켓 그룹을 드롭인으로 바꿉니다.
- PHP-FPM이 `www-data`로 돌면 보고합니다. 같은 서버의 PHP도 hjv-api 소켓을 부를 수 있다는 뜻입니다(키, 자격 증명, DB는 다른 사용자라 읽지 못함).
- `www-data` 그룹에 다른 계정이 들어 있으면(`== www-data group members`가 0이 아니면) 보고합니다. 소켓은 `root:www-data` 0660이라 그 계정의 프로세스도 hjv-api 소켓에 닿습니다. 그 계정에 이미 비밀번호 없는 전체 sudo가 있으면 새로 열리는 권한이 없으므로 그대로 둡니다. 아니면 5단계 전에 사용자와 정합니다(그 계정을 그룹에서 빼거나, 7-3의 소켓 드롭인에 `SocketUser=www-data`와 `SocketMode=0600`을 둠).
- 바깥 연결(프록시 없이): github.com이 000이거나, release-assets와 objects가 둘 다 000이면 멈춥니다(7-3, 7-6, 8-1의 내려받기). raw.githubusercontent.com만 000이면 진행하고, 8-1의 3번은 `incoming/`으로 받는 길로 합니다. api.telegram.org가 000이면 질문 12(경보 경로)를 정하기 전에 사용자에게 알립니다. `proxy variables`가 0이 아니면 보고합니다. hjv-api는 프록시 없이 SSO로 바로 나가므로, 셸의 프록시로 되는 연결은 증거가 되지 않습니다.
- systemd가 254보다 낮으면 멈추고 판을 보고합니다. 경보 유닛의 `ImportCredential=`은 254부터입니다.
- `has-tpm2`가 `yes`이거나 `/dev/tpm*`이 있으면 보고하고, 5단계 전에 사용자에게 알려 TPM 방식을 정합니다. `for the TPM choice` 줄(가상화, 펌웨어, Secure Boot 상태, 자동 재부팅)도 함께 알립니다. 기본은 TPM에는 묶되 PCR에는 묶지 않는 `--tpm2-pcrs=`이고, 사용자는 TPM을 쓰지 않는 `--with-key=host`를 고를 수도 있습니다(7-4 머리). `Automatic-Reboot`가 `true`이면(`unset`은 기본값 false) 업데이트 뒤 사람 없이 재부팅하므로, 자격 증명을 풀지 못하는 일이 생기면 사람이 없는 시간에 드러납니다.
- IP 필터의 두 줄: 첫 줄이 000이면 웹 서버가 127.0.0.1에서 받지 않는 것이니 판단하지 않고 보고합니다. 첫 줄은 000이 아닌데 둘째 줄도 000이 아니면, 이 커널에서는 systemd의 IP 필터가 효과가 없으므로 보고하고 7-3의 나가는 연결 제한(선택)을 넣지 않습니다.
- cron에 `nginx`나 `auto_block`이 든 작업이 보이면(`a user crontab` 포함) nginx를 다시 읽는 시각을 사용자에게 확인합니다. 7단계와 9단계는 그 시각을 피합니다(7-0).
- 백업이나 동기화 작업이 `/var/lib`나 `/etc`를 서버 밖으로 보내면 보고합니다. hjv-api의 DB와 자격 증명은 서버 밖 사본을 두지 않기로 했습니다(7-14). 사용자가 정한 운영자의 백업이 `/etc/hjviewer`나 `/var/lib/hjviewer`를 서버 밖으로 보내는 것은 그대로 둡니다(7-14의 "운영자의 서버 밖 백업"). 그 백업이 `/var/lib/hjviewer-api`, `/etc/credstore.encrypted`, `/var/lib/systemd/credential.secret` 가운데 하나라도 보내면 멈춥니다. `/etc/cron.daily/dpkg`와 `dpkg-db-backup.timer`는 Ubuntu 기본 작업(dpkg 상태를 서버 안의 `/var/backups`에 남김)이므로 보고하지 않습니다.
- etckeeper가 있거나 `ETC_IS_GIT`이 나오면 멈추고 사용자와 정합니다. `/etc/hjviewer`(문 2 코드, 최고운영자의 포탈 ID)와 `/etc/credstore.encrypted`가 `/etc/.git`에 커밋됩니다.

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
getent passwd hjviewer-api || sudo useradd --system --user-group --home-dir /var/lib/hjviewer-api --no-create-home --shell /usr/sbin/nologin hjviewer-api || { echo 'STOP: hjviewer-api 사용자를 만들지 못함'; exit 1; }
sudo install -m 755 bin/hjv-api /opt/hjviewer/bin/
sudo install -m 644 LICENSE.txt NOTICE.txt SOURCE.txt THIRD_PARTY_NOTICES.txt /opt/hjviewer/share/
sudo install -m 644 web/* /opt/hjviewer/share/web/    # 계정 API의 nginx 파일 둘, 그리고 새 저장소 주소의 사본
sudo install -m 600 systemd/hjviewer-api.env.example /etc/hjviewer/hjviewer-api.env   # root만 읽음(학교 주소 대역, 최고운영자의 포탈 ID)
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

설정 파일을 채웁니다. 비밀값(자격 증명)은 없지만 학교 주소 대역과, 7-15에서 최고운영자의 포탈 ID가 들어가므로 root만 읽습니다(600). 같은 서버의 다른 계정(WordPress의 PHP-FPM 등)은 읽지 못하고, systemd가 root로 읽어 서비스에 넘깁니다. 이 런북에서 이 파일을 읽고 고치는 명령은 모두 sudo이고, `sed -i`, `tee -a`, `sudoedit`는 권한을 그대로 둡니다. hbbs 공개키는 이 서버의 것을 넣고, SSO 주소와 client-id, 학교 주소 대역은 사용자에게 받습니다. SSO 값은 같은 서버의 WordPress SSO 로그인 플러그인(projecttf-auth)이 쓰는 것과 같은 등록입니다(결정 D2).

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

- 마지막 줄의 기본값을 바꾸지 않습니다: `HJV_API_BIND_OS=windows`, `HJV_API_SECOND_FACTOR=off`(결정 U1), `HJV_API_TRUST_REFRESH=120`(60~300), `HJV_API_TRUST_TTL=86400`(U4, 72시간 이하), `HJV_API_PASSWORDLESS_KINDS=remote,file,camera`(U3). 개발 PC의 `scripts/ci.sh deploy`는 이 가운데 `BIND_OS`, `SECOND_FACTOR`, `TRUST_REFRESH`, `TRUST_TTL` 넷을 묶음의 예시에서 검사하고, 이 서버의 설정 파일 사본으로도 검사할 수 있습니다(`HJV_API_ENV_FILE`, [ci.md](ci.md)). 사본에서는 `HJV_API_OPERATORS`와 `HJV_API_CAMPUS_NETS` 줄을 뺍니다. 포탈 ID와 학교 주소 대역이 개발 PC로 가지 않게 하기 위해서입니다: `sudo grep -vE '^[[:space:]]*HJV_API_(OPERATORS|CAMPUS_NETS)[[:space:]]*=' /etc/hjviewer/hjviewer-api.env > ~/hjviewer-api.env.copy`(운영 에이전트의 홈. 설치 블록이 `cd /`로 끝나므로 상대 경로를 쓰지 않음). 사본은 개발 PC로 옮긴 뒤 서버에서 지웁니다. `PASSWORDLESS_KINDS`는 검사하지 않으므로 이 출력으로 확인합니다.
- nginx 작업 프로세스의 그룹이 `www-data`가 아니면(7-2) 소켓 그룹을 드롭인으로 바꿉니다. `www-data`면 할 일이 없습니다. `sudo systemctl edit`은 편집기를 열어 운영 에이전트가 쓸 수 없고, 표준 입력을 받는 `systemctl edit --stdin`은 systemd 256부터입니다(Ubuntu 24.04는 255). 그래서 다음 줄로 둡니다.
  ```bash
  G='<그 그룹>'                      # ← 7-2의 nginx workers 줄에 나온 그룹
  getent group "$G" > /dev/null || { echo 'STOP: 그 그룹이 없음'; exit 1; }
  sudo install -d -m 755 /etc/systemd/system/hjviewer-api.socket.d && printf '%s\n' '[Socket]' "SocketGroup=$G" | sudo tee /etc/systemd/system/hjviewer-api.socket.d/group.conf > /dev/null && sudo systemctl daemon-reload
  ```
- (선택, 사용자 승인 뒤) 나가는 연결 제한: 7-2의 IP 필터 시험에서 둘째 줄만 000이었으면 `/etc/systemd/system/hjviewer-api.service.d/egress.conf`에 `[Service]`, `IPAddressDeny=any`, `IPAddressAllow=localhost <SSO 주소들> <DNS 서버 주소, 127.0.0.53이 아닐 때>`를 둡니다. `IPAddressAllow=`는 이름이 아니라 주소만 받습니다. SSO 주소들은 SSO 토큰 주소와 사용자 정보 주소의 호스트마다 `getent ahosts <호스트>`로 나온 모든 주소입니다(두 호스트가 다를 수 있고, 이름 하나가 주소 여럿을 가리킬 수 있음). 물을 때마다 주소가 바뀌어 나오는 이름이면 이 제한을 넣지 않습니다. SSO 서버의 주소가 바뀌면 로그인이 멈추므로, 넣었다는 것을 보고에 적습니다.

### 7-4. 키와 자격 증명 만들기 (5단계, 사용자만)

**멈춤.** 운영 에이전트는 이 단계를 하지 않고, 사용자가 끝났다고 알려 줄 때까지 기다립니다. 사용자는 에이전트 세션의 터미널이 아닌 곳에서 합니다. 개인키는 화면에 나오지 않고, 화면에는 공개키, 키 번호, 지문만 나옵니다.

이 절과 7-14, 7-15의 `systemd-creds encrypt`에는 모두 `--tpm2-pcrs=`(빈 값)를 붙여, TPM이 있으면 자격 증명을 TPM에는 묶되 PCR에는 묶지 않습니다. 기본값은 PCR 7(Secure Boot 상태와 키 목록)에도 묶어서, Secure Boot 키 목록 갱신이나 펌웨어 설정 변경 뒤 재부팅하면 자격 증명을 모두 풀지 못해 hjv-api가 시작하지 않고 서버에만 있는 목록 키 1번, 명령 키 1번, 기록 키를 잃기 때문입니다. TPM이 없는 서버에서는 달라지는 것이 없습니다. PCR에 묶지 않아도 TPM을 지우거나 바꾸면(펌웨어 초기화로 fTPM이 지워짐, VM을 옮기며 vTPM을 가져가지 않음) 자격 증명을 풀지 못합니다. 그래서 7-2에서 TPM이 보였으면 이 단계 전에 사용자가 방식을 정합니다. 이것도 피하려면 `--with-key=host`(호스트 키 `/var/lib/systemd/credential.secret`만 씀)를 골라, 이 절과 7-14, 7-15의 모든 `encrypt` 줄에서 `--tpm2-pcrs=` 자리에 씁니다. 그때는 서버의 디스크나 그 백업을 가진 사람이 자격 증명을 풀 수 있습니다. 따로 정하지 않으면 `--tpm2-pcrs=` 그대로입니다.

**(가) 오프라인 키**: 봉인 키, 목록 키 2번, 명령 키 2번. 서버가 없어져도 되살리거나 교체할 수 있게 서버 밖에 둡니다.

1. 에이전트가 돌지 않는 리눅스 환경(오프라인 PC나 Ubuntu 24.04 라이브 USB)에서, 7-3과 같은 서버 묶음(체크섬을 확인한 것)의 `bin/hjv-api`로 이동식 매체에 바로 만듭니다. AI 에이전트가 도는 개발 PC(WSL 포함)에서는 만들지 않습니다. 묶음의 실행 파일은 Ubuntu 24.04(glibc 2.39)에서 빌드하므로, 오프라인 PC도 Ubuntu 24.04 이상(또는 그 라이브 USB)을 씁니다. 22.04에서는 돌지 않을 수 있습니다.
   ```bash
   M='/media/<매체>'                                                 # ← 이동식 매체
   ./hjv-api keygen seal --out "$M/hjviewer-seal.key"                # 화면: 봉인 키 ID와 공개키(HJV_LOGIN_SEAL_PUBKEY)
   ./hjv-api keygen list --kid 2 --out "$M/hjviewer-list-2.key"      # 화면: 2:<base64>와 지문
   ./hjv-api keygen command --kid 2 --out "$M/hjviewer-command-2.key" # 원격 켜기용. 화면: 2:<base64>와 지문
   ```
2. 매체를 하나 더 만들어(파일 복사) 서로 다른 곳에 둡니다. 프로젝트 폴더, NAS의 부서 공유 폴더, 그 폴더로 가는 백업 대상에는 두지 않습니다.
3. 서버에는 지금 봉인 키만 넣습니다. 목록 키 2번과 명령 키 2번은 교체할 때 넣습니다(7-15). 서버 디스크에 평문이 남지 않게 표준 입력으로 넣습니다. 그 계정의 sudo가 비밀번호를 묻지 않으면 리눅스·macOS 셸에서 한 줄로 됩니다(PowerShell에는 입력 `<`가 없음).
   ```bash
   ssh <서버> 'sudo install -d -m 700 /etc/credstore.encrypted && sudo systemd-creds encrypt --tpm2-pcrs= --name=hjviewer-api-login-key - /etc/credstore.encrypted/hjviewer-api-login-key' < /media/<매체>/hjviewer-seal.key
   ```
   sudo가 비밀번호를 묻는 계정이면 위 줄은 터미널이 없어 실패합니다. 그때는 서버의 메모리 파일 시스템 `/dev/shm`을 잠깐 거칩니다. 디스크의 파일로는 남지 않지만 메모리가 모자라면 스왑에 쓰일 수 있으므로, 넣자마자 지웁니다. 아래 세 줄은 **Windows PowerShell이나 리눅스·macOS 셸에서만** 합니다(Windows에서는 `scp`의 원본만 `E:\hjviewer-seal.key`처럼 바꿈). 명령 프롬프트(cmd)는 작은따옴표를 따옴표로 보지 않고 `&&`와 `>`를 자기가 처리하므로, 첫 줄과 셋째 줄은 서버에서 실패하고 `scp`만 성공해 평문 키가 `/dev/shm`에 남습니다.
   ```bash
   ssh <서버> 'rm -f /dev/shm/hjv-key; umask 077; : > /dev/shm/hjv-key; stat -c %a:%U /dev/shm/hjv-key'
   scp /media/<매체>/hjviewer-seal.key <서버>:/dev/shm/hjv-key
   ssh -t <서버> 'sudo install -d -m 700 /etc/credstore.encrypted && sudo systemd-creds encrypt --tpm2-pcrs= --name=hjviewer-api-login-key /dev/shm/hjv-key /etc/credstore.encrypted/hjviewer-api-login-key; rm -f /dev/shm/hjv-key'
   ```
   - 첫 줄은 남아 있던 파일을 지우고 이 계정만 읽는 빈 파일을 새로 만듭니다. 출력이 `600:<그 계정 이름>`이 아니면 둘째 줄로 가지 않습니다. `scp`는 이미 있는 파일의 권한을 바꾸지 않고 내용만 씁니다.
   - 어느 줄이든 실패하거나 중간에 끊겼으면 `ssh <서버> 'rm -f /dev/shm/hjv-key'`로 지웁니다.

**(나) 서버에서 바로 만드는 것**: SSO 자격 증명, 기록 키, 목록 키 1번, 명령 키 1번, 출입증 키 1번. 사용자가 서버에 SSH로 들어가 합니다.

```bash
sudo -v
sudo install -d -m 700 /etc/credstore.encrypted
# SSO 자격 증명: WordPress SSO 로그인 플러그인(projecttf-auth)과 같은 값(D2)
systemd-ask-password -n "SSO client secret:" | sudo systemd-creds encrypt --tpm2-pcrs= --name=hjviewer-sso-client-secret - /etc/credstore.encrypted/hjviewer-sso-client-secret
systemd-ask-password -n "SSO api uuid:" | sudo systemd-creds encrypt --tpm2-pcrs= --name=hjviewer-sso-api-uuid - /etc/credstore.encrypted/hjviewer-sso-api-uuid
# 서버에만 있는 키(오프라인 사본 없음). 비밀은 keygen의 표준 출력에서 systemd-creds로 바로 감. 줄마다 sudo 하나, 어느 쪽이 실패해도 STOP
sudo bash -c 'set -o pipefail; /opt/hjviewer/bin/hjv-api keygen log-key | systemd-creds encrypt --tpm2-pcrs= --name=hjviewer-api-log-key - /etc/credstore.encrypted/hjviewer-api-log-key' || echo 'STOP: 기록 키를 넣지 못함'
sudo bash -c 'set -o pipefail; /opt/hjviewer/bin/hjv-api keygen list --kid 1 | systemd-creds encrypt --tpm2-pcrs= --name=hjviewer-api-list-key - /etc/credstore.encrypted/hjviewer-api-list-key' || echo 'STOP: 목록 키 1번을 넣지 못함'           # 화면: 1:<base64>와 지문
sudo bash -c 'set -o pipefail; /opt/hjviewer/bin/hjv-api keygen command --kid 1 | systemd-creds encrypt --tpm2-pcrs= --name=hjviewer-api-command-key - /etc/credstore.encrypted/hjviewer-api-command-key' || echo 'STOP: 명령 키 1번을 넣지 못함'  # 원격 켜기용. 화면: 1:<base64>와 지문
sudo bash -c 'set -o pipefail; /opt/hjviewer/bin/hjv-api keygen ticket --kid 1 | systemd-creds encrypt --tpm2-pcrs= --name=hjviewer-api-ticket-key - /etc/credstore.encrypted/hjviewer-api-ticket-key' || echo 'STOP: 출입증 키 1번을 넣지 못함'     # 교직원 전용 출입증용. 화면: 1:<base64>와 지문
sudo ls -l /etc/credstore.encrypted/
```

- SSO client secret은 토큰 요청의 `Authorization: Basic ` 뒤에 오는 값 그대로입니다. `-n`은 끝에 줄바꿈을 붙이지 않습니다.
- `keygen`의 `--encrypt-to`는 `systemd-creds encrypt`를 `--tpm2-pcrs=` 없이 부르므로 쓰지 않습니다. 비밀은 `keygen`의 표준 출력으로 나가 파이프의 `systemd-creds encrypt`에 바로 들어가고(표준 출력이 터미널이면 `keygen`이 거부함), 화면에는 표준 오류의 공개값만 나옵니다.
- 키 네 줄은 줄마다 `sudo` 하나로 돌고(두 `sudo`가 한꺼번에 비밀번호를 묻지 않음) `pipefail`을 켜므로, `keygen`과 `systemd-creds` 가운데 어느 쪽이 실패해도 `STOP:` 줄이 나옵니다. `keygen`은 비밀을 파이프에 쓴 뒤 공개값을 내므로 `systemd-creds`가 실패해도 공개키와 지문이 화면에 나오고, `keygen`이 실패하면 `systemd-creds`는 빈 입력으로 자격 증명을 만듭니다. 그래서 `STOP:`이 나오면 그 줄에서 나온 공개키와 지문은 버리고 그 줄만 다시 합니다(다시 하면 새 키이고, 자격 증명 파일도 새것으로 바뀜). 마지막의 `sudo ls -l`에 일곱 파일이 모두 있는지 봅니다.
- 일곱 자격 증명(SSO 둘, 봉인 키, 기록 키, 목록 키, 명령 키, 출입증 키)이 모두 있어야 6단계에서 서비스가 시작합니다. 유닛이 명령 키(`hjviewer-api-command-key`)와 출입증 키(`hjviewer-api-ticket-key`)도 요구합니다.
- `keygen list`와 `keygen command`는 화면에 공개키(`N:<base64>`)와 지문(`fingerprint: N:<SHA-256 16진수>`)을 냅니다. `keygen command`의 첫 줄은 `N:<base64>`뿐이라 `HJV_COMMAND_PUBKEY`의 항목으로 그대로 씁니다. 지문은 적어 두었다가 6단계의 `selfcheck`, 8단계의 빌드 요약과 대조합니다. `keygen ticket`도 같은 두 줄을 내고, 첫 줄이 그대로 `HJV_API_TICKET_PUBKEYS`의 값이자 hbbs의 `/etc/hjviewer/access.conf`의 `KEY=` 값입니다([access-tickets.md](access-tickets.md) 2-5). 앱은 출입증 키를 고정하지 않으므로 그 지문은 6단계의 `selfcheck`와만 대조합니다.
- 기록 키를 잃으면 새로 만듭니다(옛 기록의 꼬리표와 이어지지 않을 뿐). 목록 키 1번과 명령 키 1번을 잃으면 오프라인의 2번으로 바꿉니다(7-15). 출입증 키는 오프라인 사본을 두지 않습니다. 잃으면 서버에서 새로 만들고 `HJV_API_TICKET_PUBKEYS`와 `access.conf`의 `KEY=`를 함께 바꿉니다. 그때까지 나간 출입증은 모두 거절되고, 로그인한 앱과 묶인 PC가 새로 받습니다([access-tickets.md](access-tickets.md) 6-4).
- 최소 버전 작업의 업데이트 키(서명 키 1·2, 루트 키 101·102)는 여기서 만들지 않습니다. 앱 빌드가 공개 저장소의 `client/update-keys.env`에서 읽으므로, 사용자가 7-1의 4번(첫 push와 서버 묶음 `server-<버전>`) 뒤에 만들어 공개 줄을 그 파일에 넣고, 출시 빌드 전에 push합니다([distribution.md](distribution.md)의 "순서" 2단계. 루트 키는 그 릴리스 태그로 빌드한 도구로 깨끗한 컴퓨터에서, `tools/hjv-release/README.md` 1장). 서버에는 그 공개 파일만 둡니다(8-1의 3번).

**(다) 알려 줄 것**: 공개값만 운영 에이전트와 개발 PC에 알립니다. 모두 비밀이 아닙니다.

- `HJV_LOGIN_SEAL_PUBKEY`: 봉인 키의 공개키(base64 한 개)와 키 ID
- `HJV_TRUST_PUBKEYS`: `1:<base64>,2:<base64>`와 두 지문
- `HJV_COMMAND_PUBKEY`: `1:<base64>,2:<base64>`와 두 지문
- `HJV_API_TICKET_PUBKEYS`: 출입증 키의 `1:<base64>`와 지문. 운영 에이전트에게만 알립니다(앱 빌드에는 넣지 않음)

### 7-5. 공개키 대조값, 시작, selfcheck, 로그인 방식 (6단계)

```bash
TRUST='1:<base64>,2:<base64>'      # ← 사용자가 알려 준 HJV_TRUST_PUBKEYS
COMMAND='1:<base64>,2:<base64>'    # ← 사용자가 알려 준 HJV_COMMAND_PUBKEY
TICKET='1:<base64>'                # ← 사용자가 알려 준 HJV_API_TICKET_PUBKEYS(출입증 키)
E=/etc/hjviewer/hjviewer-api.env
sudo sed -i -e "s|^#\?HJV_API_TRUST_PUBKEYS=.*|HJV_API_TRUST_PUBKEYS=$TRUST|" \
  -e "s|^#\?HJV_API_COMMAND_PUBKEYS=.*|HJV_API_COMMAND_PUBKEYS=$COMMAND|" \
  -e "s|^#\?HJV_API_TICKET_PUBKEYS=.*|HJV_API_TICKET_PUBKEYS=$TICKET|" "$E"
[ "$(sudo grep -cE '^HJV_API_(TRUST|COMMAND|TICKET)_PUBKEYS=.+' "$E")" = 3 ] || { echo 'STOP: 공개키 대조값 세 줄이 설정 파일에 없음'; exit 1; }
sudo grep -E '^HJV_API_(TRUST|COMMAND|TICKET)_PUBKEYS=' "$E"
sudo systemctl enable --now hjviewer-api.socket hjviewer-api.service
sleep 3; systemctl is-active hjviewer-api.socket hjviewer-api.service      # active active
sudo stat -c '%a %U:%G %n' /run/hjviewer-api.sock /var/lib/hjviewer-api /var/lib/hjviewer-api/hjv-api.sqlite3 /var/log/hjviewer-api /var/log/hjviewer-api/operator.log
sudo chattr +a /var/log/hjviewer-api/operator.log && sudo lsattr /var/log/hjviewer-api/operator.log
A="sudo /opt/hjviewer/bin/hjv-api admin"
$A passwordless status | head -2   # 첫 줄에 off
$A selfcheck
sudo curl -s --unix-socket /run/hjviewer-api.sock http://localhost/api/hjv/source; echo
sudo journalctl --namespace=hjviewer -u hjviewer-api -p warning --since -10min -o cat | wc -l
```

- 소켓은 `660 root:www-data`(또는 7-3에서 바꾼 그룹), 상태 디렉터리와 로그 디렉터리는 `700 hjviewer-api`, DB와 `operator.log`는 `600`이어야 합니다. 로그 디렉터리를 root로 만들거나 나중에 `chown`·`chmod`하지 않습니다. 추가 전용 속성이 걸린 파일 때문에 systemd가 소유자를 고치다 실패해 서비스가 시작하지 않습니다([server/api/README.md](../server/api/README.md)의 "The operator record").
- `STOP: 공개키 대조값 …`이면 아무것도 시작하지 않은 채 멈춥니다. 설정 파일에 `HJV_API_TRUST_PUBKEYS=`, `HJV_API_COMMAND_PUBKEYS=`, `HJV_API_TICKET_PUBKEYS=` 가운데 없는 줄이 있는 것이니 묶음의 env 예시와 비교합니다.
- `selfcheck`는 SSO 주소의 DNS·TCP·TLS, hbbs 콘솔, 목록 키, 명령 키, 출입증 키를 봅니다. 키마다 두 줄입니다: 고정한 공개키의 지문(`list keys the apps pin …`, `command keys the relays pin …`, `ticket keys hbbs takes …`)과 쓰는 키가 고정한 그 번호의 키인지(`list key in use (HJV_API_LIST_KID 1): the pinned key 1, …`, `command key in use (HJV_API_COMMAND_KID 1): the pinned key 1, …`, `ticket key in use (HJV_API_TICKET_KID 1): the key 1 hbbs takes, …`). 7단계 전에는 `hbbs console` 줄 하나만 `[FAIL]`(패치 0004 전)이고 나머지는 `[ OK ]`여야 합니다. 7-6을 먼저 했으면(hjv-api를 시작하기 전에 hbbs를 0004로 바꿈) 그 줄도 `[ OK ] hbbs console: answers hjv-pk (patch 0004)`여야 하고, 7-6에서 건너뛴 확인이 이것입니다.
- 목록 키, 명령 키, 출입증 키 줄이 `[FAIL]`이면 멈춥니다. 고정한 목록 키가 다르면 모든 PC가 모든 목록을 거절하고, 명령 키가 다르면 모든 중계기가 켜기 명령을 거절하고, 출입증 키가 다르면 hbbs가 출입 확인을 켠 뒤 모든 연결과 PC 등록을 거절합니다. `HJV_API_COMMAND_PUBKEYS`나 `HJV_API_TICKET_PUBKEYS`가 비어 있어도 쓰는 키를 대조할 수 없어 `[FAIL]`입니다. 출입증 키 가운데 공개된 시험 키가 있어도 `[FAIL]`입니다(`… is a public test key …`).
- 지문 줄(공개값)은 사용자가 5단계에서 키를 만들 때 본 지문과 같아야 하고, 8단계의 대조에 쓰므로 목록 키와 명령 키 것을 모두 보고에 옮깁니다. 출입증 키의 지문도 보고에 옮깁니다.
- 서비스가 시작하지 않으면 `sudo journalctl --namespace=hjviewer -u hjviewer-api -n 20 -p err -o cat | grep -vE 'acct=|dev=|ip=|tag:'`의 오류 줄을 사용자에게만 보여 줍니다(7-0: 꼬리표가 든 줄은 거름. 그 밖의 줄이 필요하면 사용자가 서버에서 직접 봄). 설정 파일의 모르는 `HJV_API_` 이름, 빠진 SSO 값, 없는 자격 증명(명령 키와 출입증 키 포함, 7-4)이 흔한 원인입니다.

로그인 방식을 확인합니다. 설정 파일에 `HJV_API_LOGIN` 줄이 없거나 `open`이면(기본값, 2026-10-14 사용자 결정) 연세 SSO를 통과한 교직원은 누구나 로그인하고, 처음 로그인할 때 명단에 저절로 올라갑니다. 교직원만 되는 것은 연세 SSO가 아니라 hjv-api의 ID 규칙 때문입니다. 입력한 ID를 대문자로 바꿔 Z나 Y로 시작하고 ZA로 시작하지 않아야 SSO에 묻고, SSO가 알려 준 그 사람의 pcode도 같은 규칙에 맞아야 합니다(행정허브 플러그인은 Z와 B를 받음). 그래서 명단을 미리 넣을 필요가 없습니다. `selfcheck`의 마지막 줄이 `[ OK ] login (HJV_API_LOGIN): open, …`이고, `report`의 둘째 줄이 `login: open (…)`이어야 합니다.

```bash
sudo /opt/hjviewer/bin/hjv-api admin report | head -3
```

**모두가 거절되면**(시작한 뒤): 맞는 비밀번호로 로그인한 교직원이 '이 계정으로는 HJViewer를 쓸 수 없습니다. …'를 받고 저널의 사유가 `user call: refused (access denied)`인데(`report`의 `login denied-user-refused`) 서로 다른 사람이 잇달아 그러면, 행정허브 등록의 사용자 정보 호출이 사람이 아니라 이 서버의 요청을 거절하는 것입니다. 운영 에이전트는 값을 출력하지 않고 사용자에게 알리고, 사용자가 설정 파일의 `HJV_API_SSO_CLIENT_ID`(7-3)와 자격 증명 `hjviewer-sso-api-uuid`(7-4)가 WordPress SSO 로그인 플러그인(projecttf-auth)의 값과 같은지 확인합니다. 다르면 고치고(자격 증명은 7-15의 "SSO 자격 증명이 바뀔 때"처럼) `sudo systemctl restart hjviewer-api`를 합니다. 이 거절은 사람마다의 답으로 보므로 SSO 호출이 멈추지 않고 503 대신 모두가 이 문구를 받습니다. 그래서 서로 다른 계정 3개가 10분 안에 잇달아 거절되면(그 사이 사용자 정보 호출이 한 번도 받아들여지지 않음) 저널에 `login: the SSO's user call refused 3 different accounts in a row …` 한 줄(crit)이 나오고 5분 점검이 `sso_user` 경보를 냅니다(7-13, 7-14).

명단 방식(`HJV_API_LOGIN=list`, 명단에 있는 ID만 로그인)으로 시작하기로 사용자가 정했을 때만 명단을 넣습니다. 사용자가 포탈 ID를 한 줄에 하나씩 적은 파일을 서버에 두고(예: `/root/hjviewer-allow.txt`, 600) `sudo /opt/hjviewer/bin/hjv-api admin allow import /root/hjviewer-allow.txt`로 가져옵니다(7-0: 사람의 ID를 넣는 명령은 사용자가 함). 운영 에이전트는 그 파일을 열지 않고, 그 뒤 `sudo /opt/hjviewer/bin/hjv-api admin allow list | head -1`의 개수만 봅니다. 가져온 뒤 파일은 사용자가 지웁니다. 명단은 DB에 있습니다.

감시와 백업 타이머를 켭니다(7-14).

```bash
for t in /etc/systemd/system/hjviewer-api-*.timer; do [ -e "$t" ] && sudo systemctl enable --now "$(basename "$t")"; done
systemctl list-timers 'hjviewer-api-*' --no-legend
```

### 7-6. hbbs 0004로 교체 (7단계)

조용한 시간에 합니다. hbbs만 다시 시작하므로 PC는 몇 초 안에 다시 등록되지만, 그때 진행 중인 원격 세션은 끊깁니다. 최소 버전 작업의 hbbs 0005가 함께 나가면 같은 실행 파일에 들어 있으므로 다시 시작은 한 번입니다. 그 전에 8-1의 1, 2, 4번(특히 `hjviewer.env`의 `HJV_MIN_CONTROLLER_FILE` 줄)을 해 두면 이 다시 시작으로 함께 적용됩니다. 8-1의 3번은 출시 태그 뒤에 합니다.

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
sudo journalctl -q -u hjviewer-hbbs --since -2min -o cat | grep -E 'ERROR|panicked' | tail -5   # config.rs 두 줄 말고는 없어야 함(아래)
sudo /opt/hjviewer/bin/hjv-api admin selfcheck | grep 'hbbs console'   # 7-5 전(hjv-api가 아직 돌지 않음)에는 실패함: 건너뛰고 7-5에서 봄
cd / && rm -rf "$T"
```

- 지금 운영 중인 서버의 유닛은 `server-2026.10.10` 묶음의 것이고, 1.6.0 묶음의 유닛과는 `Documentation=` 줄(저장소를 옮기기 전의 주소라 지금은 404)만 다릅니다. 그래서 비교에서 그 줄을 빼고, 같으면 두 유닛을 새것으로 바꿔 링크도 고칩니다. hbbr은 다시 시작하지 않습니다. 바뀐 것이 설명 줄뿐이라 `daemon-reload`로 충분합니다.
- 이 절은 hbbr의 실행 파일을 바꾸지 않습니다(유닛은 위처럼 설명 줄만 바뀜). 설치된 hbbr은 지금(2026-10-15)도 `server-2026.10.10`의 것입니다. 서버 패치 0001–0003은 그 뒤 바뀌지 않았고(패치 머리의 `[PATCH n/m]` 번호만 바뀜), 0004와 0005는 hbbs의 파일만 고치기 때문입니다. hbbr은 나중의 B 경로 업데이트(6절)로만 바꿉니다.
- `STOP: hbbs 유닛도 바뀜`(또는 `hbbr`)이면 아무것도 바꾸지 않은 채 멈추고, `diff`가 보인 차이를 보고합니다. 그 차이는 "B 경로 업데이트"처럼 사용자 승인을 받아 따로 반영합니다.
- `grep -E 'ERROR|panicked'` 줄에는 아래 둘 말고 아무것도 없어야 합니다. 다른 줄이 있으면 멈추고 보고합니다.
  - 다시 시작할 때마다 나오는 hbb_common의 클라이언트 설정 줄은 해가 없으므로 세지 않습니다: ERROR `config.rs:566` `Failed to load config '<홈>/.config/rustdesk/RustDesk.toml'`과 ERROR `config.rs:605` `Failed to store config: …`, 그리고 같은 때의 WARN `ID is invalid, generating new one`과 INFO `Generated id …` 한 쌍(grep에는 나오지 않음)입니다. upstream hbbs는 시작할 때 RustDesk 클라이언트의 설정 파일을 한 번 읽는데, 지금 운영 서버처럼 hbbs 사용자의 홈에 있는 `~/.config/rustdesk` 디렉터리에 실행 권한(x)이 없으면(600) 그 파일을 열지도 쓰지도 못해 이 줄들이 납니다. hbbs의 서버 기능은 그 파일을 쓰지 않으므로 고칠 것이 없습니다.
- `hbbs console: answers hjv-pk (patch 0004)`가 나와야 합니다. 이 확인은 hjv-api의 관리 소켓으로 하므로, hjv-api를 아직 시작하지 않았으면(7-5 전에 이 절을 하는 경우) 실패합니다. 그때는 이 줄을 건너뛰고 7-5의 `selfcheck`에서 `hbbs console` 줄을 봅니다. 5절의 첫 블록을 다시 실행해 `/ws/id`, `/ws/relay`가 101인지도 봅니다. 사용자는 바깥에서 `hjv-probe`로 확인합니다.
- `/etc/hjviewer/min-controller.conf`가 이미 있는 서버(8-3의 파일을 넣은 뒤)에서는 hbbs의 `min-controller` 줄도 봅니다(8-3의 "hbbs를 다시 시작한 뒤" 블록). `min-controller <MIN> from …`(또는 `min-controller off: …`)여야 하고, ERROR(`… no minimum is in force`)면 hbbs가 최소 버전 없이 도는 것이니 멈추고 보고합니다.
- 되돌리기: 보관한 `/root/hjv-backup/hbbs-before-<VERSION>`을 같은 방법으로 넣고 hbbs만 다시 시작합니다(7-12).

### 7-7. 앱과 중계기 빌드, 지문 대조 (8단계, 개발 PC)

서버에서 할 일은 `sudo /opt/hjviewer/bin/hjv-api admin selfcheck`의 지문 줄을 다시 보고하는 것뿐입니다. 나머지는 개발 PC와 사용자가 합니다([ci.md](ci.md)의 "계정 기능의 공개키", "hjv-relay 빌드").

1.6.0의 첫 출시 빌드(2026-10-14)는 Google Play가 최소 SDK 22인 번들을 받지 않아(자동 보호의 요건) 1.6.1로 다시 빌드했습니다. 이 절과 뒤의 절에서 기능을 말하는 '1.6.0'은 그 기능이 처음 들어간 판입니다([distribution.md](distribution.md)의 "순서").

출시 태그는 `client-1.6.1`과 `relay-1.6.0`이고, 두 태그의 커밋이 다릅니다.

- `client-1.6.1`은 아래 2번의 출시 빌드 커밋 992fefe(1.6.1의 출시 실행 38568720786)에 있습니다. 2026-10-14 22:47(KST)에 push했습니다(4번).
- `relay-1.6.0`은 8622975에 있습니다. 빌드 저장소의 Release `relay-1.6.0`('hjv-relay 1.6.0')을 만든 커밋입니다(1.6.0의 출시 실행 38559428611). 중계기의 판은 `relay/Cargo.toml`의 것이고 1.6.1에서 바뀌지 않았습니다. 그래서 1.6.1 실행의 Release 이름도 `relay-1.6.0`이었고, 그 Release가 이미 있어 건드리지 않고 자기 중계기 빌드를 아티팩트에만 남겼습니다([ci.md](ci.md)의 "hjv-relay 빌드" 3번).
- 1.6.1 실행의 중계기 아티팩트는 나눠 주지 않습니다. 그 안의 `README-DSM.md`는 빌드한 커밋의 relay-dsm.md, 곧 b3f58bb가 잘못 고친 판이라서 `hjviewer-relay-1.6.1` 파일 이름을 적기 때문입니다. hjv-relay는 빌드한 커밋을 실행 파일에 넣으므로, NAS의 작업 화면에 적는 SHA-256도 그 Release의 것입니다.
- 기록(2026-10-15, 7-11의 1번): 주석 태그 `relay-1.6.0`(메모에 묶음과 hjv-relay의 SHA-256)을 0시 5분쯤(KST) 8622975에 붙여 push했습니다. 실행 요약의 명령 키 지문이 7-5의 `selfcheck`와 같은 것을 본 뒤, 0시 8분에 빌드 저장소의 Release 'hjv-relay 1.6.0'을 게시했습니다(비공개 저장소).

1. 비공개 빌드 저장소의 시크릿에 `HJV_LOGIN_SEAL_PUBKEY`, `HJV_TRUST_PUBKEYS`, `HJV_COMMAND_PUBKEY`를 넣습니다. 공개값입니다. 최소 버전 작업의 업데이트 키는 시크릿이 아니라 빌드하는 커밋의 `client/update-keys.env`에서 읽습니다([ci.md](ci.md)).
2. 2단계의 커밋으로 Windows(`HJViewer.exe`), Android, iOS, hjv-relay를 **`testflight=false`**로 빌드합니다. Windows·iOS 전용 코드는 2단계의 확인 빌드에서 이미 컴파일해 본 것이고(7-1), 여기서는 공개키를 넣은 출시 빌드를 만듭니다. 실패하면 개발 PC에서 고치고 다시 push한 커밋으로 빌드합니다. 서버 코드가 바뀌었으면 서버 묶음과 3~7단계의 해당 부분도 다시 합니다.
3. 빌드 요약의 고정 공개키 지문이 `selfcheck`의 지문, 그리고 5단계에서 사용자가 본 지문과 같은지 사람이 대조합니다. 봉인 키 ID는 지문의 앞 16자리입니다.
4. 같으면 2번의 커밋에 출시 태그 `client-1.6.1`을 붙입니다. 아직 나눠 주지 않습니다(TestFlight 업로드는 9단계 뒤). 7-1의 5번의 현장 시험 결과와 TCP 펀치 시험은 처음에는 이 태그 전에 끝낼 것이었지만, 사용자 결정("지금 태그, WOL은 뒤에")으로 태그가 아니라 배포(7-9)를 막습니다. 태그는 2026-10-14에 그 시험들보다 먼저 992fefe에 붙였습니다(메모에 패키지의 SHA-256). 9단계의 nginx 전환도 사용자 결정으로 그 전(같은 날 21:45)에 했습니다. `relay-1.6.0`은 2026-10-15에 붙였습니다(위 목록의 기록).

### 7-8. nginx 전환 (9단계)

**관문**: 다음이 모두 끝났는지 사용자에게 확인합니다. 하나라도 아니면 멈춥니다.
- 질문 7의 답(개인정보 처리자와 연락처, 정보보호팀의 회신: 공공기관 개인정보파일 등록, AI 운영 에이전트가 처리위탁·국외 이전에 해당하는지)을 반영한 [privacy.md](privacy.md)가 공개 저장소에 있음. 2단계의 push에 이미 들어갔으면 됩니다. 답이 그 뒤에 왔으면 privacy.md의 처리 근거, 처리자, 연락처(7, 9, 10절, 두 언어)를 고쳐 문서만 한 번 더 push합니다(`scripts/ci.sh leak` 통과 뒤. 앱과 서버 묶음은 그대로).
- 사용자 약 10명에게 무엇이 저장되는지 미리 알림.
- 날짜: 같은 서버의 다른 서비스 일정과 겹치지 않는 조용한 저녁(질문 9). cron의 nginx 재적재 시각은 피함.

절차:

1. 같은 443 포트의 모든 사이트의 지금 응답 코드를 기록합니다. 4-3 (나)의 2번 블록과 같습니다(`/tmp/hjv-sites-before.txt`).
2. 설치하고 검사합니다. 실패하면 되돌리고 멈춥니다.
   ```bash
   L=/etc/nginx/snippets/hjviewer-locations.conf W=/opt/hjviewer/share/web
   [ "$(grep -c '^location \^~ /api/ {' "$L")" = 1 ] || { echo 'STOP: 경로 파일의 /api/ 블록이 예상과 다름'; exit 1; }
   grep -q '^location = /api/hjv/version {' "$W/nginx-hjviewer-locations.conf" || { echo 'STOP: 묶음의 경로 파일에 버전 정책 위치가 없음(1.6.0 묶음이 아님)'; exit 1; }
   sudo install -d -m 700 /root/hjv-backup && sudo cp -p "$L" /root/hjv-backup/hjviewer-locations.conf.before-api
   sudo install -m 644 /opt/hjviewer/share/web/nginx-hjviewer-api-zones.conf /etc/nginx/conf.d/hjviewer-api-zones.conf
   sudo install -m 644 /opt/hjviewer/share/web/nginx-hjviewer-api.conf /etc/nginx/snippets/hjviewer-api.conf
   N=$(mktemp)
   awk '/^location \^~ \/api\/ \{/ { print "include snippets/hjviewer-api.conf;"; skip = 1; next }
     skip { if ($0 ~ /^\}/) skip = 0; next } { print }' "$W/nginx-hjviewer-locations.conf" > "$N"
   sudo install -m 644 "$N" "$L" && rm -f "$N"
   sudo diff -u /root/hjv-backup/hjviewer-locations.conf.before-api "$L"   # /api/ 블록이 include 한 줄로 바뀌고, 버전 정책 위치(8-1의 5번)가 더해지고, /api/ 블록 위 주석 두 줄이 include 줄 위로 옮겨진 것만
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
   echo "version        $(c "https://$DOMAIN/api/hjv/version")"         # 404 {"error":"no version policy"} (정책 파일 전, 8-1의 5번)
   echo "x.php          $(c "https://$DOMAIN/api/x.php")"               # 404 JSON (사이트의 PHP로 가지 않음)
   echo "peers          $(c "https://$DOMAIN/api/peers")"               # 401 (토큰 없음)
   while read -r name before; do
     after=$(curl -s -o /dev/null -w '%{http_code}' --max-time 10 --resolve "$name:443:127.0.0.1" "https://$name/")
     echo "$name $before -> $after"; [ "$before" = "$after" ] || echo "  DIFFERENT"
   done < /tmp/hjv-sites-before.txt
   ```
   5절의 첫 블록으로 `/ws/id`, `/ws/relay`가 101인지도 봅니다.
4. 1.5.1 앱은 이 전환의 영향을 받지 않습니다. 로그인하지 않으므로 계정 경로를 부르지 않습니다.
5. 최소 버전 작업의 nginx 위치(`location = /api/hjv/version`)는 2번이 묶음 파일로 만든 스니펫에 들어 있어 같은 reload로 들어갑니다. 2번의 `diff`가 그 위치 말고 다른 줄의 차이도 보이면(이 서버에서 손으로 고친 줄) 멈추고 보고합니다. `/api/` 블록 위 주석 두 줄(`# The apps call https://<domain>/api/...`로 시작)의 이동은 다른 줄로 보지 않습니다. diff는 그 두 줄을 지웠다가 include 줄 위에 다시 넣은 것으로 보입니다.

### 7-9. 배포와 켜기 (10~13단계)

**관문**: 다른 사용자에게 나눠 주기 전에(10단계의 Windows·Android 산출물, TestFlight와 Google Play 내부 테스트에 다른 사람을 더하는 것, 13단계) 7-1의 5번의 현장 시험 결과와 TCP 펀치 시험이 끝났는지 사용자에게 확인합니다. 7-1의 6번 (라)에서 필수로 적은 두 기기 시험(0042의 Android 앱 안 설치, 0038의 화면 바꾸기 거절)도 이 관문에 듭니다. 하나라도 아니면 나눠 주지 않습니다. 현장 시험과 TCP 펀치 시험은 처음에는 출시 태그(8단계)를 막았지만, 사용자 결정으로 태그를 2026-10-14에 먼저 붙이고 이 관문으로 옮겼습니다(7-7의 4번).

- 기록(2026-10-15): 현장 시험은 끝났습니다(운영자 PC는 켜지지 않음, 신호는 PC의 랜카드까지 닿음, 사용자 결정으로 원격 켜기는 그대로 둠, 7-1의 5번). TCP 펀치 시험은 근거로 갈음했습니다. 두 기기 시험(0038, 0042)은 사용자가 완료로 정했습니다(2026-10-15). 이 관문은 끝났습니다. 사용자 안내를 보내고 다른 테스터를 더하는 일(13단계)은 사용자가 합니다. Google Play 첫 출시와 루트 키 리허설은 사용자 결정으로 보류합니다(2026-10-15).
- 기록(2026-10-14): 사용자 결정으로 문 2(23:24, 8-4)와 hbbs 파일(23:43, 8-3)을 이 관문 전에, 사용자 안내 없이 넣었습니다. 그래서 서버를 거치는 1.5.x 컨트롤러는 그때부터 거절되고, 거절 문구(S1)의 문 2 링크로 1.6.1의 Windows·Android 패키지를 받을 수 있습니다. 안내를 보내고 테스터를 더하는 일은 여전히 이 관문 뒤입니다.

1. **배포(10단계, 개발 PC)**: 7-7의 커밋으로 iOS 잡을 `testflight=true`로 다시 돌려 TestFlight 내부 그룹에 올립니다(빌드 번호만 바뀜). 1.6.1은 2026-10-14에 같은 커밋의 iOS 실행 38574804439로 올렸습니다(TestFlight 빌드 262871353. 출시 실행의 iOS 산출물은 빌드 262871309). 1.6.2(아이폰 한글 입력, 클라이언트 패치 0049)는 iOS만 2026-10-15에 434c091의 iOS 실행 38679181196으로 올렸고(TestFlight 빌드 262880406), 사용자가 아이폰에서 한글 입력을 확인한 뒤 태그 `client-1.6.2`를 붙였습니다. Windows·Android는 1.6.1 그대로입니다. 정책 seq 2(iOS 최신 1.6.2, 위치 `e1-s2`)는 사용자가 서명했고, 2026-10-15 16:19(KST)에 운영 에이전트가 사용자 승인으로 8-2대로 넣었습니다. `/api/hjv/version`의 SHA-256은 `6cfea642…8c7c`입니다. 1.7.0(새 화면, 클라이언트 패치 0050–0057)은 2026-10-15에 4c3baaf의 출시 실행 38724716986으로 세 플랫폼을 빌드했고, 같은 실행이 iOS를 TestFlight에 올렸습니다(빌드 262881109). 사용자가 기기에서 확인한 뒤 태그 `client-1.7.0`을 붙였습니다. 정책 seq 3(세 플랫폼 최신 1.7.0, 최소 1.6.1 그대로, Windows 무인 설치 2026-10-17 22:13(KST)부터, 위치 `e1-s3`)은 사용자가 서명했고, 2026-10-16 07:31(KST)에 운영 에이전트가 사용자 승인으로 8-2대로 넣었습니다. `/api/hjv/version`의 SHA-256은 `e13116cc…8c90`입니다. 넣은 뒤 운영자 PC를 먼저 [지금 업데이트]로 올립니다([distribution.md](distribution.md)의 '운영자: 새 버전을 낼 때' 7번). 2026-10-16 08:38에 그렇게 올렸습니다(1.6.1+5 → 1.7.0+8, 서비스는 자동 시작으로 실행 중, 앱의 정책은 순번 3). Windows·Android 산출물을 나눠 줍니다(Google Play 내부 테스트는 그 작업의 문서대로). nginx 전환 뒤에 올리는 이유는 1.6.0 아이폰이 전환 전의 `/api` 404를 받지 않게 하려는 것입니다.
2. **운영자 PC(11단계, 사용자)**: 운영자의 Windows PC에 1.6.1(`HJViewer-1.6.1-install.exe`)을 1.5.1 위에 설치하고 로그인합니다. 1.6.0 설치본은 첫 정책의 최소 버전 아래입니다(7-10). 알림에 "관리자가 이 기능을 켜면 바로 쓸 수 있습니다"가 나오고, 설정 → 계정에 "관리자가 일시 중지함"이 보여야 합니다. PC 비밀번호(영구 비밀번호)가 정해져 있는지 보고, 아이폰에서도 로그인합니다.
3. **운영자 계정부터 켜기(12단계, 사용자, 7-0)**: 사용자가 서버에서 운영자 PC의 ID로 그 계정만 켭니다. PC ID는 운영 에이전트에게 알리지 않습니다.
   ```bash
   A="sudo /opt/hjviewer/bin/hjv-api admin"
   $A passwordless on --owner-of 123456789     # ← 운영자 PC의 ID
   $A passwordless status
   ```
   운영 에이전트는 그 뒤 `sudo /opt/hjviewer/bin/hjv-api admin passwordless status | head -2`만 봅니다(둘째 줄 `accounts switched one by one: 1`). 5분 안에 그 PC의 설정 줄이 "비밀번호 없이 연결 켜짐"으로 바뀌어야 합니다. 일주일 동안 실기기 점검(설계 3판 7-6: 재부팅 뒤 로그온 화면, 다른 Windows 사용자, 로그아웃, 다시 설치와 제거, 업데이트 뒤 로그인 유지, 아이폰의 Keychain과 복원 안내 등)을 하며 쓴 뒤, 사용자 승인을 받아 모두에게 켭니다.
   ```bash
   sudo /opt/hjviewer/bin/hjv-api admin passwordless on
   sudo /opt/hjviewer/bin/hjv-api admin report | head -8
   ```
4. **다른 사용자(13단계)**: 각자 PC에 설치하고 로그인합니다. 명단에 미리 넣을 필요는 없습니다(첫 로그인 때 저절로 올라감, 7-15). 아이폰 사용자는 먼저 App Store Connect 사용자로 초대해 TestFlight 내부 그룹에 넣어야 합니다([distribution.md](distribution.md)의 "아이폰 사용자 추가").

### 7-10. 최소 버전 강제 (14단계)

먼저 8-1의 3번(업데이트 공개키 파일)을 합니다. 출시 태그(8단계) 뒤라야 받을 수 있고, 첫 정책(8-2)의 서버 확인이 그 파일을 읽습니다. 2026-10-14에 `client-1.6.1`의 파일로 했습니다. 그 뒤 [8절](#8-최소-버전과-업데이트-배포-160)을 따릅니다.

- **첫 정책(세대 1, `seq` 1)**: 사용자가 2026-10-14에 서명했습니다. 모든 항목의 최소 버전이 1.6.1이고 유예가 없습니다(사용자 결정. 만든 명령은 도구 README 2장의 "첫 정책"). 서버에 넣는 즉시(8-2) 정책을 받은 1.6.x PC는 1.6.1 미만의 컨트롤러(1.5.x와 1.6.0)를 거절합니다. 1.6.0 앱은 나가는 연결이 막히고 업데이트 화면을 보이며, 내 기기 목록의 설치된 1.6.0 PC는 1.6.1을 강제로 설치합니다(정책에 무인 설치 시각이 없어도). 1.5.1 앱과 PC는 정책을 읽지 않으므로 그대로입니다. Android 5.1~6.0 휴대폰은 1.6.1을 설치할 수 없어 1.5.1에 남고, 1.6.x PC에는 연결할 수 없습니다. 기록: 허브-운영이 2026-10-14 23:30쯤(KST) 8-2로 넣었고, `/api/hjv/version`이 그 파일을 내줍니다(SHA-256 `91f2f142…5be8db`).
- **따로 강제하는 정책은 없습니다.** 옛 계획의 처음 강제(정책 `seq` 2와 hbbs 파일) 가운데 남은 것은 hbbs의 최소 버전 파일(8-3)뿐입니다. 이 파일은 넣는 즉시 서버를 거치는 1.5.x 컨트롤러를 모두 거절하므로 실제로는 배포 단계입니다. 넣을지와 때는 사용자가 따로 정하고, 보통은 7-9의 관문과 날짜를 적은 사용자 안내 뒤에 넣습니다. 문 2(8-4)도 따로 정합니다.
- 기록(2026-10-14, 사용자 결정): 둘 다 7-9의 관문 전에, 사용자 안내 없이 했습니다. 문 2는 23:24(KST)에 열었고, 2026-11-30 0시(KST)까지 열려 있습니다(8-4). hbbs 파일은 23:43에 넣었습니다(8-3, `MIN=1.6.1`, `FROM`=정책의 발행 시각 `2026-10-14T14:06:02Z`, 문 2 링크가 든 S1 문구). 그때부터 서버를 거치는 1.5.x 컨트롤러는 모두 거절되고, Android 5.1~6.0 휴대폰은 서버를 거쳐 어느 기기에도 연결할 수 없습니다.

### 7-11. NAS 중계기 승인과 켜기 (15단계)

nginx는 9단계에서 이미 준비됐습니다(중계기 등록 한도의 zone 포함).

1. (게시와 태그는 2026-10-15에 함, 아래 기록. 이 단계는 NAS 설치부터입니다.) 먼저 개발 PC가(사용자 승인) 1.6.0 출시 실행(38559428611) 요약의 명령 키 지문을 `hjv-api selfcheck`의 지문과 대조한 뒤, 빌드 저장소의 초안 Release `relay-1.6.0`을 게시하고 공개 저장소의 8622975에 태그 `relay-1.6.0`을 붙입니다([ci.md](ci.md)의 "hjv-relay 빌드" 4번, 7-7). 1.6.1 실행의 중계기 아티팩트는 쓰지 않습니다. 그다음 사용자가 NAS에 hjv-relay를 설치합니다([relay-dsm.md](relay-dsm.md): File Station으로 올리고, 작업 화면에 SHA-256을 적음). NAS의 관리자 계정은 에이전트에게 주지 않습니다.
   - 그 Release의 `SHA256SUMS`는 두 줄입니다: 묶음 `hjviewer-relay-1.6.0-linux-x86_64.tar.gz`와 그 안의 `hjviewer-relay-1.6.0-linux-x86_64/hjv-relay`. NAS의 작업 화면에는 `…/hjv-relay` 줄의 값을 적습니다([relay-dsm.md](relay-dsm.md) 3장).
   - 기록(2026-10-15): 태그는 0시 5분쯤(KST) push했고, 지문이 7-5의 `selfcheck`와 같은 것을 본 뒤 0시 8분에 Release 'hjv-relay 1.6.0'을 게시했습니다(7-7). `SHA256SUMS`의 두 값은 묶음 `f7b3cd96…125c`, hjv-relay `abc519c2…aa57`(NAS 부트업 작업의 첫 줄에 적는 값)입니다. 그 NAS는 사용자 자신의 부서 것이라 부서의 허락(7-1의 5번, 질문 14)은 정해졌습니다.
   - 기록(2026-10-15 오전): 사용자가 NAS에 설치했고, 운영 에이전트가 사용자 승인으로 4번의 명령을 실행했습니다(PC 주인은 사용자 자신, 이름은 기본값). 판정은 `on the PC's network: yes (the same network and gateway)`, `the same egress address as the PC: no`였습니다. 나가는 주소가 다른 것은 정상입니다. 그 망은 공인 주소망이라 장비마다 자기 주소로 나가고, hjv-api는 나가는 주소를 사설 망에서만 견줍니다(`server/api/src/wolinfo.rs`의 `judge`).
   - 기록(2026-10-15 오후): 아이폰 앱의 켜기로 켜기 작업이 생기고 NAS가 신호 12개를 보냈지만 운영자 PC는 켜지지 않았습니다. 까닭은 그 PC가 잠든 동안 랜카드 전원을 끊는 것입니다(7-1의 5번, [wol-pc-setup.md](wol-pc-setup.md) 2장의 결과).
2. 사용자가 로그의 **등록 코드**와 그 NAS로 켤 PC 하나의 ID를 확인합니다. 둘 다 운영 에이전트에게 알리지 않고, 3·4·6번은 사용자가 서버에서 직접 합니다(7-0: 그 PC는 사람의 PC이고, `relay list`는 계정과 PC의 꼬리표를 보임). 운영 에이전트는 그 앞뒤에 `sudo /opt/hjviewer/bin/hjv-api admin relay list | head -1`(개수)만 봅니다.
3. 승인하기 전에 사용자가 그 PC의 주인에게 이미 아는 길(대면, 전화, 알려진 메일)로 확인합니다. PC ID는 비밀이 아니므로 ID만으로는 주인을 확인할 수 없습니다.
4. 사용자가 서버에서 실행합니다(질문 17, 7-0).
   ```bash
   A="sudo /opt/hjviewer/bin/hjv-api admin"
   $A relay list
   $A relay approve KZDV-VJ2U-MNDU-YAUF --owner-of 123456789 --name '연구실 NAS'   # ← 등록 코드, PC ID, 앱에 보일 이름
   $A relay list     # 승인한 뒤: 그 계정의 PC마다 판정을 다시 봄
   ```
   - 승인 도구의 기본 출력은 대기 중인 중계기와 그 PC가 보고한 네트워크를 비교한 **판정**만 보입니다: 같은 망인지(`on the PC's network: yes (…)`), 나가는 주소가 같은지(`the same egress address as the PC: yes`), 중계기의 첫 요청과 PC의 보고가 얼마나 됐는지. 주소와 망의 원래 값은 사람이 `--reveal`로만 봅니다.
   - 판정이 같은 망(`on the PC's network: yes`)이 아니면 승인 도구가 아무것도 바꾸지 않고 스스로 멈춥니다(`nothing changed: …`). 그 PC의 네트워크에 닿지 못하는 엉뚱한 중계기일 수 있으므로 거기서 멈춥니다. PC 주인이 코드를 확인했는데도 판정이 나오지 않을 때만(PC가 아직 켜기 정보를 보내지 않아 `unknown`일 때 등) 사용자가 같은 명령에 `--force`를 붙입니다. 판정이 `no (a private network of another site: the egress addresses differ)`이면 `--force`로 승인해도 그 PC를 켤 수 없으므로 붙이지 않습니다(7-1의 5번의 열린 질문).
   - 같은 망 판정은 중계기가 스스로 알린 랜카드 정보를 PC의 서명된 보고와 비교한 것이고, hjv-api가 직접 본 것은 나가는 주소뿐입니다(출력의 괄호 줄 `on the PC's network: by the relay's own report …`). 틀린 코드나 다른 NAS는 걸러 내지만, 일부러 그 PC의 네트워크를 알리는 중계기는 막지 못합니다. 승인의 근거는 3번의 주인 확인이므로, 판정이 `yes`여도 `--force`를 쓸 때도 3번을 건너뛰지 않습니다.
   - `--name`은 앱이 켜기 진행 창에 그대로 보이는 중계기 이름입니다(1~32자). "연구실 NAS"처럼 혼자서 읽히는 이름이 좋고, 정하지 않으면 `NAS`와 코드 첫 묶음(`NAS KZDV`)입니다. 이미 승인한 중계기에 같은 명령을 `--name`과 함께 다시 하면 이름만 바뀝니다.
   - 중계기가 아직 등록을 요청하지 못했으면 같은 명령으로 코드를 미리 승인할 수 있습니다(`approved ahead: …`). 7일 안에 그 키로 요청이 오면 바로 승인됩니다. 이때는 판정 없이 승인되므로, 중계기가 붙은 뒤 사용자가 `relay list`의 판정을 보고 같은 망이 아니면 폐기합니다(6번).
5. 1분 안에 NAS 로그에 `등록이 승인되어 켜기 명령을 기다립니다.`가 찍힙니다. 사용자가 앱에서 꺼진 PC의 "켜기"로 확인합니다. 켜지지 않으면 PC 쪽 준비([wol-pc-setup.md](wol-pc-setup.md))부터 봅니다.
6. 폐기(사용자, 7-0): `sudo /opt/hjviewer/bin/hjv-api admin relay revoke <중계기 ID>`(등록 코드로도 됨). 중계기는 명령을 받지 않고 1시간마다만 묻습니다. 폐기한 중계기의 키는 다시 승인되지 않습니다(7-12의 4번).

### 7-12. 되돌리기

1. **비밀번호 없는 연결만 멈출 때**: `sudo /opt/hjviewer/bin/hjv-api admin passwordless off`. 그 뒤 **hjv-api를 24시간 동안, 또는 `report`의 `PCs holding their binding that have not synced since the last switch change`가 0이 될 때까지** 켜 둡니다. 그러면 모든 PC가 PC 비밀번호로만 연결을 받습니다. 그 전에 hjv-api를 멈추거나 nginx를 되돌리면, 그동안 잠들어 있던 PC는 꺼짐을 받지 못하고 받아 둔 목록을 받은 때부터 24시간까지 씁니다. 1.9.0 이상의 PC는 꺼짐을 받는 대로 쓰고 있는 비밀번호 없는 연결도 끊습니다. 쓰는 사람이 있는 시간이면 미리 알립니다.
2. **계정 기능 전체**: 먼저 1번을 하고 그 조건까지 기다립니다. 그다음 경로 파일을 묶음의 `nginx-hjviewer-locations.conf` 그대로로 바꿉니다. 그 파일에는 1.5.1처럼 404를 주는 `location ^~ /api/` 블록과 버전 정책 위치(`/api/hjv/version`)가 함께 있습니다. 7-8의 백업(`…before-api`)은 쓰지 않습니다. 1.6.0 전의 파일이라 버전 정책 위치가 없어, 앱의 정책 확인이 `/api/` 블록의 404가 되고 새 정책(키 도난 뒤의 선언까지)이 기기에 닿지 않습니다(8-6). zones 파일과 `snippets/hjviewer-api.conf`는 남겨도 해가 없습니다.
   ```bash
   L=/etc/nginx/snippets/hjviewer-locations.conf W=/opt/hjviewer/share/web
   grep -q '^location = /api/hjv/version {' "$W/nginx-hjviewer-locations.conf" || { echo 'STOP: 묶음의 경로 파일에 버전 정책 위치가 없음'; exit 1; }
   sudo install -d -m 700 /root/hjv-backup && sudo cp -p "$L" /root/hjv-backup/hjviewer-locations.conf.before-rollback
   sudo install -m 644 "$W/nginx-hjviewer-locations.conf" "$L"
   out=$(sudo nginx -t 2>&1); echo "$out"
   if echo "$out" | grep -qE 'emerg|test failed'; then
     sudo cp -p /root/hjv-backup/hjviewer-locations.conf.before-rollback "$L"; echo 'STOP: 설정 오류, 되돌림'; exit 1
   fi
   sudo systemctl reload nginx
   DOMAIN=$(sed -n 's/^HJV_DOMAIN=//p' /etc/hjviewer/hjviewer.env); [ -n "$DOMAIN" ] || exit 1
   c() { curl -s --max-time 10 --resolve "$DOMAIN:443:127.0.0.1" -o /tmp/hjv-body -w '%{http_code}' "https://$DOMAIN$1"; echo " $(head -c 60 /tmp/hjv-body)"; }
   echo "version  $(c /api/hjv/version)"     # 정책 파일이 있으면 200과 그 파일, 없으면 404 {"error":"no version policy"}
   echo "peers    $(c /api/peers)"           # 404(본문 없음): 계정 API가 꺼짐
   ```

   로그인한 앱은 회수할 수 없으므로 로그인 창과 "내 PC" 탭에 오류가 남습니다. 1.9.0부터 이 탭의 오류는 "내 PC 목록을 불러오지 못했습니다. 잠시 뒤 다시 시도해 주세요."이고 원문 오류는 보이지 않습니다. hjv-api를 멈추면 PC들의 목록은 24시간 안에 만료되어 저절로 비밀번호로 돌아갑니다. hjv-api를 일부러 멈춰 둘 때는 점검 타이머와 백업 타이머를 먼저 멈춥니다(`sudo systemctl disable --now hjviewer-api-check.timer hjviewer-api-backup.timer`). 그대로 두면 점검과 백업이 실패할 때마다 경보가 납니다. 다시 켤 때 타이머도 켭니다(7-5의 마지막 블록).
3. **hbbs**: 보관한 옛 실행 파일(`/root/hjv-backup/hbbs-before-<VERSION>`)을 넣고 조용한 시간에 hbbs만 다시 시작합니다. 로그인은 되지만 새 PC 묶기와 켜짐 확인은 안 되고, 0005가 있었다면 최소 버전 거절도 멈춥니다.
4. **중계기**(사용자, 7-0): `hjv-api admin relay revoke <중계기 ID>`. NAS에서는 사용자가 공유 폴더에 `hjv-relay.stop`을 두고 작업을 끕니다(재부팅 없이, [relay-dsm.md](relay-dsm.md) 10장). 폐기한 중계기의 키는 다시 승인되지 않습니다(`relay approve`가 거절). 잘못 폐기했으면 사용자가 NAS에서 중계기의 상태 폴더를 지워(같은 장의 "제거") 새 키와 새 등록 코드를 만든 뒤 7-11처럼 다시 승인받습니다.
5. **hjv-api 새 판**: 설치 전에 남긴 스냅숏과 설정 파일을 옛 실행 파일과 함께 되돌립니다(7-14의 업그레이드). App Review 데모 계정(7-17)을 켠 뒤라면 먼저 7-17의 7번대로 그 계정과 설정 줄을 지웁니다.
6. **앱을 1.5.1로**: 사용자에게 먼저 그 기기에서 로그아웃하라고 안내합니다. 로그인한 채로 1.5.1을 설치하면 설정이 남고, 1.5.1은 저장된 성명(또는 포탈 ID)을 연결하는 PC마다 보냅니다. 최소 버전 강제(7-10)가 켜진 뒤에는 이 길이 닫힙니다.
7. **최소 버전**: 정책, hbbs 파일, 문 2는 8-6에 있습니다. hbbs의 거절은 `MIN=`으로 10초 안에 멈출 수 있습니다. 정책은 옛 파일로 되돌리거나 지우지 않고, `seq`가 더 높은 새 정책으로만 되돌립니다(앱이 이미 받은 정책은 옛 파일로 풀리지 않음. 예외는 키 도난 때의 8-5 (라) 2번뿐).

### 7-13. 사고 대응

키나 계정이 샜거나 hjv-api 서버가 장악됐을 수 있을 때입니다.

1. `passwordless off`. 7-12의 1번 규칙대로 hjv-api를 켜 둡니다.
2. 필요하면 `strict on`(연결마다 확인). 서버에 닿는 PC는 증명이 올 때마다 목록을 새로 받습니다.
3. 목록 키를 2번으로 바꿉니다(7-15). 서버에 닿는 PC는 늦어도 5분 안에 2번 목록을 받고, 그 뒤로 1번 목록을 받지 않습니다.
4. `user revoke-all --all`로 모든 계정 키와 토큰을 폐기합니다. 모두 기기마다 다시 로그인합니다. PC의 묶음은 남습니다. 한 계정의 포탈 비밀번호만 샜으면 사용자가 `user revoke-all tag:<T>`로 그 계정만 폐기하고(7-0), 사용자에게 비밀번호를 바꾸게 합니다. 포탈 비밀번호를 바꿔도 이미 로그인한 기기는 남으므로 폐기가 필요합니다. 그 계정이 다시 로그인하지 못하게 해야 하면 사용자가 `block tag:<T>`를 합니다(폐기와 함께 다음 로그인도 막음, 7-15). 모르는 계정이 잇달아 새로 등록되면(`report`의 셋째 줄, 저널의 `was registered` 줄) 명단 방식으로 바꿔 새 등록을 멈춥니다(7-15).
5. 위의 폐기와 스위치는 새 연결을 막고, 1.9.0 이상의 PC는 서버에 닿는 대로(늦어도 5분, 오래 멈췄다 돌아온 직후 10분) 그 키·그 계정의 열린 비밀번호 없는 연결도 끊습니다([architecture.md](architecture.md) 5-6의 8번). PC 로그에 `same-account session … ended:` 줄이 남습니다. 1.9.0 전의 PC(사용자가 `hjv-api admin binding list`의 버전 열로 봄, 7-0)와 서버에 닿지 못하는 PC의 열린 연결은 남으므로 사용자에게 알려 끊게 합니다: PC의 연결 창에서 끊거나, 자기 기기에서 PC 비밀번호로 그 PC에 연결해 '원격 PC 다시 시작'을 누르거나, PC를 다시 시작합니다.
6. 개인정보가 샜을 가능성이 있으면 사용자와 학교 정보보호팀에 바로 알립니다. 2026-09-11부터 시행된 개정 개인정보 보호법은 유출 가능성도 통지하게 합니다.

| 샌 것 | 할 수 있는 것 | 되돌리기 |
|---|---|---|
| 봉인 키 | 가로챈 로그인의 비밀번호 열기(TLS 가로채기도 필요) | 새 봉인 키로 앱을 다시 빌드해 나눠 주고, 자격 증명에서 옛 키를 뺌(그 뒤 옛 앱의 로그인은 실패) |
| 목록 키 | PC의 요청에 답할 수 있을 때만 거짓 목록 | 2번으로 교체 |
| 명령 키 | 중계기의 네트워크에서 성가신 켜기 | 2번으로 교체 |
| 중계기 키 | 거짓 결과, PC들의 MAC과 네트워크 정보 보기 | `relay revoke` |
| 계정 키(+토큰) | 그 계정의 묶인 PC | `key revoke <ckid>`(보통 2분, 늦어도 5분. 이미 열린 연결은 남음: 5번) |
| 기기 키 | 그 PC로서 동기화(목록 보기), 목록 올리기 선점 | 지금과 같은 경계 |

**비밀번호 시험이 의심될 때**(열린 로그인, [architecture.md](architecture.md) 5-7): 5분 점검의 `sso_new` 경보(명단에 없는 포탈 ID들의 SSO 호출이 한도 `HJV_API_LIMIT_SSO_NEW`를 채움), 저널의 `login: SSO call limit of the portal IDs not on the allowlist reached` 줄, 한 ID나 한 주소가 그 호출의 4분의 1을 다 쓴 `holds its share` 줄(`acct=…` 또는 `ip=…`), 명단에 없는 ID의 `login: ID limit reached (acct=…, an ID not on the allowlist)` 줄이 늘면, 누군가 hjv-api로 여러 교직원 계정에 비밀번호를 시험하고 있을 수 있습니다. 운영 에이전트는 줄의 개수만 셉니다: `sudo journalctl --namespace=hjviewer -u hjviewer-api --since -1h -o cat | grep -cE 'holds its share|not on the allowlist reached|an ID not on the allowlist\)'`(첫 로그인의 등록 줄은 세지 않음). 그동안 처음 로그인하는 사람은 실패하고, 이미 명단에 있는 사람은 영향이 없습니다. 성공한 처음 로그인은 호출을 돌려받으므로 몫에는 아무도 로그인하지 못한 호출만 쌓입니다. 주소 하나로는 몫을 채우지 못하지만, 주소 다섯 개(IPv6는 /64 다섯 개. IPv6 가입자는 /64를 여러 개 받는 일이 흔함)면 시간당 틀린 비밀번호 10번으로 한 시간 동안 처음 로그인을 모두 막을 수 있습니다.

1. 사용자에게 개수와 함께 알립니다. 새 사용자 안내 직후처럼 처음 로그인하는 사람이 몰려 비밀번호를 틀린 것이면 그대로 두거나, 사용자가 정한 대로 `HJV_API_LIMIT_SSO_NEW`를 올립니다(7-15). 학내 NAT처럼 여러 사람이 쓰는 주소가 몫을 다 써서(`an address holds its share` 줄) 그 주소의 다른 사람이 처음 로그인하지 못하면, 사용자가 그 줄의 꼬리표로 `unlock tag:<꼬리표>`를 합니다(운영 에이전트는 그 줄을 보지 않음, 7-0).
2. 그렇지 않거나, 적은 요청으로 몫이 되풀이해 차면(주소 몇 개로 처음 로그인을 막는 것) 사용자 승인 뒤 명단 방식으로 바꿉니다(7-15의 "명단 방식으로 바꾸기"). 그러면 명단에 없는 ID는 SSO에 닿지 않습니다. 그동안 새로 써야 하는 사람은 사용자가 `allow import <파일>`로 넣습니다(7-0).
3. 연세 SSO가 실패가 잦은 계정을 잠그면 남의 계정이 잠겼을 수 있고, 연세 쪽에서는 이 서버의 주소(같은 서버의 WordPress 로그인도 씀)가 원인으로 보입니다. 정보화처에 알릴지는 사용자가 정합니다.
4. 잦아들면 사용자 승인 뒤 열린 로그인으로 되돌립니다.

**모두가 거절될 때**(맞는 비밀번호에도 "이 계정으로는 HJViewer를 쓸 수 없습니다. …"): 5분 점검의 `sso_user` 경보나 저널의 `login: the SSO's user call refused 3 different accounts in a row …` 줄(crit)은 연세 SSO의 사용자 정보 호출이 서로 다른 계정 3개를 10분 안에 잇달아 거절했다는 뜻입니다(그 사이 받아들여진 사용자 정보 호출 없음, `report`의 `login denied-user-refused`). 이 거절은 사람마다의 답으로 보므로 SSO 호출이 멈추지 않고, 그동안 맞는 비밀번호로 로그인한 교직원이 모두 이 문구를 받습니다. 한두 사람이 거절되는 것은 그 사람의 문제지만, 모두가 거절되면 이 서버의 설정이 행정허브 등록과 맞지 않는 것입니다.

1. 운영 에이전트는 값을 출력하지 않고 사용자에게 알립니다(그 줄과 `login denied-user-refused`의 개수).
2. 사용자가 설정 파일의 `HJV_API_SSO_CLIENT_ID`와 자격 증명 `hjviewer-sso-api-uuid`를 WordPress SSO 로그인 플러그인(projecttf-auth)의 값과 견줍니다. 다르면 고치고(자격 증명은 7-15의 "SSO 자격 증명이 바뀔 때"처럼) `sudo systemctl restart hjviewer-api`를 합니다.
3. 둘이 같은데도 그러면 같은 서버의 WordPress SSO 로그인이 되는지 보고, 행정허브 등록이 바뀌었는지 정보화처에 물을지는 사용자가 정합니다.

### 7-14. 감시와 백업

hjv-api가 멈추거나 동기화만 실패해도, 비밀번호 없는 연결은 PC가 받아 둔 목록으로 하루 동안 계속되므로 아무도 모릅니다. 24시간 뒤 모든 PC가 한꺼번에 비밀번호 창으로 돌아갑니다. 그래서 감시를 둡니다. 동작 원리와 명령은 [server/api/README.md](../server/api/README.md)의 "Monitoring"과 "Backup and restore"에 있고, 시험 서버에서 한 연습의 기록은 [hjv-api-drill.md](hjv-api-drill.md)에 있습니다.

- **감시**: `hjviewer-api.service`, 점검 유닛, 백업 유닛이 실패하면 `OnFailure=`로 경보 유닛(`hjviewer-api-alert@.service`)이 돕니다. 5분 타이머(`hjviewer-api-check.timer`)의 점검(`hjv-api check`)은 `report`의 줄이 아니라 소켓(`/run/hjviewer-api.sock`)의 `GET /api/hjv/source`와 관리 소켓의 `health`를 봅니다. 경보가 되는 것은 다음입니다.
  - 소켓이나 관리 소켓이 답하지 않음
  - 묶음을 가진 PC 가운데 켜진 PC가 있는데 그중 어느 것도 10분 안에 동기화하지 않음. 켜진 PC는 hbbs가 저장된 키로 90초 안에 등록한 것으로 본 PC이고, 지난 4분 동안의 점검마다 그렇게 보인 것만 셉니다(막 깨어난 PC가 첫 동기화를 할 시간)
  - hbbs 콘솔이 답하지 않음
  - SSO 호출 중지(지금 또는 지난 10분 안). `invalid_client`(7-15의 "SSO 자격 증명이 바뀔 때")와 주소 거절을 구분해 알립니다. 이 경보가 되풀이되면(앱에서는 새 로그인이 2분씩 통째로 막힘) 매일 보고에 그날의 정지와 SSO 오류의 줄 수를 넣습니다: `sudo journalctl --namespace=hjviewer -u hjviewer-api --since yesterday -o cat | grep -c 'login: SSO calls paused'`, 같은 명령의 `grep -c 'login: SSO error:'`(개수만). SSO 장애 없이 정지가 되풀이되면 한 ID가 정지를 계속 여는지 사용자가 봅니다(7-1의 5번 "실제 연세 SSO의 답")
  - 명단에 없는 포탈 ID들의 SSO 호출이 한도를 채움(`sso_new`, 지난 10분 안, 열린 로그인에서만). 누군가 여러 포탈 ID에 비밀번호를 시험하고 있을 수 있으므로 7-13의 "비밀번호 시험이 의심될 때"대로 합니다
  - 연세 SSO의 사용자 정보 호출이 서로 다른 계정 3개를 10분 안에 잇달아 거절함(`sso_user`, 마지막 거절이 지난 10분 안). 모두가 거절되면 `HJV_API_SSO_CLIENT_ID`나 자격 증명 `hjviewer-sso-api-uuid`가 틀린 것이므로 7-13의 "모두가 거절될 때"대로 합니다
  - 1시간 넘게 묻지 않은 활성 NAS 중계기. NAS를 밤에 끄거나 재우는 곳에서는 그때마다 이 경보가 나므로, 그런 일정이 있으면 사용자에게 알려 기준을 정합니다(질문 15)

  꺼진 PC는 아무것도 말하지 않으므로 모든 PC가 잠든 밤은 경보가 아닙니다. 새 키로 다시 설치되어 그 PC에서 다시 로그인하기를 기다리는 PC(`report`의 needs-relogin)도 빼고 봅니다. 결과가 있으면 줄마다 저널에 오류를 쓰고 `/run/hjviewer-api-check/findings`에 남긴 뒤 유닛이 실패합니다. 이상이 없으면 저널에 한 줄입니다.
- **경보 경로(질문 12)**: 경보 유닛은 언제나 저널(이름 공간 `hjviewer`)에 오류 한 줄을 씁니다. 텔레그램을 쓰지 않으면 이 줄이 경보이고, 운영 에이전트의 매일 보고가 그 수를 셉니다(개수만): `sudo journalctl --namespace=hjviewer -p warning --since yesterday -o cat | wc -l`과 `sudo /opt/hjviewer/bin/hjv-api admin report`. nginx가 hjv-api 앞에서 끊은 로그인과 중계기 등록(주소별 한도)은 hjv-api에 닿지 않아 저널과 `report`에 없으므로, 그 수도 셉니다. 이 줄은 HJViewer 경로가 든 server 블록의 `error_log`에 남고, 그 블록에 자기 `error_log`가 없을 때만 http나 main의 것(보통 `/var/log/nginx/error.log`)에 남습니다. 사전 점검이나 `sudo nginx -T 2>/dev/null | grep -nE '^\s*error_log\s'`로 그 블록의 파일을 찾은 뒤 개수만 셉니다: `E=<그 파일>; sudo grep -c -E 'limiting (requests|connections).*zone "hjv_(login|login_conn|relay_hello)"' "$E"`. 그 `error_log`의 수준이 `error`보다 높으면(`crit` 등) 이 줄이 남지 않아 수가 늘 0이므로, 그때는 사용자에게 알립니다. 같은 주소(학내 NAT, 공용 Wi-Fi) 뒤의 누군가가 한도를 채우면 묶인 PC의 로그인도 nginx에서 막히므로, 이 수가 늘면 사용자에게 알립니다. 텔레그램으로 받기로 하면 사용자가 봇을 서버에 직접 넣습니다(에이전트 없이). 경보 유닛은 이 자격 증명이 있을 때만 가져옵니다(`ImportCredential=`).
  ```bash
  { printf 'TELEGRAM_BOT_TOKEN='; systemd-ask-password -n "bot token:"; printf '\nTELEGRAM_CHAT_ID=%s\n' '<채팅 번호>'; } |
    sudo systemd-creds encrypt --tpm2-pcrs= --name=hjviewer-api-telegram - /etc/credstore.encrypted/hjviewer-api-telegram
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
- **운영자의 서버 밖 백업**: 사용자가 따로 두는 서버 백업(예: 매일 NAS로 보내는 백업)이 `/etc/hjviewer`와 `/var/lib/hjviewer`를 담을 수 있습니다. 그러면 문 2의 코드(`door2.conf`), hjv-api 설정 파일(최고운영자의 포탈 ID, 학교 주소 대역), hbbs 개인키도 그 백업에 들어가므로, 그 백업은 사용자 계정만 읽을 수 있어야 합니다. hjv-api의 DB(`/var/lib/hjviewer-api`)와 자격 증명(`/etc/credstore.encrypted`, `/var/lib/systemd/credential.secret`)은 그 백업에도 넣지 않습니다(위의 "서버 밖 사본은 1.6.0에서 만들지 않습니다").
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
  $A passwordless status | head -2; $A allow list | head -1; $A report | head -7
  $A relay list | head -1         # 개수만. 다시 활성인 중계기는 사용자가 아래 기록으로 봄(7-0)
  ```
  - 두 번째 `passwordless off`가 `(no change)`가 아니면(복원한 DB가 꺼진 채로 시작하지 않았다는 뜻) 그대로 보고합니다.
  - `STOP: 지금 DB를 백업하지 못함`이면 사용자에게 보고하고 점검 타이머를 다시 켭니다(`sudo systemctl start hjviewer-api-check.timer`). DB가 망가져 백업이 되지 않는 경우에만, 사용자 승인 뒤 `rm -f` 대신 그 파일들을 `/root/hjv-backup/before-restore-<시각>/`(700)로 옮기고, 그 사본은 14일 안에 지웁니다(개인정보 처리방침의 백업 14일).
  - 중계기도 백업 때의 상태로 돌아옵니다. 백업 뒤에 폐기한 중계기는 다시 활성이 되고, hjv-api는 복원한 백업에 활성 중계기가 있으면 시작할 때 저널에 그 수를 적습니다. 무엇을 폐기했었는지는 운영자 기록에만 있습니다: 사용자가 `sudo grep -a 'cmd="relay revoke" targets=' /var/log/hjviewer-api/operator.log | awk '{print $1, $6}'`(시각과 중계기 ID)로 보고, 백업 파일 이름의 시각(UTC)보다 뒤에 폐기한 것을 다시 `relay revoke`합니다(7-0). 되살아난 중계기가 받는 것은 그 계정의 PC를 주인이 켤 때의 켜기 명령뿐이고, PC에 연결할 권한은 없습니다.
  - 백업 뒤의 계정 삭제, 막기와 막기 풀기, 묶음 풀기도 되돌아갑니다. 지운 계정의 행(포탈 ID, 성명, PC, 켜기 정보와 기록)이 돌아오고, 백업 뒤에 막은 사람은 다시 로그인할 수 있고 백업 뒤에 막기를 푼 사람은 다시 막히며, 풀린 PC 가운데 그 사이 동기화하지 못한 PC는 이전 주인의 기기에 다시 열립니다. `user revoke-all --all` 바로 뒤에 사용자가 운영자 기록에서 백업 시각(UTC) 뒤의 그런 명령을 찾습니다(출력에 꼬리표가 있으므로 운영 에이전트는 하지 않음, 7-0): `sudo grep -a -E 'cmd="(user delete|app account-delete|block|unblock|allow remove|binding revoke|device remove)" targets=' /var/log/hjviewer-api/operator.log | awk '{print $1, $4, $5, $6}'`(시각, 명령, 꼬리표). 사용자가 다시 실행합니다. `user delete tag:<T>`, `block tag:<T>`, `unblock tag:<T>`, `binding revoke tag:<T>`는 기록의 꼬리표로 되고, `allow remove`는 기록에 남은 꼬리표마다 `block tag:<T>`로 합니다. `app account-delete`(1.9.1부터 사용자가 앱에서 지운 계정, 7-15)도 그 꼬리표로 `user delete tag:<T>`를 합니다. 다만 그 꼬리표의 끝 줄(`acct=<T> done=…`)이 모두 `failed`이면 지운 적이 없으므로 건너뜁니다(`sudo grep -a 'cmd="app account-delete" acct=<T> ' /var/log/hjviewer-api/operator.log`). 그 꼬리표가 App Review 계정의 것(7-17의 `T`)이면 다르게 합니다. **심사 중**(`/etc/hjviewer/review.conf`가 있고 `UNTIL` 전)이면 `user delete`로 다시 지우지 않습니다. 복원의 `user revoke-all --all`이 그 로그인들을 이미 끝냈고, `user delete`는 명단 항목을 지우며 1.9.2부터는 그 계정의 데모 PC도 풀어(서명된 `revoked`) 오프라인으로 만들기 때문입니다. 심사가 끝났으면 7-17의 6번(끝내기)대로 합니다. 심사 중의 복원 뒤에는 7-17 "데모 PC 묶기" 4단계의 데모 PC 확인을 합니다. 데모 PC의 묶음이 없으면(데모 PC를 묶기 전의 백업) 그 PC는 동기화에서 `unbound`를 받고 받아 둔 출입증이 끝나면(최대 24시간) 오프라인이 되므로, 사용자에게 알리고 같은 3단계로 다시 묶습니다(데모 PC에서 이 계정으로 로그인). 심사자가 백업 뒤에 앱에서 이 계정을 지웠으면(운영 에이전트가 이 꼬리표의 줄만 봄: `sudo grep -a "cmd=\"app account-delete\" acct=$T done=\"ok: [0-9]" /var/log/hjviewer-api/operator.log | awk '{print $1}'`의 시각이 백업 파일 이름의 시각(UTC)보다 뒤), 복원이 지우기 전의 계정 행과 그 심사 기기들의 키를 되살린 것입니다. 사용자에게 알리고 둘 가운데 하나를 고르게 합니다. (가) 그대로 둠: 데모 PC는 온라인으로 남지만, 되살아난 키들은 복원의 `user revoke-all --all`로 관리자가 끝낸 기기가 되어, 다시 로그인한 심사자의 로그인한 기기 목록에 30일 동안 끝난 기기로 보입니다(`GET /api/hjv/keys`의 `ended`, `left`가 `admin`). (나) `user delete "tag:$T"`로 지우고, 7-17의 3번(명단에 다시 넣기), "데모 PC 묶기"의 3단계(사용자가 데모 PC에서 다시 로그인)와 7-17의 4번(iPhone의 [계정 삭제]로 그 로그인들을 끝냄)을 다시 합니다. 그동안 데모 PC는 몇 분 오프라인입니다. `block tag:<T>`가 `no account or allowlist entry has that tag`로 실패하면(백업 뒤에 처음 로그인해 명단에 오른 사람이나, 계정 없이 파일로 막은 ID는 복원한 DB에 없음), 7-15대로 서버 밖에 적어 둔 그 사람의 ID로 사용자가 파일을 만들고 `allow remove <파일>`로 막습니다. 열린 로그인에서는 백업 뒤에 로그인으로 명단에 오른 사람이 다음 로그인 때 다시 오르므로, 이 막기를 빠뜨리면 막은 사람이 다시 들어옵니다. 앱에서 빼거나 PC에서 로그아웃해 풀린 묶음은 DB에만 기록이 있으므로, 복원 뒤 사용자들에게 "백업 뒤에 빼거나 넘긴 PC가 내 목록에 다시 보이면 다시 빼 주세요"라고 알립니다. 호스트를 재구축하면 기록 키가 새로 생겨 옛 꼬리표가 맞지 않으므로, 삭제와 막기 요청은 7-15대로 날짜와 함께 서버 밖에 적어 둔 것을 씁니다. 앱에서 지운 계정은 서버 밖의 메모가 없어, 재구축 뒤에 넣은 옛 백업에서는 찾아 다시 지울 수 없습니다(드문 경우의 한계). 그 사람의 계정이 다시 보이면 그 사람이 다시 로그인해 앱에서 지우거나 운영자에게 요청합니다.

  사용자가 막힌 계정(`allow list`)과 계정별 스위치(`passwordless status`)를 확인한 뒤(7-0), 운영 에이전트가 사용자 승인을 받아 `passwordless on`과 `sudo systemctl start hjviewer-api-check.timer`를 합니다. 모두 휴대폰과 PC에서 한 번 다시 로그인합니다. PC의 묶음은 남으므로, 그 PC로 들어오는 연결을 위해 PC에서 다시 로그인할 필요는 없습니다. 백업 뒤에 풀렸던 묶음이 서버에 되살아나도, 허용 표시가 이미 지워진 PC는 증명을 받지 않습니다(`listed`).
- **업그레이드**: hjv-api의 저장 이전은 한 방향입니다(옛 실행 파일은 새 스키마를 거부). 새 판을 설치하기 전에 `sudo /opt/hjviewer/bin/hjv-api admin backup /var/lib/hjviewer-api/backup/before-<VERSION>.sqlite3`로 스냅숏을 남기고, 옛 실행 파일과 설정 파일도 `/root/hjv-backup/`에 복사합니다(`sudo cp -p /opt/hjviewer/bin/hjv-api /root/hjv-backup/hjv-api-before-<VERSION>`, `sudo cp -p /etc/hjviewer/hjviewer-api.env /root/hjv-backup/hjviewer-api.env-before-<VERSION>`). 되돌릴 때는 옛 실행 파일, 스냅숏, 설정 파일을 함께 되돌립니다. 옛 실행 파일과 설정 파일을 제자리에 넣고(설정 파일은 600을 지키도록 `sudo cp -p /root/hjv-backup/hjviewer-api.env-before-<VERSION> /etc/hjviewer/hjviewer-api.env`), 위의 복원 블록을 `B`=스냅숏으로 합니다. 새 판이 돌지 않아 `backup` 줄이 실패하면, 사용자 승인 뒤 그 줄 대신 지금 DB 파일들을 `/root/hjv-backup/before-restore-<시각>/`(700)로 옮기고 14일 안에 지웁니다. 스냅숏도 복원한 백업이므로 모든 계정이 꺼진 채로 시작하고, 확인과 `user revoke-all --all` 뒤 사용자 승인을 받아 켭니다. 스냅숏은 다른 백업처럼 14일 뒤 지워지므로 되돌리기는 그 안에 정합니다. 기록: 2026-10-15 0시쯤(KST) 이 절차로 hjv-api를 3b283b2로 바꿨습니다(서버 묶음 `server-2026.10.14.3`, Release는 pre-release. `server-2026.10.14.2`와 다른 것은 `bin/hjv-api`와 `SOURCE.txt`뿐). 문 2 페이지가 바깥 사이트를 가리키지 않게 한 판입니다(8-4). hbbs와 hbbr은 다시 시작하지 않았습니다.
- **호스트 재구축**: 봉인 키는 오프라인 사본에서 되살립니다(7-4의 (가) 3번). 목록 키 1번과 명령 키 1번은 서버에만 있었으므로 오프라인의 2번을 넣고 `HJV_API_LIST_KID=2`, `HJV_API_COMMAND_KID=2`로 바꿉니다(7-15). 앱과 hjv-relay 빌드가 1번과 2번을 함께 고정하므로 앱과 NAS는 건드리지 않습니다. 기록 키는 새로 만듭니다. 출입증 키도 새로 만들고(7-4 (나)), `HJV_API_TICKET_PUBKEYS`와 hbbs의 `access.conf`의 `KEY=`를 그 값으로 바꿉니다. 앱은 출입증 키를 고정하지 않으므로 앱은 건드리지 않습니다. 백업이 남아 있으면 위의 복원처럼 넣어 꺼진 채로 시작하고, `user revoke-all --all` 뒤 사용자 승인을 받아 켭니다. 백업도 없으면 빈 DB로 시작하므로 막기를 서버 밖에 적어 둔 것으로 다시 하고(꼬리표가 맞지 않으므로 사용자가 쓴 ID 파일로 `allow remove <파일>`, 명단 방식이면 명단도 다시 넣음, 7-5), 모두 PC에서도 다시 로그인하며, 중계기는 다시 등록을 요청해 다시 승인받습니다(7-11). 이 절차들은 시험 서버에서 연습했습니다([hjv-api-drill.md](hjv-api-drill.md)).

### 7-15. 일상 운영

관리 명령은 `sudo /opt/hjviewer/bin/hjv-api admin <명령>`이고, 출력은 가려져 있습니다(포탈 ID는 앞 4자, 이름·기기 이름·주소는 없음, 대신 꼬리표). 모든 명령은 운영자 기록에 남습니다(5분 점검이 읽는 `health`만 예외: 개수만 돌려주고 아무것도 바꾸지 않음). 전체 목록은 `hjv-api admin help`와 [server/api/README.md](../server/api/README.md)의 "Admin"에 있습니다.

- **누가 로그인하나**: `HJV_API_LOGIN=open`(기본)이면 연세 SSO를 통과한 교직원은 누구나 로그인하고, 처음 로그인할 때 명단에 저절로 올라갑니다(저널에 `login: a portal ID that was not on the allowlist logged in and was registered (acct=<꼬리표>)` 한 줄). 교직원인지는 hjv-api의 ID 규칙(Z나 Y로 시작, ZA는 아님)으로 가립니다. 규칙 밖의 ID는 SSO에 묻지 않고 바로 "연세대학교 교직원의 연세포탈 ID로만 로그인할 수 있습니다. …"를 받고(`report`의 `login id-rule`, 주소의 실패로만 셈), SSO가 비밀번호를 받아들였어도 사용자 정보 호출이 거절하거나, 그 사람의 pcode가 규칙 밖이거나, 그 사람의 SSO 자료(pcode나 성명)의 값이 계약에 어긋나면 "이 계정으로는 HJViewer를 쓸 수 없습니다. …"를 받습니다(`report`의 `login denied-user-refused`, `login denied-pcode-rule`, `login denied-bad-value`, 저널에 사유 한 줄, 틀린 비밀번호로 세지 않음). 아이폰 사용자를 초대하기 전에도 명단에 넣을 일이 없습니다. `allow list`는 명단의 항목마다 꼬리표, 가린 ID, 상태(`registered` 또는 `blocked`), 들어온 길(`login` 또는 `import`)과 날짜를 보여 주고(사용자가 봄, 7-0), `report`의 둘째·셋째 줄은 로그인 방식과 명단의 개수(로그인으로 오른 수, 막힌 수)입니다. 명단에 없는 ID는 한도가 따로 낮습니다: ID마다 하루 실패 2번(`HJV_API_LIMIT_NEW_ID`), 그런 ID 모두 합쳐 SSO 호출 시간당 10번(`HJV_API_LIMIT_SSO_NEW`). 그 호출은 한 ID와 한 주소가 각각 4분의 1(2번)까지 쓰고, 성공한 처음 로그인은 돌려받습니다. 그래서 같은 주소(학내 NAT)에서 처음 로그인이 한 시간에 두 번 실패하면 그 주소의 다른 사람도 그동안 처음 로그인하지 못합니다(저널의 `an address holds its share of the SSO calls of the portal IDs not on the allowlist (ip=<꼬리표>)` 줄, 7-13). 숫자는 사용자가 정하고, 바꿀 때는 아래 "명단 방식으로 바꾸기"처럼 설정 파일에 줄을 넣고 다시 시작합니다. 처음 로그인하는 사람이 비밀번호를 두 번 틀리면 하루 동안 로그인하지 못합니다. 그 사람이 요청하면 사용자가(운영 에이전트는 하지 않음, 7-0) 저널의 `login: ID limit reached (acct=<꼬리표>, an ID not on the allowlist)` 줄의 꼬리표로 `unlock tag:<꼬리표>`를 하거나, 그 ID를 `allow import <파일>`로 미리 넣습니다(미리 넣은 ID는 명단에 있는 ID의 한도를 씀).
- **막기와 풀기**(사용자만, 7-0): 어떤 사람이 HJViewer를 쓰지 못하게 하려면 `block tag:<T>`(꼬리표는 `users`나 `allow list`). 그 계정의 토큰과 계정 키가 바로 폐기되어 그 사람의 기기가 모두 로그아웃되고, 그 계정에 묶인 PC는 남되 키 없는 목록을 받습니다(그 PC를 다른 사람에게 넘길 때는 아래처럼 풉니다). 그 사람이 다시 로그인하면 SSO가 비밀번호를 받아들인 뒤에 "이 계정은 관리자가 HJViewer 사용을 막았습니다. 다시 쓰려면 관리자에게 문의하세요."로 거절합니다. 비밀번호를 아는 사람에게만 보이는 답입니다. 로그인한 적이 없어 꼬리표를 모르는 ID는 사용자가 쓴 파일로 `allow remove <파일>`(막기와 같음, 7-5의 파일처럼)을 합니다. 풀기는 `unblock tag:<T>`이고, 그 사람의 기기는 다시 로그인합니다. `allow import`는 막힌 항목을 풀지 않습니다. 막을 때와 풀 때는 사용자가 날짜와 그 사람의 포탈 ID를 서버 밖에 적어 둡니다. 복원한 DB나 재구축한 서버에는 그 뒤의 막기가 없고 꼬리표도 맞지 않을 수 있어, 그 기록으로 다시 막기 때문입니다(7-14). 막기와 풀기는 SSO가 같은 사람에게 준 다른 ID(그 계정의 pcode, 같은 사람의 다른 계정)에도 함께 걸리고, 출력에 `another ID the SSO names for the same person`으로 셉니다. 그 사람의 계정 하나를 지운 뒤에는 서버가 그 ID들을 함께 알지 못하므로, 풀 때는 `allow list`에 막힌 채 남은 항목마다 `unblock tag:<T>`를 합니다.
- **명단 방식으로 바꾸기**(사고 대응이나 나중의 결정, 사용자 승인 뒤 조용한 시간에): 설정 파일에 `HJV_API_LOGIN=list` 줄을 넣고 `sudo systemctl restart hjviewer-api`. 사람은 `sudoedit /etc/hjviewer/hjviewer-api.env`로 해도 되지만, 편집기를 쓸 수 없는 운영 에이전트는 아래 블록으로 넣습니다. 그때부터는 명단에 있는 ID(가져온 것과 그때까지 로그인으로 오른 것)만 로그인하고, 명단에 없는 ID는 SSO에 묻지 않고 틀린 비밀번호와 같은 답을 받습니다. 이미 로그인한 기기는 그대로입니다. 새 사람은 사용자가 `allow import <파일>`로 넣습니다(7-0). 되돌릴 때는 그 줄을 `HJV_API_LOGIN=open`으로 바꾸고(아래 블록의 `list` 두 곳을 `open`으로) 다시 시작합니다. 어느 쪽인지는 `selfcheck`의 마지막 줄과 `report`의 둘째 줄로 확인합니다.
  ```bash
  E=/etc/hjviewer/hjviewer-api.env
  if sudo grep -q '^#\?HJV_API_LOGIN=' "$E"; then sudo sed -i 's/^#\?HJV_API_LOGIN=.*/HJV_API_LOGIN=list/' "$E"; else echo 'HJV_API_LOGIN=list' | sudo tee -a "$E" > /dev/null; fi
  sudo grep -n '^HJV_API_LOGIN=' "$E"
  ```
- **PC를 계정에서 풀기**(사용자만, 7-0): `binding list`로 보고 `binding revoke <PC ID>`로 풉니다. `device remove <PC ID>`는 같은 명령의 옛 이름입니다. 어느 이름이든 그 PC의 묶음 ID를 180일 동안 무덤으로 남기므로, 그 PC는 다음 동기화에서 서명된 `revoked`를 받아 허용 표시를 지웁니다. 그 뒤 그 PC의 빈 보고에는 서명 없는 `unbound`가 갑니다.
- **PC를 넘기거나 버릴 때**(사용자 안내): 그 PC에서 **온라인일 때** 로그아웃한 뒤 넘기거나 제거합니다. 제거만 해도 그 PC의 허용 표시는 지워지지만 서버의 묶음은 남아, 이전 주인의 카드에 "서버와 동기화 안 됨(마지막 확인 n일 전)"이 보이고 새 사용자의 로그인은 "다른 계정에 등록된 PC"가 됩니다. 그때는 이전 주인이 다른 기기에서 "내 목록에서 빼기"를 하거나 사용자가 `binding revoke <PC ID>`를 합니다(7-0).
- **PC의 기기 키가 바뀌었을 때**: 설정을 잃은 PC(설정까지 지우고 다시 설치 등)는 같은 ID로 새 키를 등록합니다. 서버는 그 PC의 첫 동기화에서 hbbs에 키를 묻고, 진짜 hbbs가 새 키를 가졌으면 그 PC를 `needs-relogin`으로 둡니다(`report`의 `PCs that need a login on them`). 목록을 새로 고친 컨트롤러는 그 PC에 비밀번호 창을 띄웁니다. **그 PC에서 같은 계정으로 다시 로그인**하면 새 키가 저장되고 묶음 ID는 그대로라 비밀번호 없는 연결이 돌아옵니다. PC가 새 ID를 받았으면 새 ID는 그 PC에서 다시 로그인해야 목록에 오르고, 옛 ID의 카드는 주인이 빼거나 180일 정리가 지웁니다.
- **대상 가리키기**: 포탈 ID나 주소를 명령 인자로 치지 않습니다. sudo가 명령줄 전체를 시스템 로그(`/var/log/auth.log`, 저널)에 남기고, 사람이 친 명령은 셸 기록에도 남기 때문입니다(운영자 기록에는 꼬리표만 남음). 사람 운영자는 `users --reveal`로 꼬리표를 찾아 `tag:<T>`로 가리키고, PC는 `--owner-of <PC ID>`나 `binding list`의 꼬리표로 가리킵니다.
- **계정 키**(사용자만, 7-0): `key list [tag:T]`(키마다 PC별 마지막 비밀번호 없는 연결과 30일 횟수), `key revoke <ckid>`(잃어버린 기기를 사용자가 직접 로그아웃시킬 수 없을 때), `user revoke-all tag:<T>`. 한 계정에는 활성 키가 20개까지이고, 넘으면 새 로그인이 그 계정에서 가장 오래 쓰지 않은 키를 로그아웃시킵니다(저널의 `login: the account had 20 active keys`). 사용자가 모르는 기기가 많으면 비밀번호 유출로 보고 7-13의 4·5번을 합니다.
- **앱에서 계정 삭제**(1.9.1부터): 사용자는 앱에서 자기 계정을 바로 지웁니다(설정 → 계정 → [계정 삭제], `POST /api/hjv/account/delete`). 지우는 것과 남는 것은 아래 `user delete`와 같고(App Review 계정만 명단 항목이 남음, 7-17), 운영자가 할 일은 없습니다. 서비스가 운영자 기록에 두 줄을 씁니다. 지우기 전에 `cmd="app account-delete" targets=<꼬리표>`, 끝난 뒤 같은 꼬리표로 `acct=<꼬리표> done="ok: <n> PCs, <n> account keys, <n> tokens, <n> relays, allowlist entry <removed | kept (App Review) | stays blocked | none>"`입니다(1.9.2부터 App Review 계정의 데모 PC가 남으면 `kept` 뒤, `(App Review)` 앞에 `, <k> demo PCs kept`가 붙고 `<n> PCs`는 풀린 PC만 셈). 지우지 못한 시도의 끝 줄은 `done="failed: database"`(DB 오류)나 `done="failed: the login ended"`(요청이 기다리는 사이 그 로그인이 끝남)이고, `done="ok: already deleted"`는 같은 계정의 다른 요청이나 `user delete`가 먼저 지운 경우입니다. 앞 줄을 쓸 수 없으면(관리 소켓이 꺼졌거나 기록 파일에 쓸 수 없음) 지우지 않고 앱에 "삭제 기록을 남기지 못해 계정을 지우지 않았습니다. …"를 돌려줍니다. `report`와 하루 요약은 `account deleted-by-user`로 세고, 저널에는 꼬리표와 개수만 든 `account: deleted at its own request (acct=<꼬리표>): …` 한 줄이 남습니다(그 밖의 줄은 9-10). 복원 뒤 다시 지우는 근거는 운영자 기록의 앞 줄이므로(7-14), 앱 삭제는 서버 밖에 적어 둘 것이 없습니다.
- **계정 삭제 요청**(운영자에게 온 것, 사용자만 함, 7-0): 요청을 받은 날짜를 사용자가 서버 밖에 적어 둡니다(복원·재구축 뒤 다시 지우기 위해, 7-14). `user delete tag:<T>`. 명단의 항목도 계정과 함께 지워집니다. 열린 로그인에서는 그 사람이 나중에 다시 로그인하면 다시 오르고, 명단 방식에서는 `allow import <파일>`로 다시 넣어야 로그인합니다. 막힌 항목은 계정을 지워도 남습니다(막기를 지키는 데 필요한 포탈 ID만 남음). `user delete`는 그 계정의 PC를 먼저 풀고(무덤) 그 계정의 중계기를 폐기한 뒤, 계정과 그 키·토큰·사용 기록·켜기 기록을 지웁니다. 남는 것은 묶음 무덤(180일), 폐기된 중계기의 ID·키·시각, 꼬리표로 남은 로그인·기기·키 기록(6개월), 운영자 기록이고, 백업은 14일 뒤 사라진다고 요청한 사람에게 알립니다. App Review 계정을 이렇게 지우면 명단 항목도 지워지고 데모 PC도 풀리므로(1.9.2), 심사가 아직 남았으면 7-17의 3번으로 다시 넣고 데모 PC를 "데모 PC 묶기"의 3단계로 다시 묶습니다. 1.9.1 앱을 쓰는 사람에게는 앱에서 직접 지울 수 있다고(설정 → 계정 → [계정 삭제]) 알려도 됩니다.
- **열람 요청**(사람 운영자만, 운영 에이전트는 하지 않음): `users --reveal`, `devices --reveal`, `key list tag:<T> --reveal`, `binding list --reveal`, `relay list --reveal`로 그 사람의 것을 봅니다. 로그인 기록(`events`), 켜기 기록(`wake_jobs`), 켜기 정보(`wol_info`), PC마다 마지막 동기화와 그 주소(`devices --reveal`에 없음)는 관리 명령이 없으므로, root가 DB를 읽기 전용으로 열어 아래 블록으로 봅니다. 서버에 `sqlite3` 명령이 없어도 되도록(이 런북은 그 패키지를 설치하지 않음) python3의 sqlite3 모듈로 열고(`mode=ro`, 쓰기는 거부됨), 값은 SQL 글에 넣지 않고 `?`로 묶습니다. 꼬리표는 인자로, 포탈 ID는 표준 입력으로 주므로 포탈 ID가 명령줄과 sudo 기록에 남지 않습니다(위 "대상 가리키기"). 포탈 ID는 hjv-api처럼 앞뒤 공백을 빼고 소문자로 바꿔 찾으므로, 대문자로 쳐도 됩니다. 결과는 요청한 사람에게만 전하고 서버나 대화에 남기지 않습니다.

```bash
T='<꼬리표>'                              # ← users --reveal로 찾은 꼬리표
read -r -p '포탈 ID: ' ID                # 명령줄과 셸 기록에 남기지 않음
printf '%s\n' "$ID" | sudo python3 -I -B -c 'import sqlite3, sys
tag, key = sys.argv[1], sys.stdin.readline().strip().lower()
db = sqlite3.connect("file:/var/lib/hjviewer-api/hjv-api.sqlite3?mode=ro", uri=True)
for name, sql, arg in (
        ("events", "SELECT at, kind, device_tag, ip, result FROM events WHERE account_tag = ?", tag),
        ("wake_jobs", "SELECT created_at, device_id, relay_id, state, finished_at FROM wake_jobs WHERE account_tag = ?", tag),
        ("wol_info", "SELECT w.device_id, w.reported_at, w.info_json FROM wol_info w JOIN devices d USING (device_id) JOIN accounts a ON a.id = d.account_id WHERE a.login_key = ?", key),
        ("devices", "SELECT d.device_id, d.hostname, d.last_sync_at, d.last_sync_ip FROM devices d JOIN accounts a ON a.id = d.account_id WHERE a.login_key = ?", key)):
    print("==", name)
    for row in db.execute(sql, (arg,)):
        print(*row, sep="\t")' "$T"; unset ID
```

- **최고운영자 정하기**(사용자 승인): 최고운영자는 앱에서 이 서비스의 사용자와 PC(이름, 주소)를 보기만 하는 사람입니다([server/api/README.md](../server/api/README.md)의 "Admin overview"). 누구로 할지는 사용자가 정하고, 포탈 ID는 저장소에도 대화에도 두지 않으므로 사용자가 서버에서 직접 넣습니다: `sudoedit /etc/hjviewer/hjviewer-api.env`에서 `HJV_API_OPERATORS=` 줄 끝에 포탈 ID를 적습니다(여럿이면 쉼표로). 운영 에이전트는 사용자 승인을 받아 조용한 시간에 `sudo systemctl restart hjviewer-api`와 첫 줄의 확인만 합니다(`sudo /opt/hjviewer/bin/hjv-api admin operators | head -1`, 7-0). 그 첫 줄이 `super operators (HJV_API_OPERATORS): 1 configured, 1 with an account`이고, 사용자가 본 그 사람의 줄에서 `account`와 `allowlist`가 `yes`이면 됩니다(아직 로그인하지 않았으면 `account`가 `no`이니 로그인한 뒤 다시 봅니다. 열린 로그인에서는 처음 로그인할 때 명단에 저절로 올라 `allowlist`가 `yes`가 됩니다. `blocked`이면 막힌 것이니 사용자가 `unblock`합니다. 명단 방식(`HJV_API_LOGIN=list`)에서 `no`이면 사용자가 `allow import`로 넣습니다). 출력의 ID는 가려져 있고, 그 줄의 꼬리표가 운영자 기록에 남는 `cmd="app overview" acct=<꼬리표>` 줄의 것입니다. 그만두게 할 때는 그 줄의 값을 지우고 같은 방법으로 다시 시작합니다. 볼 때마다 운영자 기록과 저널에 한 줄씩 남는다는 것은 개인정보 처리방침에 있습니다([privacy.md](privacy.md) 3절, 9절).
- **SSO 자격 증명이 바뀔 때**: hjv-api는 같은 서버의 WordPress SSO 로그인 플러그인(projecttf-auth)과 같은 SSO 등록을 씁니다. 그 플러그인의 client secret이나 api uuid가 바뀌면 같은 날 사용자가 7-4의 (나)처럼 `hjviewer-sso-client-secret`(또는 `hjviewer-sso-api-uuid`)를 다시 만들고 `sudo systemctl restart hjviewer-api`를 합니다. 그대로 두면 어느 값이 바뀌었는지에 따라 다르게 드러납니다. client secret이 바뀌었으면 토큰 호출이 `invalid_client`로 거절되어 모든 로그인이 "연세 인증 서버에 지금 연결할 수 없습니다"가 되고 감시가 `sso` 경보를 냅니다. api uuid(또는 설정 파일의 `HJV_API_SSO_CLIENT_ID`)가 바뀌었으면 사용자 정보 호출이 모두를 거절할 수 있어, 맞는 비밀번호에도 '이 계정으로는 HJViewer를 쓸 수 없습니다. …'가 나오고 감시가 `sso_user` 경보를 냅니다(7-13의 "모두가 거절될 때").
- **목록 키 바꾸기**(목록 키 1번이 샜거나 잃었을 때):
  1. 사용자가 오프라인 매체의 목록 키 2번(`hjviewer-list-2.key`)을 7-4의 (가) 3번처럼 표준 입력으로 넣습니다. 자격 증명 이름은 그대로 `hjviewer-api-list-key`이고(명령의 `--name=`과 파일 이름만 그것으로 바꾸고 `--tpm2-pcrs=`(7-4에서 `--with-key=host`를 골랐으면 그것)는 그대로 둠), 그 안의 키가 2번으로 바뀝니다.
  2. 설정 파일에 `HJV_API_LIST_KID=2`를 넣고 `sudo systemctl restart hjviewer-api`, `admin selfcheck`로 확인합니다(`HJV_API_TRUST_PUBKEYS`는 그대로 `1:…,2:…`).
  3. 모든 1.6.x 앱은 이미 2번을 믿습니다. PC는 2번 목록을 한 번 받으면 1번 목록을 다시 받지 않고, 서버에 닿는 PC는 늦어도 5분 안에 받습니다.
  4. 다음 앱 빌드는 2번과 새 3번을 고정하고, 그 빌드가 나가면 서버의 `HJV_API_TRUST_PUBKEYS`도 `2:…,3:…`로 바꿉니다. `report`의 `PCs by app version`으로 아직 옛 빌드인 PC를 봅니다.
- **명령 키 바꾸기**(명령 키 1번이 샜거나 잃었을 때): 같은 방법으로 `hjviewer-command-2.key`를 `hjviewer-api-command-key`에 넣고 `HJV_API_COMMAND_KID=2`, 다시 시작한 뒤 `admin selfcheck`의 명령 키 두 줄을 봅니다([relay/PROTOCOL.md](../relay/PROTOCOL.md) 3.2). hjv-relay 빌드가 1번과 2번을 함께 고정하므로 NAS는 건드리지 않고, 승인한 중계기는 다시 승인받지 않습니다. 다음 hjv-relay 빌드가 2번과 새 3번을 고정합니다.
- **봉인 키 바꾸기**: 자격 증명 `hjviewer-api-login-key`는 여러 줄을 받고 첫 줄이 지금 키입니다. 사용자가 새 키 파일과 옛 키 파일(오프라인 매체)을 이 순서로 이어(`cat 새 옛`) 7-4의 (가) 3번처럼(`--tpm2-pcrs=`나 7-4에서 고른 `--with-key=host` 포함) 넣고 `sudo systemctl restart hjviewer-api`를 합니다. 새 공개키로 빌드한 앱이 모두 나간 뒤 새 키만 다시 넣습니다.

### 7-16. 보고 (7절)

아래 형식으로 알려 주세요. 포탈 ID, 성명, IP 주소, 기기 이름은 적지 않습니다.

```
단계: <끝낸 단계 번호>
hjv-api: <hjv-api version의 판과 커밋>, 소켓 <권한 소유자>, DB 600(예/아니오), operator.log 추가 전용(예/아니오)
selfcheck: [ OK ] <n>줄, [FAIL] <n>줄(<어느 항목>), 고정 목록 키 지문 <1:…, 2:…>, 고정 명령 키 지문 <1:…, 2:…>(공개값)
스위치: passwordless <off | on>(모든 계정), 계정별로 바꾼 계정 <n>, strict <off | on>(passwordless status | head -2)
로그인: <open | list>, 명단: <로그인할 수 있는 n, 그중 로그인으로 오른 n, 막힌 n>(report 둘째·셋째 줄)
nginx: /api/hjv/source <코드>, /api/login-options <코드와 본문>, /api/heartbeat <코드>, /api/x.php <코드>, 다른 사이트 응답 코드 변화 없음(예/아니오), /ws/id·/ws/relay 101(예/아니오)
report: 계정 <n>, 활성 키 <n>, 묶음을 가진 PC <n>, 그중 10분 안에 동기화 <n>, 스위치 뒤 아직 동기화하지 않은 PC <n>, needs-relogin <n>
감시·백업: 타이머 활성(예/아니오), 마지막 백업 <sudo ls -t /var/lib/hjviewer-api/backup | head -1 의 파일 이름에 있는 UTC 시각>, 경보 경로 <텔레그램 | 저널>
중계기: <사용 중 n, 대기 n, 폐기 n, 미리 승인한 코드 n>(relay list 첫 줄)
바꾼 것: <설치한 파일, 만든 사용자, 고친 설정 파일, 드롭인>
문제: <있으면>
```

### 7-17. App Review 데모 계정 (`/etc/hjviewer/review.conf`)

Apple의 App Review가 iOS 앱 심사에 쓸 데모 계정을 요구했습니다. 사용자 결정(2026-10-16): ID와 비밀번호는 서버의 이 파일에만 있고, 비밀번호는 해시로만 둡니다. 이 계정은 로그인만 되고 파일의 `UNTIL`에 끝납니다. 연세 SSO를 부르지 않고, 새 버전을 받지 못하고, 자기가 만들지 않은 계정에는 들어가지 못합니다. PC는 1.9.2의 hjv-api(9-11)부터 파일의 `PC=` 줄에 적은 데모 PC만 이 계정에 묶을 수 있고(소유자 결정 2026-10-28), 그 밖의 PC는 목록에 올리지 못합니다. 동작은 [server/api/README.md](../server/api/README.md)의 "The App Review account", 심사에 넣는 값은 [distribution.md](distribution.md)의 "iOS: 심사용 데모 계정"에 있습니다.

1.9.1부터 심사자는 앱에서 이 계정을 지울 수 있습니다(지침 5.1.1(v), 설정 → 계정 → [계정 삭제]). 계정과 키·토큰은 다른 계정처럼 지워지지만 명단 항목은 남고 파일도 그대로이므로, 다음 심사 로그인은 이름 "App Review"의 빈 계정으로 시작합니다. 1.9.2부터는 이 계정이 가진 데모 PC가 같은 트랜잭션에서 만든 같은 ID의 새 빈 계정으로 넘어가, 묶음과 출입증이 이어지고 데모 PC가 온라인으로 남습니다(저널 줄 끝에 `, 1 demo PCs kept`). 심사자의 내 PC 목록에는 데모 PC가 보이지 않고(예전처럼 빈 목록), 심사자는 데모 PC의 ID와 영구 비밀번호로 연결합니다. 운영자가 할 일은 없습니다(저널 줄은 5번).

- 파일은 사용자가 에이전트 세션이 아닌 자기 터미널(Linux, macOS나 WSL)에서 `python3 -I deploy/review-conf.py --until <YYYY-MM-DD> --out ~/review.conf [--pc <데모 PC ID>]...`로 만들어(ID와 비밀번호는 도구가 터미널에서 물음) 운영 에이전트의 홈으로 보냅니다. 형식은 [deploy/review.conf.example](../deploy/review.conf.example)에 있습니다.
- **데모 PC 줄**(1.8.0, 교직원 전용): 출입 확인을 켠 뒤(`access.conf`의 `MODE=enforce`) 이 계정은 `PC=` 줄의 PC(1~3대)에만 연결합니다. 이 줄이 없으면 아무 PC에도 연결하지 못합니다. `--pc`를 주면 도구가 이 줄을 씁니다. 데모 PC를 둘지, 어느 PC로 할지는 사용자가 정합니다(추천: 웹캠을 붙인 전용 물리 Windows PC 한 대. 심사 메모가 카메라 보기가 열린다고 말하기 때문입니다. VM은 웹캠이 없어 카메라 보기가 "No cameras" 오류이므로, 심사 메모의 웹캠 없음 변형(store/README.md의 "심사 메모")과 함께만 씁니다. 운영자의 업무 PC는 쓰지 않음). 이 줄은 출입증을 아는 새 hjv-api만 읽습니다. 옛 hjv-api는 이 줄이 있는 파일 전체를 거부해 계정이 없어지므로, 새 hjv-api를 넣은 뒤에 이 줄이 든 파일을 넣습니다.
- **데모 PC의 묶음**(1.9.2, 9-11의 hjv-api부터): 데모 PC는 교직원 계정이 아니라 **이 계정으로 묶습니다**(아래 "데모 PC 묶기"). 묶인 데모 PC는 동기화마다 등록만 하는 출입증(`staff`/`pc`, 24시간, `UNTIL`을 넘지 않음)을 받아 재부팅해도 온라인이고, 심사자가 앱에서 이 계정을 지워도, 운영자가 `user revoke-all`을 해도, hjv-api가 다시 시작해도 그대로 남습니다. 비밀번호 없는 연결은 받지 않으므로(스위치와 상관없이) 심사자는 영구 비밀번호로 연결합니다. 교직원 계정에 묶어 둔 PC는 그 계정의 것으로 남고(이 계정의 로그인은 `other_account`) 그 계정을 앱에서 지우면 풀리므로, 먼저 그 PC에서 그 교직원 로그인을 로그아웃해 풉니다. 1.9.2 전의 hjv-api에서는 이 계정이 PC를 묶지 못합니다.
- 운영 에이전트는 이 파일을 출력하지 않고(`cat`, `less` 등), ID를 보고·대화·명령 인자에 옮기지 않습니다. 옮겨도 되는 것은 `UNTIL`과 저널의 꼬리표뿐입니다. 이 계정으로 로그인해 보지 않습니다. 확인은 사용자가 앱으로 합니다(4번).
- 아래 명령의 `T`는 이 계정의 꼬리표입니다(`T='<꼬리표>'`로 둠). 2번의 read 줄에 처음 나오고, 보고(8번)마다 적습니다. 저널은 30일만 남으므로 그 뒤에는 보고에 적은 값을 씁니다. 저널에 아직 있으면 `sudo journalctl --namespace=hjviewer -u hjviewer-api -o cat | grep 'App Review file (HJV_API_REVIEW_FILE) read:' | grep -o 'acct=[0-9a-f]\{16\}' | tail -1`로도 얻습니다.
- **데모 PC 묶기**(1.9.2, 사용자와 운영 에이전트, 9-11의 hjv-api가 돈 뒤): 순서가 중요합니다. `PC=` 줄의 ID는 데모 자격 증명을 가진 누구나 이 계정에 묶을 수 있고, hbbs는 처음 등록한 키로 그 ID를 기억하므로, **아직 hbbs가 모르는 ID를 미리 넣으면** 그 ID를 먼저 자기 키로 등록한 사람이 그 ID를 이 계정에 묶어 심사자를 자기 PC로 부를 수 있습니다.
  1. 사용자가 데모 PC에 HJViewer 1.9.2를 설치하고 켭니다. 출입 확인이 켜진 서버는 아직 그 PC의 등록을 받지 않지만(오프라인) 그 키는 기억합니다. 사용자가 데모 PC의 ID를 메모해 둡니다(보고와 대화에는 적지 않음).
  2. 사용자가 `--pc <그 ID>`로 파일을 만들어 보내고, 운영 에이전트가 1번 블록으로 넣은 뒤(지금 파일의 `PC=` 줄과 다르면 블록이 멈추므로, 데모 PC를 넣는 파일이라고 사용자가 확인한 뒤 `OK_PC=1`로), `PC=`의 ID마다 hbbs가 키를 아는지 봅니다. ID는 파일에서 바로 넘기므로 화면에 나오지 않습니다. `PC=`의 ID마다 한 줄이 나오고 모두 `known`이어야 합니다. `unknown`이 하나라도 있으면 멈추고 보고합니다(1단계가 아직이거나 ID가 틀림). `no answer`는 hbbs 콘솔이 답하지 않은 것이니 멈추고 보고합니다.
     ```bash
     sudo sed -n 's/^PC=//p' /etc/hjviewer/review.conf | tr ',' '\n' | while read -r id; do
       timeout 3 bash -c 'exec 3<>/dev/tcp/127.0.0.1/21115 && printf "hjv-pk %s" "$1" >&3 && cat <&3' _ "$id" |
         awk 'NR == 1 { print ($3 == "-" ? "unknown" : "known") } END { if (NR == 0) print "no answer" }'
     done
     ```
  3. 바로 사용자가 데모 PC의 HJViewer에서 이 계정으로 로그인합니다. 안내 창에 "이 PC를 내 PC 목록에 추가했습니다."가 나오고([이 PC 빼기] 단추는 없음), 1~2분 안에 PC가 온라인이 됩니다(창 왼쪽의 상태 줄).
  4. 운영 에이전트가 **데모 PC 확인**을 합니다(사용자가 묶었다고 한 뒤, 그리고 심사 중 매일).
     ```bash
     A="sudo /opt/hjviewer/bin/hjv-api admin"
     $A binding list | awk -v t="$T" '$3 == t { print $4, $5, $6 }'   # 데모 PC마다 한 줄: disabled present <마지막 동기화, UTC 날짜>(오늘, UTC). ID와 꼬리표는 찍지 않음
     $A ticket status | grep -o 'sync pc [0-9]*'        # 데모 PC의 출입증 수(시작한 뒤). 몇 분 뒤 다시 보면 늘어 있어야 함
     sudo sed -n 's/^PC=//p' /etc/hjviewer/review.conf | tr ',' '\n' | while read -r id; do
       timeout 3 bash -c 'exec 3<>/dev/tcp/127.0.0.1/21115 && printf "hjv-pk %s" "$1" >&3 && cat <&3' _ "$id" |
         awk 'NR == 1 { print ($4 == "-" || $4 > 90 ? "offline" : "online") } END { if (NR == 0) print "no answer" }'
     done                                               # PC= 줄의 ID마다 한 줄: 모두 online(hbbs가 90초 안에 등록을 받음)
     sudo journalctl --namespace=hjviewer -u hjviewer-api --since today -o cat | grep -c "login: a demo PC joined the App Review account (acct=$T"   # 사용자가 묶은 수와 같아야 함
     ```
     첫 줄이 아무것도 찍지 않으면 묶이지 않은 것입니다(3단계의 로그인이 `not_eligible`이면 그 PC가 `PC=`에 없거나 설치형이 아님, `check_failed`이면 hbbs가 아는 키와 다름, `other_account`이면 다른 계정에 묶여 있음). `joined` 줄이 사용자가 묶은 수보다 많으면 누가 다른 때에 묶은 것이므로 바로 보고합니다. 사용자가 `binding list`에서 그 PC를 찾아 `binding revoke "tag:<그 PC의 꼬리표>"`로 풉니다(7-0: 운영 에이전트는 그 꼬리표를 보지 않음). `sync pc`가 늘지 않으면 데모 PC가 동기화하지 않거나(꺼짐, 네트워크) 파일이 활성이 아닙니다(2번 블록의 read 줄). 셋째 블록은 hbbs가 그 PC의 등록을 받았는지(온라인)를 봅니다. `hjv-pk` 답의 넷째 칸은 hbbs가 마지막으로 받아들인 등록 뒤의 초이고(한 시간 안이 아니면 `-`), 출입증 없이 거절된 등록은 이 시각을 바꾸지 않습니다. 5분 점검(`hjv-api check`)도 같은 값으로 온라인을 봅니다. 묶음과 출입증이 있는데 `offline`이면 hbbs가 그 출입증을 받지 않는 것이므로(`NOT_BEFORE`를 옮긴 직후라면 다음 동기화까지 몇 분 기다림) 바로 보고합니다. `no answer`는 hbbs 콘솔이 답하지 않은 것입니다. 마지막 동기화 칸은 UTC 날짜라서 한국 시각 오전 9시 전에는 정상이어도 어제 날짜로 보이고, 오늘 아침에 끊긴 PC도 오늘로 보이므로, 온라인은 셋째 블록으로 판단합니다.
  5. 심사 중에는 데모 PC의 HJViewer에서 **어떤 계정으로도 로그인하거나 로그아웃하지 않습니다.** 데스크톱의 [로그아웃]은 앱이 요청 전에 스스로 묶음을 풀어 서버가 막지 못합니다(소유자가 받아들인 위험). 4번의 확인(iPhone의 [계정 삭제])이 데스크톱의 로그인도 끝내므로, 그 뒤 데모 PC의 HJViewer는 로그인되지 않은 채 묶음만 가집니다. 데스크톱은 그것을 다음 계정 요청의 401에서야 알고 그때 "이 기기의 로그인이 끝났습니다" 창을 띄우므로, [계정 삭제] 바로 뒤 데모 PC에서 설정 → 계정을 새로 열어 그 창을 지금 띄우고 [닫기]로 닫습니다(7-17의 4번). 다시 묶어야 하면 3단계부터 합니다.
  6. 아직 준비되지 않은 둘째 데모 PC의 ID는 넣지 않습니다. 더할 때는 1단계부터 하고, 새 파일(1번 블록, `PC=` 줄이 바뀌므로 사용자 확인 뒤 `OK_PC=1`) 뒤에 바로 3단계를 합니다. `PC=`에서 뺀 PC는 계정에 남으므로(출입증은 받지 않음) 사용자가 `binding list`에서 그 PC의 꼬리표를 찾아 `binding revoke "tag:<그 PC의 꼬리표>"`로 풉니다(7-0).
  7. 심사 중에 데모 ID를 바꾸면(새 파일의 ID가 다름) 옛 ID의 계정이 데모 PC를 가진 채 출입증을 받지 못하고, 새 ID는 그 PC를 묶지 못합니다(`other_account`). 먼저 `$A user delete "tag:<옛 T>"`로 데모 PC를 풀고, 새 파일을 넣은 뒤 데모 PC에서 새 ID로 다시 묶습니다(3단계).

1. **넣기**(사용자 승인 뒤). 바꿀 때(새 `UNTIL`, 새 비밀번호)도 같은 블록이고, 다시 시작하지 않습니다.
   ```bash
   SRC=/home/<사용자>/review.conf                  # ← 사용자가 보낸 파일
   OK_PC=                                          # ← PC= 줄을 바꾸는 파일이라고 사용자가 확인했을 때만 OK_PC=1
   [[ $SRC == /home/*/* ]] && sudo test -f "$SRC" && ! sudo test -L "$SRC" || { echo "STOP: $SRC 이(가) 홈의 보통 파일이 아님"; exit 1; }
   [ "$(sudo grep -cE '^(ID|HASH|UNTIL)=' "$SRC")" = 3 ] && [ "$(sudo grep -c '^HASH=pbkdf2-sha256\$' "$SRC")" = 1 ] || { echo "STOP: $SRC 이(가) review-conf.py가 쓴 파일이 아님"; exit 1; }
   if sudo test -f /etc/hjviewer/review.conf && [ -z "$OK_PC" ] &&
      [ "$(sudo sed -n 's/^PC=//p' "$SRC" | sha256sum)" != "$(sudo sed -n 's/^PC=//p' /etc/hjviewer/review.conf | sha256sum)" ]; then
     echo 'STOP: PC= 줄이 지금 파일과 다름(데모 PC의 수나 ID). 사용자 확인 뒤 OK_PC=1로 다시'; exit 1
   fi
   sudo sed -n 's/^UNTIL=//p' "$SRC"                           # 끝나는 시각(UTC)
   sudo sed -n 's/^PC=//p' "$SRC" | tr ',' '\n' | grep -c .    # 데모 PC의 수(0~3, 0이면 줄 없음)
   sudo install -o root -g hjviewer-api -m 0640 "$SRC" /etc/hjviewer/review.conf.new &&
     sudo mv -f /etc/hjviewer/review.conf.new /etc/hjviewer/review.conf && sudo rm -f "$SRC"
   ```
   새 `UNTIL`이나 새 비밀번호로 다시 만든 파일은 `PC=` 줄이 지금 파일과 같아야 합니다(도구에 같은 `--pc`를 같은 순서로). 다르면 블록이 ID를 보이지 않고 멈춥니다. 사용자가 데모 PC를 더하거나 빼는 것이라고 확인하면("데모 PC 묶기"의 6단계) `OK_PC=1`로 다시 합니다. 심사 중에 `PC=` 줄이 빠진 파일이 들어가면 데모 PC는 새 출입증을 받지 못해 최대 24시간 뒤 오프라인이 되고, 그 사이 심사자가 앱에서 계정을 지우면 데모 PC가 계정과 함께 풀립니다. 바꾼 파일은 이 계정의 다음 로그인이나 요청 때 읽힙니다(교직원 로그인으로는 읽히지 않음). 그래서 바꾼 뒤에는 사용자가 앱으로 한 번 로그인해 확인하고(심사 중이면 4번과 달리 계정은 지우지 않음), 운영 에이전트는 그 뒤 저널의 read 줄을 봅니다(2번 블록의 마지막 줄, `--since`만 넓혀서). 새 비밀번호로 바꾼 파일이면(사용자가 알려 줌) 옛 비밀번호로 받은 세션은 파일을 바꿔도 끝나지 않으므로 `sudo /opt/hjviewer/bin/hjv-api admin user revoke-all "tag:$T"`로 끝냅니다.
2. **새 hjv-api와 설정**(처음 한 번, 사용자 승인 뒤 조용한 시간에). 이 계정은 `HJV_API_REVIEW_FILE`을 아는 hjv-api에서만 됩니다. 2026-10-15부터 운영 중인 hjv-api(3b283b2, 7-14)는 이 이름을 몰라서, 설정 파일에 이 줄이 있으면 `unknown setting HJV_API_REVIEW_FILE`로 시작하지 않고, 그동안 로그인과 PC 동기화가 모두 멈춥니다. 그래서 먼저 이 기능이 든 새 서버 묶음으로 hjv-api만 바꿉니다. 7-14의 "업그레이드"대로 스냅숏, 옛 실행 파일, 설정 파일을 남긴 뒤 6절 "B 경로 업데이트"의 4번을 합니다(hbbs와 hbbr은 다시 시작하지 않음). 그다음 아래 블록으로 설정 줄을 넣고 다시 시작합니다. 첫 줄은 설치된 hjv-api가 이 이름을 아는지 봅니다.
   ```bash
   sudo grep -qa HJV_API_REVIEW_FILE /opt/hjviewer/bin/hjv-api || { echo 'STOP: 설치된 hjv-api가 HJV_API_REVIEW_FILE을 모름(먼저 새 묶음으로 바꿈)'; exit 1; }
   E=/etc/hjviewer/hjviewer-api.env
   if sudo grep -q '^#\?HJV_API_REVIEW_FILE=' "$E"; then sudo sed -i 's|^#\?HJV_API_REVIEW_FILE=.*|HJV_API_REVIEW_FILE=/etc/hjviewer/review.conf|' "$E"; else echo 'HJV_API_REVIEW_FILE=/etc/hjviewer/review.conf' | sudo tee -a "$E" > /dev/null; fi
   sudo grep -n '^HJV_API_REVIEW_FILE=' "$E"
   sudo systemctl restart hjviewer-api
   sleep 3; systemctl is-active hjviewer-api                   # active
   sudo journalctl --namespace=hjviewer -u hjviewer-api --since -1min -o cat | grep -e 'App Review file (HJV_API_REVIEW_FILE)' -e 'no App Review account' | tail -2
   ```
   - `active`가 아니면 7-5의 마지막 항목처럼 오류 줄(`-p err`, 꼬리표가 든 줄은 거름)을 사용자에게만 보여 주고, `sudo sed -i '/^HJV_API_REVIEW_FILE=/d' "$E"`로 그 줄을 지운 뒤 다시 시작해 `active`인지 봅니다.
   - `login: the App Review file (HJV_API_REVIEW_FILE) read: until <UTC>, <n> demo PC(s) (acct=<꼬리표>)`가 나오고, 그 시각이 지금보다 뒤이고, 그 뒤에 `login: no App Review account (…)` 줄이 없어야 합니다. 이 꼬리표가 `T`입니다. `<n>`은 `PC=` 줄의 PC 수입니다(1번 블록에서 센 수와 같아야 함).
   - `… is not used: <이유>`이면 파일이 거부된 것입니다(이유에 ID나 해시는 없음). 그 줄을 보고하고 사용자가 파일을 다시 만듭니다.
   - `login: no App Review account (…)`만 나오면 파일이 없습니다. read 줄(지난 시각)과 이 줄이 함께 나오면 `UNTIL`이 지났습니다.
3. **명단**(두 로그인 방식 모두): 그 ID를 명단에 미리 넣습니다. 명단에 없는 ID는 열린 로그인에서도 첫 로그인 전까지 `HJV_API_LIMIT_NEW_ID`(하루 실패 2번)를 받아, 심사자가 비밀번호를 두 번 틀리면 하루 동안 맞는 비밀번호로도 로그인하지 못합니다. 명단에 있으면 다른 ID처럼 `HJV_API_LIMIT_ID`(10분에 3번, 하루 5번)입니다. 명단 방식에서는 넣어야 로그인합니다. ID는 파일에서 바로 넘기므로 화면과 명령줄에 나오지 않습니다.
   ```bash
   sudo sed -n 's/^ID=//p' /etc/hjviewer/review.conf | sudo /opt/hjviewer/bin/hjv-api admin allow import /dev/stdin   # allowlist: 1 added, …
   ```
4. **확인**(사용자, 처음 제출하기 전, 데모 PC를 묶은 뒤): 사용자가 App Store Connect에 넣을 값 그대로 iPhone이나 iPad의 앱으로 로그인해 봅니다(distribution.md의 "iOS: 심사용 데모 계정" 2번). 내 PC 목록은 비어 있어야 하고, 로그인한 채로 데모 PC에 그 PC의 ID와 영구 비밀번호로 연결해 봅니다(비밀번호 없이 들어가지 않고 비밀번호를 물어야 함). 출입 확인이 켜진 서버에서는 다른 PC에 연결하면 거절(A2)되어야 합니다. 그다음 그 앱의 **[계정 삭제]로 끝냅니다.** 확인한 기기와 데모 PC 데스크톱의 로그인이 심사자의 기기에 "새 기기" 알림이나 지난 기기로 뜨지 않게 하려는 것이고, 명단 항목은 남고 데모 PC는 새 빈 계정으로 넘어가 온라인으로 남습니다. **[계정 삭제] 바로 뒤 데모 PC에서** HJViewer의 설정 → 계정을 새로 엽니다(홈의 계정 줄을 눌러도 되고, 열려 있던 설정 탭은 닫고 다시 엶). 설치된 PC의 계정 탭은 열릴 때 서버에 이 PC를 묻는 계정 요청(`GET /api/hjv/this-device`)을 보내고, 그 401로 "이 기기의 로그인이 끝났습니다" 창(L4)이 바로 뜹니다. [닫기]로 닫고([로그인]은 누르지 않음), 계정 탭에 로그인이 없고 창 왼쪽의 상태 줄이 온라인인지 봅니다. 그러지 않으면 데스크톱은 다음 계정 요청(로그인의 출입증을 그 수명의 반에 새로 받는 요청, 몇 시간 뒤일 수 있음)에서야 이 창을 띄워, 심사자 앞에 나올 수 있습니다. 사용자가 다시 로그인해 데모 PC에 한 번 더 연결해 보면 삭제 뒤의 모습까지 확인한 것이고, 그 로그인도 [계정 삭제]로 끝냅니다. 운영 에이전트는 저널에 `account: deleted at its own request (acct=<T>, the App Review account): … , <n> demo PCs kept` 줄이 있는지(5번의 넷째 줄), 데모 PC 확인(위 4단계)이 그대로인지 봅니다. **`user delete`와 명단 다시 넣기는 하지 않습니다**: `user delete`는 데모 PC를 풀어 오프라인으로 만듭니다. 로그아웃으로 끝냈으면 사용자 승인 뒤 `sudo /opt/hjviewer/bin/hjv-api admin user revoke-all "tag:$T"`로 그 로그인들을 끝냅니다(데모 PC는 남음).
   확인 로그인이 실패하고 `sudo journalctl --namespace=hjviewer -u hjviewer-api --since -1h -o cat | grep -c "names the ID of an account that is not the App Review account's"`가 0이 아니면, 그 ID의 계정이 이미 있습니다(규칙 전에 만들어진 계정). 그 줄의 꼬리표는 다른 계정의 것이라 줄 수만 셉니다(7-0). 이 계정은 그 계정에 들어가지 않으므로, 사용자가 다른 ID로 파일을 다시 만듭니다.
5. **저널 보기**: 매일 보고에 이 계정의 줄 수만 넣습니다. 틀린 비밀번호가 ID의 실패 한도를 채우면 심사자도 그동안 로그인하지 못합니다. 셋째 줄(`login: ID limit reached (acct=<꼬리표>, …)`)이 0이 아니면 바로 사용자에게 알리고, 사용자 승인 뒤 `sudo /opt/hjviewer/bin/hjv-api admin unlock "tag:$T"`로 풉니다.
   ```bash
   J=$(sudo journalctl --namespace=hjviewer -u hjviewer-api --since yesterday -o cat)
   echo "$J" | grep -c 'login: the App Review account logged in'
   echo "$J" | grep -c 'login: a wrong password for the App Review account'
   echo "$J" | grep -c "login: ID limit reached (acct=$T"
   echo "$J" | grep -c "account: deleted at its own request (acct=$T"   # 앱에서 이 계정을 지운 수(1.9.1)
   echo "$J" | grep -c "login: a demo PC joined the App Review account (acct=$T"   # 데모 PC를 묶은 수(1.9.2): 사용자가 묶은 때만 늘어야 함
   echo "$J" | grep -c "account delete: refused from a demo PC of the App Review account (acct=$T"   # 데모 PC 자신의 [계정 삭제](1.9.2, 거절됨)
   ```
   데모 PC를 묶은 줄이 사용자가 묶지 않은 날에 늘었으면 바로 보고합니다("데모 PC 묶기"의 4단계).
6. **끝내기**(심사가 끝났거나 사용자가 정한 때, 사용자 승인 뒤): 파일을 지우면 그 계정의 토큰이 바로 멈추고 앱은 다음 요청에서 로그아웃됩니다. 그다음 `admin user delete`로 계정(키, 토큰, 명단 항목)을 지웁니다. **데모 PC를 끝내는 것도 이 `user delete`입니다**: 데모 PC가 풀리고 다음 동기화(보통 2분)에 서명된 `revoked`를 받아 몇 분 안에 오프라인이 됩니다. 파일만 지우면 데모 PC는 새 출입증을 받지 못할 뿐, 받아 둔 출입증으로 최대 24시간(`UNTIL`까지) 더 등록되어 있습니다. `HJV_API_REVIEW_FILE` 줄은 두어도 됩니다(파일이 없으면 계정도 없음). hjv-api를 이 기능 전의 판으로 되돌릴 때만 지웁니다(7번).
   ```bash
   sudo rm /etc/hjviewer/review.conf
   sudo /opt/hjviewer/bin/hjv-api admin user delete "tag:$T"
   ```
   심사자가 앱에서 이미 지웠어도 데모 PC를 가진 새 계정이 남으므로 `user delete`는 그 계정과 데모 PC를 함께 지우고 풉니다(`deleted the account with <n> devices, …`). 데모 PC가 없던 계정을 심사자가 지웠고 그 뒤 다시 로그인하지 않았으면 `no such account`로 끝나고 명단 항목만 남습니다. 사람의 포탈 ID가 아니므로 그대로 둡니다(계정 없는 항목은 명단에 오른 지 1년 뒤의 정리가 지움). 끝낸 뒤 `sudo /opt/hjviewer/bin/hjv-api admin binding list | awk -v t="$T" '$3 == t' | wc -l`이 0인지 봅니다.
7. **되돌리기**(hjv-api를 이 기능 전의 판으로 되돌릴 때, 사용자 승인 뒤): 옛 판은 `HJV_API_REVIEW_FILE`을 몰라 그 줄이 있으면 시작하지 않고, 이 계정을 따로 다루지도 않습니다. 옛 판에서는 이 계정의 토큰이 파일이나 `UNTIL`과 상관없이 보통 토큰처럼 통하고(쓰지 않아도 90일), 새 버전과 짧은 링크도 받습니다. 그래서 새 판이 도는 동안 먼저 계정을 지우고, 설정 줄을 지운 뒤 옛 실행 파일로 바꿉니다. 1.9.2의 hjv-api(9-11)를 그 전 판(1.9.1의 b389ea2 등)으로만 되돌릴 때도 **먼저 데모 PC를 풉니다**(아래 블록의 `user delete`, 또는 사용자의 `binding revoke`, 7-0). 옛 판은 PC를 가진 이 계정의 로그인을 `review-existing`으로 거절하고, 묶인 데모 PC에는 교직원 PC처럼 `staff`/`both`를 주기 때문입니다.
   ```bash
   sudo rm -f /etc/hjviewer/review.conf
   sudo /opt/hjviewer/bin/hjv-api admin user delete "tag:$T"   # 이미 지웠으면 no such account
   sudo sed -i '/^HJV_API_REVIEW_FILE=/d' /etc/hjviewer/hjviewer-api.env
   sudo install -m 755 /root/hjv-backup/hjv-api-before-<VERSION> /opt/hjviewer/bin/hjv-api   # ← 2번 전에 남긴 옛 실행 파일
   sudo systemctl restart hjviewer-api
   sleep 3; systemctl is-active hjviewer-api                   # active
   sudo /opt/hjviewer/bin/hjv-api admin selfcheck
   ```
   새 판이 3b283b2에 이 기능만 더한 판이면 DB 스키마가 같아 이 블록으로 끝납니다. 다른 서버 변경도 함께 들어간 판이면 앞의 세 줄 뒤에 7-14의 "업그레이드"대로 옛 실행 파일, 스냅숏, 설정 파일을 함께 되돌립니다.
8. **보고**: `review.conf <넣음 | 바꿈 | 지움>, UNTIL <UTC>, 데모 PC <n>대(binding list의 묶음 <n>, disabled present <n>), hbbs online <n>/<n>, sync pc <n>, acct=<꼬리표>, 저널 <read | is not used <이유> | no App Review account>, 어제 로그인 <n>줄, 틀린 비밀번호 <n>줄, ID 한도 <n>줄, 앱 삭제 <n>줄, 데모 PC 묶음 <n>줄, 데모 PC의 삭제 거절 <n>줄, user delete <했음 | 안 함 | no such account>`. ID와 비밀번호, 데모 PC의 ID는 적지 않습니다.

## 8. 최소 버전과 업데이트 배포 (1.6.0)

1.6.0부터 앱은 서명된 **버전 정책**을 받아 스스로 업데이트를 안내하거나 막고, PC는 오래된 컨트롤러의 로그인을 거절하며, hbbs는 1.5.1을 포함한 오래된 컨트롤러를 거절할 수 있습니다. 새 버전의 설치 파일은 hjv-api가 확인한 뒤 직접 보냅니다. 이 절은 그 서버 쪽 절차입니다. 7절의 계정 API를 마친(또는 같은 날 하는) B 경로 nginx 서버가 대상입니다. Caddy와 Apache는 8-9에 있습니다.

- 정책을 만들고 서명하는 쪽(개발 PC, 사용자)은 `tools/hjv-release/README.md`에 있습니다. 규칙의 근거는 최소 버전 설계(`build/minver/design.md`, 공개 저장소에는 없음)와 신뢰 규칙 명세(`build/minver/trust-rules/spec.md`)입니다.
- 서버에 두는 것:

| 위치 | 소유자·권한 | 내용 | 읽는 쪽 |
|---|---|---|---|
| `/etc/hjviewer/version-policy.json` | root:root 0644 | 서명된 정책(봉투). nginx가 `https://<도메인>/api/hjv/version`으로 그대로 내줌 | nginx, hjv-api |
| `/etc/hjviewer/update-keys.env` | root:root 0644 | 앱 빌드가 박은 업데이트 공개키와 하한. 공개 저장소 태그 `client-<v>`의 `client/update-keys.env` 그대로. 8-1의 3번에서 넣고 8-5의 (마)에서만 바꿈(운영 서버는 `client-1.6.1`의 것, 8-2) | 서버 확인 스크립트 |
| `/etc/hjviewer/min-controller.conf` | root:hjviewer 0640 | hbbs의 최소 컨트롤러 버전과 거절 문구(문 2 링크가 들어 있음) | hbbs |
| `/etc/hjviewer/door2.conf` | root:hjviewer-api 0640 | 문 2의 코드와 닫는 시각 | hjv-api |
| `/srv/hjviewer-updates/` | root:hjviewer-api 0750 | 업데이트 구역. WordPress 문서 루트와 `/var/lib/hjviewer` 밖 | — |
| `/srv/hjviewer-updates/pkg/` | root:hjviewer-api 0750, 파일 0640 | 패키지(`HJViewer-<v>-windows-x86_64.exe`, `HJViewer-<v>-android-arm64.apk`)와 `notices-<v>/`(0750) | hjv-api |
| `/srv/hjviewer-updates/incoming/` | root:root 0700 | 사용자가 보낸 것을 확인하고 남기는 곳: 정책 묶음은 `<버전>-<위치>/`(8-2), hbbs·문 2 파일은 `conf-<UTC 시각>/`(8-3) | 운영 에이전트 |
| `/opt/hjviewer/bin/hjv-update-verify.sh` | root 0755 | 서버 확인 스크립트(서버 묶음의 `bin/`, python3와 openssl만 씀) | 운영 에이전트 |

### 8-0. 이 절의 규칙

- **서명과 키는 사용자만** 다룹니다. 운영 에이전트는 `hjv-release sign`, 키 만들기, 키 파일 열기를 하지 않습니다. 루트 키는 사용자가 깨끗한 컴퓨터에서만 엽니다(도구 README 3장의 5-0).
- 정책 파일은 **서버 확인(`hjv-update-verify.sh`)을 통과한 것만** 넣습니다. 손으로 고치지 않습니다(서명이 깨져 앱이 거부합니다). 서버 확인이 거부한 파일은 어떤 이유로도 넣지 않습니다.
- **정책은 앞으로만 바꿉니다.** 옛 정책 파일로 되돌리거나 정책 파일을 지우지 않습니다. 되돌리기는 언제나 사용자가 서명한, `seq`가(또는 세대가) 더 높은 새 정책을 8-2로 넣는 것입니다(8-6). 앱은 이미 받은 것보다 오래된 정책을 받지 않고, hjv-api는 오래된 파일을 읽으면 저널에 ERROR(`… went back to epoch …`)를 남기면서 그 파일의 설치 파일을 내주어 앱이 찾는 버전과 어긋납니다. 파일이 없으면 앱의 확인은 404이고 hjv-api는 설치 파일을 내주지 않습니다.
- **위 둘의 예외는 하나입니다(8-5의 (라) 2번): 사용자가 서명한 적 없는 파일이 걸려 있을 때**(서명 키 도난). 그때만 사용자 승인을 받아 보관소의 마지막 내 파일로 되돌립니다. 그 파일은 걸린 도둑 파일과 비교하지 않고(비교하면 `Rollback` 등으로 거부됨) 그 파일 자체만 서버 확인으로 봅니다. 이때 hjv-api의 `went back` ERROR는 예상된 것입니다. 사용자가 서명한 정책을 고치는 일에는 이 예외를 쓰지 않습니다.
- `--relaxes`는 사용자가 `sign`의 출력으로 알려 준 항목만 줍니다. 알려 주지 않았으면 주지 않습니다.
- 순서는 **패키지 먼저, 정책 나중**입니다. 정책, hbbs 파일, 문 2 파일은 바꿔도 nginx reload나 서비스 재시작이 필요 없습니다(hbbs는 10초마다, hjv-api는 바뀔 때마다 다시 읽음). 그래도 운영 서버 변경이므로 사용자 승인을 받고 합니다.
- **문 2의 코드는 반쯤 비밀입니다.** 코드는 `door2.conf`와 `min-controller.conf`(TEXT의 링크) 두 파일에만 있습니다. 이 두 파일을 `cat`·`less`로 열거나 내용을 보고와 대화에 옮기지 않습니다. 확인은 아래 블록의 `grep`과 저널 줄(코드의 해시 앞 8자)로만 합니다.
- **hbbs를 trace 수준으로 돌리지 않습니다**(`RUST_LOG=trace`). 그 수준에서는 WebSocket 라이브러리가 모든 프레임을 16진수로 남기므로 거절 문구 속 문 2 링크가 로그에 남습니다. 이미 그렇게 돌렸으면 사용자에게 알리고 문 2의 코드를 바꿉니다(8-4).
- 같은 서버의 WordPress(php-fpm) 설정은 8-8의 선택 항목 말고는 건드리지 않습니다.

### 8-1. 처음 한 번: 업데이트 구역, 키 파일, 확인 스크립트

7절의 순서에 끼워 넣습니다. 1, 2, 4번은 7-3(hjv-api 설치) 뒤, **7-6(hbbs 교체)보다 먼저** 합니다. 그래야 hbbs를 한 번만 다시 시작합니다. 3번은 출시 태그(8단계) 뒤, 첫 정책(8-2, 14단계)보다 먼저 합니다. 3번이 받는 것은 출시 태그의 `client/update-keys.env`인데, 그 태그는 8단계에 생기고 첫 push의 그 파일은 키 줄이 비어 있기 때문입니다. nginx 위치(5번)는 7-8과 같은 reload에 들어갑니다.

1. 확인 스크립트를 설치하고 이 서버의 python3와 openssl로 돌아가는지 봅니다. 묶음은 7-3과 같은 Release입니다.
   ```bash
   VERSION=2026.10.20                # ← 7-3과 같은 서버 묶음의 Release 버전 (입력값)
   T=$(mktemp -d) && cd "$T" || exit 1
   base="https://github.com/yonsei-projecttf/hjviewer/releases/download/server-$VERSION"
   pkg="hjviewer-server-$VERSION-linux-x86_64"
   if ! curl -fsSLO --max-time 120 "$base/$pkg.tar.gz" || ! curl -fsSLO --max-time 30 "$base/SHA256SUMS"; then echo 'STOP: 내려받기 실패'; exit 1; fi
   grep " $pkg.tar.gz\$" SHA256SUMS | sha256sum -c - || { echo 'STOP: 체크섬이 다름'; exit 1; }
   tar -xzf "$pkg.tar.gz" && cd "$pkg" || exit 1
   [ -f bin/hjv-update-verify.sh ] || { echo 'STOP: 묶음에 bin/hjv-update-verify.sh가 없음'; exit 1; }
   sudo install -m 755 bin/hjv-update-verify.sh /opt/hjviewer/bin/
   /opt/hjviewer/bin/hjv-update-verify.sh --self-test
   cd / && rm -rf "$T"
   ```
   마지막 줄이 `... verify Ed25519 as the apps need`가 아니면 멈춥니다. python3나 openssl이 없거나, 이 openssl이 Ed25519를 RFC 8032대로 확인하지 못하는 것입니다.
2. 업데이트 구역을 만듭니다. `hjviewer-api` 그룹은 7-3에서 생겼습니다.
   ```bash
   getent group hjviewer-api > /dev/null || { echo 'STOP: hjviewer-api 그룹이 없음(7-3 먼저)'; exit 1; }
   [ -e /srv/hjviewer-updates ] && { echo 'STOP: /srv/hjviewer-updates가 이미 있음'; exit 1; }
   sudo install -d -o root -g hjviewer-api -m 0750 /srv/hjviewer-updates /srv/hjviewer-updates/pkg
   sudo install -d -o root -g root -m 0700 /srv/hjviewer-updates/incoming
   sudo stat -c '%a %U:%G %n' /srv/hjviewer-updates /srv/hjviewer-updates/pkg /srv/hjviewer-updates/incoming
   sudo grep -E '^#?HJV_API_(VERSION_POLICY|PACKAGE_DIR|DOOR2_FILE)=' /etc/hjviewer/hjviewer-api.env
   ```
   `750 root:hjviewer-api` 둘과 `700 root:root` 하나여야 합니다. hjv-api의 세 설정은 주석 줄(기본값: `/etc/hjviewer/version-policy.json`, `/srv/hjviewer-updates/pkg`, `/etc/hjviewer/door2.conf`) 그대로 둡니다.
3. 업데이트 공개키 파일을 공개 저장소의 출시 태그에서 받습니다. 출시 태그(8단계) 뒤에 합니다(위). 사용자가 태그 이름과, 자기 PC에서 본 그 파일의 SHA-256을 알려 줍니다.
   ```bash
   TAG=client-1.6.1                  # ← 사용자가 알려 준 출시 태그
   WANT='<사용자가 알려 준 SHA-256>'   # ← 64자리
   T=$(mktemp -d) && cd "$T" || exit 1
   curl -fsSL --max-time 30 -o update-keys.env "https://raw.githubusercontent.com/yonsei-projecttf/hjviewer/$TAG/client/update-keys.env" || { echo 'STOP: 받지 못함'; exit 1; }
   echo "$WANT  update-keys.env" | sha256sum -c - || { echo 'STOP: 사용자가 본 파일과 다름'; exit 1; }
   grep -E '^HJV_(UPDATE_PUBKEY|UPDATE_ROOT_PUBKEY|POLICY_FLOOR)=' update-keys.env
   sudo install -o root -g root -m 0644 update-keys.env /etc/hjviewer/update-keys.env
   cd / && rm -rf "$T"
   ```
   - `raw.githubusercontent.com`에 닿지 않으면 멈춥니다. 사용자가 그 파일을 `incoming/`으로 보내고, 같은 SHA-256 대조 뒤 설치합니다.
   - 키 줄이 비어 있으면(`HJV_UPDATE_PUBKEY=`) 멈춥니다. 그 태그의 앱은 정책을 확인하지 않고, 서버 확인도 할 수 없습니다.
   - 이 파일은 공개값입니다. 바꾸는 때는 8-5의 (마)뿐입니다.
4. hbbs의 설정 파일에 최소 버전 파일의 경로를 넣습니다. 파일 자체는 아직 두지 않습니다(없으면 아무도 거절하지 않음). 7-6에서 hbbs를 다시 시작할 때 적용됩니다.
   ```bash
   E=/etc/hjviewer/hjviewer.env
   grep -q '^HJV_MIN_CONTROLLER_FILE=' "$E" || printf '%s\n' '' '# hbbs의 최소 컨트롤러 버전(런북 8-3)' 'HJV_MIN_CONTROLLER_FILE=/etc/hjviewer/min-controller.conf' | sudo tee -a "$E" > /dev/null
   grep -n '^HJV_MIN_CONTROLLER_FILE=' "$E"
   ```
   7-6 뒤에는 hbbs 로그에 `min-controller`가 든 줄이 없어야 정상입니다(파일이 없을 때는 debug 수준에서만 한 줄).
5. **nginx 위치**: 1.6.0 묶음의 `nginx-hjviewer-locations.conf`에는 `location = /api/hjv/version`이 들어 있습니다. 7-8은 이 묶음 파일로 스니펫을 새로 만들므로 같은 reload에 들어갑니다(따로 reload하지 않음). 7-8의 3번 확인에 이 위치의 줄이 있습니다.
   - 정책 파일이 없는 동안 그 위치는 `404 {"error":"no version policy"}`로 답하고, 앱은 "정책 없음"으로 읽어 아무것도 막지 않습니다. 접속 기록과 오류 기록에 줄을 남기지 않고, 사이트에 `error_page 404`가 있어도 그 페이지로 넘기지 않습니다.
   - 이미 7-8을 마친 서버(이 위치가 없는 1.6.0 이전 묶음으로 한 서버)는 6절의 "nginx 경로 파일" 절차로 묶음 파일과 비교해 바꿉니다.

### 8-2. 릴리스마다: 서버 확인, 패키지 먼저, 정책 나중

사용자는 서명 키로 하는 정책 서명에 개발 PC의 `build/signer/hjv-release`를 씁니다(루트 키를 여는 서명은 깨끗한 컴퓨터에서 릴리스 태그로 빌드한 도구로 합니다. 도구 README 3장 5-0). `tools/hjv-release`를 바꾼 push 뒤마다 먼저 `scripts/build-signer.sh`가 push한 main에서 그것을 다시 빌드하고, `sign`과 `version`이 그 도구의 커밋을 보여 줍니다([ci.md](ci.md)의 "서명하는 hjv-release"). CI 폴더의 `build/ci/target-hjv-release/release/hjv-release`는 마지막으로 CI를 돌린 트리의 것이라 서명에 쓰지 않습니다. 사용자가 개발 PC에서 `hjv-release publish-dir`로 만든 디렉터리(아래 모양)를 서버의 자기 홈으로 보내고, 네 가지를 알려 줍니다: 버전, 정책의 위치(`sign`이 보관소에 쓴 파일 이름 `policy-e<세대>-s<seq>.json`의 `e<세대>-s<seq>`, 예: `e1-s3`), `sign`이 출력한 정책 파일의 SHA-256, 완화하는 항목(`--relaxes`, 보통 없음).

첫 정책(1.6.1, 위치 `e1-s1`)은 2026-10-14 23:30쯤(KST) 이 절차로 넣었습니다. `/api/hjv/version`이 내주는 파일의 SHA-256은 `91f2f142…5be8db`입니다(7-10). seq 2(1.6.2, 위치 `e1-s2`, iOS 최신 1.6.2만 바뀜, 패키지는 1.6.1 그대로)는 2026-10-15 16:19(KST)에 넣었고 SHA-256은 `6cfea642…8c7c`입니다. seq 3(1.7.0, 위치 `e1-s3`, 세 플랫폼 최신 1.7.0, 최소 1.6.1 그대로, Windows 무인 설치 2026-10-17 22:13(KST)부터)은 2026-10-16 07:31(KST)에 넣었고 SHA-256은 `e13116cc…8c90`입니다. 패키지는 1.7.0의 둘과 `notices-1.7.0/`이고, 직전 버전 1.6.1의 패키지는 `pkg/`에 남겼습니다(6번). 서버의 `update-keys.env`는 이 절의 규칙대로 `client-1.6.1`의 것(하한 1:0)으로 두었습니다. `client-1.6.2`와 `client-1.7.0`의 파일도 키 집합은 같고 하한만 1:1입니다. `client-1.7.0`은 빌드할 때 걸려 있던 seq 2의 위치(1:2)를 하한으로 썼어야 했습니다(도구 README 2장 1번의 `floor --live`를 빠뜨림, 태그 메모). 그래서 새로 설치한 1.7.0은 seq 1도 받지만, seq 1이 1.7.0에 요구하는 것은 없습니다. 다음 빌드는 다시 `floor --live`로 하한을 정합니다.

seq 4(1.8.0, 위치 `e1-s4`, 세 플랫폼 최소·최신 1.8.0, 유예 없음)는 2026-10-18에 넣었고 SHA-256은 `12d9e78c…60ac`입니다(9-6). seq 5(1.9.0, 위치 `e1-s5`, 발행 2026-10-20T06:50:10Z, 세 플랫폼 최소·최신 1.9.0, 유예 없음)는 2026-10-20 17:01:43(KST)에 넣었고 SHA-256은 `2319222c…905a`입니다. 정책의 Windows 무인 설치 시각(2026-10-21 22:00 KST)은 유예가 없어 쓰이지 않았습니다(최소 버전 아래인 PC는 무인 설치 시각과 상관없이 강제 설치함). 패키지는 1.9.0의 둘과 `notices-1.9.0/`이고, 같은 작업에서 `/opt/hjviewer/share/`의 `NOTICE.txt`와 `SOURCE.txt`를 1.9.0 서버 묶음의 것으로 바꿨습니다(지금 것을 `/root/hjv-backup/share-before-1.9.0/`에 남긴 뒤, 9-9의 '넣기'에서 빠졌던 단계). 서비스 재시작과 nginx reload는 없었고, 서버의 `update-keys.env`는 그대로 `client-1.6.1`의 것입니다. hbbs 파일은 그다음 8-3으로 바꿨습니다.

seq 6(1.9.1, 위치 `e1-s6`, 발행 2026-10-26T17:07:46Z, Windows·Android 최신 1.9.1, 최소는 1.9.0 그대로, iOS 항목은 seq 5 그대로, Windows 무인 설치 2026-10-27 22:00(KST)부터)은 2026-10-27 09:18:43(KST)에 넣었고 SHA-256은 `a15cc47c…b6b9`입니다. 그 전에 1.9.1의 hjv-api와 이 절의 확인 스크립트를 넣었습니다(9-10의 기록). 패키지는 1.9.1의 둘과 `notices-1.9.1/`이고, 직전 버전 1.9.0의 패키지는 `pkg/`에 남겼습니다(6번). 서비스 재시작과 nginx reload는 없었고, 서버의 `update-keys.env`는 그대로 `client-1.6.1`의 것이며, hbbs 파일(`MIN=1.9.0`)도 그대로입니다. iOS의 1.9.1은 TestFlight에만 있으므로 정책의 iOS 항목은 App Store에 출시된 뒤에 올립니다(아래). 같은 날 사용자 결정으로 `pkg/`의 1.6.1·1.7.0·1.8.0 패키지와 고지를 지웠습니다. `pkg/`에는 1.9.0과 1.9.1만 남고, `incoming/`의 묶음(`e1-s1`~`e1-s6`)은 그대로입니다.

seq 7(1.9.2, 위치 `e1-s7`, 발행 2026-10-28T05:51:31Z, 1610바이트, Windows·Android 최신 1.9.2, 최소는 1.9.0 그대로, iOS 항목은 그대로, Windows 무인 설치 2026-10-30 14:51(KST)부터)은 허브-운영이 이 절대로 2026-10-28 16:34(KST)에 넣었고(`POLICY_INSTALLED`) SHA-256은 `a3c36693…05fb`입니다. 패키지는 Windows 1.9.2.22(`HJViewer-1.9.2-windows-x86_64.exe`, 28,286,464바이트, SHA-256 `f7717534…`)와 Android 1.9.2(versionCode 263012506, 68,445,512바이트, SHA-256 `f0d4f9ca…`), 그리고 `notices-1.9.2/`입니다. 서비스 재시작과 nginx reload는 없었고, hbbs 파일(`MIN=1.9.0`)과 문 2의 `UNTIL`도 그대로입니다. 5번의 정책 read 줄은 넣을 때는 아직 없었고, 운영자 PC가 1.9.2로 업데이트할 때(그 내려받기 요청) 17:10:08(KST)에 처음 나왔습니다. 넣기 전 읽기 점검(16:33:15) 뒤로 저널에 `went back`과 ERROR·WARN 줄은 없었습니다. iOS의 1.9.2(빌드 263010510)는 TestFlight에만 있으므로 정책의 iOS 항목은 App Store에 출시된 뒤에 올립니다(아래). 그 뒤 사용자 결정으로 `pkg/`의 1.9.0 패키지를 지웠습니다. `pkg/`에는 1.9.1과 1.9.2만 남고, `incoming/`의 사본은 모두 그대로입니다.

**iPhone 항목은 하나입니다.** 정책의 `ios` 항목 하나를 TestFlight 설치본과 App Store 설치본(1.9.1부터 업데이트 화면이 App Store를 엶)이 함께 씁니다. 그래서 App Store 배포를 시작한 뒤에는 iOS의 `latest`와 `min`을 **App Store에 출시된 버전으로만** 올립니다. TestFlight에만 있는 빌드로 올리지 않고, App Store 심사 중에는 심사받는 버전으로 올리지 않습니다(심사하는 앱은 샌드박스 영수증이라 TestFlight 길을 봄). 어기면 App Store 설치본이 스토어에 아직 없는 업데이트를 안내받거나 `min`에 막힙니다. 서버 확인은 이것을 보지 않으므로 정책을 만드는 사용자가 지킵니다([distribution.md](distribution.md)의 "iPhone"). iOS의 App Store 항목을 따로 두는 것은 다음 판의 열린 문제입니다.

**App Store 필드와 서버 확인 스크립트(1.9.1부터).** 정책의 `ios` 항목에 App Store에 나온 버전을 적는 선택 필드 `appstore`가 생겼습니다(`hjv-release policy --appstore-live`, 스토어 정책, [store-release.md](store-release.md)). 그 필드가 든 정책은 커밋 63c18d8 이후의 `deploy/hjv-update-verify.sh`만 받습니다. 그래서 **첫 `--appstore-live` 정책을 넣기 전에** 서버의 `/opt/hjviewer/bin/hjv-update-verify.sh`를 그 판(63c18d8 이후의 서버 묶음 `bin/`, 또는 그 커밋의 `deploy/hjv-update-verify.sh`)으로 바꾸고 8-1의 1번처럼 `--self-test`를 돌립니다. 옛 스크립트는 그 정책을 거부합니다. 거부된 정책은 넣지 않으므로 앱에는 해가 없지만, 그 정책의 배포가 멈춥니다. 바꾼 스크립트는 `appstore` 필드가 없는 정책도 전처럼 받습니다.

**정책만 바뀌는 경우**(최소 버전만 올리는 정책, 8-5의 비상 정책, 8-6의 되돌리기)도 이 절차 그대로입니다. 버전은 그대로여도 위치가 다르므로, 서버의 디렉터리 `incoming/<버전>-<위치>`는 겹치지 않습니다. 그런 디렉터리에는 패키지가 없거나 이미 설치한 것과 같은 패키지만 있습니다(서버 확인은 없는 패키지를 `/srv/hjviewer-updates/pkg`에서 찾음).

```text
publish-<v>/version-policy.json        서명된 정책
publish-<v>/pkg/HJViewer-<v>-windows-x86_64.exe, HJViewer-<v>-android-arm64.apk   (정책만 바뀌면 없을 수 있음)
publish-<v>/pkg/notices-<v>/LICENSE.txt NOTICE.txt SOURCE.txt THIRD_PARTY_NOTICES.txt
publish-<v>/SHA256SUMS                 위 파일 모두
```

1. `incoming/<버전>-<위치>`로 옮기고 묶음의 체크섬을 봅니다. 맞으면 홈의 전송 사본은 지웁니다(다음에 같은 이름으로 보내도 섞이지 않게).
   ```bash
   V=1.6.1                           # ← 사용자가 알려 준 버전
   P=e1-s3                           # ← 사용자가 알려 준 위치(policy-e1-s3.json이면 e1-s3)
   SRC="/home/<사용자>/publish-$V"    # ← 사용자가 보낸 디렉터리
   [[ $V =~ ^[0-9]{1,3}\.[0-9]{1,3}\.[0-9]{1,2}$ && $P =~ ^e[1-9][0-9]*-s[1-9][0-9]*$ && $SRC == /home/*/publish-* ]] || { echo 'STOP: 버전, 위치, 디렉터리의 꼴이 다름'; exit 1; }
   D=/srv/hjviewer-updates/incoming/$V-$P
   sudo test -e "$D" && { echo "STOP: $D 이(가) 이미 있음"; exit 1; }
   sudo test -f "$SRC/version-policy.json" && sudo test -f "$SRC/SHA256SUMS" || { echo 'STOP: version-policy.json이나 SHA256SUMS가 없음'; exit 1; }
   if sudo find "$SRC" ! -type f ! -type d | grep -q .; then echo 'STOP: 보통 파일과 디렉터리가 아닌 것이 있음'; exit 1; fi
   sudo cp -r "$SRC" "$D" && sudo chown -R root:root "$D" && sudo chmod -R go-rwx "$D"
   sudo sh -c 'cd "$1" && sha256sum -c --quiet SHA256SUMS' _ "$D" && echo SUMS_OK && sudo rm -r "$SRC"
   sudo sha256sum "$D/version-policy.json"
   ```
   `SUMS_OK`가 아니거나 마지막 줄의 SHA-256이 사용자가 알려 준 값과 다르면 멈춥니다. 그 디렉터리는 지우지 않고 옆으로 치운 뒤 사용자에게 보고합니다. 사용자가 다시 보내면 1번부터 다시 합니다(같은 이름의 디렉터리가 없으므로 `STOP: … 이미 있음`에 걸리지 않습니다).
   ```bash
   V=1.6.1; P=e1-s3                  # ← 1번과 같게
   sudo mv "/srv/hjviewer-updates/incoming/$V-$P" "/srv/hjviewer-updates/incoming/refused-$V-$P-$(date -u +%Y%m%dT%H%M%SZ)"
   ```
   `refused-*` 디렉터리는 정책의 사본이 아닙니다. 6번의 정리와 8-5의 (라)·(바)가 SHA-256으로 찾는 `incoming/<버전>-<위치>/`(숫자로 시작하는 이름)에 들지 않고, 사용자가 확인한 뒤 지워도 됩니다.
2. 서버 확인을 돌립니다. 패키지는 `$D/pkg`에서 찾고, 없으면 `/srv/hjviewer-updates/pkg`(이미 설치한 것)에서 찾습니다.
   ```bash
   V=1.6.1; P=e1-s3; D=/srv/hjviewer-updates/incoming/$V-$P   # ← 1번과 같게
   RELAXES=                          # ← 사용자가 알려 준 항목(예: windows,android). 없으면 빈 채로
   sudo /opt/hjviewer/bin/hjv-update-verify.sh ${RELAXES:+--relaxes "$RELAXES"} "$D/version-policy.json"
   ```
   마지막 줄이 `OK: install it, ...`이면 3번으로 갑니다. `OK, nothing to install: it is the live file`이면 이미 걸린 파일이니 아무것도 하지 않습니다. `REFUSED <이유>`이면 멈추고 그 줄을 사용자에게 보고합니다. 이유와 할 일은 이 소절 끝의 표에 있습니다. `new:` 줄의 `epoch`와 `seq`가 위치와 다르면(`e1-s3`이면 `epoch 1, seq 3`) 그것도 멈추고 보고합니다.
3. 패키지를 넣습니다. 같은 이름의 파일은 임시 이름으로 넣은 뒤 `mv`로 바꿉니다. 패키지가 없는 디렉터리(정책만 바뀜)에서는 목록만 보입니다.
   ```bash
   V=1.6.1; P=e1-s3                  # ← 1번과 같게
   sudo bash -s -- "/srv/hjviewer-updates/incoming/$V-$P" << 'EOF'
   set -eu
   D=$1 PKG=/srv/hjviewer-updates/pkg
   for f in "$D"/pkg/*; do
     [ -f "$f" ] || continue
     n=$(basename "$f")
     install -o root -g hjviewer-api -m 0640 "$f" "$PKG/.$n.new"
     mv -f "$PKG/.$n.new" "$PKG/$n"
   done
   for d in "$D"/pkg/notices-*; do
     [ -d "$d" ] || continue
     n=$(basename "$d")
     install -d -o root -g hjviewer-api -m 0750 "$PKG/$n"
     for f in "$d"/*; do install -o root -g hjviewer-api -m 0640 "$f" "$PKG/$n/"; done
   done
   find "$PKG" -maxdepth 2 -printf '%m %u:%g %s %p\n' | sort -k4
   EOF
   ```
4. 정책을 넣습니다. 옮길 바로 그 파일(`.new`)을 설치한 패키지로 한 번 더 확인하고, 통과할 때만 `mv`합니다. 거부되면 스크립트가 `.new`를 지웁니다.
   ```bash
   V=1.6.1; P=e1-s3; D=/srv/hjviewer-updates/incoming/$V-$P   # ← 1번과 같게
   RELAXES=                          # ← 2번과 같게
   N=/etc/hjviewer/version-policy.json.new
   sudo install -o root -g root -m 0644 "$D/version-policy.json" "$N"
   sudo /opt/hjviewer/bin/hjv-update-verify.sh ${RELAXES:+--relaxes "$RELAXES"} --packages /srv/hjviewer-updates/pkg "$N" &&
     sudo mv -f "$N" /etc/hjviewer/version-policy.json && echo POLICY_INSTALLED
   ```
5. 확인합니다.
   ```bash
   DOMAIN=$(sed -n 's/^HJV_DOMAIN=//p' /etc/hjviewer/hjviewer.env); [ -n "$DOMAIN" ] || exit 1
   curl -s --max-time 10 --resolve "$DOMAIN:443:127.0.0.1" "https://$DOMAIN/api/hjv/version" | sha256sum
   curl -s -o /dev/null -D - --max-time 10 --resolve "$DOMAIN:443:127.0.0.1" "https://$DOMAIN/api/hjv/version" | grep -iE '^(HTTP|content-type|cache-control)'
   sudo journalctl --namespace=hjviewer -u hjviewer-api --since -10min -o cat | grep 'the version policy' | tail -2
   ```
   - SHA-256이 사용자가 알려 준 값과 같아야 합니다(`200`, `application/json; charset=utf-8`, `no-cache`).
   - hjv-api 저널의 `update: the version policy (HJV_API_VERSION_POLICY) read: epoch E, seq S; packages: windows V, android V`가 새 세대·`seq`·버전이어야 합니다. 이 줄은 hjv-api가 정책을 처음 쓸 때(내려받기 요청이 올 때) 나오므로, 아직 없으면 사용자가 운영자 PC의 [지금 업데이트]로 확인합니다.
   - 그 저널에 `went back to epoch …`(ERROR)가 있으면 걸린 파일이 hjv-api가 전에 읽은 것보다 오래된 것입니다. 멈추고 보고합니다(8-0). 8-5의 (라) 2번으로 되돌린 뒤의 그 줄은 예상된 것입니다.
   - 문 2가 열려 있으면 닫는 시각만 봅니다: `sudo sed -n 's/^UNTIL=//p' /etc/hjviewer/door2.conf`.
6. 정리: `incoming/<버전>-<위치>`의 `version-policy.json`은 지우지 않습니다. 서버 확인을 거쳐 넣은 정책의 사본이고, 8-5의 (라)·(바)에서 SHA-256으로 찾아 씁니다. 공간이 모자라면 지금 걸린 정책과 그 직전 것의 디렉터리 말고는 `pkg/`만 지웁니다(`sudo rm -r <디렉터리>/pkg`). `/srv/hjviewer-updates/pkg/`의 옛 패키지는 정책이 더 이상 가리키지 않으면 지우되, 직전 버전 하나는 남깁니다(문제가 생겼을 때 손으로 다시 설치하기 위해). hbbs 파일은 사용자가 새 파일을 보낼 때만 바꿉니다(8-3).

서버 확인이 보는 것과 거부 이유입니다. 스크립트는 앱(libsodium, 클라이언트 코어)과 같은 규칙으로 판단하고, 도구(`hjv-release`)가 쓴 꼴의 파일만 읽습니다.

| 번호 | 보는 것 | 거부 이유 | 할 일 |
|---|---|---|---|
| 1 | 파일이 도구가 쓴 꼴 그대로(한 줄과 줄바꿈, 필드 순서, 이스케이프, base64) | `Canonical` | 사용자에게 보고. 서버에서 고치지 않음 |
| 2 | 서명이 엄격한지: R이 작은 차수인 서명(OpenSSL만으로는 받음)과 S가 군의 차수 이상인 서명을 거부 | `WeakSignature` | 사용자에게 보고 |
| 3 | 최신 빌드를 새로 설치한 앱(키 파일의 하한)이 받는지. hjv-api가 Windows·Android 항목을 읽는지(`source`는 64자 이하, 패키지 이름은 그 항목의 확장자 `.exe`·`.apk`이고 `.`으로 시작하지 않으며 `..`가 없음) | 앱의 이유(`BadSignature`, `UnknownKey`, `Epoch`, `Field:…` 등), `HjvApi` | 사용자에게 보고. 키 파일이 오래되었을 수도 있음(8-5의 (마)). `HjvApi`는 hjv-api가 파일 전체를 "정책 없음"으로 읽어 문 1·짧은 링크·문 2가 모두 404가 되는 파일(도구는 쓰지 않음) |
| 4 | 지금 걸린 파일을 가진 앱이 받는지 | 앱의 이유. 걸린 파일을 읽지 못하면 `LiveUnreadable` | `Rollback`, `Fork`, `Relaxation`인데 걸린 파일이 사용자가 서명한 것이 아니면 서명 키 도난일 수 있음: 8-5의 (라). `LiveUnreadable`은 `the key file does not verify the live file`이면 8-5의 (마), `the live file is not one this check reads`이면 (바) |
| 5 | 새 루트 선언이면: 걸린 정책보다 발행이 늦고, 걸린 정책보다 완화하는 항목이 `--relaxes`와 같음 | `LiveIssued`, `LiveRelaxes` | 입력 오류(`--relaxes`가 빠지거나 틀림)면 고쳐서 **같은 파일로** 다시 확인. 오래된 파일로 만든 선언이면 사용자가 새로 만듦. 걸린 파일이 사용자가 서명한 것이 아니면 8-5의 (라) |
| 6 | 정책(과 새 선언)의 발행 시각이 이 서버 시계보다 300초 넘게 늦지 않음 | `Clock` | 서버 시계(`timedatectl`)를 보고 사용자에게 보고 |
| 7 | 새 선언이면: 세대가 걸린 정책보다 높고, 걸린 선언의 폐기를 모두 담고, 101–199(루트 키 번호)를 폐기하지 않음 | `DeclarationEpoch`, `DeclarationRevoked` | 사용자에게 보고 |
| 8 | 정책이 가리키는 패키지가 그 크기와 SHA-256으로 있음(보통 파일만) | `Package` | 패키지가 아직 없으면 사용자가 그 파일을 보낸 뒤 `sudo install -D -o root -g root -m 0600 <보낸 파일> "$D/pkg/<그 이름>"`으로 넣고 홈의 사본을 지운 다음, 2번부터 **같은 파일로** 다시 확인(크기와 SHA-256은 정책과 대조됨) |

- `--relaxes`를 줬는데 새 선언이 없거나 걸린 정책이 없으면 `LiveRelaxes`로 거부합니다(엉뚱한 파일일 수 있음).
- 종료 코드 2(`cannot check: …`)는 확인 자체를 못 한 것입니다(키 파일 없음·잘못됨, 서버 이름 없음, openssl 이상). 사용자에게 보고합니다.
- 정책의 SHA-256(`file SHA-256:` 줄)과 키 집합 지문(`key set …:` 줄)은 공개값이므로 보고에 옮겨도 됩니다.

### 8-3. hbbs의 최소 컨트롤러 버전 (`/etc/hjviewer/min-controller.conf`)

hbbs는 이 파일을 10초마다 다시 읽고, `MIN`보다 낮은 버전의 컨트롤러가 연결을 시작하면(punch hole) `TEXT`로 거절합니다. 1.5.1을 포함한 모든 앱이 그 문구를 "연결 오류" 창에 보이고 다시 연결하지 않습니다. hbbs는 컨트롤러가 PC에 닿기 전에 거절하는 유일한 곳이라, 1.5.x PC로 가는 연결에서 1.5.x 컨트롤러를 막는 것은 이것뿐입니다. 첫 정책을 받은 1.6.x PC는 로그인에서 1.5.x 컨트롤러를 이미 거절합니다(7-10). 파일 형식은 [deploy/min-controller.conf.example](../deploy/min-controller.conf.example)에 있고, 실제 파일은 사용자가 `hjv-release hbbs-conf`로 만들어 보냅니다(`MIN`과 `FROM`은 도구가 정함).

- **기준은 하나입니다.** hbbs는 `FROM`부터 `MIN` 미만을 거절하고, 그 전에는 아무도 거절하지 않습니다. 그래서 `MIN`을 올리면서 `FROM`이 미래인 파일로 바꾸면 그 시각까지 거절이 멈춥니다. 도구는 그럴 때 파일을 쓰지 않고 다시 돌릴 시각을 알려 줍니다. 손으로 만든 파일을 일찍 넣지 않습니다.
- 첫 파일: 사용자가 첫 정책으로 만든 파일(`MIN=1.6.1`, `FROM`=정책의 발행 시각)을 보내고, 그 파일을 바로 넣습니다. 첫 정책에는 유예가 없어 `FROM`이 이미 지났으므로, 넣는 즉시 서버를 거치는 1.6.1 미만의 컨트롤러(1.5.x 모두)를 거절합니다.
  - 그래서 `FROM`이 지난 파일을 넣는 것은 실제로는 배포 단계입니다. 넣을지와 때는 사용자가 정하고(7-10), 보통은 7-9의 관문과, 그 날짜를 적은 사용자 안내([distribution.md](distribution.md)의 "사용자에게 보내는 안내") 뒤에 넣습니다.
  - S1 문구(문 2 링크)의 파일은 서버에 `door2.conf`가 먼저 있어야 합니다(8-4의 열기를 먼저). 문 2 없이 넣는 파일은 사용자가 `hbbs-conf --text s2 --out <파일>`로 만든 S2 문구입니다. `--door2-out`의 디렉터리에 `door2.conf`가 없는데 명령에 `--door2-until`이 있으면, 도구는 새 코드로 새 `door2.conf`를 씁니다. 곧 새 문 2이므로, 그 파일을 함께 보내 8-4대로 먼저 넣습니다.
  - 기록(2026-10-14): 사용자가 1.5.x 컨트롤러를 모두 바로 거절한다는 설명을 들은 뒤 "지금 바로 넣기"로 정해, 23:43(KST)에 넣었습니다. `MIN=1.6.1`, `FROM=2026-10-14T14:06:02Z`(정책의 발행 시각, 이미 지남), 문구는 그날 23:24에 연 문 2의 링크가 든 S1입니다. 7-9의 관문 전이고 사용자 안내도 하기 전입니다(사용자 결정). 10분 뒤 거절은 0이었습니다.
  - 기록(2026-10-18): 정책 seq 4와 함께 `MIN=1.8.0`, `FROM=2026-10-18T06:53:56Z`(S1)로 바꿨습니다(9-6).
  - 기록(2026-10-20): 정책 seq 5가 걸린 뒤, 사용자가 `hjv-release hbbs-conf`로 만든 파일을 17:28:47(KST)에 넣었습니다. `MIN=1.9.0`, `FROM=2026-10-20T06:50:10Z`(seq 5의 발행 시각, 이미 지남), 문구는 S1 그대로입니다(문 2의 코드와 `UNTIL`은 바뀌지 않음, 위의 코드 대조 1). hbbs는 다시 시작하지 않았고, 저널은 `min-controller 1.9.0 from 2026-10-20T06:50:10Z`, 넣은 뒤 10분 동안 거절은 0이었습니다.

보낸 파일 옮기기(이 파일과 8-4의 `door2.conf`): 둘 다 문 2의 코드가 들어 있습니다. 홈의 전송 사본은 옮긴 뒤 지우고, `incoming/`의 사본은 root만 읽습니다(0600).

```bash
set -- "/home/<사용자>/min-controller.conf"         # ← 사용자가 보낸 파일(문 2 파일도 보냈으면 그 경로를 뒤에 더함)
I=/srv/hjviewer-updates/incoming/conf-$(date -u +%Y%m%dT%H%M%SZ)
sudo install -d -o root -g root -m 0700 "$I"
for SRC in "$@"; do
  [[ $SRC == /home/*/* ]] && sudo test -f "$SRC" && ! sudo test -L "$SRC" || { echo "STOP: $SRC 이(가) 홈의 보통 파일이 아님"; exit 1; }
  sudo install -o root -g root -m 0600 "$SRC" "$I/" && sudo rm -f "$SRC" && echo "F=$I/$(basename "$SRC")"
done
```

넣기와 확인(지금 파일은 `/root/hjv-backup/min-controller.conf.before`로 남김):

```bash
F=/srv/hjviewer-updates/incoming/conf-20261122T060000Z/min-controller.conf   # ← 위 블록이 출력한 F= 줄
sudo test -f "$F" || { echo 'STOP: 파일이 없음'; exit 1; }
sudo grep -cE '^(MIN|FROM|TEXT)=' "$F"                          # 3
sudo grep -E '^(MIN|FROM)=' "$F"                                # TEXT 줄은 보이지 않음(문 2 링크)
B=/root/hjv-backup/min-controller.conf.before
sudo install -d -m 700 /root/hjv-backup
if sudo test -e /etc/hjviewer/min-controller.conf; then sudo cp -p /etc/hjviewer/min-controller.conf "$B"; else sudo rm -f "$B"; fi
sudo install -o root -g hjviewer -m 0640 "$F" /etc/hjviewer/min-controller.conf.new &&
  sudo mv -f /etc/hjviewer/min-controller.conf.new /etc/hjviewer/min-controller.conf
sleep 12
sudo journalctl -q -u hjviewer-hbbs --since -1min -o cat | grep 'min-controller' | grep -v 'min-controller refused' | tail -3
```

- 마지막 줄 `min-controller <MIN> from <FROM>`의 두 값이 위의 `sudo grep -E '^(MIN|FROM)=' "$F"`가 보인 `MIN`과 `FROM`이어야 합니다(`FROM`이 비면 `from now`). 첫 파일이면 `min-controller 1.6.1 from 2026-10-14T14:06:02Z`입니다. `FROM`이 이미 지났으면 넣은 때부터 거절합니다.
- `ERROR … min-controller file …: line N: …; … stays in force`이면 hbbs가 새 파일을 받지 않고 이전 규칙을 계속 쓰는 것입니다(이유는 그 줄에 있고, 문구는 옮기지 않음). 1시간마다 같은 ERROR가 다시 나오고, 그 파일을 둔 채 hbbs가 다시 시작하면 최소 버전이 꺼집니다. 바로 이전 파일로 되돌리고 그 줄을 사용자에게 보고합니다.
  ```bash
  B=/root/hjv-backup/min-controller.conf.before
  if sudo test -f "$B"; then
    sudo install -o root -g hjviewer -m 0640 "$B" /etc/hjviewer/min-controller.conf.new &&
      sudo mv -f /etc/hjviewer/min-controller.conf.new /etc/hjviewer/min-controller.conf
  else
    sudo rm -f /etc/hjviewer/min-controller.conf   # 전에는 파일이 없었음
  fi
  sleep 12; sudo journalctl -q -u hjviewer-hbbs --since -1min -o cat | grep 'min-controller' | grep -v 'min-controller refused' | tail -2
  ```
- **hbbs를 다시 시작한 뒤**(6절의 B 경로 업데이트, 7-6, 재부팅): 이 파일이 있으면 12초 뒤 같은 줄을 봅니다. 시작할 때 파일이 hbbs 규칙에 맞지 않으면 hbbs는 최소 버전 없이 돌고 ERROR(`no minimum is in force`)를 남깁니다. 그때는 멈추고 보고합니다.
  ```bash
  if sudo test -e /etc/hjviewer/min-controller.conf; then sleep 12; sudo journalctl -q -u hjviewer-hbbs --since -1min -o cat | grep min-controller | grep -v 'min-controller refused' | tail -2; fi
  ```
- 끄기(즉시): `MIN=`만 남긴 파일로 바꾸거나 파일을 지웁니다. 10초쯤 뒤 `min-controller off: …`가 나옵니다. 되돌리기와 비상 때 씁니다(8-6).
- 거절 수: `sudo journalctl -q -u hjviewer-hbbs --since yesterday -o cat | grep -c 'min-controller refused'`(개수만. 그 줄에는 주소와 PC ID가 있음). 10초에 30줄을 넘으면 `min-controller refused N more`로 묶이니 그 N도 더합니다.
- 사용자가 `hbbs-conf --current`에 주는 파일은 사용자가 보낸 파일 그대로입니다(개발 PC의 `~/hjviewer-secrets/door2/min-controller.conf`). 도구는 hbbs가 쓰는 온전한 파일만 읽으므로(`MIN`, `FROM`, `TEXT` 세 줄과 끝 줄바꿈) 두 줄만 옮긴 파일은 거부합니다. 서버의 파일이 그 파일과 같은지는 `sudo grep -E '^(MIN|FROM)=' /etc/hjviewer/min-controller.conf`의 두 줄과 `sudo sha256sum /etc/hjviewer/min-controller.conf`의 해시로 대조합니다(`TEXT` 줄은 보이지 않음: 문 2 링크). 다르면 사용자에게 알리고, 서버의 파일을 사용자가 보낸 파일로 다시 넣을지 묻습니다.
- **Docker로 hbbs를 돌리는 서버**(A 경로, 이 런북 밖): 파일이 아니라 **디렉터리**를 컨테이너에 넣습니다. 파일 하나를 bind mount하면 `mv -f`로 바꾼 새 파일을 컨테이너가 보지 못하고(옛 inode를 봄) `MIN`을 비워도 거절이 멈추지 않습니다. 예: hbbs 서비스에 `HJV_MIN_CONTROLLER_FILE=/etc/hjviewer-min/min-controller.conf`와 볼륨 `./min-controller:/etc/hjviewer-min:ro`를 compose의 덮어쓰기 파일로 더하고(`COMPOSE_FILE`에 그 파일을 덧붙임), 파일은 `deploy/min-controller/`에 둡니다. 바꾼 뒤에는 `sudo docker compose logs --no-log-prefix hbbs | grep min-controller | tail -3`으로 같은 줄을 봅니다.

### 8-4. 문 2 (`/etc/hjviewer/door2.conf`)

1.5.x 사용자가 새 버전(걸린 정책의 `latest`. 2026-10-28의 seq 7부터 1.9.2)을 받는 비밀 링크 `https://<도메인>/api/hjv/get/<코드>/`입니다. 링크는 hbbs 거절 문구(S1)와 운영자가 사용자에게 직접 보내는 메시지에만 나옵니다. 형식은 [deploy/door2.conf.example](../deploy/door2.conf.example)에 있고, 실제 파일은 사용자가 `hjv-release hbbs-conf --door2-out`으로 만들어 보냅니다.

- **페이지**: 정책의 Windows·Android 패키지(크기와 SHA-256)와 그 옆의 알림 파일(LICENSE, NOTICE, SOURCE, THIRD_PARTY_NOTICES)을 보입니다. 바깥 사이트는 가리키지 않습니다(TestFlight 앱을 여는 링크만 있음. 사용자 요청, hjv-api 3b283b2부터, 7-14). 소스 코드(출시 태그)를 받는 곳은 아래 알림 파일의 SOURCE에 있다고 말하고, AGPL 6(d)의 '목적 코드 옆의 분명한 안내'는 그 SOURCE 고지가 맡습니다.
- **기록(2026-10-14)**: 사용자가 "windows 문 2 열자"로 정해 23:24(KST)에 열었습니다. `UNTIL=2026-11-29T15:00:00Z`(2026-11-30 0시 KST)이고, 페이지는 1.6.1의 Windows와 Android 패키지를 내주었습니다. 7-9의 관문 전입니다(7-10). 정책 seq 3을 넣은 2026-10-16부터는 1.7.0, seq 4(2026-10-18)부터는 1.8.0, seq 5(2026-10-20)부터는 1.9.0, seq 6(2026-10-27)부터는 Windows·Android 1.9.1, seq 7(2026-10-28)부터는 1.9.2의 패키지를 내줍니다(`UNTIL`은 그대로).
- **열기(여는 때는 사용자가 정함)**: 사용자가 보낸 `door2.conf`를 8-3의 "보낸 파일 옮기기" 블록으로 `incoming/`에 옮긴 뒤 넣습니다. 그 뒤 hbbs 파일(8-3)을 넣을 때는 S1 문구(같은 코드의 링크)가 든 파일을 넣습니다. S1 파일은 이 열기 뒤에만 넣습니다. 그 전에 넣으면 거절 문구의 링크가 404입니다. hbbs 파일을 넣는 일 자체는 배포 단계이므로 따로 정합니다(8-3의 첫 파일).
  ```bash
  F=/srv/hjviewer-updates/incoming/conf-20261020T060000Z/door2.conf   # ← 8-3의 옮기기 블록이 출력한 F= 줄
  sudo grep -c '^CODE=[0-9a-f]\{32\}$' "$F"                        # 1
  sudo sed -n 's/^UNTIL=//p' "$F"                                 # 닫는 시각(UTC)
  sudo install -o root -g hjviewer-api -m 0640 "$F" /etc/hjviewer/door2.conf.new &&
    sudo mv -f /etc/hjviewer/door2.conf.new /etc/hjviewer/door2.conf
  DOMAIN=$(sed -n 's/^HJV_DOMAIN=//p' /etc/hjviewer/hjviewer.env); [ -n "$DOMAIN" ] || exit 1
  code=$(sudo sed -n 's/^CODE=//p' /etc/hjviewer/door2.conf)
  # 주소는 curl의 표준 입력으로 줍니다(명령줄은 이 서버의 모든 사용자가 /proc에서 읽음). printf는 셸 안에서 돕니다
  echo "door 2: $(printf 'url = "https://%s/api/hjv/get/%s/"\n' "$DOMAIN" "$code" | curl -s -o /dev/null -w '%{http_code}' --max-time 10 --resolve "$DOMAIN:443:127.0.0.1" -K -)"
  unset code
  sudo journalctl --namespace=hjviewer -u hjviewer-api --since -2min -o cat | grep "door 2's file" | tail -1
  ```
  `door 2: 200`과 `update: door 2's file (HJV_API_DOOR2_FILE) read: open until <UTC> (code=<8자리>)`가 나와야 합니다. 8자리는 코드의 SHA-256 앞부분이라 옮겨도 됩니다. 코드 자체는 화면에도, 다른 프로세스의 명령줄에도 나오지 않습니다(위 블록은 변수와 표준 입력으로만 다룸). 코드를 명령의 인자로 쓰지 않습니다.
  - 파일 권한을 나중에 고치지 않습니다. hjv-api는 읽지 못한 파일을 권한만 고쳐서는 다시 읽지 않을 수 있습니다. 위처럼 권한을 정해 임시 이름으로 넣고 `mv`합니다.
- **코드 바꾸기**(링크가 퍼졌거나, hbbs를 trace로 돌렸을 때): 사용자가 `--door2-rotate`로 새 `door2.conf`와 새 hbbs 파일을 함께 만들어 보냅니다. 두 파일을 8-3의 옮기기 블록으로 함께 옮기고, `door2.conf`를 넣고 바로 이어서 hbbs 파일을 8-3대로 넣습니다. 옛 링크는 바로 404가 되고, 그 사이 10초쯤은 hbbs 문구가 옛 링크를 보입니다.
- **닫는 시각 바꾸기**: 사용자가 `--door2-until`로 만든 파일을 위처럼 넣습니다.
- **닫기**: 아래를 모두 만족하면 사용자 승인 뒤 닫습니다.
  - hbbs가 1.5.x를 거절하기 시작한 지(8-3의 파일을 넣은 때) 30일이 지났거나, hbbs 기록에 거절이 7일 동안 없음(8-3의 거절 수). 운영 서버는 2026-10-14 23:43부터 거절했지만 사용자 안내는 7-9의 관문 뒤에 나갑니다. 30일을 거절이 시작된 때부터 셀지, 안내한 날부터 셀지는 사용자가 정합니다.
  - 운영자의 PC 목록과 `sudo /opt/hjviewer/bin/hjv-api admin report`의 앱 버전별 PC에 1.5.x가 없음(hbbs는 오래된 PC를 보지 못함).
  - 순서: hbbs 파일이 있으면(2026-10-14부터 있음) 먼저 S2 문구의 hbbs 파일(사용자가 `hbbs-conf --text s2 --out`으로 만듦, 링크 없음)을 8-3대로 넣고, 그다음 문 2 파일을 지웁니다. hbbs 파일이 없으면 문 2 파일만 지웁니다. 그때는 S2 파일을 넣지 않습니다. 넣으면 서버가 그때부터 거절을 시작합니다.
    ```bash
    DOMAIN=$(sed -n 's/^HJV_DOMAIN=//p' /etc/hjviewer/hjviewer.env); [ -n "$DOMAIN" ] || exit 1
    code=$(sudo sed -n 's/^CODE=//p' /etc/hjviewer/door2.conf)
    sudo rm /etc/hjviewer/door2.conf
    echo "door 2: $(printf 'url = "https://%s/api/hjv/get/%s/"\n' "$DOMAIN" "$code" | curl -s -o /dev/null -w '%{http_code}' --max-time 10 --resolve "$DOMAIN:443:127.0.0.1" -K -)"   # 404
    unset code
    sudo sh -c 'rm -f /srv/hjviewer-updates/incoming/conf-*/door2.conf'   # 옮겨 둔 사본도
    ```

### 8-5. 비상 절차

정책을 고치는 일은 모두 사용자가 개발 PC(루트 키는 깨끗한 컴퓨터)에서 하고, 서버에서는 8-2로 넣습니다. 앱 버전이 그대로인 정책도 위치(`e<세대>-s<seq>`)가 새로우므로 8-2의 디렉터리 이름이 겹치지 않습니다. 아래는 서버 쪽에서 할 일과 순서입니다. 도구 쪽 명령은 `tools/hjv-release/README.md` 3장에 있습니다.

- **(가) 무인 설치를 멈출 때**(새 버전에 문제가 보임): 사용자가 `auto_install_after`를 지운 정책(`--auto-after windows=none`)을 서명 키로 만듭니다. 최소 버전 아래의 PC가 그 버전을 강제로 설치하면 안 되면 `--withdraw-package windows`도 줍니다. 8-2로 넣습니다(`--relaxes` 없음). 내려받는 중이거나 600초 예고 중인 PC도 설치를 시작하지 않습니다. 고친 버전은 더 높은 버전 번호로 냅니다.
- **(나) 업데이트 경로가 오래 멈췄을 때**(hjv-api가 멈췄거나 패키지가 없어짐): 막힌 Windows 실행본과 Android 사용자는 나가는 연결도 앱 안 업데이트도 못 합니다.
  1. 먼저 hjv-api와 패키지를 고칩니다(7-14, 8-2의 3번).
  2. 오래 걸리면 hbbs 쪽은 바로 풀 수 있습니다: `MIN=`만 남긴 파일로 바꿉니다(8-3, 10초 안).
  3. 앱 쪽은 사용자가 루트 선언으로 잠시 낮춥니다(`--relax --min all=<낮은 버전>`, 깨끗한 컴퓨터). `sign`이 출력한 완화 항목을 `--relaxes`로 주어 8-2로 넣습니다.
  4. 고친 뒤 사용자가 서명 키로 다시 올리고(8-2), hbbs 파일은 `hbbs-conf`가 만든 것으로 되돌립니다(8-3).
- **(다) 잘못된 정책을 냈을 때**: 너무 엄격하면(최소 버전이 너무 높음, 기한이 너무 이름) (나)의 3번과 같습니다(루트 선언, `--relaxes`). 너무 느슨하거나 문구가 틀렸으면 다음 정책을 서명 키로 냅니다(`--relaxes` 없음). 옛 정책 파일을 다시 넣지 않습니다(8-0, 8-6).
- **(라) 서명 키를 도난당했을 수 있을 때**: 서버 확인이 걸린 파일을 기준으로 `Rollback`, `Fork`, `Relaxation`, `LiveRelaxes`를 냈는데 사용자가 그 걸린 파일을 서명한 적이 없거나, 사용자의 주 1회 점검(`hjv-release check --live`)이 "보관소에 없는 걸린 정책"을 경고할 때입니다. 도구는 보관소에 없는 걸린 파일로는 선언을 만들지 않고(X10), 서버 확인은 도둑 파일과 비교해 사용자의 선언을 `LiveRelaxes`로 거부합니다. 그래서 먼저 걸린 파일을 사용자의 마지막 파일로 되돌립니다(신뢰 규칙 명세와 도구 README의 5-8).
  1. 멈추고 사용자에게 알립니다. 걸린 파일의 SHA-256과 첫 줄(`sudo head -c 200 /etc/hjviewer/version-policy.json`)은 공개값이므로 보고에 넣어도 됩니다.
  2. 사용자가 걸린 파일을 서명한 적이 없다고 확인하고(보관소에 그 SHA-256이 없음), 보관소의 마지막 내 파일(`void`하지 않은 가장 높은 `policy-e<세대>-s<seq>.json`)의 SHA-256을 키 매체의 보관소 사본에서 읽어 알려 주면, 사용자 승인을 받아 그 파일로 되돌립니다. 8-0의 유일한 예외입니다. 그 파일은 걸린 도둑 파일과 비교하지 않고, 그 파일 자체만 서버 확인으로 봅니다(`--live`로 자기 자신을 줌).
     ```bash
     WANT='<사용자가 알려 준 SHA-256>'   # ← 보관소의 마지막 내 파일, 64자리
     F=$(sudo sh -c 'sha256sum /srv/hjviewer-updates/incoming/[0-9]*/version-policy.json' | awk -v w="$WANT" 'tolower($1) == tolower(w) { print $2; exit }')
     [ -n "$F" ] || { echo 'STOP: incoming/에 그 파일이 없음(사용자가 보낸 것을 8-2의 1번으로 옮긴 뒤 다시)'; exit 1; }
     echo "$F"
     sudo /opt/hjviewer/bin/hjv-update-verify.sh --live "$F" --packages /srv/hjviewer-updates/pkg "$F" &&
       sudo install -o root -g root -m 0644 "$F" /etc/hjviewer/version-policy.json.new &&
       sudo mv -f /etc/hjviewer/version-policy.json.new /etc/hjviewer/version-policy.json && echo POLICY_PUT_BACK
     sudo sha256sum /etc/hjviewer/version-policy.json
     ```
     서버 확인의 마지막 줄은 `OK, nothing to install: it is the live file`이어야 합니다(앱이 받는 파일인지와 패키지만 봄). `Package`로 거부되면 그 디렉터리로 8-2의 3번을 한 뒤 다시 하고, 다른 거부면 멈추고 보고합니다. 되돌린 뒤 hjv-api 저널의 `went back to epoch …`(ERROR)는 예상된 것입니다(도둑 파일보다 낮은 위치). 도둑 파일을 이미 받은 앱은 이 파일을 받지 않고(3번의 선언이 풀어 줌), 아직 받지 않은 앱은 도둑 파일을 더 받지 않습니다. 사용자가 서버의 파일을 다시 받아 그 SHA-256인지 확인합니다.
  3. 사용자가 깨끗한 컴퓨터에서 그 보관소 파일을 `--prev`로, 다시 받은 서버의 파일을 `--live`로 주어 도난 키를 폐기하는 루트 선언과 그 세대의 새 정책을 만들어 보냅니다. 8-2로 넣습니다(`--relaxes` 없음: 걸린 파일이 이제 사용자의 것이라 선언이 완화하는 항목이 없음. `sign`이 완화 항목을 출력했으면 그 항목만). hbbs 파일은 사용자가 바꾸라고 할 때만 바꿉니다.
  4. 키 파일(`update-keys.env`)은 바로 바꾸지 않습니다. 다음 빌드의 태그 뒤, 걸린 정책이 새 키 파일이 박은 키로 서명된 다음에 (마)대로 바꿉니다.
- **(마) 업데이트 키 파일을 바꿀 때**(사용자의 계획된 키 교체나 도난 뒤 새 빌드): 사용자가 새 태그를 알려 주면, 지금 걸린 정책이 새 키 파일로 읽히는지 먼저 봅니다. 읽히지 않으면 바꾸지 않습니다.
  ```bash
  TAG=client-1.6.1                  # ← 사용자가 알려 준 새 출시 태그
  WANT='<사용자가 알려 준 SHA-256>'   # ← 64자리
  DOMAIN=$(sed -n 's/^HJV_DOMAIN=//p' /etc/hjviewer/hjviewer.env); [ -n "$DOMAIN" ] || exit 1
  T=$(mktemp -d) && cd "$T" || exit 1
  curl -fsSL --max-time 30 -o update-keys.env "https://raw.githubusercontent.com/yonsei-projecttf/hjviewer/$TAG/client/update-keys.env" || { echo 'STOP: 받지 못함'; exit 1; }
  echo "$WANT  update-keys.env" | sha256sum -c - || { echo 'STOP: 사용자가 본 파일과 다름'; exit 1; }
  sudo /opt/hjviewer/bin/hjv-update-verify.sh --keys "$T/update-keys.env" --server "$DOMAIN" /etc/hjviewer/version-policy.json &&
    sudo install -o root -g root -m 0644 update-keys.env /etc/hjviewer/update-keys.env && echo KEYS_REPLACED
  cd / && rm -rf "$T"
  ```
  걸린 파일을 걸린 파일과 비교하므로 `OK, nothing to install: it is the live file`이 나와야 바뀝니다. `LiveUnreadable`이나 다른 거부면 멈춥니다(걸린 정책이 아직 새 키 파일이 박지 않은 키로 서명됨).
- **(바) 걸린 파일을 읽지 못할 때**(`LiveUnreadable: the live file is not one this check reads …`): 서버 확인을 거치지 않은 파일이 걸려 있는 것입니다(손으로 고쳤거나 다른 파일을 넣음). 앱은 그 파일을 거부하고 전에 받은 정책을 지킵니다(서명된 부분은 그대로이고 봉투의 꼴만 바뀐 파일이면 같은 정책으로 받음). 어느 쪽이든 마지막으로 넣은 파일이 기준입니다.
  1. 멈추고 (라)의 1번처럼 보고합니다. 걸린 파일은 그대로 둡니다(8-0).
  2. 사용자에게 마지막으로 넣은 정책의 SHA-256을 묻고 `incoming/`에서 그 파일을 찾습니다. 없으면 사용자가 다시 보낸 것을 8-2의 1번으로 옮긴 뒤 찾습니다.
     ```bash
     WANT='<사용자가 알려 준 SHA-256>'   # ← 64자리
     sudo sh -c 'sha256sum /srv/hjviewer-updates/incoming/[0-9]*/version-policy.json' | awk -v w="$WANT" 'tolower($1) == tolower(w) { print $2; exit }'
     ```
  3. 사용자가 보낸 새 정책을 8-2대로 넣되, 2번과 4번의 확인 명령에 `--live <2번이 출력한 파일>`을 더해 그 파일을 가진 앱을 기준으로 확인합니다. 4번의 확인이 거부하면 `.new`가 남으므로(`--live`가 다른 곳을 가리켜 스크립트가 지우지 않음) `sudo rm -f /etc/hjviewer/version-policy.json.new`로 지웁니다. 그 옛 파일을 걸린 자리로 옮기지는 않습니다((라)의 2번과 다름).

### 8-6. 되돌리기

- **정책**: 옛 정책 파일로 되돌리거나 정책 파일을 지우지 않습니다(8-0. 예외는 키 도난 때 사용자가 서명한 적 없는 파일을 바꾸는 8-5의 (라) 2번뿐). 되돌리기는 언제나 `seq`가 더 높은 새 정책을 8-2로 넣는 것입니다. 최소 버전이나 기한을 낮추는 것이면 사용자가 루트 선언으로 완화한 정책(8-5의 (다)), 문구·`latest`·무인 설치를 고치는 것이면 서명 키로 만든 다음 정책입니다. 옛 파일은 새 파일을 이미 받은 앱이 거부하고(`Rollback`), hjv-api는 `went back` ERROR를 남기면서 옛 파일의 설치 파일을 내주므로 앱이 찾는 버전과 어긋납니다. 지우면 앱의 확인이 404가 되고 hjv-api는 설치 파일을 내주지 않습니다.
- **hbbs**: `MIN=`만 남긴 파일로 바꾸거나 파일을 지우면 10초 안에 거절이 멈춥니다. 다시 켤 때는 이전 파일을 8-3대로 넣습니다.
- **패키지**: 정책이 크기와 SHA-256을 정하므로 같은 이름의 파일을 바꿔 넣을 수 없습니다(hjv-api가 404, 앱이 거부). 다른 빌드는 새 버전 번호와 새 정책으로 냅니다. 직전 버전 패키지는 손으로 다시 설치할 때를 위해 남깁니다.
- **nginx 위치**: 지우지 않습니다. 지우면 그 경로가 `/api/` 블록(1.6.0에서는 hjv-api)으로 가서 앱이 정책을 받지 못합니다.
- **문 2**: `door2.conf`를 지우면 바로 닫힙니다(8-4). hbbs 파일이 S1 문구(문 2 링크)를 쓰는 동안에는 8-4의 순서로 닫습니다: 먼저 S2 파일, 그다음 `door2.conf` 삭제. 비상 때는 `door2.conf`를 지우고 곧바로 S2 파일을 넣습니다. 그 사이 거절 문구의 링크는 404입니다.

### 8-7. PC를 치우거나 다른 사람에게 줄 때

사용자가 `sudo /opt/hjviewer/bin/hjv-api admin device remove <PC ID>`(= `binding revoke`, 7-15)로 묶음을 풉니다(7-0). 풀린 PC의 서비스는 기기 서명으로 새 버전을 받지 못합니다(문 1). 그 PC에 다시 설치할 때는 그 PC에서 로그인하거나 짧은 링크·문 2·직접 전달을 씁니다.

### 8-8. 허브-운영 확인 항목

처음 설치 때(8-1) 한 번 보고, 바뀐 것이 있으면 보고합니다. 내용(사용자 이름, 경로)은 개수와 예/아니오로만 보고합니다.

1. **nginx 작업 프로세스와 php-fpm 풀의 사용자**: 그 둘이 업데이트 구역과 문 2 파일을 읽지 못해야 합니다.
   ```bash
   ps -o user= -C nginx | sort -u
   sudo grep -hE '^\s*user\s*=' /etc/php/*/fpm/pool.d/*.conf 2>/dev/null | sort -u
   for f in /srv/hjviewer-updates/pkg /etc/hjviewer/door2.conf /etc/hjviewer/min-controller.conf; do sudo test -e "$f" || echo "NOT YET: $f"; done
   for u in $(ps -o user= -C nginx | sort -u) $(sudo grep -hE '^\s*user\s*=' /etc/php/*/fpm/pool.d/*.conf 2>/dev/null | sed 's/.*=\s*//' | sort -u); do
     [ "$u" = root ] && continue
     id -nG "$u" | grep -qwE 'hjviewer|hjviewer-api' && echo "IN A HJVIEWER GROUP: $u"
     for f in /srv/hjviewer-updates/pkg /etc/hjviewer/door2.conf /etc/hjviewer/min-controller.conf; do
       sudo runuser -u "$u" -- test -r "$f" && echo "READABLE by $u: $f"
     done
   done; echo CHECKED
   ```
   `READABLE`이나 `IN A HJVIEWER GROUP` 줄이 나오면 멈추고 보고합니다(그 사용자가 `hjviewer-api`나 `hjviewer` 그룹에 들어 있는 것). nginx는 정책 파일(0644)만 읽으면 됩니다. 읽기 확인은 `runuser`로 합니다. root가 어느 사용자로든 실행하므로, `sudo -u`와 달리 운영 에이전트의 sudo가 root로만 실행을 허락해도 거절되지 않습니다(거절되면 `READABLE` 줄이 없어 통과처럼 보임). `NOT YET` 줄이 있으면(8-1 때는 `door2.conf`와 `min-controller.conf`가 아직 없음) 8-3과 8-4 뒤에 이 블록을 한 번 더 돌립니다. 운영 서버는 2026-10-14에 둘 다 했고, 그때마다 다시 돌렸습니다: 8-4 뒤 `NOT YET` 1(`min-controller.conf`), 8-3 뒤 `NOT YET` 0, 둘 다 `READABLE` 0, `IN A HJVIEWER GROUP` 0.
2. **매일 20:00의 `auto_block_ips.sh`가 무엇을 세는지**(어느 기록의 어떤 줄: 404, 경로). HJViewer의 `/api/` 경로와 정책 위치는 접속 기록을 남기지 않고, 짧은 링크와 문 2는 오류 기록도 남기지 않습니다. 그 스크립트가 다른 기록(예: 방화벽이나 hjv-api 저널)을 센다면 사용자에게 알립니다. 학내 NAT 뒤의 사용자들이 한 주소로 막히면 443 전체, 곧 연결 중계까지 막힙니다.
3. **사이트의 `error_page 404`**: `sudo nginx -T 2>/dev/null | grep -n 'error_page'`. 정책 위치는 자기 404를 쓰므로 상관없지만, 7-8 전(1.5.1 배치)의 `/api/` 블록은 그 페이지(WordPress 등)로 넘어갑니다. 7-8 뒤의 계정 API 블록은 스스로 답합니다. 있으면 보고합니다.
4. **(선택) php-fpm에서 HJViewer 파일 숨기기**: WordPress 플러그인의 파일 읽기 취약점까지 막으려면 php-fpm 유닛에 drop-in을 둡니다. PHP가 띄운 셸에도 걸리므로 `open_basedir`보다 강합니다. WordPress 쪽 변경이라 사용자와 WordPress 운영자의 동의가 필요하고, php-fpm 재시작이 들므로 조용한 시간에 합니다.
   ```bash
   U=php8.3-fpm                      # ← systemctl list-units 'php*-fpm*'로 확인한 유닛
   sudo mkdir -p "/etc/systemd/system/$U.service.d"
   printf '%s\n' '[Service]' 'InaccessiblePaths=-/srv/hjviewer-updates -/etc/hjviewer' | sudo tee "/etc/systemd/system/$U.service.d/hjviewer.conf" > /dev/null
   sudo systemctl daemon-reload && sudo systemctl restart "$U" && systemctl is-active "$U"
   ```
   사이트의 응답 코드를 4-3 (나)의 2번·5번처럼 전후로 비교하고, 달라지면 drop-in을 지우고 `daemon-reload`, `restart`로 되돌립니다.

### 8-9. Caddy, Apache, Docker

- **Caddy(A 경로)와 Apache에는 업데이트 정책이 없습니다.** 그 웹 서버들은 `/api/hjv/…`를 hjv-api의 소켓으로 넘기므로 `/api/hjv/version`은 hjv-api가 없으면 503, 있으면 404입니다. 앱은 "정책 없음"으로 읽어 아무것도 막지 않습니다(설계의 실패 개방). 1.6.0의 앱을 그런 서버에 붙여도 업데이트 안내와 PC의 컨트롤러 거절은 켜지지 않습니다.
- hbbs의 최소 버전 파일(8-3)은 웹 서버와 상관없이 hbbs가 읽으므로 어느 경로에서나 쓸 수 있습니다. Docker는 8-3의 마지막 항목대로 디렉터리를 넣습니다.
- 문 2와 짧은 링크, 앱 안 내려받기는 hjv-api가 있어야 합니다(7절).
- **개발 PC의 CI가 이 경로들을 시험하는 때(2026-10-27부터)**: `scripts/ci.sh deploy`는 운영의 조합인 B 경로의 systemd 유닛과 nginx를 매번 시험합니다. Apache(4-3), Docker로 띄우는 hbbs/hbbr(`docker-compose.behind-proxy.yml`), A 경로(Docker와 Caddy, 3절)는 그 경로가 읽는 설정 파일, 이미지와 패키지, 시험 코드가 마지막 통과 뒤에 바뀐 실행에서만 시험하고, 바뀌지 않았으면 기록에 `unchanged since <시각> (<커밋>): <경로>` 한 줄만 남깁니다([ci.md](ci.md)의 "바뀐 때만 도는 경로"). 그래서 이 경로로 서버를 준비하기 전에는 개발 PC에서 `HJV_CI_DEPLOY_ALL=1 scripts/ci.sh deploy`로 세 경로를 모두 한 번 더 시험합니다.

### 8-10. 보고 (8절)

아래 형식으로 알려 줍니다. 문 2의 코드, 거절 기록의 주소와 PC ID는 적지 않습니다.

```
단계: <8-1 | 8-2 (<버전>-<위치>) | 8-3 | 8-4 열기·코드 바꾸기·닫기 | 8-5 (<가~바>)>
확인 스크립트: --self-test <통과 | 실패>, 키 집합 <key set 줄의 지문 앞 16자리>, 하한 <e:s>
서버 확인: <OK | OK, nothing to install | REFUSED <이유와 그 줄>>, --relaxes <항목 | 없음>
정책: /api/hjv/version <코드>, SHA-256 <값>(사용자 값과 같음: 예/아니오), hjv-api 저널 <epoch E, seq S; packages: …>, went back <없음 | 있음(8-5 (라) 2번 뒤면 예상된 것)>
패키지: <pkg/에 넣은 파일과 크기>, 남긴 직전 버전 <버전>
hbbs: <min-controller 줄 그대로(MIN, FROM) | off>, 어제 거절 <n>줄
문 2: <열림, UNTIL <UTC>, 저널 code=<8자리> | 닫힘>
허브-운영 확인: READABLE 줄 <0 | n>, IN A HJVIEWER GROUP 줄 <0 | n>, NOT YET 줄 <0 | n>, auto_block_ips.sh <세는 것>, error_page 404 <있음 | 없음>
바꾼 것: <설치한 파일, 권한, 설정 파일 줄>
문제: <있으면>
```

## 9. 교직원 전용 출입 제한 (1.8.0)

1.8.0부터 서버는 연세대학교 교직원이 쓰는 서비스입니다. hjv-api가 출입증에 서명하고, hbbs가 연결 요청, 중계 요청, PC 등록에서 그것을 확인합니다. 로그인한 앱, 교직원 계정에 묶인 PC, 이용 비밀번호를 넣은 쪽, 교직원이 만든 지원 코드를 넣은 PC(1.9.0부터, 2시간. 그 hjv-api는 9-9), 심사 계정(데모 PC에 연결만)과 그 계정에 묶인 데모 PC(등록만, 1.9.2부터. 그 hjv-api는 9-11)가 출입증을 받습니다. 형식과 판정은 [access-tickets.md](access-tickets.md)(계약), 동작은 [architecture.md](architecture.md) 9절, 사용자 안내는 [distribution.md](distribution.md)의 "1.8.0: 교직원 전용"에 있습니다. 7절의 hjv-api와 8절을 마친 B 경로 nginx 서버가 대상입니다.

| 위치 | 소유자·권한 | 내용 | 읽는 쪽 |
|---|---|---|---|
| `/etc/credstore.encrypted/hjviewer-api-ticket-key` | 7-4와 같음 | 출입증 키(비밀키). 사용자가 서버에서 만듦(7-4 (나)) | hjv-api |
| `/etc/hjviewer/hjviewer-api.env`의 `HJV_API_TICKET_PUBKEYS` | 7-3과 같음 | 출입증 키의 공개키 `<kid>:<base64>`(바꾸는 동안 둘) | hjv-api의 selfcheck |
| `/etc/hjviewer/access.conf` | root:hjviewer 0640 | `MODE`, `KEY`(공개키), `NOT_BEFORE`, 선택으로 `TEXT`. 형식은 [deploy/access.conf.example](../deploy/access.conf.example) | hbbs(10초마다) |
| `/etc/hjviewer/hjviewer.env`의 `HJV_ACCESS_FILE` | 4-1과 같음 | 위 파일의 경로. 없으면 hbbs가 확인하지 않음 | hbbs(시작할 때) |
| `/etc/hjviewer/guest.conf` | root:hjviewer-api 0640 | 이용 비밀번호의 해시와 `UNTIL`. 형식은 [deploy/guest.conf.example](../deploy/guest.conf.example) | hjv-api(바뀔 때) |
| `/etc/hjviewer/review.conf`의 `PC=` | 7-17 | 심사 계정의 데모 PC | hjv-api |

### 9-0. 이 절의 규칙

- **출입증 키와 이용 비밀번호는 사용자만** 만듭니다. 운영 에이전트는 `keygen ticket`을 실행하지 않고, `guest.conf`를 만들지 않으며, 이용 비밀번호를 묻거나 보거나 옮기지 않습니다. `guest.conf`도 출력하지 않습니다(`cat`, `less` 등). 옮겨도 되는 것은 `UNTIL`과 해시의 꼬리표(`hash <8자리>`)뿐입니다.
- `access.conf`에는 비밀이 없습니다(공개키와 설정). `TEXT`는 거절된 누구에게나 보이므로, 앱 사용자에게만 줄 링크(문 2 등)를 넣지 않습니다. 보통은 `TEXT` 줄 없이 hbbs의 기본 문구를 씁니다.
- **`access.conf`를 지우거나 비우지 않습니다.** 출입 통제라서 파일이 없거나 고장 나면 hbbs가 모두 거절합니다(닫힘, 계약 6-3). 끄는 길은 `MODE=off`입니다(9-3). `hjviewer.env`의 `HJV_ACCESS_FILE=`도 빈 값으로 두지 않습니다(빈 값도 닫힘).
- **공개된 시험 키를 쓰지 않습니다.** `access.conf`의 `KEY=`는 `HJV_API_TICKET_PUBKEYS`에서만 옮기고(9-2, 9-3), 그 값은 `selfcheck`가 공개된 시험 키(`scripts/public-test-keys.txt`)인지 봅니다(`… is a public test key …`이면 `[FAIL]`). `[FAIL]`이면 그 값을 `access.conf`에 넣지 않고 멈춥니다.
- `MODE`를 바꾸는 것은 운영 서버의 접근 정책을 바꾸는 일입니다. 사용자 승인을 받고 합니다. `enforce`는 출시일 순서(9-6)의 마지막 단계에서만 씁니다.
- **hbbs를 trace 수준으로 돌리지 않습니다**(`RUST_LOG=trace`). 그 수준에서는 WebSocket 라이브러리가 출입증이 든 프레임까지 남깁니다(8-0과 같음).
- `enforce`의 거절 줄(`access refused …`, `access closed reg …`)에는 주소와 기기 ID가 있습니다. 운영 에이전트는 그 줄을 보지 않고 `grep -c`로 줄 수만 셉니다(7-0). 규칙 줄을 보는 블록(9-2, 9-3, 9-7)은 그래서 `access closed reg` 줄을 걸러 냅니다. 콘솔의 `HJVACCESS1` 줄은 개수뿐이라 그대로 옮겨도 됩니다.
- **Docker로 hbbs를 돌리는 서버**(A 경로, 이 런북 밖): 8-3의 마지막 항목처럼 파일이 아니라 디렉터리를 컨테이너에 넣습니다(`HJV_ACCESS_FILE=/etc/hjviewer-access/access.conf`, 볼륨 `./access:/etc/hjviewer-access:ro`). 파일 하나를 넣으면 `mv`로 바꾼 새 파일을 컨테이너가 보지 못해 `MODE=off`로 바꿔도 거절이 멈추지 않습니다.

### 9-1. 출입증 키 (처음 한 번)

출입증이 든 서버 묶음(hjv-api의 `/api/hjv/ticket`, hbbs의 서버 패치 0006~0008)을 6절 "B 경로 업데이트"로 넣을 때, 그 4번이 출입증 키를 요구하고 사용자가 7-4 (나)의 출입증 키 줄로 만듭니다(화면: `1:<base64>`와 지문). 사용자가 알려 준 첫 줄을 6절의 블록대로 `HJV_API_TICKET_PUBKEYS`에 넣은 뒤 다음을 봅니다.

```bash
sudo /opt/hjviewer/bin/hjv-api admin selfcheck | grep 'ticket key'      # 두 줄 모두 [ OK ]
sudo /opt/hjviewer/bin/hjv-api admin ticket status | sed -n '1,3p'
```

- 첫 줄이 `tickets: on, key 1 (<지문>), the key 1 of HJV_API_TICKET_PUBKEYS`이고, 그 지문이 사용자가 키를 만들 때 본 것과 같아야 합니다. 둘째 줄 `hbbs's access.conf takes it as KEY=1:<base64>`의 값이 9-2에서 `access.conf`에 들어갑니다.
- `tickets: off (no credential …)`이면 자격 증명이 없습니다(로그인과 동기화는 되지만 출입증이 없음). `NOT the key …`나 `no key of HJV_API_TICKET_PUBKEYS …`이면 `HJV_API_TICKET_PUBKEYS`가 쓰는 키와 다릅니다. 둘 다 멈추고 보고합니다. 그대로 `enforce`로 가면 hbbs가 모든 출입증을 거절합니다.
- 키를 잃으면 7-4 (나)의 줄로 새로 만들고, `HJV_API_TICKET_PUBKEYS`와 `access.conf`의 `KEY=`를 함께 바꿉니다(9-7). 앱은 출입증 키를 고정하지 않으므로 앱은 다시 빌드하지 않습니다.

### 9-2. hbbs의 출입 파일 (처음 한 번, `MODE=off`)

출입 확인을 꺼 둔 채로(`MODE=off`) 파일과 설정 줄을 넣고 hbbs를 다시 시작합니다. 앱에는 아무것도 바뀌지 않습니다. hbbs를 다시 시작하므로 진행 중인 원격 세션이 끊깁니다. 사용자 승인 뒤 사용이 적은 시간에 합니다. 6절의 업데이트와 같은 날이면 그 재시작에 합칩니다(이 블록의 `restart` 줄 하나로).

```bash
sudo grep -qa hjv-access-file /opt/hjviewer/bin/hbbs || { echo 'STOP: 설치된 hbbs가 출입 확인(서버 패치 0006)을 모름: 먼저 새 묶음으로 바꿈(6절)'; exit 1; }
TICKET=$(sudo sed -n 's/^HJV_API_TICKET_PUBKEYS=//p' /etc/hjviewer/hjviewer-api.env)
[ -n "$TICKET" ] || { echo 'STOP: HJV_API_TICKET_PUBKEYS가 비어 있음(9-1 먼저)'; exit 1; }
sudo /opt/hjviewer/bin/hjv-api admin selfcheck | grep 'ticket key' | grep -q FAIL && { echo 'STOP: selfcheck의 ticket key 줄이 FAIL'; exit 1; }
F=/etc/hjviewer/access.conf
! sudo test -e "$F" || { echo "STOP: $F 이(가) 이미 있음(바꾸기는 9-3)"; exit 1; }
{ echo 'MODE=off'; printf 'KEY=%s\n' ${TICKET//,/ }; echo 'NOT_BEFORE='; } | sudo tee "$F.new" > /dev/null
sudo chown root:hjviewer "$F.new" && sudo chmod 0640 "$F.new" && sudo mv -f "$F.new" "$F"
sudo grep -c '^KEY=' "$F"                                        # HJV_API_TICKET_PUBKEYS의 키 수(보통 1)
E=/etc/hjviewer/hjviewer.env
sudo grep -q '^HJV_ACCESS_FILE=' "$E" || echo 'HJV_ACCESS_FILE=/etc/hjviewer/access.conf' | sudo tee -a "$E" > /dev/null
sudo grep -c '^HJV_ACCESS_FILE=/etc/hjviewer/access.conf$' "$E"  # 1
sudo systemctl restart hjviewer-hbbs
sleep 3; systemctl is-active hjviewer-hbbs                       # active
sudo journalctl -q -u hjviewer-hbbs --since -1min -o cat | grep -E 'access (off|count|enforce|closed)|access file' | grep -v 'access closed reg' | tail -2
timeout 3 bash -c 'exec 3<>/dev/tcp/127.0.0.1/21115 && printf hjv-access >&3 && cat <&3'   # HJVACCESS1 mode=off
```

- 저널의 마지막 줄이 `access off`이고 콘솔이 `HJVACCESS1 mode=off`여야 합니다.
- `access closed`나 `access file /etc/hjviewer/access.conf: …`(ERROR)이면 hbbs가 모두 거절하고 있습니다. 바로 설정 줄을 빼고 다시 시작한 뒤 보고합니다: `sudo sed -i '/^HJV_ACCESS_FILE=/d' /etc/hjviewer/hjviewer.env && sudo systemctl restart hjviewer-hbbs`. 그 ERROR 줄의 까닭(줄 번호와 이유)은 보고에 옮겨도 됩니다(파일 내용은 들어 있지 않음).
- PC들이 다시 등록하는지는 5절과 같은 방법으로 봅니다. `MODE=off`이므로 1.7.0 전과 같아야 합니다.

### 9-3. `MODE`와 `NOT_BEFORE` 바꾸기

같은 블록으로 `off`, `count`, `enforce` 사이를 오가고, 비상에는 `NOT_BEFORE`를 지금으로 씁니다. `KEY` 줄은 늘 `HJV_API_TICKET_PUBKEYS`에서 다시 쓰고(키 바꾸기도 이 블록, 9-7), `TEXT` 줄은 그대로 옮깁니다. hbbs는 10초 안에 새 규칙을 쓰고, 재시작은 필요 없습니다. 지금 파일은 `/root/hjv-backup/access.conf.before`로 남깁니다.

```bash
MODE=count      # ← off | count | enforce (사용자가 정한 값)
NB=keep         # ← keep(지금 값 그대로) | now(비상: 지금까지 나간 출입증을 모두 끝냄)
F=/etc/hjviewer/access.conf
sudo test -f "$F" || { echo "STOP: $F 이(가) 없음(9-2 먼저)"; exit 1; }
case $MODE in off|count|enforce) ;; *) echo 'STOP: MODE는 off, count, enforce 중 하나'; exit 1 ;; esac
case $NB in
  keep) NB=$(sudo sed -n 's/^NOT_BEFORE=//p' "$F" | head -1) ;;
  now) NB=$(date -u +%Y-%m-%dT%H:%M:%SZ) ;;
  *) echo 'STOP: NB는 keep 또는 now'; exit 1 ;;
esac
TICKET=$(sudo sed -n 's/^HJV_API_TICKET_PUBKEYS=//p' /etc/hjviewer/hjviewer-api.env)
[ -n "$TICKET" ] || { echo 'STOP: HJV_API_TICKET_PUBKEYS가 비어 있음'; exit 1; }
sudo /opt/hjviewer/bin/hjv-api admin selfcheck | grep 'ticket key' | grep -q FAIL && { echo 'STOP: selfcheck의 ticket key 줄이 FAIL'; exit 1; }
echo "KEY: $(sudo grep '^KEY=' "$F" | sed 's/^KEY=//; s/:.*//' | paste -sd,) -> $(printf '%s\n' ${TICKET//,/ } | sed 's/:.*//' | paste -sd,)"   # 키 번호: 지금 -> 새 파일
sudo install -d -m 700 /root/hjv-backup && sudo cp -p "$F" /root/hjv-backup/access.conf.before
{ echo "MODE=$MODE"; printf 'KEY=%s\n' ${TICKET//,/ }; echo "NOT_BEFORE=$NB"; sudo grep '^TEXT=' "$F"; } | sudo tee "$F.new" > /dev/null
sudo chown root:hjviewer "$F.new" && sudo chmod 0640 "$F.new" && sudo mv -f "$F.new" "$F"
sleep 12; sudo journalctl -q -u hjviewer-hbbs --since -1min -o cat | grep -E 'access (off|count|enforce|closed)|access file' | grep -v 'access closed reg' | tail -2
timeout 3 bash -c 'exec 3<>/dev/tcp/127.0.0.1/21115 && printf hjv-access >&3 && cat <&3'
D=$(sudo sed -n 's/^HJV_DOMAIN=//p' /etc/hjviewer/hjviewer.env); sleep 30
curl -s --max-time 5 --resolve "$D:443:127.0.0.1" "https://$D/api/hjv/access"; echo   # 앱이 보는 모드
```

- 마지막 저널 줄이 새 규칙이어야 합니다: `access off`, `access count: keys 1`, `access enforce: keys 1`(`NOT_BEFORE`가 있으면 `, not before <UTC>`가 붙음). 콘솔 줄의 `mode=`도 같습니다.
- 마지막 줄은 앱이 보는 모드입니다(`{"mode":"enforce"}` 등, 계약 3-6). hjv-api는 hbbs에 30초에 한 번만 물으므로 30초를 기다립니다. 콘솔 줄과 같은 모드여야 합니다. `{"mode":"unknown"}`이면 hjv-api가 hbbs 콘솔에 닿지 못한 것입니다. 판정은 그대로이고 앱의 안내만 조건부("서버가 교직원 전용이면")로 바뀌므로, 보고에 적고 `/etc/hjviewer/hjviewer-api.env`의 `HJV_API_HBBS_CONSOLE`(기본 `127.0.0.1:21115`)과 hbbs를 봅니다.
- `access file …: line N: …; <규칙> stays in force`(ERROR)이면 hbbs가 새 파일을 받지 않고 이전 규칙을 씁니다. 한 시간마다 같은 ERROR가 다시 나오고, 그 파일을 둔 채 hbbs가 다시 시작하면 닫힘(모두 거절)입니다. 바로 남긴 파일로 되돌리고 보고합니다.
  ```bash
  B=/root/hjv-backup/access.conf.before
  sudo install -o root -g hjviewer -m 0640 "$B" /etc/hjviewer/access.conf.new && sudo mv -f /etc/hjviewer/access.conf.new /etc/hjviewer/access.conf
  sleep 12; sudo journalctl -q -u hjviewer-hbbs --since -1min -o cat | grep -E 'access (off|count|enforce|closed)|access file' | grep -v 'access closed reg' | tail -2
  ```
- `NB=now`(비상)는 그때까지 나간 출입증을 모두 거절하고, 들어와 있던 PC를 10초 안에 끊습니다. 로그인한 앱은 다음 연결에서 새 출입증을 받고, 묶인 PC는 다음 동기화(보통 2분) 뒤 다시 들어오고, 이용 비밀번호로 들어온 쪽은 비밀번호를 다시 넣습니다(계약 6-4). 한 번 쓴 `NOT_BEFORE`는 그대로 두어도 됩니다(그 뒤에 만든 출입증은 받음).
- `off`(또는 `count`)에서 `enforce`로 바꾸면, `off`인 동안 들어온 등록에는 hbbs가 기억한 출입증이 없어서 그 PC들이 10초 안에 한 번 끊깁니다(계약 6절). 1.8.0 PC는 곧 출입증을 실어 다시 등록하고, 묶인 PC는 늦어도 다음 동기화 뒤 다시 들어옵니다. 그동안 그 PC는 새 연결을 받지 못하고, 진행 중인 원격 세션도 끊길 수 있습니다. 그래서 켜는 것은 조용한 시간에 합니다. 2026-10-19의 기록: 켠 직후 묶인 PC가 등록을 두 번 거절당한 뒤 1분 안에 출입증을 받아 다시 들어왔습니다(9-6의 7단계).
- `enforce`(또는 닫힘)에서 `count`나 `off`로 바꾸면, 거절되었던 PC는 다음 등록 시도에 다시 들어옵니다. 거절된 PC는 30초마다 다시 시도하므로 다시 보이기까지 40초쯤 걸릴 수 있습니다(E2E A14). 새 출입증을 받은 PC(묶인 PC의 다음 동기화, 이용 비밀번호)는 바로 다시 등록합니다.
- **거절 수**(`enforce`): `sudo journalctl -q -u hjviewer-hbbs --since yesterday -o cat | grep -c 'access refused'`와 `… | grep -c 'access closed'`(개수만. 그 줄에는 주소와 ID가 있음). 10초에 30줄을 넘으면 `access refused or closed N more in the last 10 seconds`로 묶이니 그 N도 더합니다.

### 9-4. 이용 비밀번호 (`/etc/hjviewer/guest.conf`)

로그인하지 않는 사람이 쓰는 비밀번호 하나입니다(계약 3-5). 파일에는 해시와 끝나는 때(`UNTIL`)만 있고, hjv-api는 파일이 바뀌면 다시 읽습니다(재시작 없음). 파일이 없거나 `UNTIL`이 지나면 아무도 이용 비밀번호로 들어오지 못하고, 틀린 비밀번호와 같은 401을 받습니다.

- **만들기**(사용자, 에이전트 세션이 아닌 자기 터미널에서. Linux, macOS나 WSL): `python3 -I deploy/guest-conf.py --until <YYYY-MM-DD> --out ~/guest.conf`(비밀번호를 두 번 물음) 또는 `--random`(도구가 만들어 그 터미널에 한 번만 보임). 12~256자의 출력 가능한 ASCII이고, 공백으로 시작하거나 끝나지 않습니다(앱이 앞뒤 공백을 지움). `--until`은 90일까지입니다. 비밀번호는 개발 PC의 `build/leak-patterns.txt`에 넣고, 저장소, 채팅, 문서, 커밋, App Store와 Play의 메모에는 쓰지 않습니다. 필요한 사람에게만 직접 알립니다. 만든 파일은 운영 에이전트의 홈으로 보내고 자기 PC의 사본은 지웁니다.
1. **넣기**(사용자 승인 뒤). 바꿀 때(새 비밀번호, 새 `UNTIL`)도 같은 블록입니다.
   ```bash
   SRC=/home/<사용자>/guest.conf                  # ← 사용자가 보낸 파일
   [[ $SRC == /home/*/* ]] && sudo test -f "$SRC" && ! sudo test -L "$SRC" || { echo "STOP: $SRC 이(가) 홈의 보통 파일이 아님"; exit 1; }
   [ "$(sudo grep -cE '^(HASH|UNTIL)=' "$SRC")" = 2 ] && [ "$(sudo grep -c '^HASH=pbkdf2-sha256\$' "$SRC")" = 1 ] || { echo "STOP: $SRC 이(가) guest-conf.py가 쓴 파일이 아님"; exit 1; }
   sudo sed -n 's/^UNTIL=//p' "$SRC"                           # 끝나는 시각(UTC)
   sudo install -o root -g hjviewer-api -m 0640 "$SRC" /etc/hjviewer/guest.conf.new &&
     sudo mv -f /etc/hjviewer/guest.conf.new /etc/hjviewer/guest.conf && sudo rm -f "$SRC"
   sudo /opt/hjviewer/bin/hjv-api admin ticket status | grep '^access password'
   ```
   설정 줄을 넣은 뒤(2번)라면 첫 줄이 `access password: on until <UTC> (hash <8자리>)`이고 그 시각이 위의 `UNTIL`과 같아야 합니다. 바꾼 파일이면 해시의 꼬리표가 달라집니다.
2. **설정 줄**(처음 한 번, 사용자 승인 뒤). 이용 비밀번호를 아는 hjv-api만 이 이름을 압니다. 옛 판은 이 줄이 있으면 시작하지 않습니다(7-17의 2번과 같음).
   ```bash
   sudo grep -qa HJV_API_GUEST_FILE /opt/hjviewer/bin/hjv-api || { echo 'STOP: 설치된 hjv-api가 HJV_API_GUEST_FILE을 모름(먼저 새 묶음으로 바꿈)'; exit 1; }
   E=/etc/hjviewer/hjviewer-api.env
   if sudo grep -q '^#\?HJV_API_GUEST_FILE=' "$E"; then sudo sed -i 's|^#\?HJV_API_GUEST_FILE=.*|HJV_API_GUEST_FILE=/etc/hjviewer/guest.conf|' "$E"; else echo 'HJV_API_GUEST_FILE=/etc/hjviewer/guest.conf' | sudo tee -a "$E" > /dev/null; fi
   sudo grep -n '^HJV_API_GUEST_FILE=' "$E"
   sudo systemctl restart hjviewer-api
   sleep 3; systemctl is-active hjviewer-api                   # active
   sudo /opt/hjviewer/bin/hjv-api admin ticket status | grep '^access password'
   ```
   - `active`가 아니면 7-17의 2번처럼 오류 줄을 사용자에게만 보이고, 그 줄을 지운 뒤 다시 시작합니다.
   - `access password: closed: the file is not there or not used (the journal says why)`이면 `sudo journalctl --namespace=hjviewer -u hjviewer-api --since -5min -o cat | grep 'access password' | tail -2`의 `… is not used: <이유>`를 보고합니다(이유에 해시는 없음). 사용자가 파일을 다시 만듭니다.
3. **바꾸기**: 1번의 블록에 새 파일을 넣습니다. 새 파일은 그때부터의 시도에 쓰이고, 이미 나간 출입증은 12시간 안에 끝납니다. 퍼진 비밀번호라서 바로 끊어야 하면 이어서 9-3을 `NB=now`로 합니다. 그때는 교직원과 묶인 PC의 출입증도 함께 끝나고, 몇 분 안에 저절로 새로 받습니다.
4. **끄기**: `sudo rm /etc/hjviewer/guest.conf`. 다음 시도부터 아무도 이용 비밀번호로 들어오지 못합니다(이미 나간 출입증은 12시간 안에 끝남). `UNTIL`이 지나도 같습니다. 설정 줄은 두어도 됩니다. 1.9.0의 지원 코드는 이 파일을 보지 않으므로 그대로 됩니다. 지원 코드만 끄는 설정은 없습니다(9-7의 '지원 코드가 잘못 쓰임').
5. **보기**: 매일 보고에 `sudo /opt/hjviewer/bin/hjv-api admin ticket status | grep -E '^(access password|support codes)'`의 여섯 줄(파일의 상태, 수명, 어제의 결과별 시도 수, 1.9.0의 hjv-api(9-9)부터 지원 코드의 열린 수와 메모리에 둔 수, `support codes' waiting room:` 줄의 잡아 둔 대기실 요청과 내려받기가 남은 지원 링크의 수, 어제의 결과별 `code` 기록 수)을 넣습니다. 한 주소의 실패 한도에 걸린 경고(`guest: … hjv-api admin unlock tag:<꼬리표> lifts the address's`)가 잦으면 사용자에게 알립니다(줄 수만, 7-0). 풀기는 사용자가 그 줄의 명령으로 합니다.

### 9-5. `MODE=count`로 세어 보기 (선택)

`count`는 거절하지 않고, `enforce`였다면 무엇을 거절했을지만 셉니다. 1.8.0을 나눠 준 뒤 며칠 두면 누가 아직 출입증 없이 쓰는지 개수로 압니다. 9-3을 `MODE=count`로 한 뒤 하루에 한 번 콘솔 줄을 봅니다.

```bash
timeout 3 bash -c 'exec 3<>/dev/tcp/127.0.0.1/21115 && printf hjv-access >&3 && cat <&3'
```

- 줄은 `HJVACCESS1 mode=count` 뒤에 `<자리>.<이유>=<수>`가 이어집니다(hbbs가 시작한 뒤의 수, 0은 빠짐). 자리는 `ph`(연결 요청), `rr`(중계 요청), `reg`(등록), `udp`(UDP 등록)입니다. 이유는 계약 4-1과 7절에 있습니다.
- `ph.ok`, `rr.ok`, `reg.ok`는 출입증으로 들어온 수입니다. `ph.missing`과 `reg.missing`은 출입증이 없는 앱과 PC입니다(1.7.x 이하, 로그인하지 않은 1.8.0, 묶이지 않은 PC). 출시일(9-6의 7단계) 전에 이 수가 줄어야 합니다. 그 기기들이 누구인지는 개수로는 모르고, 운영 에이전트는 찾지 않습니다. 사용자에게 수를 알리고 안내(distribution.md)를 다시 보낼지 묻습니다.
- `bad_signature`, `unknown_kid`, `malformed`가 0이 아니면 키가 맞지 않는 것입니다(9-1). `enforce` 전에 멈추고 보고합니다.
- 끝나면 9-6의 순서를 이어 가거나 9-3으로 `MODE=off`로 되돌립니다.

### 9-6. 출시일 순서

순서가 중요합니다. 출입 확인보다 최소 버전을 먼저 올려야, 출입증이 없는 옛 앱이 "업데이트하세요" 안내를 받습니다. 거꾸로 하면 옛 앱은 hbbs의 기본 문구만 보고, 업데이트 안내를 받지 못합니다.

1. **서버 묶음**(사용자 승인 뒤, 조용한 시간): 출입증이 든 서버 묶음을 6절 "B 경로 업데이트"로 넣고, 출입증 키(9-1)를 만들고, `access.conf`를 `MODE=off`로 둡니다(9-2). 앱에는 아무것도 바뀌지 않습니다.
2. **1.8.0 배포**(개발 PC와 사용자): 1.8.0을 빌드하고 최신 1.8.0의 정책(운영 서버는 seq 4)을 8-2로 넣습니다. 최소 버전은 6단계에서 따로 올리거나, 운영 서버처럼 이 정책에서 함께 올립니다(아래 기록). 설치된 Windows PC는 저절로 1.8.0이 되고(무인 설치 시각), 휴대폰은 TestFlight와 Play 스토어로 받습니다. 사용자는 distribution.md의 "사용자에게 보내는 안내 (출입 확인을 켜기 전)"을 보냅니다.
   - seq 4의 알림 글(`hjv-release policy`의 `--notes`, 모든 플랫폼)에 로그인이 필요해진다는 것을 넣습니다. 앱의 업데이트 화면과 알림이 이 글을 보이므로, 출입 확인을 켜기 전에 앱이 미리 알립니다. 예: `1.8.0부터 HJViewer 서버는 연세대학교 교직원 전용입니다. 업데이트한 뒤 설정 → 계정에서 연세포탈로 로그인해 두세요. 사람이 없을 때도 연결받는 PC는 그 PC에서 로그인해 두어야 합니다.` URL은 넣지 않습니다.
3. **이용 비밀번호와 데모 PC**: 9-4로 `guest.conf`를 넣고, 심사 계정이 있으면 7-17로 `PC=` 줄이 든 `review.conf`를 넣습니다.
4. **(선택) 세어 보기**: 9-5로 `MODE=count`를 며칠 둡니다.
5. **묶인 PC 확인**(사용자, 7-0): 사용자가 `sudo /opt/hjviewer/bin/hjv-api admin binding list`에서 묶인 PC의 버전이 1.8.0인지 보고(1.7.x로 남은 PC는 출입증을 싣지 않으므로 7단계 뒤 오프라인이 됨), 운영 에이전트에게는 버전별 대수만 알려 줍니다.
6. **최소 버전 1.8.0**: 2단계의 정책이 최소 버전을 그대로 두었으면 최소 1.8.0의 정책(그다음 seq)을 8-2로 넣고, 사용자가 `hjv-release hbbs-conf`로 만든 `MIN=1.8.0`의 파일을 8-3으로 넣습니다. 1.6.x와 1.7.x 앱은 업데이트 화면을, 1.5.x 앱과 그 밖의 앱은 hbbs의 최소 버전 문구를 봅니다. 그 정책은 최신 버전이 그대로이므로 `--notes` 없이 만들면 2단계 정책의 알림 글이 그대로 남습니다(업데이트 화면이 그 글을 보임). 운영 서버는 2단계의 seq 4 하나로 최신과 최소를 함께 1.8.0으로 올렸으므로 이 정책이 없었고, hbbs 파일만 넣었습니다. seq 5는 1.9.0의 정책입니다(9-9, 8-2).
7. **출입 확인 켜기**: 9-3을 `MODE=enforce`로 합니다. 10분 뒤 거절 수(9-3의 마지막 항목)와 콘솔 줄을 보고합니다. 예상보다 많으면(예: 교직원의 `ph.missing`이 계속 늚, `bad_signature`가 있음) 바로 9-3을 `MODE=off`로 하고 보고합니다.

기록: 2026-10-19에 1단계(서버 묶음 `server-2026.10.17`, 사용자가 만든 출입증 키 1번, `MODE=off`, 16:28~16:32 KST)와 3단계의 이용 비밀번호(16:38, 2027-01-15 0시 KST까지)를 마치고, 16:39:56(KST)에 `MODE=enforce`로 켰습니다. 2·5·6단계는 2026-10-18에 했습니다(정책 seq 4가 최신과 최소를 함께 1.8.0으로 올림, hbbs `MIN=1.8.0`). 4단계(세어 보기)는 사용자 결정으로 건너뛰었습니다. 10분 보고: 콘솔 `ph.ok=1 reg.ok=1 reg.missing=23`, 거절은 모두 등록의 `missing`(묶인 PC가 켠 직후 1분 동안 2번, 묶이지 않고 로그인하지 않은 기기 1대가 30초마다), 연결·중계 요청의 거절 0, `bad_signature`·`unknown_kid`·`malformed` 0. 데모 PC(`review.conf`의 `PC=`)는 아직 없습니다(사용자 결정, 나중에).

### 9-7. 되돌리기와 사고

- **끄기**(즉시): 9-3을 `MODE=off`로 합니다. 10초 안에 모두 1.7.0 전처럼 연결합니다. 파일을 지우지 않습니다(9-0).
- **hbbs를 다시 시작한 뒤**(6절의 B 경로 업데이트, 재부팅): 12초 뒤 규칙 줄을 봅니다. `access closed`나 ERROR면 hbbs가 모두 거절하고 있으므로 바로 보고합니다. 고칠 수 없으면 설정 줄을 빼고 다시 시작합니다(9-2의 둘째 항목).
  ```bash
  if sudo grep -q '^HJV_ACCESS_FILE=' /etc/hjviewer/hjviewer.env; then sleep 12; sudo journalctl -q -u hjviewer-hbbs --since -1min -o cat | grep -E 'access (off|count|enforce|closed)|access file' | grep -v 'access closed reg' | tail -2; fi
  ```
- **hjv-api가 멈춤**: 교직원과 묶인 PC는 가진 출입증으로 끝날 때까지(24시간, 72시간) 연결합니다. 새 로그인과 이용 비밀번호는 안 됩니다. 하루를 넘길 것 같으면 사용자 승인 뒤 `MODE=off`로 둡니다.
- **이용 비밀번호가 퍼짐**: 사용자가 새 `guest.conf`를 만들고 9-4의 3번(바꾸기와 `NB=now`)을 합니다.
- **지원 코드가 잘못 쓰임**(1.9.0, 계약 3-7: 도난당한 기기, 남의 손에 간 코드): 사용자 승인 뒤 차례로 합니다.
  1. 사용자가 코드를 만든 계정을 `sudo /opt/hjviewer/bin/hjv-api admin block tag:<꼬리표>`로 막습니다(꼬리표는 `code` 기록의 것, 7-0). 그 계정의 열린 코드는 다시 열리지 않고, 나중에 `unblock`해도 그대로 끝나 있습니다. 기기 하나만이면 사용자가 `admin key revoke <ckid>`로 그 기기의 계정 키를 끝내도 그 키로 만든 코드가 끝납니다.
  2. 그 코드로 이미 받은 출입증(2시간)까지 바로 끊으려면 9-3을 `NB=now`로 합니다(교직원과 묶인 PC의 출입증도 끝나고 몇 분 안에 새로 받음).
  3. 모든 열린 코드를 한꺼번에 끝내야 하면 hjv-api를 다시 시작합니다(`sudo systemctl restart hjviewer-api`: 코드는 메모리에만 있음). 새 코드를 아예 받지 않게 하려면 hjv-api를 멈추는 수밖에 없고, 그동안 새 로그인과 이용 비밀번호도 안 됩니다(위의 'hjv-api가 멈춤'). 지원 코드만 끄는 설정이 필요하면 사용자에게 알립니다(아직 없음).
- **출입증 키 바꾸기**(잃었거나 도난당했을 때, 계약 6-4): hbbs가 새 키를 먼저 받고, hjv-api가 나중에 새 키로 서명합니다. 1~3번은 한 번에 이어서 합니다(1번 뒤 hjv-api가 다시 시작하면 새 자격 증명을 옛 번호로 읽어 hbbs가 모든 출입증을 거절함).
  1. 사용자가 7-4 (나)의 출입증 키 줄을 다음 번호(`--kid 2`)로 다시 실행해 자격 증명을 바꿉니다. 돌고 있는 hjv-api는 다시 시작할 때까지 옛 키로 서명합니다. 사용자가 새 첫 줄(`2:<base64>`)을 알려 줍니다.
  2. 운영 에이전트가 설정 파일의 `HJV_API_TICKET_PUBKEYS`를 `<옛 키>,<새 키>`로 바꾸고(hjv-api는 아직 다시 시작하지 않음), 9-3을 지금의 `MODE` 그대로 `NB=keep`으로 합니다. 저널이 `keys 1,2`여야 합니다.
  3. 설정 파일에 `HJV_API_TICKET_KID=2`를 넣고 hjv-api를 다시 시작한 뒤 9-1의 블록을 봅니다(`tickets: on, key 2 …`, selfcheck 두 줄 `[ OK ]`).
  4. 잃기만 했으면 3일(가장 긴 수명 72시간) 뒤, 도난이면 바로 옛 키를 `HJV_API_TICKET_PUBKEYS`에서 빼고 hjv-api를 다시 시작한 뒤, 9-3을 `NB=keep`(도난이면 `NB=now`)으로 합니다. 저널이 `keys 2`여야 합니다. 옛 키의 출입증으로 들어와 있던 PC는 끊기고 새 출입증으로 다시 들어옵니다.
- **hbbs를 출입 확인 전의 판으로 되돌릴 때**: 옛 hbbs는 `HJV_ACCESS_FILE`을 읽지 않으므로 확인 없이 돕니다. 되돌리기 전에 사용자에게 그 뜻(교직원 전용이 아님)을 알립니다.

### 9-8. 보고 (9절)

아래 형식으로 알려 줍니다. 이용 비밀번호, `guest.conf`의 내용, 거절 줄의 주소와 ID는 적지 않습니다.

```
단계: <9-1 | 9-2 | 9-3 (MODE <off|count|enforce>, NOT_BEFORE <값|없음>) | 9-4 (<넣음|바꿈|지움|설정 줄>) | 9-5 | 9-6 (<n>단계) | 9-7 (<무엇>) | 9-9>
hjv-api: ticket status 첫 줄 <그대로>, selfcheck ticket key 두 줄 <OK | FAIL 줄 그대로>
hbbs: 규칙 줄 <access … 그대로>, 콘솔 <HJVACCESS1 … 줄 그대로>
이용 비밀번호: <access password: … 줄 그대로>, 어제 시도 <결과별 수>
지원 코드(1.9.0, 9-9): <support codes: … open … 줄 그대로>, <support codes' waiting room: … 줄 그대로>, 저널의 support code 줄 <종류별 수>, 어제 code 기록 <결과별 수>
거절(enforce): 어제 refused <n>줄, closed <n>줄, more <합>
묶인 PC(9-6의 5단계, 사용자가 알려 준 수): 1.8.0 <n>대, 그 아래 <n>대
바꾼 것: <설치한 파일, 권한, 설정 파일 줄>
문제: <있으면>
```

### 9-9. 1.9.0의 hjv-api: 지원 코드, 지원 링크와 대기실, 내 PC 목록 (W1, TV-W6, U27)

1.9.0 앱과 함께 내는 hjv-api는 교직원이 만드는 지원 코드(계약 3-7)와 그 지원 링크·대기실·지원 확인(TV-W6, 계약 3-7), 내 PC 목록의 `synced`·`wake`(계약 3-8)를 더합니다. 이 셋은 hjv-api 릴리스 하나로 함께 넣습니다(D-3). hbbs, hbbr, `access.conf`, `guest.conf`는 그대로이고, **새 파일, 설정 줄, 자격 증명, DB 이전이 없습니다.** 지원 코드는 출입증 키(9-1)로 출입증을 만들고, 지원 확인은 이미 있는 목록 키 1번(7-4)으로 서명하므로 두 키만 있으면 됩니다. nginx에는 **지원 링크의 경로 한 줄**(`location ^~ /api/hjv/s/`, 묶음의 `nginx-hjviewer-api.conf`)이 더해집니다. 이 줄은 새 hjv-api와 같은 작업에서 **hjv-api보다 먼저** 넣고, 그 뒤로 미루지 않습니다(아래 '넣기').

- **들어 있는 것**: `POST /api/hjv/support-code`(로그인한 교직원의 앱이 코드를 만듦. 계정 요청이라 토큰과 계정 키의 서명이 필요), `POST /api/hjv/guest`의 `code`(로그인하지 않은 PC가 코드를 그 PC의 2시간짜리 `guest`/`pc` 출입증으로 바꿈. 이용 비밀번호와 같은 길, 한도, 실패 지연, nginx 위치), 1.9.0의 TV-W6 부분인 `GET /api/hjv/support-code/wait`(코드를 만든 기기의 대기실. 답을 20초까지 잡아 두며 nginx의 `proxy_read_timeout 30s` 안), `GET /api/hjv/s/<link>/`(지원 링크 페이지와 그 코드가 열린 동안 세 번까지의 HJViewer.exe), 코드로 바꾼 답의 지원 확인(`staff`), 그리고 앱에 알리는 두 표시입니다. 로그인 답과 `currentUser` 답의 `hjv`에 `"support_code":true`(심사 계정이 아닌 교직원의 로그인), `GET /api/hjv/access`의 답에 `"codes":true`(`MODE`가 `count`나 `enforce`이고 출입증 키가 있을 때)입니다. 앱에 그대로 보이는 hjv-api의 글 몇 줄도 1.9.0 앱의 말로 바뀝니다("…다시 시도해 주세요.", "내 PC 목록", "그 PC").
- **앱의 단추는 이 hjv-api 뒤에만**: 1.9.0 앱은 두 표시가 있을 때만 [지원 코드 만들기](교직원 쪽, S1)와 P1의 [지원 코드 넣기](PC 쪽)를 보입니다. 이 hjv-api를 넣기 전에는 1.9.0 앱도 두 단추를 보이지 않고, 나머지는 그대로 됩니다. 1.8.0 앱은 두 표시를 모르고 지나갑니다. 그래서 앱과 서버의 순서는 상관없지만, 사용자 안내([distribution.md](distribution.md)의 "사용자에게 보내는 안내 (1.9.0)")의 지원 코드 줄은 이 hjv-api를 넣은 뒤에 보냅니다.
- **넣기**(사용자 승인 뒤 허브-운영이, 조용한 저녁에. cron의 nginx 재적재 시각은 피함): 한 작업에서 이 순서로 합니다.
  1. **nginx 줄 먼저**: 새 묶음을 4-1처럼 내려받아 체크섬을 확인한 뒤, 그 디렉터리에서 `sudo install -m 644 web/* /opt/hjviewer/share/web/`만 하고(hbbs는 다시 시작하지 않음) 6절 "B 경로 업데이트"의 "nginx 경로 파일" 블록으로 설치된 nginx 파일과 비교합니다. `nginx-hjviewer-api.conf`만 `differ`여야 하고, `sudo diff -u /etc/nginx/snippets/hjviewer-api.conf /opt/hjviewer/share/web/nginx-hjviewer-api.conf`의 차이는 다음 세 덩어리뿐이어야 합니다. (가) 파일 머리 주석의 패키지 문단(`update, u, get` 뒤에 `and from 1.9.0 the support links' s`가 붙고, 대기실 `/api/hjv/support-code/wait`가 20초까지 잡아 둔다는 문장이 더해짐), (나) `location ^~ /api/hjv/u/` 위 주석(지원 링크가 더해져 `these two`가 `these three`가 되고, 지원 링크의 `location`을 hjv-api보다 먼저 넣는다는 두 줄이 더해짐), (다) `location ^~ /api/hjv/s/ { error_log /dev/null crit; proxy_pass …; }` 한 블록. 다른 차이가 있으면 멈추고 보고합니다. 그 블록 아래의 1–3번(백업, 설치, `nginx -t`와 reload, 5절의 확인)으로 넣습니다. 옛 hjv-api는 이 경로에 404 JSON으로 답하므로 이 순서에서 깨지는 것은 없습니다. `nginx -t`가 실패하면 되돌리고, 2번도 하지 않고 멈춥니다.
  2. **그다음 hjv-api**: 7-14의 "업그레이드"대로 스냅숏, 옛 실행 파일, 설정 파일을 남긴 뒤 같은 디렉터리에서 6절 "B 경로 업데이트"의 4번(hjv-api만)을 합니다. hbbs와 hbbr은 다시 시작하지 않으므로 원격 세션은 끊기지 않습니다.
  3. **고지 파일**: 같은 디렉터리에서 묶음의 고지를 `/opt/hjviewer/share/`에 넣습니다. `SOURCE.txt`가 새 hjv-api의 소스 커밋을 가리키게 하기 위해서입니다(`/api/hjv/source`는 hjv-api가 따로 답함). 지금 것은 먼저 남깁니다.
     ```bash
     VERSION=2026.10.20                # ← 이 묶음의 Release 버전
     sudo install -d -m 700 "/root/hjv-backup/share-before-$VERSION"
     sudo cp -p /opt/hjviewer/share/*.txt "/root/hjv-backup/share-before-$VERSION/"
     sudo install -m 644 LICENSE.txt NOTICE.txt SOURCE.txt THIRD_PARTY_NOTICES.txt /opt/hjviewer/share/
     head -3 /opt/hjviewer/share/SOURCE.txt
     ```
     2026-10-20의 1.9.0 작업에서는 이 단계가 빠져, `NOTICE.txt`와 `SOURCE.txt`를 정책 seq 5와 같은 작업에서 바꿨습니다(8-2의 기록).
  - 순서를 거꾸로 하면(새 hjv-api가 돌고 nginx 줄이 아직 없음) 지원 링크의 요청이 오류로 끝날 때(hjv-api가 다시 시작하는 몇 초, 끊긴 내려받기) nginx 오류 로그에 링크 값이 든 경로가 남고, 같은 서버의 웹 사이트 사용자가 그 로그를 읽을 수 있습니다. 그래서 nginx 줄을 hjv-api 뒤로 미루지 않습니다(짧은 링크와 문 2의 `location`과 같은 까닭). hjv-api가 다시 시작하는 몇 초 동안은 로그인, 동기화, 이용 비밀번호가 멈춥니다(가진 출입증은 그대로).
- **확인**:
  ```bash
  sudo /opt/hjviewer/bin/hjv-api admin ticket status | grep -E '^(tickets|support codes)'
  D=$(sudo sed -n 's/^HJV_DOMAIN=//p' /etc/hjviewer/hjviewer.env)
  curl -s --max-time 5 --resolve "$D:443:127.0.0.1" "https://$D/api/hjv/access"; echo   # enforce: {"codes":true,"mode":"enforce"}
  ```
  - 첫 줄이 `tickets: on, key …`이고, `support codes: 0 open, 0 held in memory (…)`, `support codes' waiting room: 0 held (…); support links with downloads left: 0`, `support codes, the last day's records: …` 세 줄이 있어야 합니다. `support codes` 줄이 없으면 옛 hjv-api가 돌고 있습니다.
  - 마지막 줄은 앱이 보는 답입니다. `MODE=enforce`(또는 `count`)인데 `codes`가 없으면 출입증 키가 없거나(`tickets: off`) 옛 hjv-api입니다. 그동안 PC는 [지원 코드 넣기]를 보이지 않습니다. 멈추고 보고합니다. `off`, 닫힘, `unknown`에서는 `codes`가 없는 것이 맞습니다(계약 3-6).
  - 지원 링크(1.9.0, TV-W6): 링크 값이 nginx 오류 로그에 남지 않게 하는 것은 불러온 설정의 줄이므로 그것을 봅니다. `sudo nginx -T 2>/dev/null | grep -F -A2 'location ^~ /api/hjv/s/'`가 그 `location` 줄 바로 다음에 `error_log /dev/null crit;`(그리고 `proxy_pass http://unix:/run/hjviewer-api.sock:;`)를 보여야 합니다. `-F`로 `^`와 `~`를 글자 그대로 찾습니다. 운영 서버의 `grep`은 다른 검색 도구를 감싼 명령이라, `-F` 없이는 줄 가운데의 `^`를 줄의 처음으로 읽어 아무것도 찾지 못했습니다(2026-10-20). 아무것도 나오지 않거나 `error_log` 줄이 없으면 1번의 nginx 줄이 들어가지 않은 것이니 멈추고 보고합니다. 오류 로그를 `grep`해 보는 것으로는 확인이 되지 않습니다. nginx는 hjv-api가 답한 404를 오류 로그에 쓰지 않고, 옛 hjv-api도 404로 답하기 때문입니다. 그리고 `admin ticket status`의 `support codes' waiting room:` 줄(첫 확인의 둘째 줄)에 잡아 둔 대기실 요청의 수와 내려받기가 남은 지원 링크의 수가 보여야 합니다. 그 줄이 없으면 TV-W6 전의 hjv-api입니다. 멈추고 보고합니다.
- **저널의 줄**: 코드와 그 꼬리표는 어느 줄에도 없습니다. 지원 코드에 관한 줄은 다음뿐이고, 주소와 계정은 꼬리표로만 나옵니다.
  - `support code: an account holds its 5 open codes (acct=<꼬리표>); the next ten minutes are not logged`: 한 계정이 쓰지 않은 코드 5개를 채웠습니다(그 계정에 10분에 한 줄). 잦으면 사용자에게 알립니다.
  - `support code: 1000 open codes are held, no more made`: 열린 코드가 모두 합쳐 1000개라 새 코드를 만들지 않습니다(503 `busy`). 바로 보고합니다.
  - `support code: more than 30 calls a minute from one key (acct=<꼬리표>, ckid=<n>)`, `support code: a request not of the form (…)`: 한 기기의 지나친 요청, 모양이 틀린 요청입니다.
  - `guest: <결과>, a support code (ip=<꼬리표>); hjv-api admin unlock tag:<꼬리표> lifts the address's`: 한 주소가 코드 시도로 한도(`limited-ip` 등)에 걸렸습니다. 이용 비밀번호와 같은 한도이므로 9-4의 5번처럼 다룹니다(풀기는 사용자가 그 명령으로, 7-0).
  - `guest: more than 20 failed tries of the access password and support codes this minute; the rest are not recorded`: 실패가 몰려 기록을 줄였습니다.
  ```bash
  sudo journalctl --namespace=hjviewer -u hjviewer-api --since yesterday -o cat | grep -E '^(support code|guest: .*a support code)' | sed -E 's/(acct|ip|dev)=[0-9a-f]+/\1=…/g; s/ckid=[0-9]+/ckid=…/g; s/tag:[0-9a-f]+/tag:…/g' | sort | uniq -c
  ```
  줄 수와 종류만 보고에 옮깁니다(꼬리표가 필요하면 사용자가 서버에서 직접 봄, 7-0). 만든 코드와 시도의 결과별 수는 저널이 아니라 `admin ticket status`의 `support codes, the last day's records` 줄(9-4의 5번)에서 봅니다.
- **1.9.0 출시의 순서**(D-2, D-3): ① 이 hjv-api를 위의 '넣기'대로 넣습니다(nginx 줄 먼저, 조용한 저녁, 허브-운영). ② 1.9.0의 패키지와 정책 seq 5(세 플랫폼 최소·최신 1.9.0)를 8-2로 넣습니다. seq 5는 ①이 끝난 뒤에 사용자가 서명합니다. 그 정책의 `--contact`는 관리자 연락처 자체이고([tools/hjv-release/README.md](../tools/hjv-release/README.md)), 그 값은 저장소와 이 런북에 적지 않습니다. 유예와 Windows 무인 설치 시각은 seq 5의 초안을 만들 때 사용자가 정합니다. ③ seq 5가 서버에 들어간 뒤 hbbs의 최소 컨트롤러를 `MIN=1.9.0`으로 올립니다(8-3, 사용자가 `hjv-release hbbs-conf`로 만든 파일). ①을 ② 뒤로 미루지 않습니다. 1.9.0 앱은 서버가 두 표시를 주지 않으면 지원 코드 단추를 보이지 않습니다.
  - 기록(2026-10-20, KST): ① 서버 묶음 `server-2026.10.20`(hjv-api 2404ffa). nginx 줄을 넣고 13:48:57에 reload, hjv-api는 13:50:43에 다시 시작했습니다(hbbs와 hbbr은 그대로). 확인: `/api/hjv/access`가 `{"codes":true,"mode":"enforce"}`, 없는 지원 링크가 404, selfcheck 통과. ② 정책 seq 5(유예 없음)를 17:01:43에 넣었습니다(8-2의 기록). ③ hbbs `MIN=1.9.0`을 17:28:47에 넣었습니다(8-3의 기록). 이 작업에서 빠진 고지 파일 단계(위 '넣기'의 3번)는 ②와 같은 작업에서 했습니다.
- **되돌리기**: 7-14의 "업그레이드"대로 옛 실행 파일로 돌아가면 두 표시가 없어지고, 앱은 다음 물음부터 두 단추를 감춥니다. 이미 바꾼 출입증은 2시간 안에 끝납니다. 잘못 쓰인 코드는 9-7의 '지원 코드가 잘못 쓰임'입니다.

### 9-10. 1.9.1의 hjv-api: 앱 안의 계정 삭제 (B1)

1.9.1 앱과 함께 내는 hjv-api는 앱 안의 계정 삭제 `POST /api/hjv/account/delete`를 더합니다(App Store 지침 5.1.1(v), 사용자 결정 2026-10-26). 동작과 형식은 [server/api/README.md](../server/api/README.md)의 "Account deletion"에 있습니다. hbbs, hbbr, `access.conf`, `guest.conf`, `review.conf`는 그대로이고, **새 파일, 설정 줄, 자격 증명, DB 이전, nginx 줄이 없습니다.** nginx는 `/api/hjv/`를 접두어로 넘기고(`location ^~ /api/hjv/`), Caddy와 Apache도 같습니다.

- **들어 있는 것**: 로그인한 앱이 계정 키로 서명한 요청 하나(본문 `{"confirm":"delete-account"}`)로 자기 계정을 바로 지웁니다. 지우는 것과 남는 것은 `user delete`와 같고(7-15), App Review 계정만 명단 항목이 남습니다(7-17). 그 계정의 키로 만든 열린 지원 코드도 메모리에서 바로 끝납니다. 요청은 앱이 연결을 끊어도 끝까지 돕니다. 새로 생기는 것은 운영자 기록의 `cmd="app account-delete"` 두 줄(7-15), `report`와 하루 요약의 기록 `account deleted-by-user`, 아래 저널 줄입니다.
- **운영자 기록이 있어야 지움**: 서비스가 운영자 기록에 앞 줄을 쓰지 못하면(관리 소켓이 꺼짐, 기록 파일에 쓸 수 없음) 계정을 지우지 않고 앱에 503 `not_recorded`를 줍니다. 운영 서버에는 관리 소켓과 기록 파일이 늘 있으므로 할 일은 없습니다.
- **넣기**(사용자 승인 뒤 허브-운영이, 조용한 시간에): 7-14의 "업그레이드"대로 스냅숏, 옛 실행 파일, 설정 파일을 남긴 뒤 새 묶음으로 6절 "B 경로 업데이트"의 4번(hjv-api만)을 합니다. hbbs와 hbbr은 다시 시작하지 않으므로 원격 세션은 끊기지 않습니다. 같은 디렉터리에서 9-9 '넣기'의 3번(고지 파일)도 합니다. 6절의 "nginx 경로 파일" 블록으로 묶음의 nginx 파일과 설치된 것을 비교해 다른 것이 없는지만 봅니다. 다르면 멈추고 보고합니다.
- **순서**: ① 이 hjv-api를 먼저 넣습니다. ② 그 뒤 1.9.1 앱을 나눕니다. App Store 빌드는 이 hjv-api가 돈 뒤에 심사에 냅니다(심사자가 데모 계정을 지워 봄). 옛 앱은 이 경로를 모르므로 바뀌는 것이 없고, 1.9.1 앱이 옛 hjv-api를 만나면 404를 받아 "지금은 앱에서 계정을 삭제할 수 없습니다. 관리자에게 삭제를 요청해 주세요."를 보입니다.
  - 기록(2026-10-27, KST): ① 서버 묶음 `server-2026.10.27`(hjv-api b389ea2)을 허브-운영이 사용자 승인 뒤 넣었습니다. 옛 hjv-api가 이 경로에 404인 것을 먼저 보고, 7-14의 스냅숏(`before-2026.10.27`, 스키마 6)과 옛 실행 파일·설정 파일을 남긴 뒤 00:14:32에 hjv-api만 다시 시작했습니다(유닛과 소켓은 그대로, hbbs와 hbbr은 다시 시작하지 않음). 고지 파일은 `/root/hjv-backup/share-before-2026.10.27/`에 남긴 뒤 바꿨고(`SOURCE.txt`가 b389ea2), nginx 파일은 비교만 했습니다(다른 것 없음). 8-2의 확인 스크립트 `hjv-update-verify.sh`도 이 묶음의 것으로 바꾸고 `--self-test`를 돌렸습니다(첫 `--appstore-live` 정책의 조건, 8-2). 확인: 토큰 없는 요청이 401(정해진 문구), `/api/hjv/source`가 b389ea2, `/api/hjv/access`가 `{"codes":true,"mode":"enforce"}`, selfcheck 통과. ② 1.9.1의 Windows와 Android를 정책 seq 6으로 나눴습니다(09:18:43, 8-2의 기록). iOS의 1.9.1은 TestFlight에 올렸고, App Store에는 아직 내지 않았습니다.
- **확인**: 토큰 없는 요청이라 아무것도 지우지 않습니다.
  ```bash
  D=$(sudo sed -n 's/^HJV_DOMAIN=//p' /etc/hjviewer/hjviewer.env)
  curl -s --max-time 5 --resolve "$D:443:127.0.0.1" -X POST -H 'Content-Type: application/json' -d '{}' "https://$D/api/hjv/account/delete"; echo
  ```
  `{"error":"로그인이 필요합니다. 다시 로그인해 주세요."}`(401)가 나와야 합니다. `{"error":"not found"}`(404)이면 옛 hjv-api가 돌고 있습니다. 멈추고 보고합니다.
- **저널의 줄**: 포탈 ID, 이름, 주소, 기기 ID는 어느 줄에도 없고, 계정은 꼬리표로만 나옵니다.
  - `account: deleted at its own request (acct=<꼬리표>): <p> PCs, <k> account keys, <t> tokens, <r> relays, allowlist entry <removed | kept | stays blocked | none>`: 한 계정이 앱에서 지워졌습니다. App Review 계정이면 꼬리표 뒤에 `, the App Review account`가 붙고, 1.9.2부터 그 계정의 데모 PC가 새 빈 계정으로 넘어갔으면 끝에 `, <d> demo PCs kept`가 붙습니다(`<p>`는 풀린 PC만, 9-11).
  - `account delete: no operator record (the admin socket is off): a deletion is refused (acct=<꼬리표>)`, `account delete: the operator record did not take the line (acct=<꼬리표>); nothing was deleted`: 운영자 기록 때문에 삭제를 거절했습니다(사용자는 "삭제 기록을 남기지 못해 계정을 지우지 않았습니다. …"를 봄). 바로 보고합니다.
  - `account delete: the operator record did not take the result line (acct=<꼬리표>); the account is deleted`: 지웠지만 기록의 끝 줄을 쓰지 못했습니다. 보고합니다.
  - `account delete: database: <오류> (acct=<꼬리표>); nothing was deleted`: DB 오류로 아무것도 지우지 않았습니다(앱은 500, 운영자 기록 끝 줄은 `failed: database`). 보고합니다.
  - `account delete: the WAL was not truncated after a deletion (acct=<꼬리표>): <오류>`: 계정은 지워졌지만 WAL을 비우지 못해, 지운 행이 WAL 파일에 잠시 남을 수 있습니다. 보고합니다.
  - `account delete: the login ended while the request waited (acct=<꼬리표>); nothing was deleted`: 요청이 기다리는 사이 그 로그인이 다른 길로 끝나(로그아웃, 다른 기기의 "다른 기기 모두 로그아웃", `user revoke-all`, `block`) 아무것도 지우지 않았습니다(앱은 401, 운영자 기록 끝 줄은 `failed: the login ended`). 할 일은 없습니다.
  - `account delete: the request's task failed`: 요청을 처리하던 서비스 안에서 오류가 났습니다(500). 보고합니다.
  - `account delete: more than 30 calls a minute from one key (…)`, `account delete: a request not of the form (…)`: 한 기기의 지나친 요청, 모양이 틀린 요청입니다.
  ```bash
  sudo journalctl --namespace=hjviewer -u hjviewer-api --since yesterday -o cat | grep -E '^account( delete)?:' | sed -E 's/(acct|ip|dev)=[0-9a-f]+/\1=…/g; s/ckid=[0-9]+/ckid=…/g; s/tag:[0-9a-f]+/tag:…/g' | sort | uniq -c
  ```
  줄 수와 종류만 보고에 옮깁니다(꼬리표가 필요하면 사용자가 서버에서 직접 봄, 7-0).
- **되돌리기**: DB 스키마가 같으므로 7-14의 "업그레이드"대로 옛 실행 파일과 설정 파일로 돌아가면 됩니다(스냅숏은 넣지 않음: 넣으면 그 뒤에 지운 계정이 돌아옴). 이 경로는 404가 되고, 1.9.1 앱은 [계정 삭제]에서 위의 404 글을 보입니다. 이미 지운 계정은 되돌아오지 않습니다.

### 9-11. 1.9.2의 hjv-api: 심사 계정의 데모 PC

1.9.2의 App Store 심사에 맞춰 낸 hjv-api는 App Review 계정(7-17)이 `review.conf`의 `PC=` 줄에 적힌 데모 PC를 직접 가지게 합니다(소유자 결정 2026-10-28, [access-tickets.md](access-tickets.md)의 1절과 2-4). 바뀌는 바이너리는 **hjv-api 하나**이고, hbbs, hbbr, `access.conf`, `guest.conf`, nginx 줄, `review.conf`의 형식(`deploy/review-conf.py`)은 그대로입니다. **새 파일, 설정 줄, 자격 증명, DB 이전(스키마 6 그대로)이 없습니다.** 앱도 그대로(1.9.2)입니다.

- **들어 있는 것**
  - 데모 PC에서 이 계정으로 로그인하면, 그 PC가 `PC=` 줄에 있고 파일이 활성일 때만 교직원 PC처럼(설치형, `HJV_API_BIND_OS`, hbbs가 가진 키와 맞는 서명) 이 계정에 묶습니다. `PC=` 밖의 기기는 묶지 않고 hbbs에 묻지도 않습니다.
  - 데모 PC의 동기화 답은 등록만 하는 출입증 `staff`/`pc`(24시간, `HJV_API_TICKET_HOURS`, `UNTIL`을 넘지 않음)를 싣습니다. 교직원 PC의 `staff`/`both`와 달리 이 출입증으로는 다른 PC에 연결할 수 없습니다. 파일이 없거나 `UNTIL`이 지나면 주지 않으므로, `UNTIL`에 데모 PC의 등록이 끝납니다.
  - 이 계정의 내 PC 목록은 비어 있고, 데모 PC를 [내 PC에서 빼기]로 빼지 못하며(404), 데모 PC에서 로그인한 직후의 [취소]도 없습니다(`this-device`의 `undo`가 거짓이고, 그래도 온 [취소]는 아무것도 바꾸지 않음). 깨우기 경로도 데모 PC를 모릅니다(404). 데모 PC의 신뢰 목록은 스위치와 상관없이 늘 `disabled`(키 없음)라 비밀번호 없는 연결이 없습니다. `admin passwordless on|off --account tag:$T`는 스위치를 적어 두기만 하고, 답 끝에 `; the App Review account's PCs take no password-less connection, whatever its switch`가 붙습니다.
  - 심사자가 앱에서 이 계정을 지우면, 계정은 지금처럼 지워지고 데모 PC의 묶음만 같은 트랜잭션에서 만든 같은 ID의 새 빈 계정으로 넘어갑니다(묶음 ID, 동기화 기록 그대로, 무덤 없음). 데모 PC 자신의 로그인에서 온 삭제는 403 `demo_pc`("이 PC는 App Review 계정의 데모 PC라서 여기서는 계정을 지울 수 없습니다. iPhone이나 iPad의 HJViewer에서 지우세요. …")로 거절합니다(PC를 계정의 로그인 키로 찾으므로 iPad의 삭제와 겹친 요청도). 교직원 계정에 묶인 `PC=`의 PC는 그 계정의 것으로 남습니다.
  - 데모 PC의 기기 키로 받는 문 1(새 버전)은 403입니다(주소의 실패로 세지 않음). 심사 중에는 Windows 정책의 `latest`를 1.9.2로 둡니다(8-2).
  - `user revoke-all`, iPad의 로그아웃, hjv-api 재시작은 데모 PC를 건드리지 않습니다. 풀거나 끝내는 것은 `user delete`와 `binding revoke`입니다(7-17의 6번).
- **넣기**(사용자 승인 뒤 허브-운영이, 조용한 시간에): 7-14의 "업그레이드"대로 스냅숏, 옛 실행 파일, 설정 파일을 남긴 뒤 새 묶음으로 6절 "B 경로 업데이트"의 4번(hjv-api만)을 합니다. hbbs와 hbbr은 다시 시작하지 않으므로 원격 세션은 끊기지 않습니다. 같은 디렉터리에서 9-9 '넣기'의 3번(고지 파일)도 하고, 6절의 "nginx 경로 파일" 블록으로 묶음의 nginx 파일과 설치된 것을 비교만 합니다(다르면 멈추고 보고).
- **순서**: ① 이 hjv-api를 넣습니다. ② 사용자가 `PC=`가 든 `review.conf`를 만들고 허브-운영이 7-17의 1번으로 넣습니다. ③ 7-17의 "데모 PC 묶기"(데모 PC를 설치해 hbbs가 키를 안 뒤에 `PC=`, 바로 데모 PC에서 이 계정으로 로그인, 데모 PC 확인). 데모 PC를 교직원 계정에 묶어 두었다면 그 전에 그 PC에서 그 교직원 로그인을 로그아웃해 풉니다. ④ 7-17의 4번(사용자의 iPhone 확인을 [계정 삭제]로 끝냄). ⑤ 심사 메모와 자체 시험(distribution.md) 뒤에 제출합니다.
- **확인**(넣은 뒤)
  ```bash
  systemctl is-active hjviewer-api                                # active
  D=$(sudo sed -n 's/^HJV_DOMAIN=//p' /etc/hjviewer/hjviewer.env)
  curl -s --max-time 5 --resolve "$D:443:127.0.0.1" "https://$D/api/hjv/source"; echo   # commit이 새 묶음의 커밋
  sudo /opt/hjviewer/bin/hjv-api admin selfcheck
  sudo /opt/hjviewer/bin/hjv-api admin ticket status | grep '^lifetimes:'   # … the App Review account's demo PCs 24 h, never past its file's UNTIL
  sudo journalctl --namespace=hjviewer -u hjviewer-api --since -5min -o cat | grep -e 'App Review file (HJV_API_REVIEW_FILE)' -e 'no App Review account' | tail -2   # 7-17의 2번처럼 read 줄
  ```
  `lifetimes:` 줄에 `demo PCs`가 없으면 옛 hjv-api가 돌고 있습니다. 멈추고 보고합니다.
- **저널의 새 줄**(포탈 ID, 기기 ID는 없고 계정과 PC는 꼬리표로만. 운영 에이전트는 7-17의 5번처럼 `grep -c`로 줄 수만 셈, 7-0)
  - `login: a demo PC joined the App Review account (acct=<T>, dev=<D>)`: 데모 PC가 이 계정에 묶였습니다. 사용자가 묶은 때만 나와야 합니다(7-17의 "데모 PC 묶기" 4단계, 5번).
  - `account delete: refused from a demo PC of the App Review account (acct=<T>); the next ten minutes are not logged`: 데모 PC 자신의 [계정 삭제]를 거절했습니다(아무것도 지우지 않음). 누가 데모 PC의 HJViewer에서 로그인해 지우려 한 것이므로 보고합니다.
  - `update: a demo PC of the App Review account asked for a package and was refused (dev=<D>); …`: 데모 PC의 자동 업데이트가 새 버전을 찾았습니다. 심사 중 Windows `latest`가 1.9.2보다 높은지 봅니다.
  - `account: deleted at its own request (acct=<T>, the App Review account): …, <d> demo PCs kept`: 심사자(나 사용자)가 앱에서 계정을 지웠고 데모 PC는 남았습니다(9-10).
- `admin ticket status`의 `sync pc <n>`은 시작한 뒤 데모 PC에 준 출입증의 수입니다(교직원 PC는 `sync both`). 데모 PC가 동기화하는 동안(보통 2분마다) 늘어야 합니다.
- **되돌리기**: 먼저 데모 PC를 풉니다(7-17의 7번: `user delete "tag:$T"`, 또는 사용자의 `binding revoke`, 7-0). 풀지 않고 옛 판으로 돌아가면 옛 판은 PC를 가진 이 계정의 로그인을 `review-existing`으로 거절하고, 데모 PC에는 교직원 PC처럼 `staff`/`both`를 줍니다. 그다음 DB 스키마가 같으므로 7-14의 "업그레이드"대로 옛 실행 파일과 설정 파일로 돌아갑니다(스냅숏은 넣지 않음).
