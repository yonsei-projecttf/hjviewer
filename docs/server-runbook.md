# 서버 배포 런북 (서버에서 작업할 에이전트·운영자용)

Ubuntu 22.04/24.04 서버 한 대에 HJViewer 서버(hbbs·hbbr)를 Docker로 띄우고 HTTPS(443)로 연결되게 하는 절차입니다. 위에서부터 순서대로 실행하고, 마지막에 **보고** 형식으로 결과를 알려 주세요. 배경 설명은 [deploy.md](deploy.md)에 있습니다. 다만 그 문서의 명령은 실행하지 말고, 이 런북의 명령만 실행하세요.

이 런북이 쓰는 이미지 빌드, compose 파일, nginx·Apache 설정 파일, Caddy 구성, 확인 명령은 개발 PC에서 `scripts/ci.sh`로 시험합니다. 개발 PC는 운영과 같은 Ubuntu 24.04와 3-1단계의 Docker 패키지를 씁니다([ci.md](ci.md)).

## 실행 규칙

- **코드 블록 하나는 새 셸 하나에서 통째로 실행합니다**(예: `bash -c '...'`). 블록 안의 `exit`는 그 셸만 끝냅니다. 사람이 SSH 터미널에 붙여 넣을 때는 먼저 `bash`를 입력해 하위 셸을 연 다음 붙여 넣으세요. 셸 변수는 다음 블록으로 이어지지 않으므로 블록마다 첫 줄에서 `DOMAIN`을 정합니다. 4단계부터는 `.env`에서 읽습니다.
- **"멈춤"**이라고 적힌 경우에는 그 자리에서 아무것도 바꾸지 마세요. 그때까지의 출력을 붙여 사용자에게 보고하고 지시를 기다립니다.
- 오래 걸리는 빌드는 백그라운드로 돌리고 로그로 확인합니다(5단계). 그 밖의 명령은 길어야 몇 분입니다.
- 비밀번호와 개인키는 채팅으로 주고받지 않습니다.

## 입력값

- `DOMAIN`: 이 서버에 붙일 **HJViewer 전용 호스트 이름**(예: `hjviewer.example.ac.kr`). 앱에 들어가는 값이라 정한 뒤에는 바꾸지 않습니다. 모르면 사용자에게 물어보세요.
- DOMAIN이 이미 이 서버의 다른 웹사이트 주소라면, 사용자가 **그 사이트와 함께 쓰기로** 정했는지 확인합니다. 정했다면 2단계 4번에서 멈추지 않고, 6-B의 "(나) 기존 사이트에 추가"로 갑니다. 그 사이트의 인증서를 그대로 씁니다.
- 인증서를 새로 받아야 할 때만 필요한 것: Let's Encrypt 알림을 받을 **이메일**과 약관 동의. 6-B단계에서 필요해지면 사용자에게 묻습니다.

## 하지 말 것

