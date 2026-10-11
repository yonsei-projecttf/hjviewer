# CI와 앱 빌드

**지금은 push나 태그로 도는 GitHub Actions를 쓰지 않습니다**(2026-10-10부터 일시 중지). 검증은 개발 PC에서 `scripts/ci.sh`로 합니다. 개발 PC는 운영 서버와 같은 환경으로 맞춥니다. Ubuntu 24.04에 [server-runbook.md](server-runbook.md) 3-1단계와 같은 Docker 패키지를 설치합니다(docker.io 29.1.3, docker-compose-v2 2.40.3, docker-buildx 0.30.1).

두 워크플로(`server.yml`, `client-build.yml`)는 저장소에 남아 있습니다. 다만 push나 태그로는 실행되지 않고, Actions 화면에서 직접 실행하거나 비공개 빌드 저장소가 `client-build.yml`을 불러 쓸 때만 돕니다(아래 "앱 빌드"). 다시 켜려면 `on:`에 `push` 트리거를 되돌립니다.

## 개발 PC의 CI: `scripts/ci.sh`

```bash
scripts/ci.sh            # server + deploy (처음에는 15분 안팎, 이후에는 캐시로 더 빠름). client는 따로 실행
scripts/ci.sh server     # 서버 테스트만
scripts/ci.sh deploy     # 배포 시험만 (server를 한 번 돌려 hjv-probe·hjv-load가 있어야 함)
scripts/ci.sh client     # 앱 패치 시험: hbb_common WebSocket 테스트와 순수 Dart 테스트 (client/patches를 바꿨을 때)
```

| 단계 | 하는 일 |
|---|---|
| client | `build/ci/upstream`에 upstream rustdesk를 새로 받아 `client/patches`를 적용하고, `hbb_common`의 WebSocket 테스트(프록시, `TCP_NODELAY`)와 앱의 순수 Dart 테스트(`CLIENT_DART_TESTS`: iPhone 한글 입력과 키보드 기록)를 돌립니다. Flutter는 앱 빌드와 같은 3.24.5를 처음 한 번 받아 Flutter 릴리스 목록의 SHA-256과 맞춰 봅니다. 생성된 bridge가 필요한 제스처·화질 표시 테스트는 GitHub Actions의 Dart tests 잡에서만 돕니다. |
| server | `build/ci/upstream`에 upstream rustdesk-server를 새로 받아 `server/patches`를 적용하고, 신호·중계 통합 테스트(`tcp_signaling`, `relay_pairing`)를 돌립니다. `hjv-probe`, `hjv-load`도 빌드합니다. |
| deploy | 작업 트리 사본(`build/ci/src`, 커밋하지 않은 변경 포함)으로 런북의 배포를 Docker에서 그대로 합니다. B 경로는 hbbs/hbbr을 127.0.0.1에 띄우고, 그 앞에 `deploy/nginx-hjviewer.conf`와 `deploy/apache-hjviewer.conf`를 차례로 둡니다. nginx는 기존 사이트에 `deploy/nginx-hjviewer-locations.conf`를 include 한 줄로 넣은 경우도 함께 시험합니다. 이 기존 사이트는 HTTP/2를 켜고 모든 경로를 로그인 페이지로 보내는 WordPress 같은 사이트입니다. 마지막으로 런북 B 경로의 Release 패키지를 `scripts/package-server.sh`로 만들고, 그 안의 systemd 유닛을 띄워 같은 nginx 뒤에서 시험합니다. 개발 PC를 건드리지 않도록 런타임 유닛, 임시 사용자, 별도 디렉터리로 바꾼 이름만 다르고, 보안 옵션과 자원 상한은 배포 파일 그대로입니다. 웹 서버는 운영 서버와 같은 Ubuntu 패키지를 내려받아 풀어서 실행할 뿐, 설치하지 않습니다. 그다음 A 경로(Caddy)를 같은 이미지와 키로 띄웁니다. |

배포 시험이 매번 확인하는 것은 다음과 같습니다.

- hbbs/hbbr이 127.0.0.1에서만 listen하는지
- 런북 7단계의 WebSocket 확인(`/ws/id`, `/ws/relay`가 101), `/api/`가 404인지, 첫 화면에 소스 안내가 있는지
- `hjv-probe` 전 단계, 그리고 `hjv-load`로 PC 3대 등록과 중계 1세션
- hbbs가 PC마다 실제 포트를 받는지(`X-Real-Port` 전달), 세 키 값(`.pub`, hbbs, hbbr)이 같은지
- 개인키 권한(700/600, root), 컨테이너 권한 제거(`cap_drop: ALL`), `.env`의 `COMPOSE_FILE`과 대역폭 상한이 적용되는지
- Apache 오류 로그에 fail2ban이 인증 실패로 세는 `AH01630`이 없는지
- 기존 사이트에 추가한 경우: HTTP/2를 켠 listener에서도 WebSocket이 열리는지, 사이트의 다른 경로(`/`, `/wp-json/`, `.php`)와 HTTP/2가 그대로인지
- 클라이언트가 보낸 `X-Real-IP`, `X-Forwarded-For`, `X-Real-Port`가 nginx와 Apache에서 실제 주소로 덮어써지는지(받은 헤더를 그대로 돌려주는 서버로 확인)
- systemd 유닛: 127.0.0.1에만 listen하는지, 키 파일이 처음부터 600인지, 권한이 없는지, `hjviewer.env`의 대역폭 상한이 적용되는지, 샌드박스 때문에 생긴 오류가 없는지

