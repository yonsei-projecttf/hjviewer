# HJViewer

모바일 앱(iOS/Android)에서 Windows PC를 원격으로 보고 제어하는 서비스입니다. **방화벽·NAT 뒤에서도 HTTPS(443)만으로 연결**되는 것이 핵심입니다.
[RustDesk](https://github.com/rustdesk/rustdesk) 클라이언트와 [rustdesk-server](https://github.com/rustdesk/rustdesk-server)를 포크해서 만듭니다.

- 설계와 동작 원리: [docs/architecture.md](docs/architecture.md)
- 서버 배포: [docs/deploy.md](docs/deploy.md)
- 앱 빌드(CI): [docs/ci.md](docs/ci.md)
- 방화벽 E2E 검증과 결과: [docs/e2e.md](docs/e2e.md)

## 빠른 시작: 실제 기기로 연결하기

1. **서버**: 도메인 하나와 리눅스 VM에서 `deploy/`를 실행합니다(80/443만 공개). → [docs/deploy.md](docs/deploy.md)
   ```bash
   cd deploy && cp .env.example .env   # HJV_DOMAIN=rd.example.com
   docker compose up -d --build && cat data/id_ed25519.pub
   ```
2. **앱 빌드**: 서버 값을 저장소 변수로 넣고 CI를 실행합니다. → [docs/ci.md](docs/ci.md)
   ```bash
   gh variable set HJV_SERVER --body rd.example.com
   gh variable set HJV_SERVER_KEY --body '<data/id_ed25519.pub 내용>'
   gh workflow run client-build.yml -f platforms=all
   ```
3. **설치**
   - **PC(Windows)**: 아티팩트 `hjviewer-…-windows-x86_64`(zip) 안의 exe를 실행합니다. 상시 접속이 필요하면 앱에서 설치(서비스 등록)를 누릅니다. 화면에 ID와 비밀번호가 표시됩니다. 서명이 없어서 SmartScreen 경고가 뜰 수 있습니다.
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
server/patches/              hbbs/hbbr 패치 (TCP/WS 등록, WebRTC 시그널링 중계, 중계 짝 맺기 경쟁 상태 수정)
client/patches/rustdesk/     앱 패치 (서버 고정, WebSocket+WebRTC 기본, 브랜딩, E2E용 헤드리스 명령)
client/patches/hbb_common/   공용 라이브러리 패치 (WebSocket 연결의 HTTP/SOCKS5 프록시 지원)
deploy/                      Docker Compose + Caddy(443, 자동 TLS) 서버 구성
tools/hjv-probe/             방화벽 진단 도구: 이 네트워크에서 서버까지 연결되는지 단계별로 확인
tools/e2e/                   방화벽 E2E: 443만 허용하는 네임스페이스 두 개에서 실제 클라이언트로 접속·화면 수신 검증
upstream/                    (git 제외) 패치가 적용된 작업 사본
```

## 개발 흐름

```bash
scripts/fetch-upstream.sh            # upstream/rustdesk-server, upstream/rustdesk 생성
cd upstream/rustdesk-server          # 수정 → hjviewer 브랜치에 커밋
cargo test --release --test tcp_signaling
cd ../.. && scripts/export-patches.sh server   # 패치 파일 갱신 후 이 저장소에 커밋
```

서버 빌드에는 Rust와 `pkg-config`, `libssl-dev`가 필요합니다. 앱(Windows/Android/iOS)은 네이티브 의존성이 많아서 GitHub Actions로 빌드합니다.

## 라이선스

RustDesk와 rustdesk-server는 AGPL-3.0입니다. 이 저장소의 패치와 배포물도 AGPL-3.0을 따르며, 앱 사용자와 서버 이용자에게 수정 소스를 제공해야 합니다. "RustDesk" 이름과 로고는 사용할 수 없습니다.
