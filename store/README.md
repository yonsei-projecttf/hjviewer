# 스토어 글 (App Store, Google Play)

App Store Connect와 Google Play Console에 그대로 보일 글의 원본입니다. 스토어 실행(`.github/workflows/store-release.yml`)의 prepare가 이 파일들을 읽어 콘솔의 칸에 넣습니다. 설계는 `build/v191/notes/auto-deploy-design.md` 2-3절입니다(개발 PC에만 있음).

## 고칠 때의 규칙

- **글은 여기서만 고칩니다. 콘솔에서 고치지 않습니다.** 콘솔에서 고친 글은 다음 prepare가 이 파일의 글로 덮어씁니다. 고칠 것이 생기면(심사의 메타데이터 거절 포함) 이 폴더를 고쳐 커밋하고 prepare를 다시 돌립니다. prepare의 dry_run은 콘솔의 값과 이 폴더의 값이 다른 칸을 보여 줍니다. 예외는 하나입니다. 스토어 실행 없이 손으로 낼 때의 심사 메모는 이 폴더의 틀을 붙여 넣은 뒤 아래 "심사 메모"의 '손으로 낼 때만 쓰는 변형'대로 콘솔에서 바꿉니다. 변형 자체를 바꿀 일이 생기면 그것도 여기서 고칩니다.
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
- **첫 App Store 버전에는 What's New 칸이 없습니다.** 첫 버전에서는 `whats_new.txt`를 보내지 않습니다(스토어 실행이 App Store Connect의 버전 상태로 첫 버전인지 압니다). 파일은 다음 버전과 TestFlight 안내를 위해 그 판의 바뀐 것으로 둡니다.
- **지원 URL**은 처리방침의 10절(문의와 변경)로 바로 갑니다. 한국어 현지화는 한국어 절(`#10-문의와-변경`, 퍼센트 인코딩), 영어 현지화는 영어 절(`#10-contact-and-changes`)입니다. 심사 지침 1.5가 지원 URL에서 연락 방법을 쉽게 찾을 수 있어야 한다고 하고, 저장소 첫 화면에는 연락처가 없기 때문입니다.
- **iOS 글에는 Android, Google, Play를 쓰지 않습니다**(심사 지침 2.3.10). 심사 메모도 같습니다.
- 고객이 읽는 글(이름, 부제목, 프로모션 텍스트, 설명, 키워드, What's New)에는 베타·체험판·데모를 뜻하는 말과 가격을 쓰지 않습니다(지침 2.2, 2.3.7). iOS 글에는 Galaxy나 Samsung 같은 다른 기기 이름도 쓰지 않습니다.
- 이름, 부제목, 키워드, 프로모션 텍스트, What's New에는 다른 앱의 이름(RustDesk, TeamViewer, AnyDesk, Chrome Remote Desktop 등)을 쓰지 않습니다(지침 4.1의 메타데이터 도움말). 설명에도 쓰지 않습니다(소유자 결정, 2026-10-26, SUBMIT 0-2 #25). RustDesk를 바탕으로 했다는 것과 관련이 없다는 것은 심사 메모와 앱의 정보 화면에 남습니다. 검사의 `OTHER_APPS_IN`에 `description.txt`가 있습니다.
- `testflight/`는 외부 TestFlight를 켤 때만 둡니다(`what_to_test.ko.txt`, `beta_description.ko.txt`). 지금은 내부 테스트만 쓰므로 없습니다.

### 심사 메모 (`review_notes.txt`)

- 영어 메모의 틀이고, 데모 PC 한 대 판입니다. 자리 표시는 `{{DEMO_PC_ID}}`와 `{{DEMO_PC_PASSWORD}}` 둘뿐이고, 스토어 실행이 environment `store`의 시크릿 `REVIEW_DEMO_PC_ID`, `REVIEW_DEMO_PC_PASSWORD`로 실행 중에 채웁니다. 데모 PC 두 대, 시험한 기기, Windows 비밀번호는 손으로 낼 때의 변형입니다(아래).
- **데모 계정 자체(ID와 비밀번호)는 메모에 쓰지 않습니다.** 앱 심사 정보 → 로그인 정보(Sign-In Information) 칸으로만 가고, 값은 시크릿에서 옵니다.
- **채운 메모는 로그에 찍지 않습니다.** 검사와 스토어 실행은 바이트 수만 찍고 해시도 찍지 않습니다. 틀이 공개돼 있으므로, 해시가 있으면 로그를 읽는 사람이 짧은 데모 PC 비밀번호를 맞혀 볼 수 있습니다. 데모 PC 비밀번호는 16–32자(ASCII)로 정합니다. 스토어 실행은 32바이트보다 긴 값을 거부합니다(`--filled`).
- 4000바이트는 글자가 아니라 바이트입니다. 검사는 데모 PC ID 16바이트, 비밀번호 32바이트(최악)로 채우고 줄바꿈을 CRLF(2바이트)로 셉니다.
- `distribution.txt`가 `unlisted`면 메모에 Apple이 요구하는 문장 `This app is intended for unlisted distribution.`이 한 문단으로 있어야 합니다(지금 첫 문단).
- 데모 계정 문단과 그다음 데모 PC 문단의 문장은 `docs/distribution.md` "iOS: 심사용 데모 계정" 3번이 인용합니다. 그 절과 함께 고칩니다.
- 영상 문단(`The screenshot of the My PCs list and the attached video show …`)은 App Store 제출에 교직원 기능(교직원 계정의 내 PC 목록과 지원 코드)의 영상을 첨부하고, 올린 스크린샷에 내 PC 목록이 있다는 전제입니다. 영상을 첨부하지 않으면 문단을 `The screenshot of the My PCs list shows what the demo account cannot: a staff account's PC list.`로 바꾸고(−41바이트), 올린 스크린샷에 내 PC 목록이 없으면 문단 첫머리를 `The attached video shows`로 바꿉니다. 둘 다 없는 제출(외부 TestFlight 등)에서는 문단을 뺍니다. 이 문단에는 비밀번호 없는 연결과 켜기를 넣지 않습니다. 비밀번호 없는 연결은 운영자가 켤 때만 되는 기능이라, 둘째 문단처럼 그 조건을 붙여서만 씁니다.
- 조건이 맞을 때 이 파일을 고쳐 더하는 문장(스토어 실행으로 낼 때도 씀). 아래 둘을 **함께** 더해도 최악의 경우(ID 16, 비밀번호 32바이트, CRLF) 4000바이트 안이고(2026-10-28의 글로 3912바이트), `scripts/test_check_store_text.py`의 `test_the_review_notes_room_for_every_conditional_sentence`가 그 합계를 고정합니다(문구를 바꾸면 그 시험도 바꿉니다). 문단의 번호는 아래 승인 문장을 넣기 전의 것입니다:
  - 학교의 서면 허락서를 첨부할 때, 둘째 문단 끝에: `The university's written permission to use its name and its SSO login is attached.` 앞의 공백까지 83바이트입니다.
  - Apple이 등록되지 않은 앱 배포(Unlisted)를 승인한 뒤 다시 낼 때, 첫 문단 바로 다음에 새 문단으로: `Apple approved unlisted app distribution for this app on <date>.` `<date>`는 승인 메일의 날짜를 영어로 씁니다(예: `October 30, 2026`). 가장 긴 날짜(`September 30, 2026`)로 76바이트, 문단 구분까지 80바이트입니다.
- 손으로 낼 때만 쓰는 변형(2026-10-28, 1.9.2의 App Review 통과 계획. 개발 PC의 `build/appstore-listing/submit/PASS-PLAN-1.9.2.md`). 스토어 실행은 위의 자리 둘만 채우므로 이 변형으로는 낼 수 없습니다. 이 파일은 고치지 않고, 콘솔에 붙여 넣은 메모를 콘솔에서 바꾸며, `<…>`도 콘솔에서만 채웁니다. 문단의 번호는 위의 승인 문장을 넣기 전의 것입니다(넣은 뒤에는 둘째 문단부터 한 칸씩 밀림). 바이트는 같은 셈의 최악값입니다:
  - 데모 PC가 두 대일 때(두 PC에 같은 영구 비밀번호): 다섯째 문단의 첫 문장을 `The demo account reaches only the demo Windows PCs, both with the password {{DEMO_PC_PASSWORD}}: ID {{DEMO_PC_ID}}, or ID <second PC ID> if the first is offline.`로 바꾸고, 여섯째 문단의 `On the demo PC, do not`을 `On a demo PC, do not`으로, 마지막 문단의 `type the demo PC's ID`를 `type a demo PC's ID`로 바꿉니다. 둘째 ID 16바이트로 +59바이트입니다.
  - 제출 전에 시험한 기기와 OS를 밝힐 때: 셋째 문단 끝에 ` Tested on <devices and OS versions>.` 실제로 시험한 기기 모델과 OS만 쓰고, 앞의 공백과 마침표까지 80바이트 안으로 둡니다.
  - 데모 PC의 Windows 사용자에게 비밀번호가 있을 때: 여섯째 문단의 `Its Windows user has no password; on the lock screen, click Sign in.`을 `On the lock screen, choose "OS password" in the session's menu and enter <Windows password>.`로 바꿉니다. 비밀번호 32바이트로 +38바이트입니다.
  - 데모 PC에 웹캠이 없을 때: 열째 문단의 `which we do not give out).`를 `which we do not give out; the demo PC has no camera).`로 바꿉니다(+27바이트). 두 대면 `which we do not give out; the demo PCs have no camera).`입니다(+29바이트).
  - 영상을 첨부하지 않을 때: 위 영상 문단의 바꿈(−41바이트).
  - 이 변형을 위의 두 문장과 함께 넣으면 4000바이트를 넘을 수 있습니다. 넘으면 Tested on 문장을 뺍니다. 시험은 1.9.2의 첫 손 제출 꼴(두 대와 Tested on, 3888바이트. 통과 계획 부록 A의 상자와 같은 글)과, 두 대에 웹캠 없음·허락서·승인 문장을 넣고 Tested on을 뺀 꼴(꼭 4000바이트)을 셉니다. 두 대에 Windows 비밀번호·허락서·승인 문장을 함께 넣으면 Tested on을 빼도 4009바이트이므로, 그때는 문장을 더 줄여 다시 셉니다.
- 2026-10-28에 1.9.2의 App Review 통과 계획에 맞춰 고쳤습니다. iOS 앱은 세션을 녹화하지 않으므로 녹화 문장을 뺐고, 앱 잠금은 Face ID와 Touch ID로, 계정 삭제는 iPhone·iPad 앱에서로 밝혔으며, 한국어 로그인 실패 문구, 데모 PC 문단, 터미널의 "Run as administrator" 항목, 카메라·터미널을 PC 주인이 끌 수 있다는 것을 더했습니다. 지침 4.8 문구는 이제 넷째 문단에 늘 있습니다. 수출 규정 문단과 iOS 13.0 문장(Info.plist와 메타데이터가 답함), Local network 문단(권한 창의 문구가 같은 것을 말함)은 위 두 문장을 함께 넣은 최악의 경우를 4000바이트 안에 두려고 뺐습니다. 둘째 문단 끝의 `and works the same in every region`도 통과 계획의 상자처럼 뺐습니다. 국가나 넓은 대역 단위의 차단이 없는지 아직 확인하지 못한 주장이기 때문입니다(통과 계획 부록 B 6번). 확인한 뒤 다시 넣으면 그 문단의 마지막 문장이 `It costs nothing, has no ads or in-app purchases, and works the same in every region.`이 되고(+33바이트), 손 제출의 상자(통과 계획 부록 A)에도 함께 넣어 둘을 같게 둡니다.

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

## 이 글이 기대는 사실 (1.9.1, 1.9.2)

- 앱 안의 계정 삭제: 설정 → 계정 → [계정 삭제](영어 화면 "Delete account"). 확인 창에 '삭제'('delete')를 넣으면 바로 지우고 "계정을 삭제했습니다" 창을 띄웁니다. 내 PC 목록의 PC가 계정에서 빠지고, 모든 기기가 로그아웃되며, 기기 목록·중계 장치 승인·켜기 요청 기록이 지워집니다. 다시 로그인하면 빈 계정으로 시작합니다. 다만 계정을 지우면 명단(allowlist) 항목도 지워지므로, 운영자가 명단에 있는 ID만 받는 동안에는 운영자가 다시 넣어야 로그인됩니다(App Review 계정은 항목이 남음. 클라이언트 0154·0155·0160, hjv-api `POST /api/hjv/account/delete`, `docs/privacy.md` 8절).
- 설정: 휴대폰의 '고급' 머리가 '고급 설정'(Advanced settings)이고, Android의 하드웨어 코덱과 녹화가 그 아래로 옮겨짐. 계정, 보안(앱 잠금), 설정, 정보는 그대로(0149·0150).
- 시범 안내 없음(0153). App Store 설치본은 업데이트 화면의 [App Store 열기]로 App Store에서 업데이트(0156).
- 휴대폰은 `hjviewer://`만 받고, QR 화면과 카메라·사진 권한이 없음. 원격 PC의 카메라 보기는 그대로(0157·0159).
- 정보 화면의 '개인정보 처리방침'과 소스 코드 링크는 앱 밖의 브라우저에서 열림. 터미널 메뉴에 베타 표시 없음(0158).
- iOS 13.0 이상, iPhone과 iPad.
- 1.9.2: '로그인이 풀렸습니다'(영어 화면 "You were logged out") 안내가 "보안 키는 로그인한 기기에만 남고, 백업이나 복원으로도 다른 기기로 옮겨지지 않습니다."라고 알림(0163). 고정 서버 빌드는 RustDesk의 페이지로 가는 단추를 보이지 않음: 연결한 PC가 보낸 안내나 그 PC에 관한 안내 창의 링크 단추(rustdesk.com의 도움말), Wayland 키보드 안내의 'Why this happens', Windows 상태 줄의 setup_server_tip(0164, 0167). iOS의 최소 판은 13.0 그대로(1.9.2에서 15.0으로 올리지 않음).
- 심사 메모(2026-10-28): iOS 앱은 세션을 녹화하지 않음(세션 메뉴의 녹화 항목은 PC가 녹화를 허락하면 보이지만 iOS에서는 아무것도 하지 않음). 앱 잠금은 기기에 따라 Face ID나 Touch ID. 로그인 실패 문구는 서버의 한국어 한 문장(`server/api/src/login.rs`의 `FAILURE_TEXT`)이고 앱이 그대로 보임. 데스크톱 계정 카드에서 로그아웃하거나 계정을 지우면 그 PC가 계정에서 빠져, 교직원 전용 서버에서는 연결을 받지 못함. 최근 세션 카드의 메뉴에 터미널과 카메라 보기가 있고, Windows PC의 'Terminal (Run as administrator)'는 그 PC의 관리자 계정을 물음. PC 권한 설정의 'Enable camera'와 'Enable terminal'은 기본으로 켜져 있고 PC 주인이 끌 수 있음.
