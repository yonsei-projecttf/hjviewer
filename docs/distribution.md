# 앱 배포: 10명 이내 비상업 사용 기준

2026-10-10에 Apple과 Google의 공식 문서, 그리고 이 앱의 실제 빌드 결과물을 확인해서 정리했습니다. Google Play 부분은 2026-10-12에 다시 확인했습니다.

## 결론: 스토어 심사 없이 배포하는 것을 권장

Apple의 App Review Guidelines 서문은 **가족이나 지인만 쓰는 앱이라면 App Store보다 Xcode 설치나 Ad Hoc 배포가 적합하다**고 안내합니다. 10명 이내의 비상업 사용에는 아래 경로가 가장 간단하고, 심사도 필요 없습니다.

| 플랫폼 | 권장 경로 | 심사 | 필요한 것 |
|---|---|---|---|
| Windows | 결과물 exe를 직접 전달 | 없음 | (서명 없음: SmartScreen 경고가 뜰 수 있음) |
| Android | 릴리스 서명 APK를 직접 설치(사이드로드) | 없음 | 업로드 키스토어 하나 |
| iOS | **Ad Hoc** 서명 IPA를 등록된 iPhone에 설치 | 없음 | Apple Developer Program, 사용자 iPhone UDID 등록 |
| iOS(대안) | TestFlight **내부** 테스트 | 없음 (App Store Connect 사용자 최대 100명, 빌드는 90일 뒤 만료) | Apple Developer Program, App Store Connect API 키, 앱 레코드, App Store 프로파일 ([ci.md](ci.md)의 "TestFlight 업로드") |
| Android(대안) | Google Play **내부** 테스트 | 업데이트는 없음 (게시 뒤 소급 심사는 가능, 테스터 최대 100명, Play 스토어에서 설치하고 자동 업데이트) | Play 개발자 계정(있음), Play Console의 앱과 첫 AAB 직접 업로드, 서비스 계정 ([ci.md](ci.md)의 "Google Play 업로드") |

실제 서버 값으로 만든 앱은 공개 아티팩트로 올리지 않습니다. 그래서 서명 빌드는 **비공개 빌드 저장소** `yonsei-projecttf/hjviewer-private`의 CI에서 합니다([deploy.md](deploy.md)의 "소규모 비공개 운영"). 이 워크플로는 iOS 서명과 Google Play용 AAB를 비공개 저장소에서만 만듭니다.

## 권장 경로에 필요한 정보

비공개 빌드 저장소의 **Secrets**에 넣습니다. 채팅이나 이슈로 보내지 마세요.

**공통**
- `HJV_SERVER`, `HJV_SERVER_KEY`: 서버 배포 후 받은 도메인과 공개키([server-runbook.md](server-runbook.md)의 "보고")

**Android (사이드로드)**
- `ANDROID_KEYSTORE_BASE64`, `ANDROID_KEYSTORE_PASSWORD`, `ANDROID_KEY_ALIAS`, `ANDROID_KEY_PASSWORD`: 만드는 방법은 [ci.md](ci.md)에 있습니다. 같은 키로 서명해야 앱 위에 업데이트를 설치할 수 있으므로, **키스토어를 잃어버리면 안 됩니다.**
- 사용자는 Android 설정에서 "출처를 알 수 없는 앱 설치"를 허용해야 합니다.

**Android (Google Play 내부 테스트, 대안)**
- `PLAY_SA_EMAIL`, `PLAY_SA_PRIVATE_KEY`: Google Cloud 서비스 계정 JSON 키의 두 값. 만드는 방법과 넣는 명령은 [ci.md](ci.md)의 "Google Play 업로드"에 있습니다.
- 위 Android 키스토어가 Play의 업로드 키가 됩니다. 새 시크릿은 이 두 개뿐입니다.
- 테스터의 Google 계정 이메일. 휴대폰 Play 스토어에 로그인한 계정이어야 합니다.
- 앱 서명 키를 기본값(Google 생성)으로 두면 Play 앱과 사이드로드 앱은 서명이 달라 서로 업데이트하지 못합니다. Play로 옮기는 휴대폰은 사이드로드 앱을 먼저 지웁니다.

