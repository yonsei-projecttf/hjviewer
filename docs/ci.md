# 앱 빌드 (GitHub Actions)

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
- **태그**: `v`로 시작하는 태그를 푸시하면 전체를 빌드합니다. 예: `git tag v0.1.0 && git push origin v0.1.0`
- **결과물**: 실행 페이지 아래의 Artifacts에서 받거나 `gh run download <run-id>`를 씁니다. 보관 기간은 7일(워크플로의 `ARTIFACT_RETENTION_DAYS`)이므로 필요한 파일은 바로 받아 두세요. 이름의 `<이름>`은 태그 빌드면 태그(`v0.1.0`), 수동 실행이면 `1.5.0-<커밋 7자리>`입니다.

## 잡 구성

| 잡 | 러너 | 하는 일 | 결과물 (artifact) |
|---|---|---|---|
| Bridge codegen | ubuntu-22.04 | 서버 변수를 확인하고 flutter_rust_bridge 코드를 생성합니다. 패치가 `src/flutter_ffi.rs`를 바꾸므로 매번 생성합니다. | 내부용 `bridge-artifact` (1일 보관) |
| Windows x64 | windows-2022 | vcpkg 의존성 → `build.py --flutter --hwcodec --vram`(Rust + Flutter) → 가상 디스플레이 드라이버, 프린터 드라이버, 프라이버시 모드용 `WindowInjection.dll` 포함 → 자동 압축 해제 exe | `hjviewer-<이름>-windows-x86_64` (exe, 서명 없음) |
| Android arm64 | ubuntu-24.04 | vcpkg(NDK) → `cargo ndk`로 `librustdesk.so` → `flutter build apk --split-per-abi` → 키스토어가 있으면 릴리스 서명 | `…-android-aarch64` 또는 `…-android-aarch64-debugsigned` |
| iOS | macos-latest | vcpkg(arm64-ios) → Rust 정적 라이브러리 → `flutter build ipa` → 인증서가 있으면 서명된 IPA | `…-ios` 또는 `…-ios-unsigned` |

Bridge codegen이 끝나면 나머지 잡이 병렬로 실행됩니다.

Windows exe 하나로 피제어 PC와 제어 측을 모두 씁니다. 실행하면 압축이 풀리고 앱이 뜹니다. 피제어 PC에서는 앱의 설치 버튼으로 서비스로 설치합니다. 서명이 없으므로 SmartScreen 경고나 백신 오탐이 나올 수 있습니다.

### upstream과 다른 점

- **vcpkg 캐시**: upstream은 `VCPKG_BINARY_SOURCES=clear;x-gha,readwrite`를 씁니다. 그런데 vcpkg가 2025년에 x-gha 제공자를 없앴고 지금은 경고만 출력하므로, upstream은 매번 ffmpeg 등 모든 포트를 새로 빌드합니다. 이 워크플로는 vcpkg의 `files` 제공자와 `actions/cache`로 빌드된 포트를 저장하고, 그 실행에서 설치한 패키지만 남깁니다.
- **서명**: upstream의 비공개 서명 서비스 대신 아래 시크릿으로 선택적으로 서명합니다. Windows는 서명하지 않습니다.
- **제외한 것**:
  - macOS·Linux·웹·Sciter 빌드
  - Windows arm64, Android armv7/x86_64/universal APK
  - MSI, SBOM, GitHub Release 업로드
- **Windows 보조 DLL**: `WindowInjection.dll`(RustDeskTempTopMostWindow)은 별도 잡 대신 Windows 잡 안에서 빌드합니다.
- **시간 제한**: 잡마다 `timeout-minutes`를 걸어, 멈춘 잡이 계속 비용을 쓰지 않게 했습니다.

## 저장소 변수 (필수)

Settings → Secrets and variables → Actions의 **Variables** 탭에 만듭니다. 비밀 값이 아니라서 변수로 둡니다.

| 이름 | 값 | 예 |
|---|---|---|
| `HJV_SERVER` | 서버 도메인 | `rd.example.com` |
| `HJV_SERVER_KEY` | hbbs 공개키(base64 ed25519) | 서버의 `deploy/data/id_ed25519.pub` 내용 ([deploy.md](deploy.md)) |

```bash
gh variable set HJV_SERVER --body rd.example.com
gh variable set HJV_SERVER_KEY --body "$(ssh <서버> cat hjviewer/deploy/data/id_ed25519.pub)"
```

두 값은 빌드할 때 Rust 코드에 박힙니다. 그래서 값을 바꾸면 앱을 다시 빌드해 배포해야 합니다. 값이 비어 있으면 앱이 순정 RustDesk처럼 공개 서버를 씁니다. 이를 막기 위해 태그 빌드는 실패하고, 수동 실행은 경고만 남깁니다.

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

준비 순서는 다음과 같습니다.

1. **App ID**: Certificates, Identifiers & Profiles → Identifiers에서 explicit App ID를 만듭니다. 패치의 번들 ID는 `com.hjviewer.app`입니다. 다른 ID를 써도 워크플로는 프로파일의 번들 ID를 따릅니다. 와일드카드 App ID는 쓸 수 없습니다.
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
- **App Store 프로파일로 만든 IPA**: Transporter 앱으로 App Store Connect에 올린 뒤 TestFlight로 설치합니다.
- **서명 없는 IPA(`-ios-unsigned`)**: 그대로는 설치할 수 없습니다. Sideloadly나 AltStore 같은 도구로 재서명해야 합니다. 무료 Apple ID로 서명하면 7일 뒤 만료됩니다.

## 소요 시간

