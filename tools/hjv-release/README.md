# hjv-release

HJViewer의 **서명된 버전 정책**(최소 버전과 강제 업데이트)을 만들고, 서명하고, 확인하는 운영자 도구입니다. 앱은 `https://<서버>/api/hjv/version`에서 이 정책을 받아, 빌드에 박힌 공개키로 서명을 확인한 뒤에만 따릅니다.

- 에이전트가 하는 일: 정책 초안 만들기(`policy`), 확인(`verify`), 서버로 보낼 디렉터리 모으기(`publish-dir`), 서버 설정 값(`hbbs-conf`, `floor`), 주 1회 점검(`check`).
- **사용자만 하는 일: 키 만들기(`keygen`)와 서명(`sign`).** 두 명령은 터미널에서 키 비밀번호를 받습니다. 터미널이 없으면(스크립트, CI, 에이전트의 셸) 실행을 거부합니다. 비밀번호는 표준 입력, 환경 변수, 인자로 받지 않습니다.
- 이 도구는 비밀키, 비밀번호, 문 2 코드를 화면·로그·오류에 쓰지 않습니다.

이 문서의 예시는 서버 이름을 `rd.example.com`으로 씁니다.

## 빌드

```sh
cd tools/hjv-release
cargo build --release --locked        # target/release/hjv-release
cargo fmt --check && cargo clippy --all-targets -- -D warnings && cargo test --locked
```

명령은 저장소 맨 위에서 실행합니다. `--keys`를 주지 않으면 `client/update-keys.env`를 읽습니다. `hjv-release help`가 모든 명령과 옵션을 보여 줍니다.

종료 코드: 0 성공, 1 거부나 오류, 2 모르는 명령, 3 `check`의 경고.

## 1. 키 (한 번, 사용자만)

키는 셋입니다. 앱 빌드에는 세 공개키가 모두 들어갑니다.

| 키 | 번호 | 쓰는 곳 | 두는 곳 |
|---|---|---|---|
| 서명 키 1 | 1 | 릴리스마다 정책 서명 | 개발 PC `~/hjviewer-secrets/hjviewer-update-1.key` + 오프라인 사본 |
| 서명 키 2 | 2 | 다음 교체용, 키 1이 새면 대신 서명 | 오프라인 매체에만 |
| 루트 키 | 101 | 키 폐기, 세대 올리기, 완화(최소 버전 내리기 등) | 오프라인 매체 두 곳에만. 개발 PC 디스크에 쓰지 않음 |

```sh
hjv-release keygen --role signing --id 1   --out ~/hjviewer-secrets/hjviewer-update-1.key
hjv-release keygen --role signing --id 2   --out /media/usb1/hjviewer-update-2.key
hjv-release keygen --role root    --id 101 --out /media/usb1/hjviewer-update-root-101.key
```

- 비밀번호를 두 번 묻습니다(12자 이상). **비밀번호를 잊으면 그 키는 다시 쓸 수 없습니다.**
- 키 파일은 비밀번호로 암호화한 파일입니다(권한 0600). 이미 있는 파일은 덮어쓰지 않습니다.
- 표준 출력에는 공개 줄 `N:<base64>` 한 줄만 나옵니다. 이 줄을 `client/update-keys.env`에 넣습니다.

```sh
# client/update-keys.env (공개 파일. 앱 빌드가 읽음)
HJV_UPDATE_PUBKEY=1:<서명 키 1의 공개 줄 값>,2:<서명 키 2의 공개 줄 값>
HJV_UPDATE_ROOT_PUBKEY=101:<루트 키의 공개 줄 값>
HJV_POLICY_FLOOR=1:0
```

- 공개 줄을 다시 보려면 `hjv-release pubkey --key FILE`(비밀번호 필요).
- 키 파일을 확인하려면 `hjv-release keys --production`. 형식이 틀리거나, 비어 있거나, 이 저장소의 시험 키가 들어 있으면 실패합니다. 약한 키와 주 부분군 밖의 점도 거부합니다. 앱은 그런 키가 하나라도 있으면 키 전체를 쓰지 않고, 그 빌드는 정책을 확인하지 않게 되기 때문입니다.

