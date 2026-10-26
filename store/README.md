# 스토어 글 (App Store, Google Play)

App Store Connect와 Google Play Console에 그대로 보일 글의 원본입니다. 스토어 실행(`.github/workflows/store-release.yml`)의 prepare가 이 파일들을 읽어 콘솔의 칸에 넣습니다. 설계는 `build/v191/notes/auto-deploy-design.md` 2-3절입니다(개발 PC에만 있음).

## 고칠 때의 규칙

- **글은 여기서만 고칩니다. 콘솔에서 고치지 않습니다.** 콘솔에서 고친 글은 다음 prepare가 이 파일의 글로 덮어씁니다. 고칠 것이 생기면(심사의 메타데이터 거절 포함) 이 폴더를 고쳐 커밋하고 prepare를 다시 돌립니다. prepare의 dry_run은 콘솔의 값과 이 폴더의 값이 다른 칸을 보여 줍니다.
- 모든 문장은 `version.txt`의 앱 버전 코드에서 사실이어야 합니다(클라이언트 패치 `client/patches`, 서버 `server/api`). 화면 이름은 앱의 글(한국어/영어 짝)과 같게 씁니다.
- 파일 하나가 칸 하나입니다. UTF-8(NFC), 끝에 줄바꿈 하나, 탭·CR·줄 끝 공백 없음. 칸의 값은 끝의 줄바꿈을 뺀 글입니다.
- 고친 뒤 검사를 돌립니다.

  ```sh
  python3 -I scripts/check-store-text.py
  python3 -I -B -m unittest discover -s scripts -p 'test_check_store_text.py'
  ```

- 앱 버전을 올리면 `appstore/version.txt`와 `play/version.txt`를 함께 올리고, 그 판의 바뀐 것에 맞게 `whats_new.txt`(두 언어)와 `release_notes.txt`를 **반드시** 다시 씁니다. 검사는 글이 어느 판의 것인지 모르므로, 다시 쓰지 않으면 앞 판의 변경이 그대로 나갑니다. 버전 파일을 올리는 커밋에 이 세 파일도 함께 넣습니다. 두 스토어의 버전이 갈라지면 검사에 `--allow-version-split`을 줍니다.

## App Store (`appstore/`)

| 파일 | App Store Connect의 칸 | 한도 |
|---|---|---|
| `version.txt` | 버전 번호(스토어 실행의 `version`과 같아야 함) | X.Y.Z |
| `distribution.txt` | 콘솔의 칸이 아닙니다. 심사 메모 검사(`unlisted`면 Apple이 요구하는 문장)와 submit의 안내에 씁니다. `unlisted`(등록되지 않은 앱) 또는 `public`. 콘솔의 배포 방법은 늘 '공개'로 두고, Unlisted는 요청이 승인되면 Apple이 바꿉니다([docs/store-release.md](../docs/store-release.md) A3) | — |
| `locales.txt` | 넣을 현지화의 App Store Connect 언어 코드, 한 줄에 하나. 첫 줄이 기본 언어 | — |
| `copyright.txt` | 버전 페이지의 저작권(언어 공통, 한 칸). ©는 Apple이 붙이므로 쓰지 않음 | `YYYY 이름` |
| `review_notes.txt` | 앱 심사 정보 → 메모(Notes), 영어 하나 | 채운 뒤 4000바이트 |
| `<언어>/name.txt` | 앱 정보 → 이름 | 2–30자 |
| `<언어>/subtitle.txt` | 앱 정보 → 부제목 | 30자 |
| `<언어>/promotional_text.txt` | 버전 페이지 → 프로모션 텍스트(새 제출 없이 바꿀 수 있음) | 170자 |
| `<언어>/description.txt` | 버전 페이지 → 설명(일반 텍스트) | 4000자 |
| `<언어>/keywords.txt` | 버전 페이지 → 키워드. 쉼표로 나누고 쉼표 옆에 공백 없음 | 100바이트 |
| `<언어>/whats_new.txt` | 버전 페이지 → 이 버전에서 업데이트된 사항 | 4000자 |
| `<언어>/support_url.txt` | 버전 페이지 → 지원 URL | https URL |
| `<언어>/marketing_url.txt` | 버전 페이지 → 마케팅 URL(선택, 빈 파일이면 비움) | https URL |
| `<언어>/privacy_url.txt` | 앱 정보 → 개인정보 처리방침 URL | https URL |

