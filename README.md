# HJViewer

모바일 앱(iOS/Android)에서 Windows PC를 원격으로 보고 제어하는 서비스입니다. **방화벽·NAT 뒤에서도 HTTPS(443)만으로 연결**되는 것이 핵심입니다.
[RustDesk](https://github.com/rustdesk/rustdesk) 클라이언트와 [rustdesk-server](https://github.com/rustdesk/rustdesk-server)를 포크해서 만듭니다.

- 설계와 동작 원리: [docs/architecture.md](docs/architecture.md)
- 서버 배포: [docs/deploy.md](docs/deploy.md)
- 앱 빌드(CI): [docs/ci.md](docs/ci.md)
- 방화벽 E2E 검증과 결과: [docs/e2e.md](docs/e2e.md)

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