## 2. 릴리스마다

1. (에이전트) `client/version.env`를 올리고, `HJV_POLICY_FLOOR`를 지금 정책 위치로 바꿉니다: `hjv-release floor --policy <지금 정책>`이 그 줄을 출력합니다. push하고 빌드 저장소에서 빌드합니다.
2. (에이전트) 산출물을 `build/release-<버전>/`에 받습니다(`gh run download <실행 번호> -R <빌드 저장소> -D build/release-<버전>`). 산출물 디렉터리마다 `release.json`이 있어야 합니다(아래 "출처 확인"). 검토하고 올린 커밋에 태그 `client-<버전>`을 만듭니다.
3. (에이전트) 초안을 만듭니다. **새 패키지가 든 초안은 빌드한 지 7일 안에 만듭니다.** `policy`가 그 실행의 산출물을 GitHub에서 직접 받아 패키지와 대조하는데, 산출물은 7일 뒤 지워집니다.

   ```sh
   hjv-release policy --prev build/policy/version-policy-7.json \
       --release-dir build/release-1.6.1 \
       --auto-after windows=+2d --notes build/release-1.6.1/notes.txt \
       --ios-valid 263120249 \
       --out build/policy/draft-8.json
   hjv-release publish-dir --policy build/policy/draft-8.json \
       --release-dir build/release-1.6.1 --out build/publish-1.6.1
   ```

4. **(사용자)** 터미널에서 서명합니다. 도구가 바뀌는 내용을 보여 주고 키 비밀번호를 묻습니다. 요약의 "새 패키지" 줄에 나온 SHA-256을 GitHub의 빌드 실행 요약에 적힌 값과 눈으로 대조합니다. `sign`은 빌드도 네트워크도 보지 않으므로, 초안에 적힌 해시가 그 빌드의 것인지는 이 대조가 마지막 확인입니다.

   ```sh
   hjv-release sign --key ~/hjviewer-secrets/hjviewer-update-1.key \
       --out build/policy/version-policy-8.json build/policy/draft-8.json
   ```

5. (에이전트) 확인하고, 서명된 정책을 보낼 디렉터리에 넣습니다.

   ```sh
   hjv-release verify --server rd.example.com \
       --prev build/policy/version-policy-7.json build/policy/version-policy-8.json
   hjv-release publish-dir --policy build/policy/version-policy-8.json \
       --release-dir build/release-1.6.1 --out build/publish-1.6.1
   ```

6. 사용자가 승인하면 `build/publish-1.6.1/`을 서버의 `incoming/`으로 보냅니다. 서버에서는 서명과 해시를 다시 확인한 뒤 **패키지를 먼저, 정책을 나중에** 넣습니다(`docs/server-runbook.md`).

`--prev`에는 직전에 서명해 둔 정책 파일을 줍니다. 서명한 정책 파일은 지우지 말고 차례대로 보관합니다.

### 처음 두 정책 (1.6.0)

```sh
# seq 1: 1.6.0이 나왔음을 알릴 뿐, 아무도 막지 않음
hjv-release policy --prev none --server rd.example.com --release-dir build/release-1.6.0 \
    --min all=1.5.0 --grace-days 0 --auto-after windows=none --ios-valid <TestFlight 빌드 번호> \
    --os-min windows=10 --os-min android=22 --os-min ios=13.0 \
    --contact "설치 파일은 관리자에게 받으세요." --out build/policy/draft-1.json

# seq 2: 모든 항목의 최소 버전을 1.6.0으로, 유예 14일(한국 시각 0시에 끝남)
hjv-release policy --prev build/policy/version-policy-1.json \
    --min all=1.6.0 --grace-days 14 --ios-valid <TestFlight 빌드 번호> \
    --out build/policy/draft-2.json
```

- 1.6.0은 손으로 설치하므로 seq 1에는 무인 설치 시각을 두지 않습니다(`--auto-after windows=none`). 1.6.1부터는 이 옵션을 주지 않으면 게시 2일 뒤부터 고리별로 설치합니다.
- 처음 강제하는 seq 2의 유예는 14일로 직접 줍니다. 그 뒤로 `min`을 올릴 때 옵션을 주지 않으면 7일입니다.