- **언어**: `ko`(기본)와 `en-GB`. 한국 App Store의 언어는 한국어와 English (U.K.)이므로 영어 글은 영국식 철자(colour, favourites, centred)로 씁니다. 영어 글에 한글은 팀 이름 `프로젝트TF` 하나만 씁니다.
- **첫 App Store 버전(1.9.1)에는 What's New 칸이 없습니다.** 첫 버전에서는 `whats_new.txt`를 보내지 않습니다. 파일은 다음 버전과 TestFlight 안내를 위해 그 판의 바뀐 것으로 둡니다.
- **지원 URL**은 처리방침의 10절(문의와 변경)로 바로 갑니다. 한국어 현지화는 한국어 절(`#10-문의와-변경`, 퍼센트 인코딩), 영어 현지화는 영어 절(`#10-contact-and-changes`)입니다. 심사 지침 1.5가 지원 URL에서 연락 방법을 쉽게 찾을 수 있어야 한다고 하고, 저장소 첫 화면에는 연락처가 없기 때문입니다.
- **iOS 글에는 Android, Google, Play를 쓰지 않습니다**(심사 지침 2.3.10). 심사 메모도 같습니다.
- 고객이 읽는 글(이름, 부제목, 프로모션 텍스트, 설명, 키워드, What's New)에는 베타·체험판·데모를 뜻하는 말과 가격을 쓰지 않습니다(지침 2.2, 2.3.7). iOS 글에는 Galaxy나 Samsung 같은 다른 기기 이름도 쓰지 않습니다.
- 이름, 부제목, 키워드, 프로모션 텍스트, What's New에는 다른 앱의 이름(RustDesk, TeamViewer, AnyDesk, Chrome Remote Desktop 등)을 쓰지 않습니다(지침 4.1의 메타데이터 도움말). 설명에도 쓰지 않습니다(소유자 결정, 2026-10-26, SUBMIT 0-2 #25). RustDesk를 바탕으로 했다는 것과 관련이 없다는 것은 심사 메모와 앱의 정보 화면에 남습니다. 검사의 `OTHER_APPS_IN`에 `description.txt`가 있습니다.
- `testflight/`는 외부 TestFlight를 켤 때만 둡니다(`what_to_test.ko.txt`, `beta_description.ko.txt`). 지금은 내부 테스트만 쓰므로 없습니다.

### 심사 메모 (`review_notes.txt`)

- 영어 메모의 틀입니다. 자리 표시는 `{{DEMO_PC_ID}}`와 `{{DEMO_PC_PASSWORD}}` 둘뿐이고, 스토어 실행이 environment `store`의 시크릿 `REVIEW_DEMO_PC_ID`, `REVIEW_DEMO_PC_PASSWORD`로 실행 중에 채웁니다.
- **데모 계정 자체(ID와 비밀번호)는 메모에 쓰지 않습니다.** 앱 심사 정보 → 로그인 정보(Sign-In Information) 칸으로만 가고, 값은 시크릿에서 옵니다.
- **채운 메모는 로그에 찍지 않습니다.** 검사와 스토어 실행은 바이트 수만 찍고 해시도 찍지 않습니다. 틀이 공개돼 있으므로, 해시가 있으면 로그를 읽는 사람이 짧은 데모 PC 비밀번호를 맞혀 볼 수 있습니다. 데모 PC 비밀번호는 16–32자(ASCII)로 정합니다. 스토어 실행은 32바이트보다 긴 값을 거부합니다(`--filled`).
- 4000바이트는 글자가 아니라 바이트입니다. 검사는 데모 PC ID 16바이트, 비밀번호 32바이트(최악)로 채우고 줄바꿈을 CRLF(2바이트)로 셉니다.
- `distribution.txt`가 `unlisted`면 메모에 Apple이 요구하는 문장 `This app is intended for unlisted distribution.`이 한 문단으로 있어야 합니다(지금 첫 문단).
- 데모 계정 문단의 세 문장은 `docs/distribution.md` "iOS: 심사용 데모 계정" 3번의 문구입니다. 그 절과 함께 고칩니다.
- 영상 문단("Screenshots of the My PCs list and the wake offer, and the attached video, …")은 App Store 제출에 로그인 기능의 영상을 늘 첨부한다는 전제입니다(SUBMIT 7-6). 올린 스크린샷 세트에 내 PC 목록과 켜기 창이 없으면 문단 첫머리를 `The attached video shows`로 바꿉니다. 영상을 첨부하지 않는 제출(외부 TestFlight 등)에서는 문단을 뺍니다.
- 조건이 맞을 때 이 파일을 고쳐 더하는 문장. 아래를 **모두** 더해도 최악의 경우(ID 16, 비밀번호 32바이트, CRLF) 4000바이트 안이고, `scripts/test_check_store_text.py`의 `test_the_review_notes_room_for_every_conditional_sentence`가 그 합계를 고정합니다(문구를 바꾸면 그 시험도 바꿉니다):
  - 학교의 서면 허락서를 첨부할 때, 둘째 문단 끝에: `The university's written permission to use its name and its SSO login is attached.`
  - 선택: 지침 4.8을 밝힐 때, 넷째 문단의 `Login uses their university portal accounts; the app has no sign-up.`을 다음으로 바꿈: `Login uses their existing university portal accounts (education accounts, guideline 4.8); the app has no sign-up.`
  - 제출 전에 실제로 시험한 기기와 OS가 있으면, 셋째 문단의 지원 범위 문장 뒤에 ` Tested on …`으로 실제 기록만 씁니다(자리 표시를 새로 만들지 않음). 앞의 공백을 넣어 **80바이트까지** 씁니다.

## Google Play (`play/`)

| 파일 | Play Console의 칸 | 한도 |
|---|---|---|
| `version.txt` | 출시 이름의 버전(스토어 실행의 `version`과 같아야 함). iOS와 버전이 갈라질 수 있어 따로 둠 | X.Y.Z |
| `ko-KR/release_notes.txt` | 비공개 테스트 트랙 `alpha`의 출시 노트(ko-KR) | 아래 두 줄을 더해 500자 |

- 업로드 단계가 노트 끝에 두 줄을 더합니다: `HJViewer <버전> (<versionCode>)`와 `소스 코드(AGPL-3.0): https://github.com/<저장소>/tree/<커밋>`. Play 빌드에는 고지 파일이 없으므로 테스터가 그 빌드의 소스를 찾게 하는 줄입니다(AGPL). 검사는 versionCode 10자리, 커밋 40자, 저장소는 둘 가운데 긴 비공개 빌드 저장소의 이름으로 세어 합이 500자 안인지 봅니다.
- Play 글에는 iPhone, iPad, iOS, App Store, TestFlight를 쓰지 않습니다.
- Play 등록정보(제목, 간단한 설명, 자세한 설명)와 그래픽은 자동화하지 않으므로 여기 없습니다. 손으로 넣습니다.

## 모든 글에 쓰지 않는 것

- 시범 안내와 체험판·시험판·'정식 서비스가 아닙니다' 같은 말. 1.9.1부터 정식 서비스입니다.
- `rustdesk://` 링크(1.9.1의 휴대폰은 `hjviewer://`만 엶), QR 화면(1.9.1에 없음).
- 채울 자리 `<…>`, TODO 같은 표시, 이메일 주소, IP 주소.

## 절대 넣지 않는 것

이 저장소는 공개입니다. 아래 값은 비공개 저장소의 시크릿(environment `store`)에서 스토어 실행이 채우거나, 소유자가 콘솔에 직접 넣습니다.

- 데모 계정의 ID와 비밀번호(App Store의 로그인 정보 칸, Play의 앱 액세스)
- 데모 PC의 ID와 비밀번호(메모의 자리 표시로만)
- 심사 연락처의 이름과 전화번호
- 테스터 이메일 목록, API 키, Issuer ID, Key ID
- 운영 서버의 주소, 실제 사람의 이름과 포탈 ID, 실제 PC의 ID
- 학교 공문, App Privacy 판단 메모, 제출 분석(`build/appstore-listing/submit`)
- 이미지(스크린샷, 아이콘, 그래픽). `build/` 밑에 두고 손으로 올립니다.

`scripts/ci.sh leak`은 이 폴더도 봅니다.

## 이 글이 기대는 1.9.1의 사실

- 앱 안의 계정 삭제: 설정 → 계정 → [계정 삭제](영어 화면 "Delete account"). 확인 창에 '삭제'('delete')를 넣으면 바로 지우고 "계정을 삭제했습니다" 창을 띄웁니다. 내 PC 목록의 PC가 계정에서 빠지고, 모든 기기가 로그아웃되며, 기기 목록·중계 장치 승인·켜기 요청 기록이 지워집니다. 다시 로그인하면 빈 계정으로 시작합니다. 다만 계정을 지우면 명단(allowlist) 항목도 지워지므로, 운영자가 명단에 있는 ID만 받는 동안에는 운영자가 다시 넣어야 로그인됩니다(App Review 계정은 항목이 남음. 클라이언트 0154·0155·0160, hjv-api `POST /api/hjv/account/delete`, `docs/privacy.md` 8절).
- 설정: 휴대폰의 '고급' 머리가 '고급 설정'(Advanced settings)이고, Android의 하드웨어 코덱과 녹화가 그 아래로 옮겨짐. 계정, 보안(앱 잠금), 설정, 정보는 그대로(0149·0150).
- 시범 안내 없음(0153). App Store 설치본은 업데이트 화면의 [App Store 열기]로 App Store에서 업데이트(0156).
- 휴대폰은 `hjviewer://`만 받고, QR 화면과 카메라·사진 권한이 없음. 원격 PC의 카메라 보기는 그대로(0157·0159).
- 정보 화면의 '개인정보 처리방침'과 소스 코드 링크는 앱 밖의 브라우저에서 열림. 터미널 메뉴에 베타 표시 없음(0158).
- iOS 13.0 이상, iPhone과 iPad.