- 이 서버의 **다른 서비스(웹 서버, 컨테이너, DB)를 멈추거나 재시작하거나 교체하지 마세요.** 웹 서버는 설정을 검사한 뒤 reload만 합니다.
- 웹 서버 전역 설정(`nginx.conf`, `apache2.conf`)과 다른 사이트의 설정 파일은 고치지 마세요. HJViewer용 파일 하나만 새로 추가합니다. 사용자가 기존 사이트와 같은 도메인을 쓰기로 정한 경우만 6-B의 해당 절차를 따릅니다.
- Docker 전역 설정(`/etc/docker/daemon.json`)을 바꾸거나, Docker를 재시작하거나 업그레이드하지 마세요. 서버의 모든 컨테이너가 함께 재시작됩니다.
- `docker system prune`, `docker image prune -a`, `docker volume prune`, `docker builder prune`, `docker compose down -v`, `--remove-orphans`를 쓰지 마세요. 다른 서비스의 컨테이너, 이미지, 볼륨이나 Caddy 인증서가 지워집니다.
- `apt upgrade`나 `full-upgrade`를 하지 마세요. 패키지는 3-1과 6-B의 명령으로만 설치합니다.
- 80/443 외의 포트를 열지 마세요. 방화벽(ufw, iptables/nftables, firewalld)과 fail2ban 규칙도 바꾸지 마세요. 꺼져 있는 ufw를 켜면 SSH가 끊길 수 있습니다.
- hbbs/hbbr 포트(21115–21119)를 외부에 공개하지 마세요. hbbs는 접속한 쪽이 보내는 `X-Real-IP`, `X-Real-Port` 헤더를 그대로 믿습니다.
- **서버 개인키 `deploy/data/id_ed25519`를 출력(cat, less, base64 등)하거나 복사하거나 전송하지 마세요.** `id_ed25519*` 같은 와일드카드도 쓰지 마세요. 에이전트가 본 출력은 기록에 남습니다. 보고할 것은 `id_ed25519.pub`(공개키)뿐입니다.
- `/opt/hjviewer`가 이미 있으면 지우거나 다시 clone하지 마세요. `git clean -x`, `git reset --hard`, `rm -rf deploy/data`도 쓰지 마세요. 개인키를 잃으면 이미 배포한 앱을 모두 다시 빌드해야 합니다.

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
echo "== outbound access for the build (000 = blocked)"
for u in https://github.com https://index.crates.io/config.json https://static.crates.io https://deb.debian.org/debian/ https://public.ecr.aws/v2/ https://registry-1.docker.io/v2/ https://acme-v02.api.letsencrypt.org/directory; do echo "$(curl -s -o /dev/null -w '%{http_code}' --max-time 10 "$u") $u"; done
echo "== MTU"; dev=$(ip route get 1.1.1.1 2>/dev/null | grep -o 'dev [^ ]*' | cut -d' ' -f2); echo "$dev $(cat "/sys/class/net/$dev/mtu" 2>/dev/null)"
echo "== firewall (information only)"; $S ufw status 2>/dev/null | head -1; systemctl is-active firewalld nftables 2>/dev/null | paste -sd' '
echo "== bridges (VMs, LXD)"; ip -br link show type bridge
echo "== existing install"; ls -ld /opt/hjviewer 2>&1
```

## 2. 판단

1단계 출력을 보고 아래를 차례로 판단합니다.

1. **sudo**: `SUDO_NEEDS_PASSWORD`면 멈춥니다. 사용자에게 두 가지 중 하나를 요청합니다. 이 작업 동안만 비밀번호 없는 sudo를 주거나, sudo 명령을 사용자가 직접 실행하는 것입니다. 비밀번호를 채팅으로 받지 마세요.
2. **경로**
   - **A (Caddy)**: 80과 443이 ss와 docker ps 어디에도 없고, nginx·apache2·caddy·haproxy·traefik이 하나도 enabled가 아닐 때.
   - **B (기존 웹 서버 뒤)**: 80/443을 쓰는 프로그램이 이 서버에 직접 설치된 `nginx` 또는 `apache2`일 때(ss의 프로세스 이름).
   - 그 밖의 경우에는 멈춥니다. 다른 컨테이너(`docker-proxy`)나 caddy·haproxy·traefik이 80/443을 쓰는 경우, 둘 중 한 포트만 쓰이는 경우, 웹 서버가 꺼져 있지만 enabled인 경우(재부팅 뒤 Caddy와 포트를 다툼)가 여기에 해당합니다.
   - 21115–21119를 이미 쓰는 프로그램이 있으면 멈춥니다(예: 기존 RustDesk 서버).
3. **DNS**
   - DNS에 A 레코드가 없으면(새 서브도메인) 멈춥니다. 사용자에게 이 서버의 공인 IPv4로 A 레코드를 만들어 달라고 요청합니다.
   - A 값이 `outgoing IPv4`나 `ip -br addr`의 주소와 같으면 진행합니다. 둘 다 다르면 멈춥니다. 그 주소의 80/443이 이 서버로 전달되는지(NAT, 포트 포워딩) 사용자에게 확인합니다.
   - AAAA가 있으면 그 주소가 `ip -br addr`에 있어야 합니다. 없으면 멈춥니다. 사용자가 AAAA를 지우거나 이 서버 주소로 고쳐야 합니다. 그대로 두면 IPv6를 먼저 쓰는 클라이언트와 인증서 발급이 실패합니다.
   - `/etc/hosts`에 DOMAIN이 있으면 서버 안에서 하는 확인이 바깥과 다르게 나올 수 있습니다. 보고에 적습니다.
4. **B 경로에서 웹 서버 설정**
   - nginx는 `nginx -t`, Apache는 `apache2ctl configtest`가 지금 이미 실패하면 멈춥니다. 원래 있던 문제를 이 작업이 떠안지 않기 위해서입니다.
   - DOMAIN이 nginx의 `server_name`이나 Apache의 `apache2ctl -S`에 이미 있으면 **기존 사이트와 같은 도메인**입니다. 사용자가 그 사이트와 함께 쓰기로 이미 정했다면 진행합니다(6-B의 "(나) 기존 사이트에 추가"). 정하지 않았다면 멈추고 고르게 합니다. 하나는 새 서브도메인(입력값을 바꿔 1단계부터 다시)이고, 다른 하나는 기존 사이트에 추가입니다. 기존 사이트에 추가하면 DNS와 인증서를 새로 만들 필요가 없습니다. 그 대신 그 사이트 설정 파일에 한 줄이 들어갑니다.
5. **Docker**
   - snap으로 설치한 Docker(`/snap/docker`가 있거나 `command -v docker`가 `/snap/bin/docker`)이거나 podman이면 멈춥니다. snap Docker는 `/opt` 아래 파일을 제대로 쓰지 못합니다.
   - Docker는 있는데 `docker compose version`이 실패하면 멈춥니다. 사용자에게 설치할 패키지를 확인합니다. docker-ce는 `docker-compose-plugin`, Ubuntu의 docker.io는 `docker-compose-v2`입니다. 이때 `docker.io`는 설치하지 마세요. docker-ce가 지워집니다.
   - A 경로인데 Docker 서버 버전이 28 미만이면 멈춥니다. 28 미만에서는 같은 네트워크의 다른 기기가 컨테이너 포트(hbbs)에 직접 닿을 수 있습니다. 업그레이드하면 모든 컨테이너가 재시작되므로 사용자가 정합니다. B 경로는 hbbs/hbbr을 127.0.0.1에만 묶으므로 버전과 상관없이 진행합니다.
   - Docker가 없으면 3-1에서 설치합니다. 다만 브리지(`virbr*`, `lxdbr*` 등)가 보이면 먼저 사용자에게 확인합니다. Docker가 FORWARD 정책을 DROP으로 바꿔서 VM이나 LXD 네트워크가 끊길 수 있습니다.
6. **메모리**: MemAvailable과 SwapFree를 더한 값으로 판단합니다.
   - 3GB 이상이면 그대로 진행합니다.
   - 2–3GB이면 4단계에서 `HJV_BUILD_JOBS=1`을 넣습니다.
   - 2GB 미만이면 4단계에서 `HJV_BUILD_JOBS=1`을 넣고 3-2에서 스왑을 만듭니다. 단, 스왑이 하나도 없고 `/swapfile`도 없을 때만입니다. 그 밖의 경우에는 멈춥니다.
7. **디스크**: Docker 데이터가 있는 파일 시스템(`/var`, 또는 DockerRootDir)에 6GB 이상 비어 있어야 합니다. 빌드 뒤에도 빌드 캐시가 약 4GB 남습니다. 모자라면 멈춥니다.
8. **외부 접속**: github.com, index.crates.io, static.crates.io, deb.debian.org, public.ecr.aws 중 하나라도 000이면 멈춥니다. 빌드에 모두 필요합니다. A 경로는 acme-v02.api.letsencrypt.org도 필요합니다. registry-1.docker.io는 기본 설정에서 쓰지 않습니다. public.ecr.aws만 막혀 있고 Docker Hub가 된다면, 4단계에서 `HJV_REGISTRY=docker.io/library`를 넣으면 됩니다.
9. **MTU**: 1500 미만(예: 1450)이면 5단계에서 MTU용 빌드 명령을 씁니다. A 경로라면 Caddy가 인증서를 받다가 멈출 수 있으므로 진행하지 말고 멈춥니다.
10. `/opt/hjviewer`가 이미 있으면 멈춥니다.
11. 대학이나 클라우드의 외부 방화벽(보안 그룹)은 서버 안에서 볼 수 없습니다. A 경로는 인바운드 TCP 80/443이, B 경로는 지금 웹 서버가 쓰는 포트가 열려 있어야 합니다. 사용자에게 확인합니다.

## 3. 준비 (해당할 때만)

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

## 4. 코드와 설정

```bash
DOMAIN=hjviewer.example.ac.kr     # ← 입력값
ROUTE=B                           # ← A 또는 B (2단계 2번)
[ -e /opt/hjviewer ] && { echo 'STOP: /opt/hjviewer가 이미 있음'; exit 1; }
sudo env GIT_TERMINAL_PROMPT=0 git clone https://github.com/ysfaculty/hjviewer.git /opt/hjviewer || exit 1
cd /opt/hjviewer/deploy || exit 1
sudo install -d -m 700 data       # 서버 개인키가 생길 곳: root만 읽을 수 있게
sudo cp .env.example .env
sudo sed -i "s/^HJV_DOMAIN=.*/HJV_DOMAIN=$DOMAIN/" .env
[ "$ROUTE" = B ] && echo 'COMPOSE_FILE=docker-compose.behind-proxy.yml' | sudo tee -a .env > /dev/null
# 2단계 6번에 해당하면:  echo 'HJV_BUILD_JOBS=1' | sudo tee -a .env
# 2단계 8번에 해당하면:  echo 'HJV_REGISTRY=docker.io/library' | sudo tee -a .env
grep -v '^#' .env | grep .
sudo docker compose config --services   # A: caddy hbbs hbbr, B: hbbs hbbr
sudo git -C /opt/hjviewer log -1 --format='%h %s'
```

- 프로젝트 이름은 compose 파일에 `hjviewer`로 고정돼 있어서, 이 서버의 다른 compose 프로젝트와 섞이지 않습니다.
- B 경로는 `.env`의 `COMPOSE_FILE` 덕분에 이후의 모든 `docker compose` 명령이 `docker-compose.behind-proxy.yml`을 씁니다. `-f`를 붙이지 않습니다. 이 파일은 hbbs/hbbr을 호스트 네트워크의 **127.0.0.1에만** 묶습니다.

## 5. 서버 이미지 빌드 (백그라운드)

빌드는 2 vCPU에서 10–20분, 1 vCPU에서는 그보다 오래 걸립니다. 포그라운드로 기다리면 에이전트 도구의 시간 제한에 걸리고, 빌드가 취소되면 처음부터 다시 컴파일해야 합니다. 그래서 세션과 분리해 실행합니다.

```bash
sudo setsid -f sh -c 'cd /opt/hjviewer/deploy && docker compose build > /var/tmp/hjv-build.log 2>&1; echo "exit=$?" >> /var/tmp/hjv-build.log' < /dev/null > /dev/null 2>&1
sleep 10; sudo tail -n 3 /var/tmp/hjv-build.log
```

2단계 9번(MTU 1500 미만)에 해당하면 위 첫 줄 대신 아래를 실행합니다. 빌드가 호스트 네트워크를 쓰게 해서 MTU 문제를 피합니다. 4단계에서 `HJV_BUILD_JOBS`나 `HJV_REGISTRY`를 넣었다면 `--build-arg` 값도 그에 맞춥니다.

```bash
sudo setsid -f sh -c 'cd /opt/hjviewer && docker build --network host --build-arg REGISTRY=public.ecr.aws/docker/library --build-arg BUILD_JOBS=2 -f deploy/Dockerfile -t hjviewer-server:local . > /var/tmp/hjv-build.log 2>&1; echo "exit=$?" >> /var/tmp/hjv-build.log' < /dev/null > /dev/null 2>&1
```

몇 분 간격으로 아래를 실행해 확인합니다. 명령은 바로 끝납니다.

```bash
sudo tail -n 3 /var/tmp/hjv-build.log
```

- 마지막 줄이 `exit=0`이면 6단계로 갑니다.
- `exit=` 뒤가 0이 아니면 `sudo tail -n 40 /var/tmp/hjv-build.log`의 출력을 붙여 보고하고 멈춥니다. 다시 시도하지 않습니다.
- 로그가 30분 넘게 그대로면 멈춥니다.
- `buildx`, `Bake` 관련 경고는 정상입니다. 레지스트리에 로그인하거나 compose 파일을 고치지 마세요.

## 6-A. Caddy 경로: 기동과 인증서

```bash
cd /opt/hjviewer/deploy || exit 1
sudo docker compose up -d && sudo docker compose ps
timeout 180 sh -c 'until sudo docker compose logs caddy 2>&1 | grep -qE "certificate obtained successfully|could not get certificate"; do sleep 5; done'
sudo docker compose logs caddy 2>&1 | grep -E 'certificate obtained successfully|could not get certificate|challenge failed' | tail -3
```

- `certificate obtained successfully`가 나오면 7단계로 갑니다.
- 실패 줄이 나오거나 3분 동안 아무것도 나오지 않으면 멈춥니다. Caddy를 재시작하지 마세요. Caddy는 스스로 재시도하는데, 재시작을 반복하면 Let's Encrypt 실패 한도(시간당 5회)에 걸립니다. 사용자에게 외부 방화벽의 인바운드 80/443을 확인해 달라고 합니다.
- A 경로의 80/443은 Docker가 ufw와 상관없이 엽니다. ufw는 건드리지 않습니다. 같은 이유로 compose 파일에 포트를 추가하지 마세요. ufw로 막을 수 없습니다.

## 6-B. 기존 웹 서버 경로: 기동, 인증서, 사이트 설정

**기동**: hbbs/hbbr이 127.0.0.1에서만 listen하는지 확인합니다.

```bash
cd /opt/hjviewer/deploy || exit 1
sudo docker compose up -d && sleep 3 && sudo docker compose ps
sudo ss -ltnup | grep -E ':2111[5-9]\b'   # 127.0.0.1:21115-21119 (UDP 21116 포함)만 보여야 함
```

그다음은 둘 중 하나입니다.

- **(가) 새 사이트**: DOMAIN이 HJViewer 전용입니다. 인증서를 마련하고, 새 사이트 파일을 추가합니다.
- **(나) 기존 사이트에 추가**: DOMAIN을 이 서버의 기존 웹사이트와 함께 씁니다. 인증서는 그 사이트 것을 그대로 쓰고, 그 사이트 설정에 include 한 줄을 넣습니다. 아래 "(나) 기존 사이트에 추가"로 바로 갑니다.

**(가) 인증서**: DOMAIN용 인증서를 다음 순서로 찾거나 받습니다.

1. **이미 DOMAIN을 포함하는 인증서가 있으면** 그것을 씁니다(1단계 certbot 출력, 또는 다른 사이트가 쓰는 와일드카드 인증서). 파일이 DOMAIN을 포함하는지는 `sudo openssl x509 -noout -ext subjectAltName -enddate -in <파일>`로 확인합니다.
2. **certbot이 있고 `/etc/letsencrypt/accounts`가 있으면**(이미 Let's Encrypt를 쓰는 서버) 바로 받습니다.
   ```bash
   DOMAIN=$(sed -n 's/^HJV_DOMAIN=//p' /opt/hjviewer/deploy/.env); [ -n "$DOMAIN" ] || exit 1
   sudo certbot certonly --nginx -d "$DOMAIN" --non-interactive --deploy-hook 'systemctl reload nginx'
   ```
3. **둘 다 아니면**, 사용자에게 이메일을 받고 [Let's Encrypt 약관](https://letsencrypt.org/repository/) 동의를 확인한 뒤 실행합니다.
   ```bash
   DOMAIN=$(sed -n 's/^HJV_DOMAIN=//p' /opt/hjviewer/deploy/.env); [ -n "$DOMAIN" ] || exit 1
   EMAIL=admin@example.ac.kr          # ← 사용자가 알려 준 이메일
   sudo env DEBIAN_FRONTEND=noninteractive NEEDRESTART_SUSPEND=1 apt-get -o DPkg::Lock::Timeout=600 install -y --no-upgrade certbot python3-certbot-nginx
   sudo certbot certonly --nginx -d "$DOMAIN" --non-interactive --agree-tos -m "$EMAIL" --deploy-hook 'systemctl reload nginx'
   ```
4. 학교가 인증서를 발급하는 경우처럼 위에 해당하지 않으면 멈춥니다.

Apache는 `python3-certbot-apache`, `--apache`, `--deploy-hook 'systemctl reload apache2'`로 바꿉니다. certbot이 실패하면 다시 시도하지 말고 멈춥니다. `--deploy-hook`은 인증서를 갱신할 때마다 웹 서버를 reload해서 새 인증서를 읽게 합니다.

**(가) 사이트 설정 (nginx)**: 저장소의 [deploy/nginx-hjviewer.conf](../deploy/nginx-hjviewer.conf)를 새 사이트로 추가합니다. 이 파일은 HJViewer 경로(`/ws/id`, `/ws/relay`, `/api/`)를 담은 [deploy/nginx-hjviewer-locations.conf](../deploy/nginx-hjviewer-locations.conf)를 include합니다.

```bash
DOMAIN=$(sed -n 's/^HJV_DOMAIN=//p' /opt/hjviewer/deploy/.env); [ -n "$DOMAIN" ] || exit 1
[ -e /etc/nginx/sites-available/hjviewer ] && { echo 'STOP: 이미 있음'; exit 1; }
[ -e /etc/nginx/snippets/hjviewer-locations.conf ] && { echo 'STOP: 이미 있음'; exit 1; }
sudo nginx -t 2>/dev/null || { echo 'STOP: 기존 설정이 이미 실패'; exit 1; }
sudo install -D -m 644 /opt/hjviewer/deploy/nginx-hjviewer-locations.conf /etc/nginx/snippets/hjviewer-locations.conf
sed "s/DOMAIN/$DOMAIN/g" /opt/hjviewer/deploy/nginx-hjviewer.conf | sudo tee /etc/nginx/sites-available/hjviewer > /dev/null
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