### `policy`의 옵션

`P`는 `windows`, `android`, `android_play`, `ios` 중 하나입니다.

| 옵션 | 뜻 |
|---|---|
| `--prev FILE\|none` | 직전 정책. 처음에만 `none`(이때 `--server` 필요) |
| `--release-dir DIR` | 새 빌드의 산출물. Windows·Android의 `latest`는 여기 있는 패키지로만 오릅니다 |
| `--min P=x.y.z`, `--min all=x.y.z` | 최소 버전. 올릴 때 유예 옵션을 주지 않으면 기본 7일입니다 |
| `--grace-days [P=]N` | 오늘(한국 날짜)부터 N일 뒤 한국 시각 0시까지 유예. `0`은 유예 없이 바로 |
| `--grace-until [P=]T` | 유예 끝. `2026-11-23`(한국 시각 0시) 또는 `2026-11-22T15:00:00Z` |
| `--latest P=x.y.z` | `latest`를 이 값으로 고정. iOS·Play에서 버전을 직접 말할 때, 그리고 내릴 때(`--relax`) |
| `--hold P` | 이 항목은 그대로 둠 |
| `--auto-after windows=+2d\|T\|none` | 설치된 Windows PC의 무인 설치 시작. 새 Windows 버전을 낼 때 주지 않으면 게시 2일 뒤입니다. `none`은 무인 설치를 하지 않거나 멈춤 |
| `--os-min P=V` | `latest`가 도는 가장 낮은 OS(ios `13.0`, android API 수준 `22`, windows `10`) |
| `--contact TEXT`, `--notes [P=]FILE` | 안내 문구와 알림 글. URL은 넣을 수 없습니다 |
| `--ios-valid BUILD` | 운영자가 TestFlight에서 확인한 빌드 번호(VALID, 내부 그룹에서 시험 중) |
| `--play-live VERSIONCODE` | 운영자가 Play 내부 트랙에서 출시(completed)를 확인한 versionCode |
| `--move-to-play`, `--no-move-to-play` | 사이드로드 Android 앱에 Play로 옮기기 안내 |
| `--relax`, `--drop P` | 완화를 허용(루트 키로 서명). `--drop`은 항목을 뺌 |
| `--revoke-key N`, `--new-epoch` | 서명 키 폐기, 세대 올리기(루트 키로 서명) |
| `--held FILE\|none` | 직전 정책이 루트 키의 정책일 때: 그 앞의 정책 파일. 루트 키의 정책을 놓친 앱도 새 정책을 받는지 확인합니다(3. 비상 절차). 여러 번 줄 수 있습니다 |
| `--seq N` | seq를 직접 정함. 보통은 쓰지 않습니다 |
| `--repo DIR`, `--build-repo`, `--public-repo`, `--allow-input NAME`, `--attest` | 출처 확인 설정 |

### 도구가 거부하는 것

고칠 수 없는 실수를 서명 전에 막습니다.

