# 앱 배포: 10명 이내 비상업 사용 기준

2026-10-10에 Apple과 Google의 공식 문서, 그리고 이 앱의 실제 빌드 결과물을 확인해서 정리했습니다.

## 결론: 스토어 심사 없이 배포하는 것을 권장

Apple의 App Review Guidelines 서문은 **가족이나 지인만 쓰는 앱이라면 App Store보다 Xcode 설치나 Ad Hoc 배포가 적합하다**고 안내합니다. 10명 이내의 비상업 사용에는 아래 경로가 가장 간단하고, 심사도 필요 없습니다.

| 플랫폼 | 권장 경로 | 심사 | 필요한 것 |
|---|---|---|---|
| Windows | 결과물 exe를 직접 전달 | 없음 | (서명 없음: SmartScreen 경고가 뜰 수 있음) |
| Android | 릴리스 서명 APK를 직접 설치(사이드로드) | 없음 | 업로드 키스토어 하나 |
| iOS | **Ad Hoc** 서명 IPA를 등록된 iPhone에 설치 | 없음 | Apple Developer Program, 사용자 iPhone UDID 등록 |
| iOS(대안) | TestFlight **내부** 테스트 | 없음 (내부 테스터 최대 100명) | 위 + App Store Connect API 키, 앱 레코드, 아래의 "업로드 차단 요소" 해결 |

실제 서버 값으로 만든 앱은 공개 아티팩트로 올리지 않습니다. 그래서 서명 빌드는 **비공개 미러 저장소**의 CI에서 합니다([deploy.md](deploy.md)의 "소규모 비공개 운영"). 이 워크플로는 iOS 서명을 비공개 저장소에서만 합니다.

## 권장 경로에 필요한 정보

비공개 미러 저장소의 **Secrets**에 넣습니다. 채팅이나 이슈로 보내지 마세요.

**공통**
- `HJV_SERVER`, `HJV_SERVER_KEY`: 서버 배포 후 받은 도메인과 공개키([server-runbook.md](server-runbook.md)의 "보고")

**Android (사이드로드)**
- `ANDROID_KEYSTORE_BASE64`, `ANDROID_KEYSTORE_PASSWORD`, `ANDROID_KEY_ALIAS`, `ANDROID_KEY_PASSWORD`: 만드는 방법은 [ci.md](ci.md)에 있습니다. 같은 키로 서명해야 앱 위에 업데이트를 설치할 수 있으므로, **키스토어를 잃어버리면 안 됩니다.**
- 사용자는 Android 설정에서 "출처를 알 수 없는 앱 설치"를 허용해야 합니다.

**iOS (Ad Hoc)**
- **Apple Developer Program** 개인 멤버십: 연 99달러이고, 판매자 이름으로 법적 이름이 표시됩니다.
- **사용자 iPhone의 UDID**: Certificates, Identifiers & Profiles → Devices에 등록합니다. 기기 종류별로 멤버십 1년에 100대까지입니다.
- **App ID** `kr.ac.yonsei.hjviewer`(explicit), **Apple Distribution 인증서**(.p12와 비밀번호), 위 기기를 담은 **Ad Hoc 프로비저닝 프로파일**
- Secrets: `IOS_DIST_CERT_P12_BASE64`, `IOS_DIST_CERT_PASSWORD`, `IOS_PROVISIONING_PROFILE_BASE64`, `IOS_TEAM_ID`
- 사용자를 추가할 때마다 기기를 등록하고, 프로파일을 다시 받아 다시 빌드해야 합니다.
- 설치 방법: Finder나 Apple Configurator로 USB 설치하거나, HTTPS 서버에 `manifest.plist`를 올려 `itms-services://` 링크로 무선 설치합니다.

## 스토어로 갈 때 (나중에 필요해지면)

### 지금 빌드에 남아 있는 업로드 차단 요소

- **iOS 개인정보 매니페스트**(ITMS-91061, ITMS-91053): App Store Connect가 업로드를 거부합니다. TestFlight도 마찬가지입니다.
  - Apple 목록에 있는 SDK 7개에 매니페스트가 없습니다: sqflite 2.2.0, file_picker 5.5.0, package_info_plus 4.2.0, device_info_plus 9.1.2, DKImagePickerController 4.3.4, DKPhotoGallery 0.0.17, SwiftyGif 5.4.4. 매니페스트가 들어간 버전으로 올려야 하는데, upstream이 고정한 Flutter 3.24.5와 호환되는지 확인이 필요합니다.
  - 앱 자체도 `PrivacyInfo.xcprivacy`가 필요합니다. Rust 코어가 파일 시각 API(stat/fstat/lstat)를 쓰기 때문입니다.