**(가) 사이트 설정 (Apache)**: [deploy/apache-hjviewer.conf](../deploy/apache-hjviewer.conf)를 같은 방법으로 추가합니다.

```bash
DOMAIN=$(sed -n 's/^HJV_DOMAIN=//p' /opt/hjviewer/deploy/.env); [ -n "$DOMAIN" ] || exit 1
[ -e /etc/apache2/sites-available/hjviewer.conf ] && { echo 'STOP: 이미 있음'; exit 1; }
sudo apache2ctl configtest 2>/dev/null || { echo 'STOP: 기존 설정이 이미 실패'; exit 1; }
sed "s/DOMAIN/$DOMAIN/g" /opt/hjviewer/deploy/apache-hjviewer.conf | sudo tee /etc/apache2/sites-available/hjviewer.conf > /dev/null
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
   DOMAIN=$(sed -n 's/^HJV_DOMAIN=//p' /opt/hjviewer/deploy/.env); [ -n "$DOMAIN" ] || exit 1
   sudo nginx -T 2>/dev/null | grep -nE '^# configuration file|server_name|location[^{]*/(ws|api)\b'
   for p in /ws/id /api/heartbeat; do echo "$p $(curl -s -o /dev/null -w '%{http_code} %{redirect_url}' --max-time 10 --resolve "$DOMAIN:443:127.0.0.1" "https://$DOMAIN$p")"; done
   ```
   - DOMAIN 사이트의 server 블록 안에 `/ws`나 `/api`로 시작하는 location이 있으면 멈춥니다.
   - 두 경로가 404이거나 그 사이트의 로그인 페이지 등으로 넘어가면(302) 사이트가 쓰지 않는 것이니 진행합니다. 200이면 사이트가 그 경로를 쓰는 것이니 멈춥니다. WordPress는 REST API를 `/wp-json/`에 두므로 보통 `/api/`를 쓰지 않습니다.