- 위치: (세대, seq)는 직전 + 1입니다. 직전 정책은 서명과 형식을 다시 확인합니다.
- Windows·Android: 패키지 파일이 `release.json`과 크기·SHA-256이 다르면, 출처 확인이 실패하면, 같은 버전인데 해시가 다르면 거부합니다. `min`은 `latest`를 넘을 수 없습니다.
- 패키지 이름은 `HJViewer-<버전>-windows-x86_64.exe`, `HJViewer-<버전>-android-arm64.apk`입니다. `install.exe`로 끝나거나 `-qs-`가 든 이름은 쓰지 않습니다.
- iOS: `latest`나 `min`을 올리려면 `--ios-valid`가 필요합니다. 올린 지 80일이 넘은 빌드는 거부합니다(90일에 만료).
- Play: `android_play`의 `latest`나 `min`을 올리려면 `--play-live`가 필요하고, 그 versionCode가 그 버전의 것 이상이어야 합니다.
- `min`을 올리거나 새 항목을 만들 때 유예 옵션이 없으면 기본 7일을 두고 그렇게 했다고 알립니다. 바로 강제하려면 `--grace-days 0`을 직접 줘야 합니다. 유예는 나중에 서명 키로 줄일 수 있지만, 늘리는 것은 루트 키만 합니다.
- `os_min`을 올리는 정책에서는 `min`을 옛 OS에서 도는 마지막 버전 이하로 둡니다.
- 문구에 URL(`://`, `www.`), 중괄호, 제어 문자, 보이지 않는 문자가 있으면 거부합니다. 정책에는 어떤 URL도 넣지 않습니다.
- 완화(`min` 내림, 같은 `min`의 유예 늦춤, `latest` 내림, 항목 뺌)는 `--relax` 없이는 거부합니다.
- **새 항목의 `min`이 지금 강제하는 값보다 낮으면** 이것도 `--relax`(루트 키)가 있어야 합니다. 앱의 규칙으로는 완화가 아니어서 서명 키의 정책으로도 받지만, hbbs의 `MIN`(모든 항목 중 가장 낮은 값)이 내려가고, PC가 Android 컨트롤러에 쓰는 기준(`android`와 `android_play` 중 낮은 값)도 내려가기 때문입니다.
- `min`을 올리면서 유예 끝을 옛 `min`의 기한보다 늦게 두면, 그때까지는 옛 `min` 미만도 막지 않게 됩니다(앱은 항목의 `min` 하나와 유예 하나만 봅니다). 앱이 서명 키의 정책으로 받는 변경이라 거부하지는 않고, `policy`와 `sign`이 그 사실을 알립니다.
- 직전 정책이 루트 키의 정책이면, 서명 키로 서명할 새 정책은 `--held`로 준 옛 정책을 가진 앱도 받는 것이어야 합니다(3. 비상 절차).
- 서명된 봉투가 16 KiB를 넘게 되면 거부합니다(앱이 그만큼만 읽음).

### 출처 확인

새 Windows·Android 패키지마다 `policy`가 확인합니다. 하나라도 틀리면 초안을 만들지 않습니다.

1. `release.json`의 실행이 빌드 저장소 `build.yml`의, 손으로 시작한, 성공한 실행인지(`gh api repos/<빌드 저장소>/actions/runs/<실행 번호>`).
2. 그 실행이 부른 워크플로가 공개 저장소의 `client-build.yml` 하나이고, `release.json`의 소스 커밋(40자리)과 같은 커밋인지.
3. 그 실행 커밋의 `build.yml`이 문서(`docs/ci.md`)의 모양 그대로인지: job 하나, `uses:`와 `source_ref`가 같은 커밋으로 고정, 자기 단계 없음, 모르는 입력 없음.
4. 그 커밋이 이 저장소의 태그 `client-<버전>`과 같은지.
5. **릴리스 디렉터리의 패키지와 `release.json`이 바로 그 실행의 산출물인지.** 도구가 `gh run download <실행 번호>`로 그 실행의 산출물을 임시 디렉터리(0700, 끝나면 지움)에 직접 받아, SHA-256이 같은 파일이 둘 다 있는지 봅니다. 하나라도 없으면 초안을 만들지 않습니다.

- 5번이 없으면, `release.json`에 정상 실행의 번호만 적어 둔 다른 빌드의 파일도 1~4번을 통과합니다. 빌드 저장소에 쓸 수 있는 사람은 다른 실행으로 그런 디렉터리를 만들 수 있습니다. `release.json`도 그 실행이 쓴 파일 그대로여야 하므로, 거기 적힌 플랫폼·버전·빌드 번호도 검토한 빌드가 쓴 값입니다.
- 산출물은 7일 뒤 지워집니다(`docs/ci.md`). 지워진 뒤에는 그 빌드로 새 패키지의 정책을 만들 수 없습니다. 버전을 올려 다시 빌드합니다. 이미 정책에 들어간 패키지는 다시 확인하지 않으므로, `min`만 올리는 정책 같은 것은 언제든 만들 수 있습니다.
- `gh`가 로그인되어 있어야 합니다(Actions 읽기 권한).
- `--attest`를 주면 `gh attestation verify`도 통과해야 합니다. 서명자를 부른 워크플로(`client-build.yml`)와 검토한 커밋(`--signer-digest`)으로 고정합니다. 요금제가 비공개 저장소의 출처 증명을 지원하고 `gh`가 그 명령을 가진 버전일 때만 쓸 수 있습니다(Ubuntu 패키지의 gh 2.45에는 `gh attestation`이 없습니다). 실제 GitHub에서는 아직 시험하지 못했습니다.