**iOS (Ad Hoc)**
- **Apple Developer Program** 개인 멤버십: 연 99달러이고, 판매자 이름으로 법적 이름이 표시됩니다.
- **사용자 iPhone의 UDID**: Certificates, Identifiers & Profiles → Devices에 등록합니다. 기기 종류별로 멤버십 1년에 100대까지입니다.
- **App ID** `kr.ac.yonsei.hjviewer`(explicit), **Apple Distribution 인증서**(.p12와 비밀번호), 위 기기를 담은 **Ad Hoc 프로비저닝 프로파일**
- Secrets: `IOS_DIST_CERT_P12_BASE64`, `IOS_DIST_CERT_PASSWORD`, `IOS_PROVISIONING_PROFILE_BASE64`, `IOS_TEAM_ID`
- 사용자를 추가할 때마다 기기를 등록하고, 프로파일을 다시 받아 다시 빌드해야 합니다.
- 설치 방법: Finder나 Apple Configurator로 USB 설치하거나, HTTPS 서버에 `manifest.plist`를 올려 `itms-services://` 링크로 무선 설치합니다.

## Windows 피제어 PC 설치

제어받을 Windows PC는 설치해 두어야 사람이 없어도 접속할 수 있습니다. 제어만 하는 PC는 설치하지 않아도 됩니다.

1. **설치**: `HJViewer-install.exe`를 더블클릭하고 **수락하고 설치**를 누른 뒤 관리자 권한(UAC)을 허용합니다. `HJViewer.exe`를 실행해 **설치하기**를 눌러도 같습니다. 서명이 없는 exe라서 SmartScreen 경고나 백신 경고가 뜰 수 있습니다.
   - 1.5.0 빌드로 설치한 PC는 자동 시작이 고장 나 있습니다(실행 파일 이름 문제). 새 빌드로 다시 설치하면 고쳐집니다. 그 전에 HJViewer 창을 모두 닫고, 작업 관리자 "세부 정보"에 `rustdesk.exe`가 있으면 끝내세요.
2. **자동 시작**: 설치하면 HJViewer 서비스가 자동 시작으로 등록됩니다.
   - PC를 켜면 Windows 로그인 전부터 실행됩니다. 원격에서 로그인 화면이 보이고, 원격 메뉴의 **Ctrl + Alt + Del 삽입**으로 로그인할 수 있습니다.
   - 로그인하면 트레이 아이콘이 뜹니다.
   - 실수로 자동 시작이 꺼지지 않도록 "서비스 중지" 메뉴는 숨겨 두었습니다. 그만 쓰려면 앱 제거로 지웁니다.
3. **영구 비밀번호**: 화면의 일회용 비밀번호는 접속에 성공하거나 앱이 다시 시작될 때마다 바뀝니다. 그래서 사람 없이 접속하려면 영구 비밀번호가 필요합니다.
   - 설치한 PC에서 설정 → 보안 → **보안 설정 잠금 해제** → **영구 비밀번호 설정** 순서로 정합니다.
   - 길고, 다른 곳에 쓰지 않는 비밀번호를 쓰세요.
4. **절전**: 전원에 연결됐을 때 절전을 "안 함"으로 둡니다. 절전·최대 절전·꺼진 PC에는 접속할 수 없습니다. 꺼진 PC를 깨우는 기능(Wake-on-LAN)은 다음 버전에서 다룹니다.
5. **열기와 업데이트**: 설치한 PC에서는 시작 메뉴나 트레이의 **열기**로 앱을 엽니다. 새 버전은 새 `HJViewer-install.exe`로 다시 설치합니다.

로그인해야 네트워크가 열리는 망(사용자별 802.1X 인증, 웹 로그인)에서는 Windows 로그인 전에는 접속되지 않을 수 있습니다.

## 스토어로 갈 때 (나중에 필요해지면)

### 업로드 요건과 남은 차단 요소