2. `# configuration file` 줄로 DOMAIN의 443 server 블록이 있는 파일을 찾아 백업하고, HJViewer 경로 파일을 설치합니다.
   ```bash
   F=/etc/nginx/sites-available/example        # ← 1번에서 찾은 파일
   sudo install -d -m 700 /root/hjv-backup && sudo cp -a "$F" /root/hjv-backup/
   [ -e /etc/nginx/snippets/hjviewer-locations.conf ] && { echo 'STOP: 이미 있음'; exit 1; }
   sudo install -D -m 644 /opt/hjviewer/deploy/nginx-hjviewer-locations.conf /etc/nginx/snippets/hjviewer-locations.conf
   ```
3. 그 파일을 편집해, `listen ... 443 ssl`과 `server_name ... DOMAIN`이 있는 server 블록 안의 `server_name` 줄 바로 아래에 다음 한 줄만 넣습니다. 같은 도메인의 443 server 블록이 둘 이상이면 각각 넣습니다. 80 포트 블록에는 넣지 않습니다. nginx-hjviewer.conf의 `location /`나 80 포트 블록은 **넣지 마세요.** 기존 사이트를 덮어씁니다.
   ```nginx
       include snippets/hjviewer-locations.conf;
   ```
4. 검사하고 reload합니다. 실패하면 백업으로 되돌리고 멈춥니다.
   ```bash
   F=/etc/nginx/sites-available/example        # ← 2번과 같은 파일
   sudo diff -u "/root/hjv-backup/$(basename "$F")" "$F"   # include 한 줄만 보여야 함
   out=$(sudo nginx -t 2>&1); echo "$out"
   if echo "$out" | grep -qE 'emerg|test failed'; then sudo cp -a "/root/hjv-backup/$(basename "$F")" "$F"; echo 'STOP: 설정 오류, 되돌림'; exit 1; fi
   sudo systemctl reload nginx
   ```