아래 값은 참고용입니다. upstream 실측은 2026-10-09 nightly(성공)의 잡별 시간이며, 공개 저장소에서 Rust 캐시만 있고 vcpkg 캐시는 없는 상태입니다. 이 저장소의 값은 비공개 저장소 러너 사양을 반영한 추정치입니다.

| 잡 | upstream 실측 | 첫 실행 (캐시 없음) | 캐시가 찬 뒤 |
|---|---|---|---|
| Bridge codegen | 7분 | 약 15분 | 약 7–10분 |
| Windows x64 | 52분 (vcpkg 16, 빌드 22, exe 4, 서명·MSI 5) | 약 1.5–2시간 | 약 45–60분 |
| Android arm64 | 31분 (vcpkg 9, Rust 11, Flutter 7) | 약 60–75분 | 약 30–40분 |
| iOS | 22분 (vcpkg 8, Rust 8, Flutter 4) | 약 30–40분 | 약 15–20분 |

- **러너 사양 차이**: 비공개 저장소의 Linux·Windows 러너는 2코어/8GB로, 공개 저장소(4코어/16GB)의 절반입니다. 그래서 컴파일이 upstream보다 오래 걸립니다. macOS는 공개·비공개 모두 3코어 M1입니다.
- **전체 시간**: 플랫폼 잡이 병렬로 돌기 때문에 가장 긴 Windows 잡이 결정합니다.

## 비용 (비공개 저장소)

GitHub 공식 문서(2026-10 확인) 기준입니다.

- **공개 저장소**: 표준 러너 사용이 무료입니다.
- **비공개 저장소**: 매달 포함 시간을 먼저 쓰고, 넘으면 분당 요금을 냅니다.
  - **포함 시간**: Free 2,000분, Pro 3,000분, Free(조직) 2,000분, Team 3,000분
  - **분당 요금**: Linux 2코어 $0.006, Windows 2코어 $0.010, macOS $0.062
  - **올림**: 잡마다 사용 시간을 분 단위로 올림합니다.
  - **저장 공간**: 아티팩트는 Free 500MB, Pro 1GB, Team 2GB입니다. 캐시는 저장소당 10GB입니다.
- **OS별 배수**: 현재 공식 문서에는 Windows·macOS 사용 시간이 포함 시간에서 몇 배로 차감되는지 나와 있지 않습니다. 실제 차감은 Settings → Billing and licensing의 사용량 화면에서 확인해야 합니다. 분당 요금으로 비교하면 macOS가 Linux의 약 10배, Windows가 약 1.7배입니다.

예를 들어 캐시가 찬 상태의 전체 빌드 1회를 분당 요금으로 환산하면 다음과 같습니다(포함 시간을 넘긴 경우).

- Linux 약 45분: $0.27
- Windows 약 50분: $0.50
- macOS 약 18분: $1.12

합계는 약 $1.9입니다. iOS가 가장 비싸므로 필요할 때만 `ios`를 고르세요.

**저장소 공개도 고려하세요.**

- RustDesk는 AGPL-3.0이라, 앱을 받는 사용자에게 수정한 소스를 제공할 의무가 이미 있습니다.
- 클라이언트 포크(이 저장소)를 공개하면 그 의무를 지키는 방법도 되고, 표준 러너도 무료가 됩니다.
- 공개해도 시크릿과 변수는 노출되지 않습니다. 다만 실행 로그와 아티팩트(서버 주소가 박힌 앱)는 누구나 볼 수 있습니다.

## 캐시

- **저장 대상**: Rust(`Swatinem/rust-cache`), vcpkg(도구와 빌드된 포트), Bridge 잡의 Flutter SDK를 GitHub 캐시에 저장합니다.
- **삭제 규칙**: 7일 동안 쓰지 않은 캐시는 지워집니다. 저장소당 10GB를 넘으면 오래된 것부터 지워집니다.
- **태그 빌드**: 다른 태그의 캐시는 쓸 수 없고, 기본 브랜치의 캐시만 씁니다. 태그를 붙이기 전에 기본 브랜치에서 수동 실행을 한 번 해 두면 빨라집니다.

## upstream 버전을 올릴 때

`scripts/upstream.env`를 바꾸면 새 커밋의 `.github/workflows/flutter-build.yml`과 `bridge.yml`을 이 워크플로와 비교하세요. 특히 다음 두 가지를 맞춰야 합니다.

- 워크플로 맨 위 `env`의 도구 버전
- 주석에 적힌 upstream 줄 번호의 단계

## 아직 확인하지 못한 것

이 워크플로는 아직 실제로 실행하지 않았습니다. 첫 실행에서 다음을 확인하세요.

- **패치 빌드**: 패치된 소스가 각 러너에서 빌드되는지 확인합니다. 모든 빌드가 `--locked`이므로, 패치가 의존성을 바꾸면 `Cargo.lock`도 패치에 포함해야 합니다.
- **iOS 서명**: 키체인, 수동 서명, IPA 내보내기 경로 전체가 처음 실행됩니다. upstream은 서명 없이만 빌드합니다.
- **macOS 이미지**: `macos-latest`는 upstream처럼 고정하지 않았습니다. 이미지가 바뀌면 Xcode 버전도 바뀝니다.
- **외부 파일**: Windows 빌드는 빌드 중에 rustdesk 쪽 GitHub 릴리스에서 파일을 받습니다. 커스텀 Flutter 엔진(`main` 릴리스), 가상 디스플레이 드라이버, 프린터 드라이버입니다. 이 파일이 바뀌거나 사라지면 결과가 달라질 수 있습니다.