- **iOS 개인정보 매니페스트**(ITMS-91061, ITMS-91053): 클라이언트 패치 0006(2026-10-10)으로 대응했습니다. 실제 업로드 결과로 확인해야 합니다.
  - 앱 자체 `PrivacyInfo.xcprivacy`를 넣었습니다. Rust 코어가 파일 시각 API(stat/fstat/lstat)를 쓰므로 C617.1(앱 컨테이너 안의 파일)을 신고하고, 서버에 등록하는 기기 ID를 앱 기능용 수집으로 적었습니다.
  - 매니페스트가 없던 Apple 목록 SDK는 Flutter 3.24.5에서 쓸 수 있는 첫 매니페스트 포함 버전으로 올렸습니다: sqflite 2.3.3+1, file_picker 8.0.7, package_info_plus 8.0.2, device_info_plus 10.1.2.
  - iOS에서 쓰지 않는 file_picker의 사진·음악 선택기를 빼서 DKImagePickerController, DKPhotoGallery, SDWebImage, SwiftyGif가 앱에서 빠졌습니다.
- **Xcode와 최소 iOS 버전**: 2026-04-28부터 Xcode 26과 iOS 26 SDK, 2026-09-09부터 최소 iOS 13이 필요합니다. 지금 빌드는 둘 다 만족합니다. 2027년 4월부터는 iOS 27 SDK와 최소 iOS 15가 필요합니다. iOS 27 SDK로 만든 앱은 UIScene 수명 주기가 없으면 실행되지 않으므로, 그 전에 Flutter를 3.38 이상으로 올려야 합니다.
- **Android 스토어 형식**: Play에는 업로드 키로 서명한 AAB를 올립니다. 비공개 저장소의 서명 빌드는 APK와 함께 arm64 AAB(`flutter build appbundle --target-platform android-arm64`)를 만들어 jarsigner로 서명하고 검사합니다([ci.md](ci.md)의 "Google Play 업로드"). `--target-platform`이 없으면 `librustdesk.so`가 없는 32비트 ARM·x86_64용 Flutter까지 들어가, Play가 앱이 시작되지 않는 기기에도 앱을 내줍니다.
- **Android 목표 API와 16KB 페이지**: 2026-08-31부터 새 앱과 업데이트는 API 36 이상을 목표로 해야 하고, 2027-02-01부터는 16KB 페이지를 지원하지 않는 업데이트를 출시할 수 없습니다. 지금 빌드는 둘 다 맞고, CI가 Play용 AAB마다 확인합니다.
- 빌드 번호는 이제 빌드마다 증가합니다(`YYDDDHHMM`).

### iOS: App Store Connect 자동화에 필요한 것

- 계정 소유자가 최신 약관에 동의하고, **App Store Connect API 접근**을 신청합니다.
- **팀 API 키**: Issuer ID, Key ID, `.p8`. `.p8`은 한 번만 내려받을 수 있습니다. TestFlight 내부 배포만 하면 Developer 역할로 충분하고, 심사 제출까지 하려면 App Manager 역할이 필요합니다.
- **앱 레코드**는 웹에서 한 번 만듭니다(API로는 만들 수 없음). 앱 이름(전 세계에서 고유), SKU, 언어가 필요합니다. 첫 빌드를 올리기 전까지는 번들 ID를 바꿀 수 있습니다.
- App Store **Connect** 배포용 프로비저닝 프로파일
- 외부 TestFlight나 App Store로 내려면 추가로 필요한 것:
  - 스크린샷: iPhone, 그리고 iPad 13인치(앱이 iPad를 지원하기 때문)
  - 설명, 키워드, 저작권, 카테고리
  - 지원 URL, 개인정보 처리방침 URL([privacy.md](privacy.md))
  - 연령 등급, 콘텐츠 권리, EU DSA 판매자 정보(웹에서만 입력)
  - 수출 규정: 앱의 암호화(libsodium, TLS)는 표준 알고리즘이라 프랑스에서 판매할 때만 프랑스 암호화 신고가 필요합니다. 프랑스를 판매 지역에서 빼면 문서가 필요 없고, Info.plist의 `ITSAppUsesNonExemptEncryption=false`도 그대로 맞습니다.
  - 심사용 **항상 켜진 데모 Windows PC의 ID와 비밀번호**
- 위험 요소: RustDesk 공식 앱이 이미 App Store에 있어서, 4.1/4.3(모방 앱) 조항으로 거절될 수 있습니다.

### Android: Google Play 내부 테스트

CI와 처음 한 번 할 일은 [ci.md](ci.md)의 "Google Play 업로드"에 있습니다. 여기에는 Play의 규칙과 정할 것을 적습니다.