- 그 사이트의 listen에 HTTP/2(`http2`)가 켜져 있어도 됩니다. 앱은 HTTP/1.1로 협상하므로 WebSocket이 열리고, 브라우저는 그대로 HTTP/2를 씁니다.
- 그 server 블록이나 http 블록에 `limit_conn`이 있으면 보고에 적습니다. PC마다 상시 연결을 하나씩 유지하므로, 같은 주소 뒤의 PC가 많으면 제한에 걸릴 수 있습니다.

**(나) 기존 사이트에 추가 (Apache)**: 그 사이트의 443 VirtualHost 안, 다른 `ProxyPass`(특히 `ProxyPass /`)보다 위에 apache-hjviewer.conf의 `ProxyPass` 두 줄과 `<Location "/ws/">` 블록만 넣습니다. `/api/`를 그 사이트가 쓰지 않으면(위 1번과 같은 확인) `Redirect 404 /api/`도 넣습니다. apache-hjviewer.conf의 `Redirect 404 /`와 `ErrorDocument` 줄은 **넣지 마세요.** 기존 사이트 전체가 404가 됩니다. 넣기 전에 파일을 백업하고, `apache2ctl configtest`가 실패하면 백업으로 되돌리고 멈춥니다.

## 7. 확인

```bash
DOMAIN=$(sed -n 's/^HJV_DOMAIN=//p' /opt/hjviewer/deploy/.env); cd /opt/hjviewer/deploy || exit 1
# WebSocket: 이 서버의 웹 서버를 거쳐 hbbs/hbbr까지. 각각 5초 뒤 101이 나오면 정상(curl 종료 코드 28도 정상)
for p in /ws/id /ws/relay; do echo "$p $(curl -s -o /dev/null -w '%{http_code}' --max-time 5 --http1.1 --resolve "$DOMAIN:443:127.0.0.1" -H 'Connection: Upgrade' -H 'Upgrade: websocket' -H 'Sec-WebSocket-Version: 13' -H 'Sec-WebSocket-Key: SGVsbG9Xb3JsZDEyMzQ1Ng==' "https://$DOMAIN$p")"; done
echo "/api/ $(curl -s -o /dev/null -w '%{http_code}' --max-time 5 --resolve "$DOMAIN:443:127.0.0.1" -X POST "https://$DOMAIN/api/heartbeat")"   # 404
echo | openssl s_client -connect 127.0.0.1:443 -servername "$DOMAIN" 2>/dev/null | openssl x509 -noout -subject -issuer -enddate
# 키: 세 값이 같아야 함
echo "pub  $(sudo cat data/id_ed25519.pub)"
for s in hbbs hbbr; do echo "$s $(sudo docker compose logs --no-log-prefix $s | grep -o 'Key: [A-Za-z0-9+/=]*' | tail -1 | cut -d' ' -f2)"; done
# 개인키 권한: 700 root data, 600 root data/id_ed25519
sudo chmod 600 data/id_ed25519 && sudo stat -c '%a %U %n' data data/id_ed25519
```