시험용 스택은 compose 프로젝트 `hjv-ci`로 띄우고, 끝나면 그 프로젝트만 지웁니다. 웹 서버와 Caddy는 127.0.0.1의 18080–18082, 18443–18448, 18599 포트를 쓰므로 이 PC의 80/443은 건드리지 않습니다. 다만 B 경로는 운영과 같이 21115–21119를 쓰므로, 이 PC에서 실제 HJViewer 서버가 돌고 있으면 시험이 시작하지 않고 멈춥니다. Docker 명령은 런북처럼 sudo로 실행합니다.

## 서버 Release 만들기 (런북 B 경로용)

1. 변경을 커밋해 공개 저장소의 main에 올립니다. 패키지에 들어가는 고지가 그 커밋을 가리키므로, 공개된 커밋으로만 만듭니다. 스크립트가 확인합니다.
2. `scripts/package-server.sh <버전>`을 실행합니다(예: `2026.10.10`). 새로 받은 upstream에 `server/patches`를 적용해 빌드하고, 바이너리, systemd 유닛, 웹 서버 설정, 라이선스 고지를 `build/release/hjviewer-server-<버전>-linux-x86_64.tar.gz`로 묶습니다. `SHA256SUMS`도 함께 만듭니다.
3. `gh release create server-<버전> build/release/hjviewer-server-<버전>-linux-x86_64.tar.gz build/release/SHA256SUMS`로 올립니다. Release를 만들어도 워크플로는 돌지 않습니다.

개발 PC는 운영 서버와 같은 Ubuntu 24.04라서, 여기서 빌드한 바이너리(glibc 2.39)가 그대로 돕니다.

## 앱 빌드

앱(Windows/Android/iOS)은 개발 PC에서 빌드하지 않습니다. iOS는 macOS, Windows 앱은 Windows가 필요하기 때문입니다. 실제 서버 값으로 빌드하는 앱은 **비공개 빌드 저장소** `yonsei-projecttf/hjviewer-private`의 GitHub Actions에서 만듭니다. 2026-10-10에 사용자가 정한 방식이고, GitHub CI는 그 저장소에서만 씁니다.

- 빌드 저장소에는 코드가 없습니다. 이 저장소의 `client-build.yml`을 불러 쓰는 워크플로 파일 하나와 Secrets만 있습니다. 코드는 매번 이 공개 저장소에서 받으므로, 두 저장소의 내용을 맞출 필요가 없습니다.
- 서버 주소와 공개키, 서명 시크릿은 그 저장소의 Secrets에만 있습니다. 빌드 로그와 결과물도 그 저장소에만 남습니다. 실제 서버 값이 든 앱과 서명된 앱이 공개 아티팩트로 남지 않게 하기 위해서입니다.
- 빌드 저장소의 워크플로는 이 저장소의 **확인을 마친 커밋**에 고정해서 부릅니다(아래 "비공개 빌드 저장소"). main이 바뀌어도, 시크릿을 쓰는 빌드는 확인한 빌드 방법으로만 돕니다.

## 앱 빌드 워크플로 (GitHub Actions, 지금은 직접 실행할 때만)

`.github/workflows/client-build.yml`이 앱을 빌드합니다. 빌드 대상은 다음 세 가지입니다.

- Windows x64: 피제어 PC와 제어 측 겸용
- Android arm64 APK
- iOS: 제어 측

macOS와 Linux는 빌드하지 않습니다.

모든 잡은 같은 흐름으로 동작합니다.

1. 이 저장소를 체크아웃하고 `scripts/fetch-upstream.sh client`를 실행합니다. 고정 커밋의 RustDesk 1.5.0과 `libs/hbb_common`을 받아 `client/patches`를 적용합니다.
2. `upstream/rustdesk` 안에서 upstream CI(`.github/workflows/flutter-build.yml`, `bridge.yml`)와 같은 단계로 빌드합니다. Flutter 3.24.5, Rust 1.75, NDK r28c, vcpkg 커밋, LLVM 15.0.6 같은 도구 버전도 upstream 값을 그대로 씁니다. 각 단계가 따라 한 upstream 줄 번호는 워크플로 주석에 적혀 있습니다.

## 실행