- **계정**: Play 개발자 계정은 이미 있습니다. 개인 계정은 Play에 법적 이름, 국가, 개발자 이메일이 표시됩니다. 내부 테스트 앱은 참여한 테스터만 봅니다. 새 개인 계정은 앱을 내기 전에 실제 Android 휴대폰으로 기기 인증을 해야 합니다.
- **내부 테스트는 바로 쓸 수 있습니다.** 2023-11-13 이후 만든 개인 계정의 "테스터 12명 이상, 14일 연속" 비공개 테스트는 **프로덕션** 출시를 신청할 때만 필요합니다. 내부 테스트는 테스터 100명까지이고, 앱 설정(스토어 등록정보, 콘텐츠 등급, 타깃층, 데이터 보안)을 마치기 전에 시작할 수 있습니다. 새 빌드는 몇 분 안에 테스터에게 갑니다.
- **심사**: 내부 테스트 업데이트는 심사하지 않습니다. 다만 게시 뒤 소급 심사는 있을 수 있습니다. 앱 설정을 모두 마친 앱은 첫 내부 출시가 심사를 받고(몇 시간에서 7일), 설정을 다 마치지 않은 앱은 예외입니다. 거절된 뒤에는 다음 제출이 심사를 통과해야 내부 테스터에게도 나갑니다.
- **첫 업로드는 Play Console에서 직접** 합니다. API로는 앱을 만들 수 없고, 빌드가 하나 이상 올라간 앱만 다룰 수 있습니다. 패키지 이름은 첫 업로드로 정해지고 바꿀 수 없습니다.
- **앱 서명 키**: 2026년부터 새 앱은 기본으로 Google이 만든 하이브리드 키(RSA 4096 + ML-DSA-65)로 서명됩니다. 이 기본값은 공개 테스트나 프로덕션에 처음 출시하기 전까지 바꿀 수 있지만, 키를 바꾸면 내부 테스터는 더 이상 업데이트를 받지 못합니다. 업로드 키는 RSA 2048비트 이상이어야 하고, 지금 키스토어(RSA 4096, 2054년까지 유효)가 맞습니다.
  - **권장: 기본값(Google 생성 키)을 그대로 두고 지금 키스토어를 업로드 키로 씁니다.** 새 시크릿이 없습니다. Play 설치본과 사이드로드 APK는 서명이 달라 서로 업데이트하지 못하므로, 휴대폰마다 한쪽만 쓰고 바꿀 때는 지우고 설치합니다. 아래 제어 전용 Play 빌드와 함께 쓰면 두 변형이 서로를 덮어쓰지 않습니다.
  - 대안: 지금 키를 Play에 제공하고 업로드 키를 새로 만듭니다. Android 16 이하에서는 두 채널이 서로 업데이트할 수 있지만, Android 17 이상의 하이브리드 서명과 어떻게 맞물리는지는 문서에 없습니다.
- **서비스 계정은 최소 권한**: Play Console에서 HJViewer에만 "테스트 트랙에 앱 출시" 권한을 줍니다. 프로덕션에는 출시할 수 없는 권한입니다. Play 계정과 Cloud 프로젝트를 연결할 필요는 없습니다. 2024-05-03 이후 만든 Google Cloud 조직은 서비스 계정 키 생성을 기본으로 막으므로, 조직이 없는 개인 Google 계정의 프로젝트를 씁니다.
- **권한과 선언**(1.5.1 번들에서 잰 값)
  - **정정**: `MANAGE_EXTERNAL_STORAGE`와 저장소 읽기·쓰기 권한은 병합된 매니페스트에 **없습니다.** upstream이 `tools:node="remove"`로 뺍니다. 그래서 "모든 파일 접근" 선언은 필요 없습니다.
  - **접근성 서비스**(`InputService`, 제어받을 때의 원격 입력): Play Console의 접근성 API 선언(목적, 데이터, 시연 영상)이 필요합니다. 접근성 도구가 아니므로 앱 안에 눈에 띄는 고지와 동의 화면도 있어야 하고, 매니페스트 설명으로 대신할 수 없습니다. 지금의 안내 대화상자는 목적만 말하고, 서비스가 화면 내용을 읽을 수 있다는 점은 말하지 않습니다. 그래서 이 요건에 못 미칠 가능성이 큽니다.
  - **포그라운드 서비스** specialUse, mediaProjection, microphone(`MainService`, 무인 접속과 화면·소리 캡처): 유형마다 설명, 영향, 시연 영상 링크, 사용 사례를 신고합니다. 모든 유형이 심사 대상입니다. 이 신고가 없어 내부 테스트 트랙 커밋이 403으로 막힌 사례가 보고되어 있습니다.
  - `SYSTEM_ALERT_WINDOW`, `REQUEST_IGNORE_BATTERY_OPTIMIZATIONS`, `RECEIVE_BOOT_COMPLETED`: 신고 양식은 없지만, 정책 심사에서는 위험 요소입니다.
  - `RECORD_AUDIO`(음성 통화), `CAMERA`(설정의 QR 읽기, 플러그인이 더함): 개인정보처리방침 URL이 필요합니다. 카메라·마이크 권한이 있는 번들은 방침 URL이 없으면 내부 트랙에서도 거절된다는 보고가 있습니다. [privacy.md](privacy.md)에 카메라·마이크 문장을 먼저 넣어야 합니다.
  - 선언이 필요한 항목이 모르게 늘지 않도록, CI가 빌드마다 이 목록을 확인합니다([ci.md](ci.md)의 "Play 선언 대상").