- `--resolve`는 요청을 이 서버의 127.0.0.1:443으로 보냅니다. 학내 NAT가 자기 공인 주소로 되돌아오는 연결(헤어핀)을 막아도 확인할 수 있습니다. 웹 서버가 특정 IP에서만 listen해서 000이 나오면, `127.0.0.1`을 그 IP로 바꿔 다시 실행합니다.
- `/ws/*`가 101이 아니면 멈춥니다. 000은 웹 서버나 TLS 문제, 404는 사이트 설정이 선택되지 않은 것, 502는 hbbs/hbbr이 떠 있지 않은 것입니다. (나) 기존 사이트에 추가한 경우, 302(로그인 페이지 등)는 그 사이트가 아직 이 경로를 받는 것입니다. include가 다른 server 블록에 들어갔거나, server 수준의 `return`·`rewrite`가 먼저 실행된 것입니다. 401은 사이트 전체에 걸린 인증(`auth_basic` 등)입니다.
- 세 키 값이 다르면 `sudo docker compose restart hbbs hbbr`로 둘 다 파일의 키를 다시 읽게 하고 다시 확인합니다. 그래도 다르면 멈춥니다. 처음 기동할 때 hbbs와 hbbr이 동시에 키를 만들면 드물게 이런 일이 생깁니다.
- 바깥 네트워크에서 하는 확인은 서버 안에서 할 수 없습니다. 보고 뒤에 사용자가 합니다(아래 "보고 뒤 사용자가 할 일").