- **수동 실행**: Actions → client → Run workflow에서 `platforms`(windows / android / ios / all)를 고릅니다. CLI로는 `gh workflow run client-build.yml -f platforms=android`입니다. 이 버튼은 워크플로 파일이 기본 브랜치에 있어야 나타납니다.
- **실제 서버용 빌드**: 비공개 빌드 저장소에서 실행합니다. 예: `gh workflow run build.yml -R yonsei-projecttf/hjviewer-private -f platforms=all`
- **TestFlight 업로드**: `testflight`를 켜면 서명된 App Store IPA를 App Store Connect에 올립니다. 예: `gh workflow run build.yml -R yonsei-projecttf/hjviewer-private -f platforms=ios -f testflight=true`. 비공개 저장소, App Store 프로파일, 아래의 App Store Connect 시크릿이 필요합니다.
- **태그**: GitHub CI를 다시 켜면 `v`로 시작하는 태그를 푸시할 때 전체를 빌드합니다(지금은 꺼 둠). 예: `git tag v0.1.0 && git push origin v0.1.0`
- **결과물**: 실행 페이지 아래의 Artifacts에서 받거나 `gh run download <run-id>`를 씁니다. 보관 기간은 7일(워크플로의 `ARTIFACT_RETENTION_DAYS`)이므로 필요한 파일은 바로 받아 두세요. 이름의 `<이름>`은 태그 빌드면 태그(`v0.1.0`), 수동 실행이면 `<앱 버전>-<커밋 7자리>`(예: `1.5.1-d2fa660`)입니다.

## 잡 구성

| 잡 | 러너 | 하는 일 | 결과물 (artifact) |
|---|---|---|---|
| Bridge codegen | ubuntu-22.04 | 서버 변수를 확인하고 flutter_rust_bridge 코드를 생성합니다. 패치가 `src/flutter_ffi.rs`를 바꾸므로 매번 생성합니다. | 내부용 `bridge-artifact` (1일 보관) |
| Dart tests | ubuntu-24.04 | 생성된 bridge로 `flutter/test`의 Dart 테스트를 모두 돌립니다. 앱 패치가 더하거나 고친 테스트(제스처, iPhone 한글 입력, 화질 표시)가 하나라도 없으면 실패합니다. Windows·Android 잡은 이 잡을 기다리지 않습니다. iOS 잡은 이 잡이 통과한 뒤에 시작하므로, 테스트가 실패한 빌드는 TestFlight에 올라가지 않습니다. | 없음 |
| Windows x64 | windows-2022 | vcpkg 의존성 → `build.py --flutter --hwcodec --vram`(Rust + Flutter) → 가상 디스플레이 드라이버, 프라이버시 모드용 `WindowInjection.dll` 포함 → 자동 압축 해제 exe (RustDesk의 프린터 드라이버는 소스가 공개되지 않아 넣지 않습니다. 원격 인쇄는 꺼져 있습니다.) | `HJViewer-<이름>-windows-x86_64` (`HJViewer.exe`와 같은 파일인 `HJViewer-install.exe`, 서명 없음) |
| Android arm64 | ubuntu-24.04 | vcpkg(NDK) → `cargo ndk`로 `librustdesk.so` → `flutter build apk --split-per-abi` → 키스토어가 있으면 릴리스 서명 | `…-android-aarch64` 또는 `…-android-aarch64-debugsigned` |
| iOS | macos-26 (Xcode 26.6) | vcpkg(arm64-ios) → Rust 정적 라이브러리 → `flutter build ipa` → 인증서가 있으면 서명된 IPA → `testflight`를 켜면 App Store Connect에 업로드 | `…-ios` 또는 `…-ios-unsigned` |

Bridge codegen이 끝나면 Dart tests, Windows, Android 잡이 병렬로 실행되고, iOS 잡은 Dart tests가 통과한 뒤에 시작합니다.

Windows exe 하나로 피제어 PC와 제어 측을 모두 씁니다. 실행하면 압축이 풀리고 앱이 뜹니다. 피제어 PC에서는 앱의 설치 버튼으로 서비스로 설치합니다. 서명이 없으므로 SmartScreen 경고나 백신 오탐이 나올 수 있습니다.

### upstream과 다른 점

- **vcpkg 캐시**: upstream은 `VCPKG_BINARY_SOURCES=clear;x-gha,readwrite`를 씁니다. 그런데 vcpkg가 2025년에 x-gha 제공자를 없앴고 지금은 경고만 출력하므로, upstream은 매번 ffmpeg 등 모든 포트를 새로 빌드합니다. 이 워크플로는 vcpkg의 `files` 제공자와 `actions/cache`로 빌드된 포트를 저장하고, 그 실행에서 설치한 패키지만 남깁니다.
- **서명**: upstream의 비공개 서명 서비스 대신 아래 시크릿으로 선택적으로 서명합니다. Windows는 서명하지 않습니다.
- **Xcode 고정**: upstream은 `macos-latest`의 기본 Xcode를 씁니다. 이 워크플로는 `macos-26`에서 Xcode 26.6(`XCODE_VERSION`)을 고르고, 26.x가 아니면 실패합니다. App Store Connect는 Xcode 26 이상으로 만든 빌드만 받습니다. 반면 Xcode 27(iOS 27 SDK)로 만들면 UIScene 수명 주기가 없는 Flutter 3.24.5 앱이 실행되지 않습니다.
- **제외한 것**:
  - macOS·Linux·웹·Sciter 빌드
  - Windows arm64, Android armv7/x86_64/universal APK
  - MSI, SBOM, GitHub Release 업로드