- **정할 것: Play에 올릴 앱.** CI는 어느 쪽이든 그대로 씁니다.

  | | A. 전체 앱 그대로 | **B. Play용만 제어 전용 (권장)** | B'. Android 전체를 제어 전용 |
  |---|---|---|---|
  | 앱 변경 | 없음(고지·동의 화면 보강이 필요할 수 있음) | Play용 AAB에서만 피제어 구성요소를 빼고 outgoing-only 화면으로 | 모든 Android 빌드에서 뺌 |
  | Play 서류 | 방침 URL, 접근성 선언과 영상, 포그라운드 서비스 3종 선언과 영상 | 방침 URL뿐 | 방침 URL뿐 |
  | 정책 위험 | 높음 | 낮음 | 낮음 |
  | Android 피제어 | Play 설치본도 가능 | 사이드로드 APK로만 | 불가 |

  B는 Play용 AAB를 빌드하기 직전에 접근성 서비스, 화면·소리 서비스, 오버레이, 부팅 수신기와 그 권한을 빼는 매니페스트를 넣고, upstream의 outgoing-only 설정을 켜는 방식입니다(별도 작업에서 설계와 기기 시험). 같은 커밋에서 CI의 `PLAY_MANIFEST` 목록도 줄입니다. 휴대폰이 PC를 제어한다는 주 용도와 맞고, 제어받아야 하는 Android 기기는 지금처럼 APK를 씁니다.
- **Play 앱은 Play로만 업데이트**: Play에서 받은 앱은 Play의 업데이트 말고 다른 방법으로 자신을 업데이트하면 안 됩니다. 앱 안에 업데이트 안내를 넣는다면, Play 설치본에서는 Play 스토어를 열어야 합니다.
- **AGPL**: Play로 받은 앱에는 고지 파일이 없습니다. 출시 노트에 이 빌드의 소스 커밋 링크가 들어가고, 앱 정보 화면에 소스 링크가 있습니다.
- **배경**: upstream RustDesk는 원격 지원 사기 문제로 2023년에 Google Play에서 스스로 내려갔습니다. 원격 입력과 무인 접속을 함께 가진 앱은 심사 위험이 큽니다.
- **프로덕션**은 계획에 없습니다. 하려면 12명 × 14일 비공개 테스트와 심사를 거쳐야 하고, 다음이 필요합니다.
  - 데이터 보안 양식, 개인정보 처리방침, 콘텐츠 등급, 대상 연령
  - 심사자 접속 정보(데모 PC)
  - 스토어 그래픽: 512px 아이콘, 1024×500 그래픽, 스크린샷 2장 이상

## 출처