## 8. 마무리

- **개인키 백업은 사용자가 직접 합니다.** 에이전트는 키를 복사하거나 출력하지 않고, 보고에 "개인키 백업 필요"라고만 적습니다. 방법은 아래 "보고 뒤 사용자가 할 일"에 있습니다.
- **중계 대역폭 상한(선택)**: `.env`에 `HJV_TOTAL_BANDWIDTH=100`(Mb/s, 전체)과 `HJV_SINGLE_BANDWIDTH=20`(Mb/s, 세션당)을 넣고 `sudo docker compose up -d`를 실행합니다. 적용됐는지는 `sudo docker compose logs hbbr | grep BANDWIDTH`로 확인합니다. compose 파일은 고치지 마세요. 그러면 `git pull`이 실패합니다.
- **빌드 캐시**: 약 4GB가 남습니다. 지워야 하면 먼저 사용자에게 묻습니다. `docker builder prune`은 다른 프로젝트의 캐시도 지웁니다.
- **업데이트**(나중에): `sudo git -C /opt/hjviewer pull --ff-only`를 실행하고, 5단계처럼 백그라운드로 빌드한 뒤 `exit=0`을 확인합니다. 그다음 `cd /opt/hjviewer/deploy && sudo docker compose up -d`를 실행합니다. B 경로도 `.env`의 `COMPOSE_FILE` 덕분에 `-f`가 필요 없습니다. nginx라면 HJViewer 경로 파일이 바뀌었는지 `sudo diff /opt/hjviewer/deploy/nginx-hjviewer-locations.conf /etc/nginx/snippets/hjviewer-locations.conf`로 보고, 바뀌었으면 다시 설치(`sudo install -m 644 ...`)한 뒤 `sudo nginx -t && sudo systemctl reload nginx`를 실행합니다.