- **Windows 보조 DLL**: `WindowInjection.dll`(RustDeskTempTopMostWindow)은 별도 잡 대신 Windows 잡 안에서 빌드합니다.
- **Dart 테스트**: upstream CI에는 없는 잡입니다. 앱 패치가 고친 제스처와 키보드 동작을 Dart 테스트로 확인합니다. 순수 Dart 테스트는 개발 PC의 `scripts/ci.sh client`에서도 돕니다.
- **앱 버전**: 각 플랫폼 잡이 빌드 전에 `scripts/set-client-version.sh`로 `client/version.env`의 버전을 적습니다(아래 "앱 버전").
- **Windows 실행 파일 이름**: 서버 값이 있는 빌드는 앱 이름이 HJViewer입니다. 그런데 설치 프로그램은 `<앱 이름>.exe`를 서비스·시작프로그램·시작 메뉴·앱 제거에 등록하면서 파일은 원래 이름대로 복사합니다. 그래서 패키지 안의 실행 파일을 `HJViewer.exe`로 바꿔 묶습니다. 1.5.0까지는 `rustdesk.exe`라서 설치해도 자동 시작이 되지 않았습니다.
- **Windows 설치 시험**: 서버 값이 있는 빌드는 러너에서 `--silent-install`로 설치해 보고, `HJViewer.exe`, 자동·실행 중인 서비스, 시작프로그램의 트레이 바로가기를 확인합니다. 설치 전에 서버 이름을 127.0.0.1로 돌려 러너가 운영 서버에 기기를 등록하지 않게 합니다. 몇 번 통과할 때까지는 실패해도 빌드를 멈추지 않습니다(continue-on-error). 결과는 그 단계의 표시로 확인합니다.
- **시간 제한**: 잡마다 `timeout-minutes`를 걸어, 멈춘 잡이 계속 비용을 쓰지 않게 했습니다.

## 비공개 빌드 저장소

`yonsei-projecttf/hjviewer-private`에는 아래 `.github/workflows/build.yml`과 README만 있습니다. 시크릿은 아래 "서명 시크릿"과 같습니다. 서버 값 `HJV_SERVER`, `HJV_SERVER_KEY`도 시크릿으로 넣습니다.

- 시크릿은 빌드 저장소의 **저장소 시크릿**(Repository secrets)이나 조직 시크릿으로 넣습니다. Environment 시크릿은 `secrets: inherit`로 넘어가지 않습니다.
- `secrets: inherit`는 같은 조직 안의 저장소끼리만 시크릿을 넘깁니다. 그래서 두 저장소는 모두 `yonsei-projecttf`에 있어야 합니다. 서버 값이 넘어오지 않으면 Bridge codegen 잡이 실패합니다. 서버 값 없이 빌드한 앱은 순정 RustDesk처럼 공개 서버를 쓰기 때문입니다.

```yaml
name: build
on:
  workflow_dispatch:
    inputs:
      platforms: { type: choice, options: [windows, android, ios, all], default: all }
      testflight: { type: boolean, default: false }
permissions:
  contents: read
jobs:
  client:
    # 확인을 마친 공개 저장소 커밋의 40자리 전체 SHA(git rev-parse HEAD).
    # 빌드 방법(uses)과 코드(source_ref)를 같은 커밋으로 둡니다.
    uses: yonsei-projecttf/hjviewer/.github/workflows/client-build.yml@<커밋>
    with:
      platforms: ${{ inputs.platforms }}
      testflight: ${{ inputs.testflight }}
      source_ref: <커밋>
    secrets: inherit
```

- 새 버전을 빌드하려면, 공개 저장소에 올린 커밋을 확인한 뒤 `<커밋>` 두 곳을 그 커밋의 40자리 전체 SHA(`git rev-parse HEAD`)로 바꿉니다. 짧은 SHA(`e6b1cfd`)는 체크아웃이 브랜치·태그 이름으로 찾다가 실패합니다.
- 불려 온 워크플로는 빌드 저장소의 실행으로 돕니다. 그래서 시크릿, 로그, 결과물, 캐시, 실행 시간은 모두 빌드 저장소 쪽에 속합니다. iOS 서명도 실행하는 저장소가 비공개일 때만 합니다.
- 각 잡은 Bridge codegen 잡이 받은 커밋을 체크아웃합니다. 빌드 중에 브랜치가 움직여도 모든 플랫폼이 같은 커밋으로 빌드됩니다.
- 나눠 줄 빌드가 끝나면 그 커밋에 공개 저장소에서 `client-<앱 버전>` 태그를 붙입니다(예: `client-1.5.1`). 앱을 받은 사람이 자기 빌드의 소스 커밋을 찾을 수 있게 하기 위해서입니다. `v`로 시작하지 않으므로, 다시 켤 태그 빌드 트리거와도 겹치지 않습니다.

### 미러에서 빌드 전용으로 바꾸기 (한 번만)

1.5.0까지 `hjviewer-private`는 이 저장소의 미러였습니다. 미러에 딸린 `client`·`server` 워크플로도 실제 시크릿으로 실행할 수 있었습니다. 그 워크플로로 빌드하면 1.5.0 코드(옛 아이콘, 자동 시작이 안 되는 `rustdesk.exe`)가 실제 서버 값으로 빌드되고, TestFlight에도 올라갈 수 있습니다. 그래서 공개 저장소에 main을 올린 직후, 다른 실행보다 먼저 바꿉니다.