- Apple: [App Review Guidelines](https://developer.apple.com/app-store/review/guidelines/), [기기 등록(Ad Hoc 100대)](https://developer.apple.com/help/account/devices/devices-overview), [TestFlight 개요](https://developer.apple.com/help/app-store-connect/test-a-beta-version/testflight-overview), [내부 테스터](https://developer.apple.com/help/app-store-connect/test-a-beta-version/add-internal-testers), [API 키 만들기](https://developer.apple.com/documentation/appstoreconnectapi/creating-api-keys-for-app-store-connect-api), [서드파티 SDK 요건](https://developer.apple.com/support/third-party-SDK-requirements/), [Required reason API](https://developer.apple.com/documentation/bundleresources/describing-use-of-required-reason-api), [스크린샷 규격](https://developer.apple.com/help/app-store-connect/reference/screenshot-specifications), [수출 규정](https://developer.apple.com/help/app-store-connect/reference/app-information/export-compliance-documentation-for-encryption)
- Google: [Publishing API 시작](https://developers.google.com/android-publisher/getting_started), [Edits(첫 업로드 수동)](https://developers.google.com/android-publisher/edits), [신규 개인 계정 테스트 요건](https://support.google.com/googleplay/android-developer/answer/14151465), [테스트 트랙](https://support.google.com/googleplay/android-developer/answer/9845334), [앱 서명](https://developer.android.com/studio/publish/app-signing), [App Bundle](https://developer.android.com/guide/app-bundle), [접근성 API 정책](https://support.google.com/googleplay/android-developer/answer/10964491), [포그라운드 서비스 신고](https://support.google.com/googleplay/android-developer/answer/13392821), [데이터 보안](https://support.google.com/googleplay/android-developer/answer/10787469)
- Google, 2026-10-12 추가: [변경 심사와 게시](https://support.google.com/googleplay/android-developer/answer/9859654), [권한 선언](https://support.google.com/googleplay/android-developer/answer/9214102), [Play 앱 서명(하이브리드 키 기본값)](https://support.google.com/googleplay/android-developer/answer/9842756), [출시 준비(출시 노트 500자, 키 변경과 테스터)](https://support.google.com/googleplay/android-developer/answer/9859348), [사용자와 권한](https://support.google.com/googleplay/android-developer/answer/9844686), [개발자 정보 표시](https://support.google.com/googleplay/android-developer/answer/13628312), [기기 인증](https://support.google.com/googleplay/android-developer/answer/14316361), [사용자 데이터 정책](https://support.google.com/googleplay/android-developer/answer/10144311), [기기 및 네트워크 악용(자기 업데이트 금지)](https://support.google.com/googleplay/android-developer/answer/9888379), [목표 API 수준](https://developer.android.com/google/play/requirements/target-sdk), [16KB 페이지](https://developer.android.com/guide/practices/page-sizes), [버전(versionCode 상한)](https://developer.android.com/studio/publish/versioning), [명령줄 빌드(번들은 jarsigner로 서명)](https://developer.android.com/build/building-cmdline), [APKs and Tracks](https://developers.google.com/android-publisher/tracks), [edits.commit](https://developers.google.com/android-publisher/api-ref/rest/v3/edits/commit), [서버 간 OAuth 2.0](https://developers.google.com/identity/protocols/oauth2/service-account), [보안 기본값 조직(키 생성 차단)](https://docs.cloud.google.com/resource-manager/docs/secure-by-default-organizations)
- GitHub: [Secure use(구조화된 값은 값마다 시크릿)](https://docs.github.com/en/actions/reference/security/secure-use)
- 개발자 보고(공식 아님): [포그라운드 서비스 신고가 없어 내부 트랙 커밋 403](https://discuss.bitrise.io/t/play-store-upload-failing-failed-to-commit-edit-error-googleapi-error-403-you-must-let-us-know-whether-your-app-uses-any-foreground-service-permissions-forbidden/23982), [개인정보처리방침 없이 내부 테스트 업로드 거절](https://b4x.com/android/forum/threads/play-store-cant-upload-internal-testing-app-without-privacy-policy.159721/), [CI 업로드 뒤 권한 선언 안내가 안 보임](https://dulanwirajith.medium.com/google-play-permissions-declaration-not-showing-heres-how-i-fixed-it-after-a-ci-cd-upload-7f3443224918), [흔한 Google Play 오류와 권한 적용 지연](https://docs.codemagic.io/troubleshooting/common-google-play-errors/), [RustDesk와 원격 지원 사기(Play 철수)](https://rustdesk.com/blog/rustdesk-and-remote-access-scams/)