CI가 산출물 디렉터리마다 넣는 `release.json`:

```json
{"platform":"windows","version":"1.6.1","build":"1.6.1.41","file":"HJViewer.exe",
 "size":25400320,"sha256":"<소문자 hex 64자>","source_sha":"<공개 저장소 커밋 40자리>",
 "run_id":"<빌드 저장소 실행 번호>","workflow":".github/workflows/build.yml"}
```

- `platform`은 `windows`, `android`, `ios`. `build`는 Windows 파일 버전, Android versionCode, iOS 빌드 번호입니다.
- `build`와 `run_id`는 문자열이나 숫자 모두 됩니다. 모르는 필드는 무시합니다.
- 같은 디렉터리에 `SHA256SUMS`가 있으면 그 줄도 파일과 같아야 합니다.

### 서명

```sh
hjv-release sign --key FILE [--key FILE] --out FILE DRAFT
```

- 서명 전에 초안을 처음부터 다시 검사합니다. 초안에 적힌 설명은 믿지 않습니다.
- 초안은 에이전트가 만든 파일이므로, 초안에서 온 글은 터미널을 움직일 수 없게 보여 줍니다. 초안의 확인 기록(notes)은 `policy`가 쓰는 모양(한 줄, 한글과 ASCII, 300자, 20줄까지)이 아니면 초안째 거부하고, 제어 문자는 어디서 왔든 `\u{1b}`처럼 글자로 바꿔 씁니다. 확인 기록은 "초안에 적힌 것"으로 따로 표시합니다. `sign`이 다시 확인한 내용이 아닙니다.
- 아래 변경은 목록으로 보여 주고 `yes`를 직접 입력해야 진행합니다.
  - 서명 키 폐기, 세대 올리기, seq가 10 넘게 뛰는 것, 루트 키로 서명하는 정책.
  - 완화, 그리고 지금 강제하는 값보다 낮은 `min`으로 들어오는 새 항목.
  - **서버에 넣는 즉시 일어나는 일**: 유예 없이 `min`을 올림, 유예 없는 새 항목, 아직 남은 유예를 없앰, 이미 지난 유예 끝, 이미 지났거나 지금 시작하는 무인 설치(`--auto-after windows=+0d` 포함).