1. 미러 워크플로를 끕니다: `gh workflow disable client -R yonsei-projecttf/hjviewer-private`, 같은 방법으로 `server`도 끕니다.
2. 빌드 저장소에 일반 커밋 하나를 올립니다. 미러 파일을 모두 지우고 위 `build.yml`과 README만 남기는 커밋입니다. 강제 push는 쓰지 않습니다.
3. 개발 PC에서 `git remote remove private`로 미러 remote를 지웁니다. 이후로는 이 저장소를 빌드 저장소에 올리지 않습니다.
4. 첫 빌드를 실행하고 모든 잡을 확인합니다. Windows 설치 시험은 실패해도 잡이 성공으로 끝나므로, 그 단계의 표시까지 확인합니다.

## 앱 버전

사용자에게 나눠 주는 빌드는 **매번 이전보다 높은 버전**이어야 합니다(2026-10-10 사용자 결정). Android는 더 낮은 versionCode로 업데이트하지 못하고, App Store Connect는 같은 빌드 번호를 받지 않기 때문입니다.

| 값 | 어디에 쓰이나 | 정하는 곳 |
|---|---|---|
| 앱 버전 (`1.5.1`) | iOS CFBundleShortVersionString, Android versionName, Windows 파일·제품 버전, 앱 정보 화면, 설치 프로그램 목록 | `client/version.env`의 `HJV_VERSION`. `scripts/set-client-version.sh`가 빌드 전에 `Cargo.toml`(Rust 코어와 압축 해제 실행기), `Cargo.lock`, `pubspec.yaml`에 적습니다. |
| iOS·Android 빌드 번호 | iOS CFBundleVersion, Android versionCode | 빌드 시각(UTC `YYDDDHHMM`). 실행할 때마다 자동으로 커집니다. |
| Windows 빌드 번호 | Windows 파일 버전의 네 번째 숫자(예: `1.5.1.12`) | 워크플로 실행 번호. Windows 버전 숫자는 65535까지라 시각을 쓸 수 없습니다. 실행할 때마다 자동으로 커집니다. |

- 새 버전을 나눠 줄 때마다 `HJV_VERSION`을 올립니다. upstream RustDesk 버전보다 낮으면 스크립트가 실패합니다. 상대 기기가 이 버전을 비교해 기능을 켜기 때문입니다.
- iOS가 허용하는 형식은 숫자 세 개(`1.5.1`)뿐입니다.
- Windows에서 내려받는 파일 이름은 `HJViewer.exe`이고, 같은 파일을 `HJViewer-install.exe`로도 냅니다(더블클릭하면 바로 설치 화면). 설치하면 `C:\Program Files\HJViewer\HJViewer.exe`가 됩니다. 압축 해제 실행기는 파일 이름을 읽습니다. 이름이 `install.exe`로 끝나면 바로 설치하고, `-qs-`가 들어가면 일회용 지원 모드로 실행합니다. 그러니 이름을 바꿀 때는 이 둘을 피하세요.

## 서버 값 (필수)

비공개 빌드 저장소에서는 두 값을 **시크릿**으로 넣습니다(위 "비공개 빌드 저장소"). 시크릿은 로그에서 가려집니다. 이 저장소에서 직접 실행할 때는 Settings → Secrets and variables → Actions의 **Variables** 탭에 변수로 만들어도 됩니다. 변수는 로그에 그대로 찍힙니다.

| 이름 | 값 | 예 |
|---|---|---|
| `HJV_SERVER` | 서버 도메인 | `rd.example.com` |
| `HJV_SERVER_KEY` | hbbs 공개키(base64 ed25519) | 서버의 `deploy/data/id_ed25519.pub` 내용 ([deploy.md](deploy.md)) |

```bash
gh variable set HJV_SERVER --body rd.example.com
gh variable set HJV_SERVER_KEY --body "$(ssh <서버> cat hjviewer/deploy/data/id_ed25519.pub)"
```

두 값은 빌드할 때 Rust 코드에 박힙니다. 그래서 값을 바꾸면 앱을 다시 빌드해 배포해야 합니다. 값이 비어 있으면 앱이 순정 RustDesk처럼 공개 서버를 씁니다. 이를 막기 위해 빌드 저장소가 부른 실행과 태그 빌드는 실패하고, 이 저장소에서 직접 실행하면 경고만 남깁니다.

수동 실행에서는 `server`, `server_key` 입력으로 이번 실행에만 쓸 값을 줄 수 있습니다. 저장소 변수보다 우선합니다. 예: 테스트 서버용 빌드 `gh workflow run client-build.yml -f platforms=android -f server=rd-test.example.com -f server_key=...`

## 서명 시크릿 (선택)

Settings → Secrets and variables → Actions의 **Secrets** 탭에 만듭니다. 시크릿이 없으면 서명 없이 빌드합니다. Android는 디버그 서명으로 빌드합니다.

### Android

| 이름 | 내용 |
|---|---|
| `ANDROID_KEYSTORE_BASE64` | 키스토어(.jks)를 base64로 인코딩한 값. 이 값이 있으면 서명합니다. |
| `ANDROID_KEYSTORE_PASSWORD` | 키스토어 비밀번호 |
| `ANDROID_KEY_ALIAS` | 키 별칭 |
| `ANDROID_KEY_PASSWORD` | 키 비밀번호. 비우면 키스토어 비밀번호를 씁니다. PKCS12 키스토어는 두 비밀번호가 같습니다. |

