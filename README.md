# HJViewer

모바일 앱(iOS/Android)에서 Windows PC를 원격으로 보고 제어하는 서비스입니다. **방화벽·NAT 뒤에서도 HTTPS(443)만으로 연결**되는 것이 핵심입니다.
[RustDesk](https://github.com/rustdesk/rustdesk) 클라이언트와 [rustdesk-server](https://github.com/rustdesk/rustdesk-server)를 포크해서 만듭니다.

- 설계와 동작 원리: [docs/architecture.md](docs/architecture.md)
- 서버 배포: [docs/deploy.md](docs/deploy.md)
- CI(개발 PC)와 앱 빌드: [docs/ci.md](docs/ci.md)
- 방화벽 E2E 검증과 결과: [docs/e2e.md](docs/e2e.md)
- 앱 배포(10명 이내, 스토어 없이)와 스토어 자동화에 필요한 것: [docs/distribution.md](docs/distribution.md)
- 서버에서 작업할 사람·에이전트용 배포 절차: [docs/server-runbook.md](docs/server-runbook.md)

## 현재 상태 (2026-10-11)

| 항목 | 상태 |
|---|---|
| 개발 PC CI (`scripts/ci.sh`): 서버 패치의 신호·중계 테스트, 런북대로 한 배포 | 통과. Release 패키지의 systemd 유닛과 compose 구성을 nginx(새 사이트, HTTP/2 기존 사이트), Apache, Caddy 뒤에서 시험. WebSocket, 등록, 중계, 키, 권한, 헤더 위조 차단 확인 |
| 앱 빌드: Windows x64 / Android arm64 / iOS | 1.5.0: GitHub Actions에서 세 플랫폼 빌드 성공. 앱 이름·ID·아이콘·라이선스 파일 확인. 1.5.1(마우스·터치·키보드 입력 개선, 새 아이콘, 앱 버전, Windows 자동 시작 수정, 보안 강화): 2026-10-11 비공개 빌드 저장소에서 세 플랫폼 빌드 성공. Windows 설치 시험(자동 시작 서비스, 트레이 바로가기)과 Dart 테스트 160개 통과, iOS는 TestFlight 내부 테스트에 배포. 빌드한 커밋은 태그 `client-1.5.1`. GitHub CI는 지금 일시 중지(직접 실행할 때만) |
| 방화벽 E2E: 실제 클라이언트, 서버로 가는 443만 허용 | 등록, 비밀번호 로그인, 데이터, **원격 화면** 모두 통과 (중계). UDP를 허용하면 WebRTC P2P로 통과 |
| 운영 | 운영 서버 배포 완료(런북 B 경로: systemd, 기존 웹 서버에 include). 실제 서버 값의 앱은 비공개 빌드 저장소에서 빌드하고, iOS는 TestFlight 내부 테스트로 배포 |
| 남은 일 | 1.5.1 실기기 테스트(입력, Windows 자동 시작), 그 결과에 따른 입력 미세 조정, 연세포탈 로그인·내 기기 목록·Wake-on-LAN(1.6.0) |

## 빠른 시작: 실제 기기로 연결하기

1. **서버**: 도메인 하나와 리눅스 VM에서 `deploy/`를 실행합니다(80/443만 공개). → [docs/deploy.md](docs/deploy.md). 이미 nginx나 Apache가 80/443을 쓰는 서버는 Docker 없이 Release의 hbbs/hbbr을 systemd로 띄웁니다. → [docs/server-runbook.md](docs/server-runbook.md)의 B 경로
   ```bash
   cd deploy && cp .env.example .env   # HJV_DOMAIN=rd.example.com
   sudo docker compose build && sudo docker compose up -d && sudo cat data/id_ed25519.pub
   ```
2. **앱 빌드**: 서버 값을 저장소 변수로 넣고 GitHub Actions의 client 워크플로를 실행합니다. GitHub CI는 지금 일시 중지 상태라 직접 실행할 때만 돕니다. → [docs/ci.md](docs/ci.md)
   ```bash
   gh variable set HJV_SERVER --body rd.example.com
   gh variable set HJV_SERVER_KEY --body '<data/id_ed25519.pub 내용>'
   gh workflow run client-build.yml -f platforms=all
   ```
3. **설치**
   - **PC(Windows)**: 아티팩트 `HJViewer-…-windows-x86_64`(zip) 안의 `HJViewer.exe`를 실행하면 화면에 ID와 비밀번호가 표시됩니다. 상시 접속이 필요한 PC는 `HJViewer-install.exe`로 설치합니다. 설치하면 PC를 켤 때 Windows 로그인 전부터 자동으로 실행됩니다. 사람 없이 접속하려면 영구 비밀번호를 정하세요([docs/distribution.md](docs/distribution.md)의 "Windows 피제어 PC 설치"). 서명이 없어서 SmartScreen 경고가 뜰 수 있습니다.
   - **Android**: `…-android-aarch64*.apk`를 설치합니다.
   - **iOS**: 서명 시크릿을 넣고 빌드한 IPA를 TestFlight나 기기에 설치합니다. 서명 없는 IPA는 설치할 수 없습니다.
4. **연결**: 앱에서 PC의 ID를 입력하고 비밀번호로 접속합니다.
5. **방화벽 점검**: 고객사 네트워크에서 `tools/hjv-probe`로 서버까지의 경로를 확인합니다.

## 저장소 구조

upstream 소스는 저장소에 넣지 않습니다. **고정 커밋 + 패치 파일**로 관리해서 upstream 업데이트를 따라가기 쉽게 했습니다.

```
scripts/upstream.env         고정한 upstream 커밋 (rustdesk 1.5.0, rustdesk-server 1.1.17)
scripts/fetch-upstream.sh    upstream/ 에 소스를 받고 패치 적용 (hjviewer 브랜치)
scripts/export-patches.sh    upstream/ 의 커밋을 다시 패치 파일로 내보내기
scripts/ci.sh                개발 PC의 CI: 서버 패치 테스트, 런북대로 한 배포 시험(Docker, systemd), 앱 패치 테스트 (docs/ci.md)
scripts/package-server.sh    서버 Release 패키지: systemd로 띄울 hbbs/hbbr, 유닛, 웹 서버 설정, 라이선스 고지
scripts/package-notices.sh   앱·서버 결과물에 넣는 고지 파일 (LICENSE, NOTICE, SOURCE, THIRD_PARTY_NOTICES)
scripts/set-client-version.sh 빌드 전에 client/version.env의 앱 버전을 Cargo.toml·Cargo.lock·pubspec.yaml에 적기
scripts/make-icons.sh        client/icons의 SVG 원본으로 client/overlay의 아이콘 파일을 다시 만들기
server/patches/              hbbs/hbbr 패치 (TCP/WS 등록, WebRTC 시그널링 중계, 중계 짝 맺기 경쟁 상태 수정)
client/patches/rustdesk/     앱 패치 (서버 고정, WebSocket+WebRTC 기본, 브랜딩, E2E용 헤드리스 명령, 보안 강화,
                             Windows 실행 파일 이름, 마우스·터치·키보드 입력과 한글 입력 개선)
client/patches/hbb_common/   공용 라이브러리 패치 (WebSocket 연결의 HTTP/SOCKS5 프록시 지원, TCP_NODELAY)
client/overlay/rustdesk/     패치 대신 통째로 덮어쓰는 파일 (아이콘 이미지, scripts/make-icons.sh로 생성)
client/icons/                아이콘 SVG 원본
client/version.env           앱 버전 (사용자에게 나눠 줄 새 빌드마다 올림)
deploy/                      서버 구성: Docker Compose + Caddy(443, 자동 TLS), 또는 기존 nginx/Apache 뒤의 systemd 서비스(deploy/systemd)
tools/hjv-probe/             방화벽 진단 도구: 이 네트워크에서 서버까지 연결되는지 단계별로 확인
tools/e2e/                   방화벽 E2E: 443만 허용하는 네임스페이스 두 개에서 실제 클라이언트로 접속·화면 수신 검증
tools/hjv-load/              부하 측정 도구: 상시 등록 PC와 중계 세션을 흉내 내어 서버 용량 확인 (docs/deploy.md 용량 산정)
upstream/                    (git 제외) 패치가 적용된 작업 사본
```

## 개발 흐름

```bash
scripts/fetch-upstream.sh            # upstream/rustdesk-server, upstream/rustdesk 생성
cd upstream/rustdesk-server          # 수정 → hjviewer 브랜치에 커밋
cargo test --release --test tcp_signaling
cd ../.. && scripts/export-patches.sh server   # 패치 파일 갱신 후 이 저장소에 커밋
scripts/ci.sh                        # 커밋 전에 개발 PC의 CI
scripts/ci.sh client                 # 앱 패치(client/patches)를 바꿨다면 이것도
```

서버 빌드에는 Rust와 `pkg-config`, `libssl-dev`가 필요하고, `scripts/ci.sh`의 배포 시험에는 운영 서버와 같은 Docker가 필요합니다([docs/ci.md](docs/ci.md)). 앱(Windows/Android/iOS)은 네이티브 의존성이 많아서 GitHub Actions 워크플로로 빌드합니다. 지금은 GitHub CI를 일시 중지해서 직접 실행할 때만 돕니다.

## License / 라이선스

HJViewer is a modified version of [RustDesk](https://github.com/rustdesk/rustdesk) and [rustdesk-server](https://github.com/rustdesk/rustdesk-server) (Copyright Purslane Tech Pte. Ltd., open-trade and the RustDesk contributors). It is licensed under the GNU Affero General Public License v3.0 only ([LICENSE](LICENSE)), as RustDesk is, and comes with no warranty. [NOTICE](NOTICE) lists the upstream versions and what HJViewer changes; every downloadable build ships its license, source location and third-party notices (TestFlight builds carry only the in-app source link). HJViewer is not affiliated with or endorsed by RustDesk.

RustDesk가 AGPL-3.0-only이므로 이 저장소도 AGPL-3.0-only입니다. 앱이나 서버를 쓰는 사람에게 수정한 소스를 제공해야 하고, 이 공개 저장소가 그 소스입니다. 사람들에게 나눠 주는 빌드는 반드시 이 저장소에 올라간 커밋으로 만들고, 결과물에 들어 있는 LICENSE·NOTICE·SOURCE·THIRD_PARTY_NOTICES 파일도 함께 전달하세요. 나눠 준 빌드의 커밋에는 `client-<앱 버전>` 태그를 붙입니다. TestFlight 빌드에는 이 파일들이 없고 앱 정보 화면의 소스 링크만 있으므로, 내부 테스터가 아닌 사람에게 나눠 주기 전에 고지를 따로 전달하세요. 앱 이름과 아이콘은 HJViewer 것이고, 출처 표시로 "Powered by RustDesk"를 남겨 두었습니다. Windows 실행 파일은 `HJViewer.exe`입니다. 모바일의 `rustdesk://` URL 스킴 같은 일부 내부 이름은 아직 rustdesk입니다. 개인정보 처리방침: [docs/privacy.md](docs/privacy.md)