- 처음 정책(seq 1)의 유예 없는 항목은 묻지 않고 "알아 둘 것"에 적습니다.
- `policy`가 만들지 않는 초안은 무엇을 입력해도 서명하지 않습니다: 같은 버전인데 패키지가 다른 초안, 무인 설치 시작이 정책 발행 시각보다 이른 초안(고리 차례 없이 한꺼번에 설치하게 됨).
- 초안은 만든 때보다 늦게 서명합니다. 그래서 새 유예 끝과 새 무인 설치 시작이 **지금부터** 얼마나 남았는지 보여 줍니다. 그 시각이 이미 지났으면 위 목록에 넣어 `yes`를 묻습니다. 이럴 때는 `policy`로 초안을 다시 만드는 것이 맞습니다.
- 새 패키지는 파일 이름, 크기, SHA-256을 따로 보여 줍니다. 이 값을 빌드 실행 요약과 대조합니다(2. 릴리스마다의 4번).
- 보통의 정책은 서명 키로, 완화가 든 정책과 루트 선언은 루트 키로만 서명됩니다. 폐기와 함께 새 정책을 낼 때는 `--key <루트 키> --key <서명 키 2>`처럼 둘을 줍니다.
- 초안에 필요한 키(서명 키, 루트 키, 또는 둘)를 먼저 알려 줍니다. `--key`로 준 파일 수가 다르면 비밀번호를 묻기 전에 멈춥니다.
- 키 파일의 공개키가 `client/update-keys.env`의 같은 번호와 다르면 서명하지 않습니다.
- 비밀번호 입력 중에 Ctrl-C를 눌러도 터미널의 입력 표시는 원래대로 돌아옵니다.
- 비밀번호 입력 중에는 Ctrl-Z(멈춤)가 듣지 않습니다. 멈췄다가 `fg`로 돌아오면 셸이 입력 표시를 켜 둔 채라 비밀번호가 화면에 보일 수 있기 때문입니다. 이때 누른 Ctrl-Z는 글자로 들어가 그 입력은 틀린 비밀번호가 됩니다(다시 묻습니다). 다른 방법으로 멈췄다가 다시 시작해도 도구가 입력 표시를 다시 끕니다.
- 서명한 봉투는 앱의 규칙(직전 정책을 가진 앱이 받는지)으로 다시 확인한 뒤에만 파일로 씁니다. 출력에 파일의 SHA-256이 나옵니다. 서버에 넣은 뒤 `curl -s https://<서버>/api/hjv/version | sha256sum`과 같아야 합니다.

## 3. 비상 절차

| 상황 | 할 일 | 서명 |
|---|---|---|
| 잘못된 릴리스가 무인 설치되고 있음 | `policy --prev … --auto-after windows=none` | 서명 키 |
| 업데이트 경로가 오래 멈춤(앱이 막혔는데 받을 길이 없음) | `policy --prev … --relax --min all=<낮은 버전>`. 복구 뒤 아래 "루트 키의 정책 다음"대로 다시 올림 | 루트 키 |
| 서명 키 1이 새어 나감 | `policy --prev … --revoke-key 1 --new-epoch` → `sign --key <루트 키> --key <서명 키 2>`. 다음 빌드에는 키 2와 새 키 3을 넣음 | 루트 키 + 서명 키 2 |
| 새어 나간 키로 가짜 정책(예: `min` 99.0.0)이 이미 퍼짐 | 위와 같이 폐기하고, 정책은 `--relax`로 만들어 루트 키로 서명 | 루트 키 |

### 루트 키의 정책 다음

앱은 새 정책을 **자기가 저장한 정책**과 비교합니다. 서버는 가장 새 정책 하나만 내주므로, 루트 키의 정책(완화)이 서버에 있던 동안 꺼져 있던 앱은 그 정책을 보지 못하고 그 앞의 정책을 가진 채로 다음 정책을 받습니다. 그 앱에게는 루트 키가 풀어 준 것이 그대로 남아 있는 서명 키의 정책이 "서명 키의 완화"입니다. 앱은 그 정책을 버리고, 그 뒤에 오는 서명 키의 정책도 계속 버립니다. 정책에 실린 폐기 선언도 함께 버려지고, 무인 설치도 멈춥니다.

그래서 루트 키의 정책 바로 다음에 서명 키로 서명할 정책은 `--held`가 있어야 만들어집니다.

```sh
# 7번: 서명 키의 정책, 8번: 루트 키의 완화. 이제 9번으로 되돌림
hjv-release policy --prev build/policy/version-policy-8.json \
    --held build/policy/version-policy-7.json \
    --min all=1.6.0 --grace-days 0 --ios-valid <빌드> --out build/policy/draft-9.json
```