```bash
keytool -genkeypair -v -keystore hjviewer-release.jks -storetype PKCS12 \
  -alias hjviewer -keyalg RSA -keysize 4096 -validity 10000 -dname "CN=HJViewer, C=KR"
base64 -w0 hjviewer-release.jks | gh secret set ANDROID_KEYSTORE_BASE64   # macOS: base64 -i hjviewer-release.jks | tr -d '\n' | …
gh secret set ANDROID_KEYSTORE_PASSWORD          # 프롬프트에 입력
gh secret set ANDROID_KEY_ALIAS --body hjviewer
```

- **키스토어 백업**: 키스토어를 잃어버리면 이미 설치된 앱을 업데이트할 수 없습니다. 비밀번호와 함께 안전한 곳에 백업하세요.
- **디버그 서명 APK**: 러너가 매번 새 디버그 키로 서명합니다. 그래서 이전 빌드 위에 덮어 설치할 수 없고, 앱을 지운 뒤 다시 설치해야 합니다.

### iOS

Apple Developer Program(유료) 계정이 필요합니다. `.p12`와 프로파일이 둘 다 있으면 서명합니다.

| 이름 | 내용 |
|---|---|
| `IOS_DIST_CERT_P12_BASE64` | Apple Distribution 인증서와 개인키를 함께 내보낸 .p12의 base64 |
| `IOS_DIST_CERT_PASSWORD` | .p12를 내보낼 때 정한 비밀번호 |
| `IOS_PROVISIONING_PROFILE_BASE64` | 배포용 프로비저닝 프로파일(.mobileprovision)의 base64 |
| `IOS_TEAM_ID` | 10자리 팀 ID(developer.apple.com → Membership details). 넣으면 프로파일의 팀과 같은지 확인하고, 비우면 프로파일의 팀을 씁니다. |
| `ASC_KEY_ID`, `ASC_ISSUER_ID` | TestFlight 업로드용 App Store Connect 팀 API 키의 Key ID와 Issuer ID (Users and Access → Integrations) |
| `ASC_PRIVATE_KEY` | 그 키의 `.p8` 파일 내용 (`gh secret set ASC_PRIVATE_KEY < AuthKey_XXXX.p8`) |
| `ASC_APP_ID` | App Store Connect 앱 레코드의 숫자 Apple ID (앱 → 앱 정보 → Apple ID). Xcode 26의 `altool --upload-package`에 필요합니다. |

준비 순서는 다음과 같습니다.

1. **App ID**: Certificates, Identifiers & Profiles → Identifiers에서 explicit App ID를 만듭니다. 패치의 번들 ID는 `kr.ac.yonsei.hjviewer`입니다. 다른 ID를 써도 워크플로는 프로파일의 번들 ID를 따릅니다. 와일드카드 App ID는 쓸 수 없습니다.
   - 앱의 entitlements는 Push Notifications와 Access Wi-Fi Information을 요청하므로 두 기능을 켜 두기를 권장합니다.
   - 켜지 않아도 서명은 됩니다. 워크플로가 경고를 남기고 해당 항목을 entitlements에서 뺍니다. 앱 코드는 두 기능을 쓰지 않습니다.
2. **인증서**: 다음 중 한 가지로 발급합니다.
   - Xcode → Settings → Accounts → Manage Certificates → + → Apple Distribution
   - 키체인 접근에서 CSR을 만들어 웹에서 발급

   발급한 뒤 키체인 접근에서 인증서를 펼쳐 개인키가 함께 선택된 상태로 .p12로 내보냅니다. OpenSSL 3로 .p12를 만들면 `-legacy` 옵션을 붙여야 macOS가 읽을 수 있습니다.
3. **프로파일**: Profiles → +에서 종류를 고릅니다. 내보내기 방식(ad-hoc / app-store / enterprise / development)은 워크플로가 프로파일을 보고 정합니다.
   - **Ad Hoc**: 등록한 기기에만 설치됩니다. 먼저 Devices에 아이폰 UDID를 등록하세요.
   - **App Store Connect**: TestFlight와 App Store용입니다.
4. **시크릿 등록**:

```bash
base64 -i dist.p12 | tr -d '\n' | gh secret set IOS_DIST_CERT_P12_BASE64
gh secret set IOS_DIST_CERT_PASSWORD
base64 -i HJViewer_AdHoc.mobileprovision | tr -d '\n' | gh secret set IOS_PROVISIONING_PROFILE_BASE64
gh secret set IOS_TEAM_ID --body ABCDE12345
```

워크플로가 서명하는 과정은 다음과 같습니다.

1. 인증서를 임시 키체인에 넣습니다.
2. Runner 타깃만 수동 서명으로 바꿉니다. 팀, 프로파일, 번들 ID는 프로파일 기준입니다.
3. ExportOptions.plist를 만들어 `flutter build ipa --export-options-plist`로 IPA를 만듭니다.

결과물별 설치 방법은 다음과 같습니다.