- **Android 스토어 형식**: Play에는 AAB(`flutter build appbundle --target-platform android-arm64`)를 올려야 하고, Gradle에서 업로드 키로 서명해야 합니다. 지금 CI는 디버그 서명 위에 다시 서명하는 APK만 만듭니다.
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
  - 수출 규정 답변: 앱이 자체 암호화를 쓰므로 판매 지역에 따라 신고가 필요할 수 있음
  - 심사용 **항상 켜진 데모 Windows PC의 ID와 비밀번호**
- 위험 요소: RustDesk 공식 앱이 이미 App Store에 있어서, 4.1/4.3(모방 앱) 조항으로 거절될 수 있습니다.

### Android: Google Play 자동화에 필요한 것

- **Play 개발자 계정**: 개인 계정은 1회 25달러이고 신분증 인증이 필요합니다. 법적 이름과 연락처가 공개됩니다.
- **2023-11-13 이후 만든 개인 계정은 프로덕션 출시 전에 비공개 테스트를 해야 합니다.** 테스터 12명 이상이 14일 연속 참여해야 해서, 첫 출시까지 4~5주를 잡아야 합니다.
- 앱 생성과 **첫 AAB 업로드는 Play Console에서 직접** 합니다. API는 빌드가 하나 이상 있는 앱만 다룹니다.
- **서비스 계정**: Google Cloud 프로젝트에서 Play Android Developer API를 켜고, JSON 키(또는 WIF)를 만들어 Play Console에 초대합니다. 권한은 "테스트 트랙에 앱 출시"만 줍니다.
- 내부 테스트 트랙은 테스터 100명까지이고, 빌드가 몇 분 만에 배포됩니다. 표준 정책 심사를 받지 않을 수 있습니다.
- 프로덕션 심사를 받으려면 필요한 것:
  - 데이터 보안 양식, 개인정보 처리방침, 콘텐츠 등급, 대상 연령
  - 심사자 접속 정보(데모 PC)
  - 스토어 그래픽: 512px 아이콘, 1024×500 그래픽, 스크린샷 2장 이상
- **Android 피제어 기능**(접근성 서비스, 화면 캡처, 포그라운드 서비스)을 남기면 각각 신고와 시연 영상이 필요합니다. 제어 전용으로 빌드하면 이 부담이 사라집니다. upstream의 outgoing-only 모드가 있습니다.

## 출처

- Apple: [App Review Guidelines](https://developer.apple.com/app-store/review/guidelines/), [기기 등록(Ad Hoc 100대)](https://developer.apple.com/help/account/devices/devices-overview), [TestFlight 개요](https://developer.apple.com/help/app-store-connect/test-a-beta-version/testflight-overview), [내부 테스터](https://developer.apple.com/help/app-store-connect/test-a-beta-version/add-internal-testers), [API 키 만들기](https://developer.apple.com/documentation/appstoreconnectapi/creating-api-keys-for-app-store-connect-api), [서드파티 SDK 요건](https://developer.apple.com/support/third-party-SDK-requirements/), [Required reason API](https://developer.apple.com/documentation/bundleresources/describing-use-of-required-reason-api), [스크린샷 규격](https://developer.apple.com/help/app-store-connect/reference/screenshot-specifications), [수출 규정](https://developer.apple.com/help/app-store-connect/reference/app-information/export-compliance-documentation-for-encryption)
- Google: [Publishing API 시작](https://developers.google.com/android-publisher/getting_started), [Edits(첫 업로드 수동)](https://developers.google.com/android-publisher/edits), [신규 개인 계정 테스트 요건](https://support.google.com/googleplay/android-developer/answer/14151465), [테스트 트랙](https://support.google.com/googleplay/android-developer/answer/9845334), [앱 서명](https://developer.android.com/studio/publish/app-signing), [App Bundle](https://developer.android.com/guide/app-bundle), [접근성 API 정책](https://support.google.com/googleplay/android-developer/answer/10964491), [포그라운드 서비스 신고](https://support.google.com/googleplay/android-developer/answer/13392821), [데이터 보안](https://support.google.com/googleplay/android-developer/answer/10787469)