- `--held`에는 루트 키의 정책 **바로 앞의 정책**을 줍니다. 루트 키의 정책이 여럿 이어졌으면 그 사이의 것도 하나씩 줍니다. 도구는 그 정책을 가진 앱이 새 정책을 받는지 확인하고, 받지 않으면 이유를 말합니다.
- 완화 전의 `min`으로 되돌릴 때 유예 끝은 **완화 전의 기한보다 늦으면 안 됩니다**. 완화 전에 이미 강제하고 있었으면 `--grace-days 0`, 아직 유예 중이었으면 `--grace-until <그때의 기한>`입니다. 옵션을 주지 않을 때 붙는 기본 7일이 그 기한보다 늦으면 도구가 거부합니다.
- 아직 다 되돌릴 수 없으면 그 정책도 루트 키로 서명합니다(`--relax`). 루트 키의 정책은 앱이 무엇을 가지고 있든 받습니다.
- 한 번 이렇게 서명 키의 정책이 이어지면, 그 뒤의 정책에는 `--held`가 필요 없습니다.
- **새어 나간 키의 가짜 정책이 퍼졌던 경우**: 가짜 정책을 받은 뒤 꺼져 있던 앱은 가짜 정책과 비교합니다. 가짜 정책 파일을 가지고 있으면(예: `check --live`에 쓴 서버의 사본) 그것도 `--held`로 줍니다. 가짜 정책의 `min`이 높았다면 서명 키의 정책은 그 앱에게 완화이므로, 그런 앱이 남아 있을 수 있는 동안은 정책을 계속 루트 키로 서명합니다(`--relax`). 폐기 선언도 정책과 함께 받아들여지므로, 정책이 거부되는 앱은 폐기도 알지 못합니다.
- 이미 낸 뒤에 알았다면 `verify --prev <완화 전 정책> <새 정책>`으로 확인하고, 거부되면 루트 키로 서명한 정책을 한 번 더 냅니다.

## 4. 서버 설정 값

### hbbs의 최소 컨트롤러 버전 (`/etc/hjviewer/min-controller.conf`)

hbbs는 플랫폼을 모르므로 기준이 하나입니다.

```sh
hjv-release hbbs-conf --policy build/policy/version-policy-2.json \
    --prev-policy build/policy/version-policy-1.json --current none --door2-out ~/hjviewer-secrets/door2
```

- `MIN` = 정책의 모든 항목 `min` 중 가장 낮은 값.
- `FROM`은 `MIN`이 바뀔 때만 다시 정합니다. 새 `MIN`에 방금 이른 항목들의 유예 끝 중 가장 늦은 값입니다(유예가 없으면 정책 발행 시각).
- `--current`에는 지금 서버에 있는 파일의 사본을 줍니다(없으면 `none`). `MIN`이 그대로면 `hbbs 파일은 바꾸지 않음`이라고 답합니다.
- `FROM`은 뒤로 가지 않습니다. `MIN`을 올리는데 새 `FROM`이 지금 파일의 `FROM`보다 이르면, 그 시각이 아직 오지 않은 동안은 거부합니다. 이미 알린 시각보다 일찍 거절하지 않기 위해서입니다. 그 시각이 지난 뒤나 `MIN`을 내릴 때(루트 키의 완화)는 지금 파일의 `FROM`을 그대로 둔 파일을 만듭니다.
- `MIN`이 지금 파일보다 **내려가면 항상 경고**합니다. hbbs가 지금 거절하는 버전을 다시 받게 되기 때문입니다. 루트 키의 완화나, 더 낮은 `min`으로 들어온 새 항목에서 생깁니다.
- 문구는 `--door2-out`이면 문 2 링크가 든 S1, 문 2를 닫은 뒤에는 `--text s2 --out FILE`입니다. `--text-file`로 직접 쓴 문구는 옛 앱이 지켜야 하는 규칙(`version not allowed` 포함, URL 뒤 공백 등)을 검사합니다.

### 문 2 (`/etc/hjviewer/door2.conf`)

```sh
# 문 2 열기(M5): door2.conf만 만듦
hjv-release hbbs-conf --door2-out ~/hjviewer-secrets/door2 --door2-only --door2-until 2026-12-27
# 코드 바꾸기: --door2-rotate. 닫는 시각 바꾸기: --door2-until
```

- 디렉터리는 다른 사용자가 읽을 수 없어야 합니다(0700). 파일은 0600으로 씁니다.
- **코드는 두 파일에만 있습니다.** 도구는 코드를 화면에 쓰지 않습니다. 문서, 메모, 대화에 옮기지 않습니다.

### 빌드 하한