- **Ad Hoc IPA**: 기기를 Mac에 연결하고 Finder나 Apple Configurator로 IPA를 끌어다 놓습니다.
- **App Store 프로파일로 만든 IPA**: `testflight`를 켜고 실행하면 워크플로가 App Store Connect에 올립니다(Transporter 앱으로 직접 올려도 됩니다). Apple의 처리가 끝나면 TestFlight에 나타납니다. 내부 테스트 그룹에 넣은 App Store Connect 사용자가 TestFlight 앱으로 설치합니다. TestFlight 빌드는 90일 뒤 만료됩니다.
  - `altool`은 업로드가 실패해도 0으로 끝날 때가 있어서, 워크플로는 출력에서 성공 메시지를 확인합니다.
  - 2026-10-10 첫 업로드(실행 38044728062, 빌드 262831026)는 Apple 처리 후 VALID가 되었고, 내부 테스트 그룹에 자동으로 배포되었습니다. 업로드 경고는 90068 하나였습니다(2027년 4월부터 최소 iOS 15).
  - Info.plist는 upstream처럼 `ITSAppUsesNonExemptEncryption=false`입니다. 앱의 암호화(libsodium, TLS)는 표준 알고리즘이라, Apple 기준으로 문서가 필요한 경우는 프랑스 App Store 판매뿐입니다. App Store에 낼 때 프랑스를 판매 지역에서 빼지 않으려면 프랑스 암호화 신고를 하고 이 값을 바꿔야 합니다.
- **서명 없는 IPA(`-ios-unsigned`)**: 그대로는 설치할 수 없습니다. Sideloadly나 AltStore 같은 도구로 재서명해야 합니다. 무료 Apple ID로 서명하면 7일 뒤 만료됩니다.

## 소요 시간

이 저장소의 실측값과 upstream 참고값입니다. 첫 실행은 2026-10-10 비공개 저장소에서 캐시 없이, 1.5.1은 2026-10-11 빌드 전용 저장소에서 캐시가 있는 상태로 돌았습니다(실행 38106290530). upstream 값은 2026-10-09 nightly(공개 저장소, Rust 캐시만 있음)입니다.

| 잡 | 첫 실행 (캐시 없음) | 1.5.1 (캐시 있음) | upstream 참고 |
|---|---|---|---|
| Bridge codegen | 7분 | **4분** | 7분 |
| Dart tests | 없음 | **2분** | 없음 |
| Windows x64 | 72분 | **24분** | 52분 (vcpkg 16, 빌드 22, exe 4, 서명·MSI 5) |
| Android arm64 | 51분 | **18분** | 31분 (vcpkg 9, Rust 11, Flutter 7) |
| iOS | 15분 | **7분** | 22분 (vcpkg 8, Rust 8, Flutter 4) |

1.5.1 결과물 크기는 Windows exe 25MB, Android APK 29MB, App Store용 iOS IPA 44MB입니다.

- **러너 사양 차이**: 비공개 저장소의 Linux·Windows 러너는 2코어/8GB로, 공개 저장소(4코어/16GB)의 절반입니다. 그래서 컴파일이 upstream보다 오래 걸립니다. macOS는 공개·비공개 모두 3코어 M1입니다.
- **전체 시간**: 플랫폼 잡이 병렬로 돌기 때문에 가장 긴 Windows 잡이 결정합니다. 1.5.1 실행은 모두 28분 걸렸습니다.

## 비용 (비공개 저장소)

GitHub 공식 문서(2026-10 확인) 기준입니다.

- **공개 저장소**: 표준 러너 사용이 무료입니다.
- **비공개 저장소**: 매달 포함 시간을 먼저 쓰고, 넘으면 분당 요금을 냅니다.
  - **포함 시간**: Free 2,000분, Pro 3,000분, Free(조직) 2,000분, Team 3,000분
  - **분당 요금**: Linux 2코어 $0.006, Windows 2코어 $0.010, macOS $0.062
  - **올림**: 잡마다 사용 시간을 분 단위로 올림합니다.
  - **저장 공간**: 아티팩트는 Free 500MB, Pro 1GB, Team 2GB입니다. 캐시는 저장소당 10GB입니다.
- **OS별 배수**: 현재 공식 문서에는 Windows·macOS 사용 시간이 포함 시간에서 몇 배로 차감되는지 나와 있지 않습니다. 실제 차감은 Settings → Billing and licensing의 사용량 화면에서 확인해야 합니다. 분당 요금으로 비교하면 macOS가 Linux의 약 10배, Windows가 약 1.7배입니다.

예를 들어 캐시가 찬 상태의 전체 빌드 1회(1.5.1 실측, 잡마다 분 단위로 올림)를 분당 요금으로 환산하면 다음과 같습니다(포함 시간을 넘긴 경우).

- Linux 약 25분(Bridge 4, Dart 3, Android 18): $0.15
- Windows 약 24분: $0.24
- macOS 약 7분: $0.43

합계는 약 $0.8입니다. macOS가 분당 가장 비싸므로 iOS가 필요할 때만 `ios`나 `all`을 고르세요.

**저장소 공개도 고려하세요.**