## 보고

아래 형식으로 알려 주세요. 앱 빌드에 앞의 두 값이 필요합니다.

```
HJV_SERVER=<DOMAIN>
HJV_SERVER_KEY=<sudo cat /opt/hjviewer/deploy/data/id_ed25519.pub 의 출력>
키 일치: .pub = hbbs = hbbr (예/아니오)
커밋: <sudo git -C /opt/hjviewer log -1 --format=%h>
경로: A(Caddy) | B(nginx|apache) (가) 새 사이트 | (나) 기존 사이트에 추가, 추가·수정한 설정 파일: <경로>
인증서: <발급자>, 만료 <날짜>, 갱신: Caddy 자동 | certbot.timer + deploy-hook | 기타(<설명>)
확인(서버 안): /ws/id <코드>, /ws/relay <코드>, /api/ <코드>
서버: <Ubuntu 버전>, <vCPU>, <메모리 전체/가용>, <Docker 디스크 여유>, Docker <서버 버전>
바꾼 것: <설치한 패키지, 추가한 설정 파일, 스왑, .env에 넣은 값>
남은 일: 개인키 백업(사용자), 바깥에서 확인(사용자), 첫 PC 등록 뒤 헤더 확인
문제: <있으면>
```

## 보고 뒤 사용자가 할 일

1. **바깥 네트워크에서 확인**: 학교 밖처럼 서버와 다른 네트워크의 Windows PC에서 PowerShell로 실행합니다. 101이 나오면 정상입니다.
   ```powershell
   curl.exe -s -o NUL -w "%{http_code}`n" --max-time 5 --http1.1 -H "Connection: Upgrade" -H "Upgrade: websocket" -H "Sec-WebSocket-Version: 13" -H "Sec-WebSocket-Key: SGVsbG9Xb3JsZDEyMzQ1Ng==" https://DOMAIN/ws/id
   ```
   단계별로 더 자세히 보려면 Rust가 있는 PC에서 `tools/hjv-probe`를 실행합니다([deploy.md](deploy.md)의 "확인").
2. **개인키 백업**: 서버에서 `sudo install -m 600 -o "$USER" /opt/hjviewer/deploy/data/id_ed25519 ~/hjviewer-id_ed25519`로 복사본을 만듭니다. 자기 PC에서 `scp <서버>:hjviewer-id_ed25519 .`로 받아 비밀번호 관리자 같은 안전한 곳에 보관합니다. 그다음 서버에서 `shred -u ~/hjviewer-id_ed25519`로 복사본을 지웁니다.
3. **첫 PC 등록 뒤 헤더 확인**: PC 앱을 하나 설치해 ID가 나오면, 서버에서 다음을 실행합니다.
   ```bash
   cd /opt/hjviewer/deploy && sudo docker compose logs --no-log-prefix hbbs | grep update_pk | tail -3
   ```
   주소가 그 PC의 공인 IP와 포트여야 합니다(포트는 0이 아님). `127.0.0.1`이나 `172.x`, `:0`이 보이면 웹 서버가 `X-Real-IP`/`X-Real-Port`를 넘기지 않는 것입니다. 이 상태에서는 같은 NAT 뒤의 PC들이 서로의 응답을 받게 됩니다.