```sh
hjv-release floor --policy build/policy/version-policy-8.json     # HJV_POLICY_FLOOR=1:8
```

다음 빌드를 내기 전에 `client/update-keys.env`의 줄을 이 값으로 바꿉니다. 그 빌드는 이보다 오래된 정책을 받지 않습니다.

## 5. 주 1회 점검

```sh
curl -s https://rd.example.com/api/hjv/version -o /tmp/live.json
hjv-release check --policy build/policy/version-policy-8.json \
    --door2 ~/hjviewer-secrets/door2/door2.conf --live /tmp/live.json
```

경고가 있으면 종료 코드 3입니다.

- TestFlight 빌드가 올린 지 75일을 넘음(90일에 만료되면 모든 아이폰 앱이 실행되지 않음).
- 문 2의 `UNTIL`이 지났거나 7일 안에 닫힘.
- 서버가 내주는 정책이 이 PC의 정책과 다름(옛 정책, 더 새로운 정책, 같은 위치의 다른 내용).
- seq가 빌드 하한에서 1000에 가까워짐.
- 남은 1.5.x PC는 이 도구가 볼 수 없습니다. 서버의 `hjv-api admin report`와 운영자 PC 목록으로 확인합니다.

## 6. 시험용 명령과 벡터

- `hjv-release test-keys`: 이 저장소에 비밀키가 공개된 시험 키 목록. `--env`는 시험 빌드용 `update-keys.env` 내용입니다. 실서버 빌드는 이 키를 믿으면 안 됩니다.
- `hjv-release test-sign --policy FILE [--root-decl FILE] [--key NAME] [--out FILE]`: 손으로 쓴 정책 JSON을 **시험 키로만** 서명합니다(E2E와 다른 묶음의 시험용). 터미널이 필요 없습니다. 실제 키를 받는 옵션은 없습니다. `--raw`는 규칙을 어긴 봉투를 일부러 만들 때 씁니다.
- `tests/vectors/`: 앱이 받아야 하는 봉투와 거부해야 하는 봉투 93건(`manifest.json`). 클라이언트 코어와 서버 확인 스크립트가 같은 파일을 읽습니다. 설명은 `tests/vectors/README.md`.
- `tests/gh/`: 출처 확인 시험에 쓰는 기록된 `gh` 출력.

## 7. 파일 형식

**봉투**(서버가 내주는 파일, 한 줄):

```json
{"v":1,"k":1,"p":"<정책 JSON 텍스트>","s":"<서명>","root":{"k":101,"p":"<루트 선언 텍스트>","s":"<서명>"}}
```

- 정책 서명은 `"hjv-version-v1"` + 0 바이트 + `p`의 바이트, 루트 선언 서명은 `"hjv-version-root-v1"` + 0 바이트 + `root.p`의 바이트에 대한 Ed25519 서명입니다(표준 base64).
- `root`는 가장 최근의 루트 선언(세대와 폐기한 서명 키 목록)이고, 한번 생기면 이후 모든 봉투에 그대로 붙습니다.
- 정책 텍스트의 필드와 규칙은 `src/policy.rs`, 앱이 받는 규칙은 `src/trust.rs`에 있습니다.

**키 파일**: age 형식(age-encryption.org/v1)의 비밀번호 암호화 파일입니다(scrypt 작업 계수 18). 이 도구가 없어도 `age -d`로 열 수 있습니다. 안에는 역할, 번호, 32바이트 seed, 공개키가 들어 있습니다.

**초안**: `policy`가 만들고 `sign`이 읽는 JSON입니다. 서명할 텍스트 그대로와 직전 봉투, `policy`가 확인한 것을 적은 줄(notes)이 들어 있고 서명은 없습니다. notes는 한 줄에 300자까지, 20줄까지이고 한글과 ASCII 글자만 씁니다(공백은 하나씩). `sign`은 그 밖의 초안을 읽지 않습니다.

**게시 디렉터리**: `publish-dir`는 링크를 따라 쓰지 않습니다. `--out` 안에 링크나 보통 파일이 아닌 것이 있으면 아무것도 쓰기 전에 멈춥니다.