- RustDesk는 AGPL-3.0이라, 앱을 받는 사용자에게 수정한 소스를 제공할 의무가 이미 있습니다.
- 클라이언트 포크(이 저장소)를 공개하면 그 의무를 지키는 방법도 되고, 표준 러너도 무료가 됩니다.
- 공개 저장소에서는 **실행 로그와 아티팩트를 누구나 볼 수 있습니다.**
  - 시크릿은 로그에서 `***`로 가려집니다. 하지만 저장소 **변수**는 각 단계의 로그에 그대로 찍힙니다. 서버 주소와 키를 로그에 남기지 않으려면 `HJV_SERVER`/`HJV_SERVER_KEY`를 시크릿으로 등록하세요(워크플로는 둘 다 받습니다).
  - 서버 주소와 키는 **앱 안에 들어갑니다**. 공개 아티팩트로 받은 앱이나 소스로 직접 빌드한 앱은 그 서버에 접속할 수 있습니다. 소수만 쓰는 서버라면 실제 서버 값으로 빌드한 앱은 공개 아티팩트로 내지 말고, 비공개 빌드 저장소에서 빌드하세요. ([deploy.md](deploy.md)의 "소규모 비공개 운영")
  - 서명된 IPA는 공개 저장소에서 아티팩트로 올리지 않습니다. 프로비저닝 프로파일에 Apple 개발자 이름, 팀 ID, Ad Hoc의 경우 기기 UDID가 들어 있기 때문입니다. 서명 단계의 로그에서도 인증서 이름과 팀을 가립니다.
- 내려받는 모든 결과물(artifact)에는 `LICENSE.txt`, `NOTICE.txt`, `SOURCE.txt`(이 빌드의 소스 위치), `THIRD_PARTY_NOTICES.txt`(Rust 크레이트, vcpkg 포트, 동봉 드라이버의 라이선스)가 함께 들어갑니다. AGPL이 요구하는 고지입니다. TestFlight로 올리는 IPA에는 이 파일이 없고, 앱 정보 화면의 소스 링크만 있습니다. 내부 테스터가 아닌 사람에게 TestFlight로 나눠 주기 전에, 고지를 앱 안에 넣거나 TestFlight의 "테스트할 내용"에 이 빌드의 소스 커밋(`client-<앱 버전>` 태그)과 고지 링크를 적어야 합니다.

## 캐시

- **저장 대상**: Rust(`Swatinem/rust-cache`), vcpkg(도구와 빌드된 포트), Bridge 잡의 Flutter SDK를 GitHub 캐시에 저장합니다.
- **삭제 규칙**: 7일 동안 쓰지 않은 캐시는 지워집니다. 저장소당 10GB를 넘으면 오래된 것부터 지워집니다.
- **태그 빌드**: 다른 태그의 캐시는 쓸 수 없고, 기본 브랜치의 캐시만 씁니다. 태그를 붙이기 전에 기본 브랜치에서 수동 실행을 한 번 해 두면 빨라집니다.

## upstream 버전을 올릴 때

`scripts/upstream.env`를 바꾸면 새 커밋의 `.github/workflows/flutter-build.yml`과 `bridge.yml`을 이 워크플로와 비교하세요. 특히 다음 두 가지를 맞춰야 합니다.

- 워크플로 맨 위 `env`의 도구 버전
- 주석에 적힌 upstream 줄 번호의 단계

## 아직 확인하지 못한 것

2026-10-10 첫 실행에서 세 플랫폼 빌드가 성공했습니다(1.5.0). 2026-10-11 빌드 전용 저장소의 첫 실행(38106290530)에서는 1.5.1의 새 빌드 경로도 모두 통과했습니다.

- `workflow_call`과 다른 저장소에서 하는 체크아웃
- 세 잡의 앱 버전 적기
- `HJViewer.exe` 이름 바꾸기와 압축 해제 실행기
- Windows 설치 시험: 자동 시작 서비스와 트레이 바로가기
- Dart 테스트 160개
- TestFlight 업로드(빌드 262840253, Apple 처리 후 VALID)

아래는 아직 실행하지 않은 경로입니다.
- **의존성 변경**: 모든 빌드가 `--locked`이므로, 패치가 의존성을 바꾸면 `Cargo.lock`도 패치에 포함해야 합니다.
- **Xcode 26.6 고정**: 러너 이미지에서 26.6이 빠지면 "Select Xcode" 단계가 실패합니다. 그때 `XCODE_VERSION`을 이미지에 있는 26.x로 바꿉니다. Apple은 2027년 4월부터 iOS 27 SDK를 요구할 예정입니다. 그 전에 Flutter를 3.38 이상으로 올리고 AppDelegate를 UIScene으로 옮겨야 합니다.
- **Flutter 플러그인**: 패치 0006이 개인정보 매니페스트 때문에 sqflite, file_picker, package_info_plus, device_info_plus를 올렸습니다. 2026-10-10에 세 플랫폼 모두 빌드되는 것까지 확인했습니다(실행 38042826319의 Windows·Android, 38044728062의 iOS). 실제 기기에서의 동작은 아직 확인하지 않았습니다.
- **외부 파일**: Windows 빌드는 빌드 중에 rustdesk 쪽 GitHub 릴리스에서 파일을 받습니다. 커스텀 Flutter 엔진(`main` 릴리스)과 가상 디스플레이 드라이버입니다. 이 파일이 바뀌거나 사라지면 결과가 달라질 수 있습니다.
