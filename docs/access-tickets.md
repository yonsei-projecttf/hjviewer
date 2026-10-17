# 출입증과 access.conf (교직원 전용, 1.8.0)

1.8.0부터 HJViewer는 연세대학교 교직원용입니다. 연세 SSO로 로그인한 사람은 지금처럼 쓰고, 로그인하지 않은 사람은 운영자가 정한 **이용 비밀번호**를 알 때만 그때그때 씁니다. 이것을 서버에서 지키는 것이 **출입증**입니다. hjv-api가 출입증에 서명하고, hbbs(서버 패치 0006)가 연결 요청, 중계 요청, PC 등록에서 출입증을 오프라인으로 확인하고, 앱은 출입증을 읽지 않고 싣기만 합니다.

이 문서는 세 쪽을 따로 만들어도 서로 맞도록 출입증의 형식, hbbs의 판정, hbbs가 읽는 `/etc/hjviewer/access.conf`, 그리고 앱이 hjv-api에서 출입증을 받는 요청과 답(3-4, 3-5)을 정합니다. 세 쪽은 같은 시험 벡터 [server/api/tests/vectors/access-tickets.json](../server/api/tests/vectors/access-tickets.json)으로 맞춥니다(8절). 설계의 까닭과 다른 방안은 [설계 요약](staff-only/brief.md)(2026-10-16)에 있습니다. 이 문서가 하나뿐인 계약입니다. hbbs 0006, hjv-api, 앱 1.8.0과 다른 문서(server/api/README.md 등)가 이 문서와 다르면 그쪽을 고칩니다.

## 1. 한눈에 보기

**사용자가 정한 것**(2026-10-16)

- 컨트롤러와 hbbs에 등록하는 PC를 모두 확인합니다.
- 이용 비밀번호는 운영자가 정하는 공용 하나입니다. 서버 파일에 해시로 두고 바꿀 수 있습니다. 화면과 문서에서는 "이용 비밀번호"(영어 "access password")라고 부르고 "손님 비밀번호"라고 쓰지 않습니다. PC 화면의 "일회용 비밀번호"와는 다른 것입니다. 코드 안의 이름(`guest`)은 그대로 둡니다.
- 앱은 이용 비밀번호를 기억하지 않습니다. 컨트롤러는 연결할 때마다, PC는 앱을 켤 때마다 넣습니다. 한 번 넣으면 그때 연결하는 PC 하나에만 닿습니다.
- 1.8.0을 내면서 유예 없이 강제합니다.
- App Store와 Play의 심사자는 심사 계정으로 로그인해 `review.conf`의 새 `PC=` 줄에 적힌 데모 PC에만 연결합니다. 그 파일의 `UNTIL`까지입니다. 심사자에게 이용 비밀번호는 주지 않습니다.
- 교직원이 만드는 일회용 코드는 1.8.x에서 더합니다.

**따로 정하지 않는 동안의 기본값**: 출입증 수명은 교직원 24시간, 묶인 PC 72시간, 이용 비밀번호 12시간입니다. hjv-api가 멈춰도 교직원은 받아 둔 출입증이 남은 동안 연결합니다. 1.8.0은 직접 IP 접속, LAN 검색, 안전하지 않은 TLS 대체를 끈 채로 고정합니다(3-3). hbbr은 아무것도 확인하지 않습니다.

| 쪽 | 하는 일 | 가진 것 |
|---|---|---|
| hjv-api | 로그인한 기기, 묶인 PC, 이용 비밀번호를 맞힌 기기에 출입증을 만들어 서명 | 출입증 키(Ed25519, 자격 증명 `hjviewer-api-ticket-key`), 이용 비밀번호의 해시(`guest.conf`) |
| hbbs | 연결 요청, 중계 요청, 등록에서 출입증을 확인 | `access.conf`의 공개키 한두 개뿐. 비밀은 없음 |
| 앱 | 받은 출입증을 그대로 실어 보냄 | 출입증 문자열. 키가 없고, 출입증을 읽지 않음 |

- 출입증은 수명 동안 여러 번 쓰는 무기명 값입니다. 연결 한 번이 같은 값을 연결 요청으로 세 번, 중계 요청으로 세 번 보내고, 다시 연결과 포트 포워딩의 로컬 연결마다 또 보내기 때문입니다.
- 출입증은 서버를 쓸 자격일 뿐입니다. PC 안으로 들어가는 것은 지금처럼 PC 비밀번호, 수락 창, 같은 계정의 증명이 정합니다.

## 2. 출입증

### 2-1. 전송 형태

```text
t1.<kid>.<본문>.<서명>
```

- 네 부분을 `.`으로 잇습니다. 출입증에 나오는 글자는 ASCII의 `A-Z`, `a-z`, `0-9`, `-`, `_`, `.`뿐입니다.
- `t1`: 형식 1입니다. 소문자 그대로이고, 다른 값은 모두 형식 오류입니다.
- `<kid>`: 서명한 출입증 키의 번호입니다. 1~65535의 10진수이고 앞에 0이 없습니다(`0`, `01`, `65536`은 형식 오류).
- `<본문>`: 본문 바이트(2-3)의 base64url입니다(RFC 4648 5절의 글자, 패딩 `=` 없음). 비어 있으면 안 됩니다.
- `<서명>`: 64바이트 Ed25519 서명의 base64url, 정확히 86자입니다.
- base64url은 엄격하게 읽습니다. 그 64글자 밖의 글자(`+`, `/`, `=`, 공백, 줄바꿈)가 하나라도 있거나, 길이를 4로 나눈 나머지가 1이거나, 디코드한 바이트를 다시 인코드한 글자가 원래와 다르면(마지막 글자의 남는 비트가 0이 아님) 형식 오류입니다. 출입증 앞뒤의 공백도 자르지 않습니다.
- **크기**: 출입증 전체가 1024바이트 이하입니다. 시각이 10자리인 동안(2286년까지) 규칙에 맞는 가장 긴 출입증은 515바이트(대상 ID 64자 셋, 벡터 `longest`)이고, 보통은 240~320바이트입니다.

### 2-2. 서명하는 바이트

```text
"hjv-ticket-v1" NUL 본문
```

- 13바이트 `hjv-ticket-v1`, 0x00 한 바이트, 그리고 본문 바이트입니다. 본문 바이트는 base64url을 디코드한 바이트이고, 글자(base64url 텍스트)가 아닙니다. 용도 문자열이 [architecture.md](architecture.md) 5-2의 다른 서명과 달라서 섞이지 않습니다.
- 서명은 Ed25519(RFC 8032, prehash 없음)입니다. hjv-api는 ed25519-dalek으로 서명하고, hbbs는 libsodium(sodiumoxide의 `sign::verify_detached`)으로 확인합니다. libsodium은 S가 군의 위수 이상인 서명과 작은 위수의 R이나 공개키를 거절합니다.
- hbbs는 `kid`가 가리키는 키 하나로만 확인합니다. 다른 `KEY` 줄의 키로 다시 해 보지 않습니다.
- 서명을 먼저 확인하고, 그 뒤에야 본문을 해석합니다. 해석한 값을 다시 직렬화해서 서명을 확인하지 않습니다(2-3의 정규형 비교는 서명 확인 뒤의 일입니다).

### 2-3. 본문

UTF-8 JSON 객체 하나입니다. 아홉 멤버가 이 순서로 모두 있고, 공백이 없습니다. 벡터 `staff-both`의 본문입니다.

```json
{"v":1,"who":"staff","use":"both","sub":"598891f1a14389ce","id":"123456789","pk":"gTl3Dqh9F19Wo1Rmw0x+zMuNipG07jeiXfYPW4/Js5Q=","to":[],"iat":1760000000,"exp":1760086400}
```

| 멤버 | JSON 형 | 값 |
|---|---|---|
| `v` | 정수 | `1` |
| `who` | 문자열 | `staff`(명단에 있고 막히지 않은 교직원 계정), `review`(App Review 계정), `guest`(이용 비밀번호) |
| `use` | 문자열 | `ctl`(연결 요청과 중계 요청), `pc`(등록), `both`(셋 다). `who`마다 받는 값은 2-4 |
| `sub` | 문자열 | 계정 꼬리표: hjv-api 기록 키의 HMAC 앞 64비트를 소문자 16진수 16자로 쓴 것(`Tagger::account`, 포탈 ID가 아님). `staff`와 `review`는 그 계정의 꼬리표, 묶인 PC는 묶은 계정의 꼬리표, `guest`는 빈 문자열. hbbs는 형식(빈 문자열이나 소문자 16진수 16자)만 봅니다 |
| `id` | 문자열 | `use`가 `pc`나 `both`면 그 PC의 ID(영문자, 숫자, `-`, `_`로 된 1~64자, hjv-api의 `valid_device_id`). `ctl`이면 빈 문자열 |
| `pk` | 문자열 | `use`가 `pc`나 `both`면 그 PC의 기기 공개키 32바이트를 표준 base64(패딩 포함 44자, 정규형)로 쓴 것. `ctl`이면 빈 문자열 |
| `to` | 문자열 배열 | 연결해도 되는 PC의 ID. 빈 배열이면 아무 PC. 개수는 2-4. 항목은 `id`와 같은 형식이고 서로 다름 |
| `iat` | 정수 | 만든 시각(유닉스 초, hjv-api의 시계) |
| `exp` | 정수 | 끝나는 시각(유닉스 초). `iat < exp ≤ iat + 604800`(7일) |

- 멤버는 늘 아홉 개 모두 있습니다. 쓰지 않는 문자열은 빈 문자열, 쓰지 않는 `to`는 빈 배열입니다.
- 문자열 값에는 이스케이프 없이 쓸 수 있는 글자만 나옵니다(위 형식이 그렇게 정함). 정수는 0 이상이고, 부호, 소수점, 지수, 앞의 0 없이 씁니다.
- **정규형**: 본문 바이트는 그 값들을 위 순서와 형으로 다시 쓴 JSON과 바이트까지 같아야 합니다. 그래서 멤버 순서가 다르거나, 같은 멤버가 두 번 나오거나, 모르는 멤버가 있거나 빠졌거나, 공백, `\u` 이스케이프, `1.0`, `1e9`, `null`, `true`가 있으면 형식 오류입니다. Rust에서는 `#[serde(deny_unknown_fields)]` 구조체(정수는 `u64`)로 읽고 `serde_json::to_vec`으로 다시 써서 비교하면 됩니다. 벡터의 본문 80개에서 이 방법의 정규형 판정이 생성 스크립트와 같은 것을 확인했습니다. 값의 규칙(위 표와 2-4)은 그 뒤에 따로 봅니다.

### 2-4. 종류

`who`와 `use`의 조합은 여섯 가지만 받습니다.

| 출입증 | 받는 곳 | `who` / `use` | `id`, `pk` | `to` | 수명 |
|---|---|---|---|---|---|
| 교직원 휴대폰 | `POST /api/hjv/ticket`, 로그인 응답 | `staff` / `ctl` | 빈 값 | `[]` | 24시간 |
| 교직원 Windows 앱(설치하지 않은 실행본 포함) | `POST /api/hjv/ticket`에 기기 서명과 함께. 설치된 PC는 로그인 응답으로도(아래) | `staff` / `both` | 그 앱의 ID와 기기 키 | `[]` | 24시간 |
| 묶인 PC | PC 동기화 응답(`POST /api/hjv/pc/sync`) | `staff` / `both` | 묶인 ID와 동기화 요청을 확인한 키 | `[]` | 72시간 |
| 교직원, 등록만 | 지금은 주지 않음 | `staff` / `pc` | 그 PC의 ID와 키 | `[]` | - |
| 이용 비밀번호, 컨트롤러 | `POST /api/hjv/guest`(3-5) | `guest` / `ctl` | 빈 값 | 대상 ID 정확히 하나 | 12시간 |
| 이용 비밀번호, PC | `POST /api/hjv/guest`(3-5). 기기 서명과 함께 | `guest` / `pc` | 그 PC의 ID와 키 | `[]` | 12시간 |
| 심사 계정 | `POST /api/hjv/ticket`, 로그인 응답 | `review` / `ctl` | 빈 값 | `review.conf`의 `PC=` ID 1~3개 | 24시간, `UNTIL`을 넘지 않음 |

- 이 밖의 조합(`guest`의 `both`, `review`의 `pc`와 `both`)과, 표와 다른 `to`의 개수(교직원의 대상, 대상 없는 `guest`의 `ctl`, 대상 둘 이상인 `guest`의 `ctl`, 대상 없거나 넷 이상인 `review`)는 형식 오류입니다. 그래서 hjv-api에 실수가 있어도 이용 비밀번호나 심사 계정의 출입증이 아무 PC에나 닿지 않습니다.
- hjv-api가 지킬 것
  - 수명은 설정으로 바꿀 수 있지만 7일을 넘기면 hjv-api가 시작하지 않습니다. hbbs가 그런 출입증을 형식 오류로 거절하기 때문입니다.
  - 심사 계정의 `exp`는 `iat + 24시간`과 `UNTIL` 가운데 이른 쪽입니다. `UNTIL`이 지났거나 `PC=` 줄이 없으면 출입증을 주지 않습니다. `to`는 `PC=` 줄의 ID를 적힌 순서대로 넣습니다. `PC=` 줄은 `review.conf`에 한 번, 서로 다른 PC ID 1~3개를 쉼표로 나눠 씁니다(`PC=100200300,100200301`). 그 밖의 모양이면 파일 전체를 쓰지 않습니다(심사 계정이 없어짐).
  - `pc`와 `both`의 `id`와 `pk`는 hjv-api가 확인한 값입니다. Windows 앱과 이용 비밀번호의 PC는 기기 서명으로 확인하고, 묶인 PC는 동기화 요청의 서명을 확인한 키를 넣습니다. 이용 비밀번호의 PC는 hbbs가 그 ID를 알면 같은 키인지도 봅니다(`hjv-pk`).
  - 로그인 응답의 출입증은 설치된 PC 가운데 로그인 서명(`hjv.sig`)이 hbbs가 가진 그 ID의 키와 맞는 것에만 `both`이고, 그 밖의 기기에는 모두 `ctl`입니다. 로그인 본문에는 기기 공개키가 없어서, 로그인에서 기기 키를 확인하는 길은 PC를 계정에 묶을 때의 그 확인뿐이기 때문입니다. 그래서 설치하지 않은 실행본, hbbs가 아직 모르는 ID, 그 확인이 안 된 PC는 로그인 응답에서 `ctl`을 받고, `both`는 기기 서명과 함께 보내는 `POST /api/hjv/ticket`으로 받습니다(3-3).
  - 출입증을 줄 때 `exp`와 서버 시각을 답에 따로 넣습니다. 앱이 출입증을 읽지 않고도 언제 새로 받을지 알게 하기 위해서입니다. 답의 모양은 3-4입니다.
- **화면 바꾸기**는 PC가 컨트롤러가 되어 연결 요청을 보내는 것이라 `ctl`이나 `both`가 필요합니다. 묶인 PC(`both`)는 되고, 이용 비밀번호의 PC(`pc`)는 안 됩니다.

### 2-5. 키

- 출입증 키는 `hjv-api keygen ticket --kid N`으로 만듭니다(명령 키와 같은 방식, [server-runbook.md](server-runbook.md) 7-4). 화면에는 공개키가 `N:<표준 base64>` 한 줄로 나오고, 그 줄이 그대로 `access.conf`의 `KEY=` 값입니다.
- 비밀키는 hjv-api의 자격 증명에만 있습니다. hbbs는 공개키만 가지므로 확인만 하고 출입증을 만들지 못합니다. hjv-api는 hbbs의 파일에 손대지 못합니다.
- 앱 빌드에는 출입증 키가 없습니다. 그래서 키를 바꾸거나 형식을 고쳐도 앱을 다시 빌드하지 않고, hjv-api와 hbbs만 함께 바꿉니다. 형식을 바꿀 때는 `t2`와 `hjv-ticket-v2`로 올립니다.
- 키 바꾸기는 6-4에 있습니다.

## 3. 싣는 곳

### 3-1. 세 자리

| 메시지 | 필드 | 보내는 쪽 | 싣는 출입증 |
|---|---|---|---|
| hbbs로 가는 `PunchHoleRequest` | `token`(5) | 컨트롤러 | `ctl`, `both` |
| hbbs로 가는 `RequestRelay` | `token`(8) | 컨트롤러 | `ctl`, `both` |
| `RegisterPk` | `hjv_ticket`(1001, 새 필드) | PC | `pc`, `both` |

- 1.7.0 이하의 HJViewer는 `token`을 비워 보냅니다(고정 서버 빌드의 `rendezvous_token`, 앱의 `src/hjviewer.rs`). 1.8.0은 그 자리에 대상에 맞는 출입증을 넣습니다. 로그인한 앱은 계정의 출입증 하나(`staff`나 `review`)를, 로그인하지 않은 앱은 그 대상의 이용 비밀번호 출입증을 넣습니다. 없으면 비워 보냅니다.
- hbbs는 `RequestRelay`를 PC로 넘기기 전에 `token`을 지웁니다(패치 0004 그대로). 그래서 출입증은 PC에 닿지 않습니다. hbbs가 PC에 보내는 `PunchHole`과 `FetchLocalAddr`에도 실리지 않습니다.
- hbbr로 가는 `RequestRelay`에는 싣지 않습니다(hbbr은 확인하지 않음). `LoginRequest`처럼 PC로 가는 메시지에도 싣지 않습니다.

### 3-2. RegisterPk의 필드 1001

- **앱**: `libs/hbb_common/protos/rendezvous.proto`의 `RegisterPk` 끝에 `string hjv_ticket = 1001;`을 더합니다(클라이언트 hbb_common 패치 0003). 1000~1099는 HJViewer의 번호입니다(클라이언트 패치 0030과 같음). 빈 문자열은 아무 바이트도 쓰지 않으므로, 출입증 없는 등록은 바이트까지 지금과 같습니다.
- **hbbs**: proto를 바꾸지 않고 모르는 필드로 읽습니다. protobuf 3.7.2는 모르는 필드를 지니고 있습니다. 읽는 규칙은 `special_fields.unknown_fields().get(1001)`의 결과와 같습니다.
  - 필드가 없으면 출입증 없음(`missing`)입니다.
  - 길이 구분형(wire type 2)으로만 있으면 마지막 값입니다(protobuf가 문자열을 읽는 방식). 그 값이 비었으면 출입증 없음입니다.
  - 다른 wire type으로 한 번이라도 있으면 형식 오류(`malformed`)입니다.
  - 값은 바이트 그대로 4-1의 판정으로 갑니다. 1024바이트를 넘으면 `oversize`, ASCII가 아니면 `malformed`입니다.
  - 기준은 `get(1001)`의 결과입니다. protobuf 3.7.2는 group 형(wire type 3, 4)인 필드를 저장하지 않고 건너뛰므로, 실제로 형식 오류가 되는 것은 varint와 고정 길이(wire type 0, 1, 5)입니다. group과 길이 구분형이 함께 있으면 길이 구분형의 값으로 읽습니다. 그 값도 서명을 확인하므로 문제가 없습니다.
- 벡터의 `register_pk` 일곱 개가 바이트 그대로의 예입니다. 첫 사례 `ticket`은 1.8.0의 생성 코드가 쓰는 바이트입니다. 두 쪽 모두 rust-protobuf 3.7.2로 확인했습니다. 필드 1001을 마지막에 선언한 `RegisterPk`가 그 바이트를 쓰고, 일곱 개를 `get(1001)`로 읽으면 위 규칙대로 나옵니다.

### 3-3. 앱이 지킬 것

- 출입증을 읽지도, 고치지도, 검사하지도 않습니다. hjv-api에서 받은 문자열을 그대로 싣습니다. 언제 새로 받을지는 답의 `exp`와 `server_time`(3-4)으로 압니다.
- **로그인의 출입증**: 로그인 답의 `hjv.access`로 받고, 수명이 반쯤 지나면 `POST /api/hjv/ticket`으로 새로 받습니다. 이 요청은 모두 합쳐 10초에 한 번까지이고, 실패가 이어지면 30초부터 두 배씩 30분까지 늘립니다. 로그인한 앱이 연결하려는데 쓸 수 있는 출입증이 없으면 연결 전에 5초까지 받아 봅니다.
- **답의 `use`**: 받을 때와 새로 받을 때는 답의 `use`를 봅니다. Windows 앱(기기 키가 있는 데스크톱 앱)은 가진 출입증이 `both`가 아니면(로그인 답의 `ctl`, `access`가 없던 로그인) 로그인한 직후 PC 부분과 함께 `POST /api/hjv/ticket`을 보내 `both`를 받고, 새로 받을 때도 늘 PC 부분과 함께 보냅니다. `both`가 있어야 등록할 수 있기 때문입니다(2-4). 휴대폰은 PC 부분 없이 보냅니다. 요청으로 받은 출입증은 무엇이든 수명이 반쯤 지날 때까지 다시 묻지 않습니다. 심사 계정의 `ctl`처럼 다시 물어도 같은 것이 오기 때문입니다.
- **고르기**: 컨트롤러는 그 대상의 이용 비밀번호 출입증을, 없으면 로그인의 출입증을 싣습니다. PC는 묶인 PC의 출입증, 이용 비밀번호의 PC 출입증, 로그인의 출입증(설치하지 않은 실행본) 가운데 `pc`나 `both`이고 아직 쓸 수 있는 첫 것을 싣습니다.
- 다시 연결할 때마다 그때의 출입증을 다시 읽습니다. 포트 포워딩의 로컬 연결도 마찬가지입니다. 1.8.0은 모든 연결이 지나는 `Client::start` 한 곳에서 출입증을 넣습니다.
- **거절 뒤**: 연결 요청이나 중계 요청이 출입 표시(5-1)로 거절되면, 로그인한 앱은 출입증을 한 번 새로 받아 다시 해 보고, 그래도 거절되면 A2를 보입니다. 로그인하지 않은 앱은 A1의 창(5-3)을 엽니다.
- **묶인 PC**: 동기화 답에 `access`가 오면 그것으로 바꿉니다. `access`가 없는 답(hjv-api에 출입증 키가 없음, 옛 hjv-api로 되돌림, 막힌 계정)은 가진 출입증을 `exp`까지 그대로 둡니다. 서명된 `revoked`와 묶음 풀기만 출입증을 버립니다.
- 새 출입증을 받지 못하면(hjv-api가 멈춤) 가진 출입증을 `exp`까지 그대로 씁니다.
- **이용 비밀번호**(3-5): 앱은 이용 비밀번호를 기억하지 않습니다. 넣은 글의 앞뒤 공백을 지우고 봉인해 보낸 뒤 버리고, 받은 출입증은 그 프로세스의 메모리에만 둡니다(파일과 설정에 쓰지 않음).
  - 컨트롤러의 출입증은 그 대상 ID에 둡니다. 그 프로세스에서 그 대상으로 가는 마지막 세션이 닫히면 버립니다. 그래서 새 연결은 다시 묻고, 같은 세션의 자동 다시 연결과 포트 포워딩의 로컬 연결은 묻지 않습니다.
  - PC의 출입증은 화면이 받아 IPC 메시지로 등록하는 프로세스(설치한 PC는 서비스가 띄운 서버 프로세스)에 넘깁니다. 그 프로세스가 끝나거나 12시간이 지나면 끝납니다.
  - 409 `clock`을 받으면 그 답의 `server_time`으로 시계를 맞춰 한 번 다시 보냅니다(로그인과 같음). 그래도 409이면 E5입니다.
- **등록**(4-4):
  - `NOT_DEPLOYED`를 받은 연결은 hbbs가 곧 끊는다고 봅니다. hbbs는 받아들이지 않은 연결에 하트비트를 보내지 않고, 30초 동안 프레임이 없으면 닫고, 빈 프레임을 받으면 바로 닫습니다. 그런 연결에 빈 프레임을 보내지 않고, 끊기면 다시 연결합니다.
  - 출입증이 오면 30초를 기다리지 않고 그때의 연결에서 출입증을 실은 `RegisterPk`를 보냅니다.
  - 받아들여진 연결에서는 출입증의 수명이 반쯤 지나면 새 출입증으로 다시 등록합니다. 그 등록이 `NOT_DEPLOYED`나 `TOO_FREQUENT`를 받아도 앞의 출입증이 `exp`까지 그대로이고 연결도 받아들여진 채이므로, `exp`보다 넉넉히 앞서 다시 해 봅니다.
  - 하트비트에서 닫히면(아무 메시지 없이 닫힘) 새 연결에서 그때의 출입증으로 다시 등록합니다.
  - 출입증으로 등록했는데 가진 출입증이 없어지면(로그아웃, 묶음이 풀림, 수명 끝) 연결을 새로 맺고 출입증 없이 등록합니다. 출입증 없는 `RegisterPk`는 hbbs가 받아들인 등록을 끝내지 않기 때문입니다.
- **고정 서버 빌드**는 '직접 IP 접속'(`direct-server`), 'LAN 검색'(`enable-lan-discovery`), '안전하지 않은 TLS 대체 허용'(`allow-insecure-tls-fallback`)을 끈 채로 고정합니다. 출입증은 hbbs로 가는 wss 연결에 실리므로, 인증서를 확인하지 않는 대체를 허용하면 가로챈 쪽이 출입증을 `exp`까지 다시 쓸 수 있기 때문입니다.
- 출입증과 이용 비밀번호를 로그에 쓰지 않습니다. 이용 비밀번호는 봉인한 뒤 바로 지웁니다.
- **알아 둘 것**: upstream 코드는 `token`이 비어 있지 않으면 연결 요청에서 `src/client.rs`의 `legacy_secure`가 참이 되어 UDP NAT 시험을 기다리지 않습니다. 고정 서버 빌드는 늘 WebSocket을 쓰므로(`allow-websocket=Y`) NAT 시험을 하지 않고 `secure_tcp`도 곧바로 돌아옵니다. 그래서 출입증을 실어도 연결 방식이 바뀌지 않습니다.

### 3-4. hjv-api의 답과 POST /api/hjv/ticket

**출입증 객체**: hjv-api는 출입증을 늘 같은 모양으로 줍니다.

```json
{"ticket":"t1.<kid>.<본문>.<서명>","use":"ctl","exp":1760086400,"server_time":1760000000}
```

- `ticket`: 앱이 그대로 싣는 문자열입니다. 앱은 읽지 않습니다.
- `use`: hbbs가 받는 자리(`ctl`, `pc`, `both`)입니다. 앱은 이것으로 등록에 쓸 수 있는지 압니다.
- `exp`: hbbs가 받지 않기 시작하는 때(유닉스 초), `server_time`: 답을 만든 때의 서버 시계입니다. 앱은 받은 때부터 `exp − server_time`을 세어 언제 새로 받을지 정합니다.
- 오는 곳: 로그인 답의 `hjv.access`, `POST /api/hjv/ticket`의 200 답 전체, 묶인 PC 동기화 답(`POST /api/hjv/pc/sync`)의 `access`(`list` 옆), `POST /api/hjv/guest`의 200 답 전체(3-5)입니다. 답마다 새 출입증입니다.

**POST /api/hjv/ticket**: 로그인한 앱의 토큰과 `X-HJV-Proof`로 보냅니다. 계정 키의 다른 요청과 함께 분당 30번까지입니다.

```text
본문 없음, 또는 {}                                                     휴대폰: staff/ctl
{"id":"<PC ID>","pk":"<기기 공개키, 표준 base64>","sig":"<표준 base64>"}   Windows 앱: staff/both
sig가 서명하는 바이트: "hjv-ticket-pc-v1" NUL id NUL pk NUL base64(SHA-256(토큰))
```

- `id`, `pk`, `sig`는 셋 다 있거나 셋 다 없습니다. 문자열만 담은 객체여야 하고, 다른 멤버, 같은 멤버 두 번, `null`, 배열은 400입니다.
- `sig`는 기기 키의 Ed25519 서명입니다. `pk`는 보낸 글자 그대로, 토큰은 그 요청의 bearer 토큰입니다. 그래서 서명 하나는 그 로그인에만 씁니다. `id`는 로그인한 기기의 ID와 달라도 됩니다(실행 중에 ID가 바뀔 수 있음). hbbs가 그 ID와 그 키의 등록에만 이 출입증을 받기 때문입니다.
- 심사 계정은 `review`/`ctl`을 받습니다(2-4). 데모 PC가 없으면 403입니다.
- **거절**: 본문은 `{"error":"<한국어 글>","hjv":"<코드>"}`입니다. 400 `bad_request`(본문의 모양), 403 `no_ticket`(출입증을 받지 않는 계정: `PC=` 줄이 없거나 `UNTIL`이 지난 심사 계정), 429 `rate_limited`, 503 `unavailable`(hjv-api에 출입증 키가 없음). 계정 요청의 401과 409도 그대로입니다. 같은 요청을 `HJV_API_RETRY_WINDOW`(30초) 안에 다시 보내면 첫 답(같은 출입증)이 옵니다.
- 막힌 계정과 로그아웃한 앱은 새 출입증을 받지 못하지만, 이미 받은 출입증은 `exp`까지 hbbs가 받습니다. 바로 끊으려면 `NOT_BEFORE`(6-4)를 씁니다.

### 3-5. 이용 비밀번호: POST /api/hjv/guest

로그인하지 않은 앱은 이용 비밀번호를 넣고 이 요청으로 출입증 하나를 받습니다. 토큰도 `X-HJV-Proof`도 없고, 로그인처럼 `Content-Type: application/json`, 64 KiB 이하, `Origin` 헤더 없음입니다.

```text
컨트롤러: {"use":"ctl","to":"<대상 PC ID>","password":"hjv-sealed-v1.<kid>.<봉인>"}
PC:       {"use":"pc","id":"<PC ID>","pk":"<기기 공개키, 표준 base64>","sig":"<표준 base64>","password":"hjv-sealed-v1.<kid>.<봉인>"}
sig가 서명하는 바이트: "hjv-guest-pc-v1" NUL id NUL pk NUL base64(SHA-256(봉인 바이트))
```

- **멤버**: 위의 멤버가 모두 있고 그 밖의 것은 없습니다. 모두 문자열입니다. `to`와 `id`는 2-3의 ID 형식, `pk`는 32바이트의 표준 base64(정규형)입니다. 어긋나면 400 `bad_request`입니다.
- **봉인**: 로그인의 봉인(`hjv-sealed-v1`, server/api/src/seal.rs)과 같은 sealed box에 이 JSON을 담습니다.

  ```json
  {"v":1,"p":"guest","use":"ctl","pw":"<이용 비밀번호>","t":1760000000,"n":"<16~64바이트의 표준 base64>","id":"<대상 PC ID>","kid":"<kid>"}
  ```

  - `use`는 바깥의 `use`와 같습니다. `id`는 출입증이 닿는 PC입니다: `ctl`이면 바깥의 `to`, `pc`이면 바깥의 `id`. 그래서 봉인 하나는 그 PC 하나의 그 용도에만 씁니다.
  - 용도 칸 `p`가 `guest`가 아닌 봉인은 여기서 받지 않고, `p`가 있는 봉인은 로그인에서 받지 않습니다. 그래서 포탈 비밀번호의 봉인과 이용 비밀번호의 봉인을 서로 바꿔 쓰지 못합니다.
  - `pw`는 1~256바이트입니다. 앱은 앞뒤 공백을 지우고 보내므로, `deploy/guest-conf.py`는 공백으로 시작하거나 끝나는 비밀번호를 받지 않습니다. `t`, `n`, `kid`는 로그인과 같습니다: `t`가 서버 시계와 10분 넘게 다르면 409 `clock`(`server_time`과 함께), 한 번 쓴 `n`은 400, 같은 본문을 30초 안에 다시 보내면 첫 답이 옵니다.
- **PC의 서명**: `sig`는 그 PC 기기 키의 Ed25519 서명입니다. 봉인 바이트는 `<봉인>`을 base64url로 푼 바이트입니다(로그인 서명의 `h`와 같음). 그래서 PC의 키가 그 봉인 하나에 묶입니다. hbbs가 그 ID의 키를 알면 같은 키여야 합니다(`hjv-pk`). hjv-api는 PC마다 분당 한 번만 hbbs에 묻고, 그동안은 그 답을 씁니다.
- **답**: 200은 출입증 객체(3-4)입니다. `ctl`은 `guest`/`ctl`에 `to`가 그 대상 하나, `pc`는 `guest`/`pc`에 그 PC의 ID와 키입니다. 수명은 12시간입니다. 대상이 있는지, 켜져 있는지는 보지 않고 답하지도 않습니다.
- **확인 순서**: 본문의 모양(400, 세지 않음), 출입증 키(없으면 503) → 주소마다 실패 한도(429) → 봉인(400, 409, 주소의 실패로 셈) → 주소마다 출입증 한도와 모든 시도의 한도(429) → 동시에 해시 2개까지(넘으면 503) → PBKDF2로 비밀번호 확인 → `pc`면 hbbs가 가진 키 → 서명.
  - 한도가 해시보다 먼저인 것은 hjv-api가 CPU 하나와 blocking 스레드를 모든 DB 작업과 나눠 쓰기 때문입니다.
  - 봉인이 모든 시도의 한도보다 먼저인 것은, 봉인이 틀린 요청이 모두의 한도를 쓰지 못하게 하기 위해서입니다. 그런 요청은 그 주소의 실패로만 셉니다.
- **같은 401**: 틀린 비밀번호와 이용 비밀번호를 받지 않는 상태(`guest.conf`가 없음, 고장 남, `UNTIL`이 지남)는 같은 401 `denied`를 줍니다. 모든 실패는 로그인처럼 요청이 온 때부터 2초 ± 0.5초 뒤에 나갑니다(`HJV_API_FAILURE_DELAY_MS`, `HJV_API_FAILURE_JITTER_MS`).
- **그 밖의 거절**: 400 `bad_request`(본문의 모양, 봉인), 409 `clock`, 403 `wrong_pc`(hbbs가 그 ID에 다른 키를 가짐), 429 `rate_limited`(한도), 503 `busy`(해시가 이미 2개 돌고 있음, 또는 받아 둔 시도가 너무 많음), 503 `unavailable`(출입증 키가 없음, hbbs에 물을 수 없음). 본문은 3-4와 같은 모양입니다. `wrong_pc`와 hbbs의 503은 비밀번호가 맞은 뒤에만 옵니다.
- **429의 `retry_after`**: 429의 본문에는 `retry_after`가 더 있습니다. 그 한도에 다시 자리가 나기까지의 초(1 이상)입니다. 앱은 이것으로 "약 N분 뒤 다시 해 주세요"라고 알립니다(5-3 E2).
- **한도**(기본값, 설정으로 바꿈): 주소마다 실패 10분에 5번·하루 20번(`HJV_API_LIMIT_GUEST_IP`), 모든 시도 시간당 60번이고 주소 하나는 그 4분의 1까지(`HJV_API_LIMIT_GUEST_ALL`), 주소마다 출입증 하루 50개(`HJV_API_LIMIT_GUEST_TICKETS`). 주소는 로그인처럼 셉니다(IPv6은 /64).
- 이용 비밀번호는 hjv-api의 `guest.conf`(`HJV_API_GUEST_FILE`)에 PBKDF2 해시와 `UNTIL`로 있습니다. 파일의 모양과 바꾸는 법은 [server/api/README.md](../server/api/README.md)와 [server-runbook.md](server-runbook.md)에 있습니다.

### 3-6. 서버가 출입증을 보는지: GET /api/hjv/access

앱은 로그인하기 전에도 이 요청으로 hbbs가 출입증을 보는지 압니다. 토큰도 본문도 없고, 답은 늘 200입니다.

```text
200 {"mode":"off"|"count"|"enforce"|"closed"|"unknown"}
```

- `mode`는 hbbs 콘솔 `hjv-access`(7)의 모드입니다. 이유별 개수는 싣지 않습니다. hbbs가 답하지 않거나 그 명령을 모르면(서버 패치 0006 전) `unknown`입니다.
- hjv-api는 hbbs에 30초에 한 번만 묻고, 그동안은 같은 답을 줍니다. 그래서 `access.conf`를 바꾸고 이 답이 바뀌기까지 hbbs가 파일을 다시 읽는 10초(6-4)와 이 30초가 걸립니다.
- 앱은 이 답으로 미리 알립니다(5-3): `enforce`나 닫힘(`closed`)이면 로그인하지 않은 컨트롤러에게 교직원 전용 서버라고 알리고, 로그아웃하거나 PC를 내 기기 목록에서 뺄 때 이 PC가 연결을 받지 못하게 된다고 알립니다. `unknown`이면 "서버가 교직원 전용이면"이라고 조건을 붙이고, `off`와 `count`면 알리지 않습니다.
- 이 답은 안내일 뿐입니다. 판정은 언제나 hbbs가 합니다. 모드는 거절 답으로도 누구에게나 드러나므로, 이 답이 더 알려 주는 것은 없습니다.

## 4. hbbs의 판정

### 4-1. 출입증 하나

hbbs는 출입증 하나를 아래 순서로 보고, 처음 걸린 것을 이유로 삼습니다. 이유의 이름은 로그, 콘솔, 벡터에서 같습니다.

| 순서 | 볼 것 | 걸리면 |
|---|---|---|
| 1 | 출입증이 비었음(빈 `token`, 필드 1001 없음) | `missing` |
| 2 | 1024바이트를 넘음 | `oversize` |
| 3 | 전송 형태(2-1)가 아님 | `malformed` |
| 4 | `kid`가 지금 규칙의 `KEY`에 없음 | `unknown_kid` |
| 5 | 그 키로 서명(2-2)이 맞지 않음 | `bad_signature` |
| 6 | 본문이 2-3과 2-4의 규칙을 어김 | `malformed` |
| 7 | `use`가 이 자리의 것이 아님. 연결 요청과 중계 요청은 `ctl`, `both`, 등록은 `pc`, `both` | `wrong_use` |
| 8 | 지금 ≥ `exp` | `expired` |
| 9 | `iat` > 지금 + 60초 | `not_yet_valid` |
| 10 | `NOT_BEFORE`가 있고 `iat` < `NOT_BEFORE` | `revoked` |
| 11 | 등록: `id`가 `RegisterPk.id`와 다르거나, `pk`가 `RegisterPk.pk`(바이트)의 표준 base64와 다름 | `wrong_pc` |
| 11 | 연결 요청과 중계 요청: `to`가 비어 있지 않고 대상 ID(`PunchHoleRequest.id`, `RequestRelay.id`)가 `to`에 없음 | `wrong_target` |
| 12 | 그 밖 | `ok` |

- 비교는 모두 바이트 그대로입니다. ID의 대소문자를 바꿔 보지 않습니다.
- **지금**은 hbbs의 `SystemTime`을 유닉스 초로 쓴 값입니다(소수 버림). hjv-api와 같은 서버의 시계라서 `exp`에는 여유를 두지 않습니다. 시계가 조금 뒤로 돌아가도 막 받은 출입증이 거절되지 않도록 `iat`에만 60초를 둡니다.
- 판정하는 함수는 지금 시각과 규칙을 인자로 받게 만들어, 벡터의 `now`와 `rule`로 시험합니다.
- 앱에는 이유를 알리지 않습니다. 모든 이유에 같은 답(5절)을 줍니다.

### 4-2. 연결 요청 (punch hole)

1. 열쇠(`licence_key`): 틀리면 `LICENSE_MISMATCH`(그대로)
2. 최소 버전(패치 0005): 낮으면 업데이트 문구(그대로)
3. **출입증**: `PunchHoleRequest.token`을 대상 `PunchHoleRequest.id`로 판정합니다. `ok`가 아니면 `PunchHoleResponse.other_failure`에 `TEXT`를 담아 답합니다.
4. 상대 찾기(그대로)

- 거절할 때는 상대를 찾지 않습니다. PC는 아무것도 듣지 못하고, 컨트롤러는 그 ID가 있는지, 켜져 있는지 알지 못합니다. WebRTC offer도 이 요청에만 실리므로 P2P도 막힙니다.
- 최소 버전을 먼저 보므로, 1.8.0 아래의 앱은 출입 문구가 아니라 업데이트 문구를 받습니다(최소 버전을 1.8.0으로 올린 뒤).

### 4-3. 중계 요청 (RequestRelay)

1. **출입증**: `RequestRelay.token`을 대상 `RequestRelay.id`로 판정합니다. `ok`가 아니면 같은 연결로 `RelayResponse.refuse_reason`에 `TEXT`를 담아 답하고 PC를 찾지 않습니다.
2. PC를 찾아 넘기기(그대로, `token`, `control_permissions`, `controlled_context`를 지움)

- 열쇠는 보지 않습니다. 앱은 hbbs로 보내는 `RequestRelay`에 `licence_key`를 넣지 않기 때문입니다(upstream의 `request_relay`). 열쇠를 보게 하면 모든 앱의 중계가 막힙니다.
- 연결 요청 없이 중계 요청만 보내는 클라이언트도 여기서 막히고, PC는 아무것도 듣지 못합니다.

### 4-4. 등록 (RegisterPk)

1. 지금의 확인(uuid나 pk가 없으면 답하지 않음, ID 길이, 주소 차단, uuid 다름, 너무 잦음): 그대로. 다만 `MODE=enforce`에서 출입증이 맞지 않는 등록은 주소마다의 횟수를 따로 셉니다(주소 차단의 키 `<주소>/noticket`). 출입증 없는 옛 PC는 30초마다 다시 등록하므로, 같은 주소(학교 NAT 등) 뒤의 출입증 가진 PC가 그 때문에 `TOO_FREQUENT`를 받지 않게 하기 위해서입니다. 옵션이 없거나 `off`, `count`이면 모든 등록이 지금처럼 하나로 셉니다.
2. 키 저장: 그대로이되, 받아들여진(출입증이 맞은) TCP/WS 연결로 온라인인 ID에는 출입증 없는 등록이 아무것도 저장하지 않습니다(키도 주소도). 오프라인인 ID와 새 ID의 키는 지금처럼 저장합니다. hjv-api는 PC를 계정에 묶을 때 hbbs가 가진 그 PC의 키로 확인하므로(`hjv-pk`), 출입증이 없는 PC의 키도 저장해야 처음 설치한 PC가 첫 로그인에 묶입니다.
3. **출입증**: 필드 1001(3-2)을 그 등록의 `id`와 `pk`로 판정합니다. `ok`면 지금처럼 그 연결이 그 ID의 신호 채널이 되고 온라인입니다. 아니면 `RegisterPkResponse`의 `result`를 `NOT_DEPLOYED`(`keep_alive`는 `OK`와 같은 30)로 답하고 오프라인으로 둡니다.

- `ok`가 아닌 등록은 그 ID의 주소, 신호 채널, 마지막 등록 시각을 바꾸지 않습니다. 다른 연결로 들어와 있는 같은 ID의 PC를 밀어내지 못하고, 같은 연결의 앞선 `ok` 등록도 그대로 둡니다. upstream의 `update_pk`는 키와 함께 주소와 등록 시각도 쓰므로, 0006은 키만 저장하는 길을 따로 둡니다.
- 출입증 없이 등록된 키는 `hjv-pk`가 나이 `-`로 답합니다. 그래서 묶기와 동기화(키만 봄)는 되고, 나이를 보는 켜기, 작업, 점검은 그 PC를 오프라인으로 봅니다.
- `NOT_DEPLOYED`를 쓰는 까닭: 옛 앱이 깔끔하게 받는 유일한 거절입니다. 30초마다 다시 해 보고 ID를 바꾸지 않습니다. `UUID_MISMATCH`는 PC가 ID를 바꾸게 하고, 새 값은 옛 앱이 모르는 값으로 보고 15초마다 다시 보냅니다.
- **들어온 등록**: hbbs는 `ok`인 등록의 출입증에서 `kid`, 그 `kid`가 가리킨 공개키, `iat`, `exp`만 그 연결에 기억합니다(메모리, 연결이 닫히면 버림). 공개키는 비밀이 아닙니다. `MODE=enforce`인 동안 하트비트(10초)마다 다음 가운데 하나면 그 연결을 닫아 오프라인으로 합니다.
  - 지금 ≥ `exp`
  - `iat` < `NOT_BEFORE`(그 사이에 `NOT_BEFORE`를 넣었을 때)
  - `kid`가 `KEY`에서 빠졌거나, 그 번호의 키가 바뀜(이유 `unknown_kid`). 도난당한 키를 같은 번호로 바꿔 써도 그 키로 들어온 연결이 닫힙니다
  - 기억한 출입증이 없음(`MODE=off`인 동안 들어온 등록, `count`에서 `ok`가 아니었던 등록)
- **기한 늘리기**: PC는 들어올 때 쓴 출입증의 남은 시간이 절반 아래로 내려가면, 그때 가진 새 출입증으로 같은 연결에서 `RegisterPk`를 다시 보냅니다. `ok`면 hbbs는 기억한 값을 바꿉니다.

### 4-5. 그 밖의 길

- **UDP**: `MODE=enforce`인 동안(그리고 4-6의 닫힘에서) UDP의 `RegisterPeer`와 `RegisterPk`는 받지 않고 답하지 않습니다. 운영에서는 UDP가 루프백에만 열려 있고 고정 서버 빌드는 UDP로 등록하지 않습니다. UDP의 연결 요청은 지금도 무시합니다.
  - 하나만 예외입니다. hbbs가 스스로를 살피는 점검(`test_hbbs`)의 UDP `RegisterPeer`(ID `(:test_hbbs:)`)에는 그때도 모르는 ID에게처럼 `RegisterPeerResponse`(`request_pk`)로 답하고, 세지 않습니다. 피어 표는 건드리지 않습니다. 이 답이 없으면 `TEST_HBBS`가 `no`가 아닐 때(systemd 단위만 `no`로 둠) hbbs가 약 26초 뒤 스스로 멈춥니다.
- 온라인 조회(`OnlineRequest`), NAT 시험, `IceCandidate`, `PunchHoleSent`, `LocalAddr`, `RelayResponse`는 확인하지 않습니다. ICE 후보는 ID로 PC에 넘어가지만, PC는 모르는 세션의 후보를 버립니다.
- hbbr은 확인하지 않습니다. 서버 공개키만 알면 누구나 두 끝을 짝지어 중계 대역을 쓸 수 있지만, 그것으로 PC에 들어가지는 못합니다.

### 4-6. MODE

| 상태 | 세 자리 | 들어온 등록 | UDP 등록 | 남기는 것 |
|---|---|---|---|---|
| 옵션 없음 | 보지 않음(0006 전과 같음) | 닫지 않음 | 받음 | 없음 |
| `off` | 보지 않고 기억하지 않음 | 닫지 않음 | 받음 | 없음 |
| `count` | 판정해서 이유별로 셈. 거절하지 않음. `ok`인 등록은 기억 | 닫지 않음 | 받음 | 개수만 |
| `enforce` | `ok`가 아니면 거절 | 4-4대로 닫음 | 받지 않음 | 거절 줄과 개수 |
| 닫힘 | 출입증을 보지 않고 모두 거절(이유 `closed`로 셈) | 들어온 것이 없음 | 받지 않음 | 오류 줄과 개수 |

- 닫힘은 옵션이 있는데 hbbs가 시작할 때 쓸 수 있는 파일이 없는 경우와, 옵션의 값이 빈 경우뿐입니다(6-3).
- `off`에서 `enforce`로 바꾸면 `off`인 동안 들어온 PC는 10초 안에 한 번 끊깁니다. 1.8.0 PC는 곧 출입증을 실어 다시 등록합니다. `count`를 거쳐도 `off`인 동안 들어온 등록에는 기억한 출입증이 없으므로 같습니다. 그 PC의 진행 중인 원격 세션도 함께 끊길 수 있으므로(hbbs를 다시 시작할 때처럼), 켜는 것은 조용한 시간에 합니다.

## 5. 거절 표시와 문구

### 5-1. 답

| 자리 | 답 | 앱이 알아보는 것 |
|---|---|---|
| 연결 요청 | `PunchHoleResponse { other_failure: TEXT }` | `access not allowed` |
| 중계 요청 | `RelayResponse { refuse_reason: TEXT }` | `access not allowed` |
| 등록 | `RegisterPkResponse { result: NOT_DEPLOYED, keep_alive: 30 }` | `NOT_DEPLOYED` |

- **표시 `access not allowed`**: 1.8.0은 문구를 소문자로 바꿨을 때 이 말이 들어 있으면 출입 거절로 압니다(`version not allowed`를 보는 `hjviewer_update::on_refusal` 옆에서). 이 말의 `not allowed` 덕분에 어느 판의 앱도 자동으로 다시 연결하지 않습니다(`check_if_retry`). `version not allowed`가 없어야 1.6.0 이후의 앱이 버전 거절로 읽지 않습니다.
- 지금의 앱은 `RelayResponse.refuse_reason`을 그대로 오류로 냅니다(`src/client.rs`의 `request_relay`). 1.8.0은 이 글도 연결 요청의 거절과 같은 길로 읽습니다.

### 5-2. TEXT 규칙

패치 0005의 TEXT 규칙에서 표시만 바꿉니다.

- 1~2000자이고 제어 문자가 없습니다.
- `access not allowed`가 소문자 그대로 들어 있습니다.
- `version not allowed`는 대소문자와 상관없이 없습니다.
- `Failed`로 시작하지 않고, 첫 낱말이 `_tip`으로 끝나지 않습니다.
- `{`, `}`, `RustDesk`, `%이름%`(`%`와 `%` 사이의 세 글자 이상의 영문자와 `_`)이 없습니다.
- URL마다 뒤에 공백이 있습니다.
- **누구에게나 보입니다.** 중계 요청은 열쇠를 보기 전에 거절하므로(4-3), 서버 공개키를 모르는 쪽도 이 글을 받습니다. 0005의 TEXT(열쇠가 맞는 앱만 봄)와 달리, 앱 사용자에게만 보일 링크나 글을 넣지 않습니다. 또 0005와 달리 출입 확인은 열쇠 확인에 기대지 않습니다. hbbs의 `-k`가 비어 있어도 확인합니다. 출입 통제가 다른 설정 때문에 저절로 꺼지면 안 되기 때문입니다.

### 5-3. 문구

**hbbs의 기본 문구**: `TEXT` 줄이 없을 때와 닫힘일 때 씁니다. hbbs 안에 이 글자 그대로 둡니다.

> HJViewer는 연세대학교 교직원용입니다. 연세포탈로 로그인하거나 이용 비밀번호를 넣어야 연결할 수 있습니다. HJViewer is for Yonsei University staff: sign in with a Yonsei portal account or enter the access password to connect. (access not allowed)

이 문구를 그대로 보는 것은 표시를 모르는 앱입니다. 1.8.0 아래의 HJViewer는 그 전에 최소 버전 문구를 받으므로, 실제로는 버전을 높게 속인 upstream RustDesk나 손으로 만든 클라이언트가 봅니다. 그래서 한국어와 영어를 함께 씁니다.

**1.8.0이 보이는 글**: 앱은 아래 글을 한국어와 영어로 둡니다(앱의 언어가 한국어면 한국어, 아니면 영어).

| 때 | 한국어 | English |
|---|---|---|
| A1. 로그인하지 않은 컨트롤러가 거절됨(세션 창) | HJViewer는 연세대학교 교직원용입니다. 연세포탈로 로그인하거나 이용 비밀번호를 넣으세요. (access not allowed) | HJViewer is for Yonsei University staff. Log in with your Yonsei portal account, or enter the access password. (access not allowed) |
| A1의 단추 | [로그인] [이용 비밀번호로 연결] [취소] | [Log in] [Connect with the access password] [Cancel] |
| A1의 입력 칸 | 이용 비밀번호. 안내: 관리자가 알려 준 비밀번호입니다. PC의 일회용 비밀번호가 아닙니다. | Access password. Hint: the password your administrator gave you, not the PC's one-time password. |
| A2. 로그인한 컨트롤러가 출입증을 새로 받아 다시 해도 거절됨 | 서버가 이 연결을 받지 않았습니다. 잠시 뒤 다시 시도하고, 계속되면 관리자에게 알려 주세요. (access not allowed) | The server did not accept this connection. Try again in a moment, and tell your administrator if it keeps happening. (access not allowed) |
| P1. 등록이 `NOT_DEPLOYED`(PC 홈, 빠른 지원 창)의 제목 | 이 PC는 아직 서버에 연결되지 않았습니다 | This PC is not connected to the server yet |
| P1의 글 | 이 PC를 연결받으려면 연세포탈로 로그인하거나 이용 비밀번호를 넣으세요. | To accept connections on this PC, log in with your Yonsei portal account or enter the access password. |
| P1의 단추 | [로그인] [이용 비밀번호] | [Log in] [Access password] |
| P2. PC의 이용 비밀번호 창 | 이용 비밀번호를 넣으면 이 PC가 최대 12시간 동안 연결을 받습니다. 끝나면 다시 넣어야 합니다. | With the access password, this PC accepts connections for up to 12 hours. After that, enter it again. |
| G1. 설치 화면과 설정의 안내 | 사람이 없을 때도 이 PC에 연결하려면 연세포탈로 로그인해 두세요. 이용 비밀번호는 앱을 켤 때마다 넣어야 합니다. | To reach this PC when nobody is at it, keep it logged in with your Yonsei portal account. The access password has to be entered each time the app starts. |
| E1. 이용 비밀번호가 틀림(401 `denied`) | 이용 비밀번호가 맞지 않거나, 지금은 서버가 이용 비밀번호를 받지 않습니다. | The access password is wrong, or the server does not take the access password right now. |
| E2. 시도가 너무 많음(429) | 시도가 너무 많습니다. 잠시 뒤 다시 해 주세요. | Too many tries. Please try again later. |
| E3. 확인할 수 없음(503, 400, 네트워크, 그 밖의 실패) | 지금은 이용 비밀번호를 확인할 수 없습니다. 잠시 뒤 다시 해 주세요. | The access password cannot be checked right now. Please try again in a moment. |
| E4. PC의 키가 다름(403 `wrong_pc`) | 이 PC의 키가 서버에 있는 것과 달라 이용 비밀번호로 연결받을 수 없습니다. 관리자에게 알려 주세요. | This PC's key differs from the one the server has, so it cannot accept connections with the access password. Please tell your administrator. |
| E5. 기기의 시계가 맞지 않음(409 `clock`, 한 번 고쳐 다시 보낸 뒤에도) | 기기의 날짜와 시간이 맞지 않습니다. 날짜와 시간을 자동으로 맞추게 한 뒤 다시 해 주세요. | The device's date and time are wrong. Set them to update automatically and try again. |

- A1과 A2는 끝에 표시를 남깁니다. 오류 창으로 가는 글이라, 자동 다시 연결을 멈추기 위해서입니다(버전 거절의 R1, R2와 같음).
- 로그인한 컨트롤러는 거절을 받으면 먼저 출입증을 한 번 새로 받아 다시 연결하고, 그래도 거절되면 A2를 보입니다.
- A1에서 이용 비밀번호를 넣으면 앱이 그 대상의 출입증(3-5)을 받고 바로 다시 연결합니다. 새 연결은 다시 묻고, 같은 세션의 자동 다시 연결은 묻지 않습니다. [로그인]은 지금의 로그인 창을 열고, 로그인하면 다시 연결합니다.
- P1은 등록이 거절된 동안(`hjv-access-pc`가 `refused`) PC 홈과 빠른 지원 창에 보입니다. 출입 확인이 꺼진 서버(`MODE=off`)는 거절하지 않으므로 P1이 나오지 않고, 앱은 지금처럼 움직입니다.
- P2의 출입증은 앱의 메모리(설치한 PC는 서비스의 메모리)에만 있으므로, 앱(설치한 PC는 PC)을 다시 시작하거나 12시간이 지나면 끝나고 등록이 다시 거절되어 P1이 나옵니다.
- 휴대폰 앱은 PC로 등록하지 않으므로 A1, A2와 E1~E3, E5만 씁니다.
- 화면은 클라이언트 패치 0061의 `flutter/lib/common/hjviewer_access.dart`에 있고, 글과 고르기는 `hjviewer_access_texts.dart`(시험 `flutter/test/hjviewer_access_test.dart`)에 있습니다. Rust는 A1, A2를 한국어 글로 msgbox에 싣고, Flutter가 그 msgbox(`hjv-access-error`)를 앱의 언어로 된 창으로 바꿉니다.

## 6. /etc/hjviewer/access.conf

### 6-1. 형식

hbbs는 옵션 `--hjv-access-file`(환경 변수 `HJV_ACCESS_FILE`, `hjviewer.env`에 한 줄)이 가리키는 파일을 10초마다 다시 읽습니다. 옵션이 없으면 0006 전과 똑같이 움직입니다. 파일은 root:hjviewer 0640입니다.

```text
# /etc/hjviewer/access.conf
MODE=enforce
KEY=1:<출입증 공개키 1>
# 키를 바꾸는 동안만 두 번째 KEY 줄
KEY=2:<다음 키>
# 빈 값은 없음. 비상에는 2026-11-02T00:00:00Z처럼
NOT_BEFORE=
# 선택. 없으면 5-3의 기본 문구
TEXT=<거절 문구>
```

- `#`는 줄의 처음에서만 주석입니다. 값 뒤에 붙인 `#`와 그 뒤의 글은 값의 일부가 되어 그 줄이 오류가 됩니다.
- `MODE`: `off`, `count`, `enforce` 가운데 하나, 소문자입니다(4-6).
- `KEY`: `<kid>:<공개키>`입니다. kid는 1~65535(앞에 0 없음), 공개키는 32바이트의 표준 base64(패딩 포함 44자, 정규형)입니다. 한 줄이나 두 줄이고, 두 줄의 번호와 키는 서로 다릅니다. 콜론 앞뒤에 공백이 없습니다. `hjv-api keygen ticket`이 보이는 줄 그대로입니다.
- `NOT_BEFORE`: 이보다 먼저 만든 출입증(`iat`가 더 작은 것)을 모두 거절합니다. `YYYY-MM-DDTHH:MM:SSZ` 꼴의 UTC 한 형태만 받습니다(0005의 `FROM`과 같음: 0001~9999년, 다른 오프셋과 소수 없음, 윤초 없음). 빈 값이면 없습니다.
- `TEXT`: 거절 문구입니다(5-2의 규칙). 줄이 없으면 기본 문구(5-3)입니다. 누구에게나 보이는 글입니다(5-2).

### 6-2. 읽는 규칙

패치 0005의 파일과 같은 틀입니다.

- 일반 파일이고 64 KiB 이하이며 UTF-8입니다. 앞의 BOM은 건너뜁니다.
- 비어 있지 않으면 마지막 줄도 줄바꿈으로 끝납니다. 쓰는 중인 파일을 더 짧은 규칙으로 읽지 않기 위해서입니다.
- 줄마다 앞뒤의 공백과 줄 끝의 CR을 무시합니다. 빈 줄과 `#`로 시작하는 줄은 주석입니다.
- 나머지 줄은 `이름=값`입니다. 이름은 `=` 바로 앞에 있습니다(`MODE =enforce`는 오류). 값 앞의 공백은 무시합니다.
- 이름은 `MODE`(한 번), `KEY`(두 번까지), `NOT_BEFORE`(한 번), `TEXT`(한 번)뿐입니다. 다른 이름이나 더 많은 줄은 오류입니다. 이 검사는 `MODE=off`에서도 합니다.
- `MODE`가 없으면 오류입니다. `MODE=off`면 다른 줄의 값은 읽지 않습니다. 끄는 것이 다른 줄의 실수에 막히지 않게 하기 위해서입니다.
- `MODE`가 `count`나 `enforce`면 `KEY` 줄 하나나 둘, `NOT_BEFORE` 줄(빈 값도 됨)이 꼭 있어야 하고, 모든 값이 6-1의 형식에 맞아야 합니다. `NOT_BEFORE`를 빈 값으로라도 쓰게 하는 것은, 줄 경계에서 잘린 파일이 `NOT_BEFORE` 없는 규칙으로 읽히지 않게 하기 위해서입니다. 그래서 6-1의 순서로 씁니다.
- 바꿀 때는 옆에 새 파일을 써서 `mv`로 바꿉니다.
- hbbs의 로그는 파일 내용을 옮기지 않습니다. 오류 줄에는 줄 번호와 까닭만 씁니다.

### 6-3. 실패하면 닫힘

| 때 | 파일 | 쓰는 규칙 |
|---|---|---|
| 옵션 없음 | - | 꺼짐(0006 전과 같음) |
| 옵션이 있지만 값이 빔(`HJV_ACCESS_FILE=`) | - | **닫힘**: 모두 거절. 오류를 남기고 한 시간마다 다시 남김 |
| 시작할 때 | 쓸 수 있음 | 그 파일의 규칙 |
| 시작할 때 | 없음, 읽을 수 없음, 규칙 오류 | **닫힘**: 모두 거절(4-6). 오류를 남기고 한 시간마다 다시 남김 |
| 도는 중 | 바뀌었고 쓸 수 있음 | 새 규칙(10초 안) |
| 도는 중 | 없어짐, 읽을 수 없음, 규칙 오류 | 마지막 좋은 규칙(처음부터 닫힘이었으면 닫힘). 오류를 남기고 한 시간마다 다시 남김 |

- 최소 버전 파일(0005)과 반대입니다. 그쪽은 파일이 없으면 최소 버전이 없습니다. 이것은 출입 통제라서 파일을 지우거나 망가뜨려도 열리지 않습니다. 끄는 길은 `MODE=off`를 쓰거나 `hjviewer.env`에서 옵션 줄을 빼는 것입니다. 빈 값(`HJV_ACCESS_FILE=`)은 빼는 것이 아니라 닫힘입니다. 채우다 만 설정이 출입 확인을 말없이 끄지 않게 하기 위해서입니다.
- 마지막 좋은 규칙은 메모리에만 있습니다. 파일이 고장 난 채로 hbbs를 다시 시작하면 닫힘입니다.

### 6-4. 바꾸기

- **켜고 끄기**: `MODE`만 바꿔 씁니다. 10초 안에 듣고, 서명 키도 루트 키도 필요 없습니다. 1.8.0은 서버가 거절할 때만 묻기 때문에, 끄면 1.8.0도 묻지 않고 연결하고 PC도 출입증 없이 온라인이 됩니다.
- **비상**: `NOT_BEFORE=<지금>`을 씁니다. 그때까지 나간 출입증이 모두 거절되고, 들어와 있던 PC는 10초 안에 끊깁니다. 로그인한 앱은 다음 연결에서 거절을 보고 새 출입증을 받고, 묶인 PC는 다음 동기화(보통 2분) 뒤 새 출입증으로 다시 들어옵니다. 막힌 계정은 새 출입증을 받지 못하고, 이용 비밀번호로 들어온 쪽은 비밀번호를 다시 넣습니다. 이용 비밀번호를 바꾸면서 이미 나간 출입증까지 바로 끊으려면 이것을 함께 씁니다.
- **키 바꾸기**
  1. 새 키를 만들어 `KEY=2:` 줄을 더합니다(두 줄).
  2. hjv-api가 새 키로 서명하게 합니다.
  3. 옛 키의 출입증이 모두 끝날 때까지 기다립니다. 가장 긴 수명이 72시간이므로 3일입니다(수명 설정을 바꿨으면 그 수명).
  4. `KEY=1:` 줄을 뺍니다. 그 키의 출입증으로 들어와 있던 PC가 남아 있으면 10초 안에 끊기고 새 출입증으로 다시 들어옵니다.
  - 키를 도난당했으면 3을 기다리지 않고 옛 줄을 바로 뺍니다.

## 7. 남기는 것과 남기지 않는 것

**hbbs**

- 출입증 값은 어느 부분도 로그, DB, 콘솔 답에 쓰지 않습니다. `sub`와 `TEXT`도 쓰지 않습니다.
- 메모리에만 두는 것: `ok`인 등록마다 그 출입증의 `kid`, 그 키, `iat`, `exp`(연결이 닫히면 버림), 그리고 이유별 개수.
- 거절과 들어온 등록을 닫은 것(`enforce`, 닫힘): 0005처럼 10초에 30줄까지 한 줄씩 남깁니다. 주소, 자리(`ph`, `rr`, `reg`), 대상 ID나 등록 ID(64자로 자르고 따옴표), 이유입니다. 넘으면 개수 한 줄입니다. 성공은 남기지 않으므로 출입증의 `sub`로 연결 기록이 쌓이지 않습니다. `count`에서는 줄을 남기지 않고 세기만 합니다.
  - 거절은 `access refused <ph|rr|reg> <주소> for "<ID>": <이유>`, 하트비트에서 닫은 것은 `access closed reg <주소> for "<ID>": <이유>`, 넘친 것은 `access refused or closed N more in the last 10 seconds`입니다.
- 규칙이 바뀌면 한 줄: `access enforce: keys 1,2, not before 2026-11-02T00:00:00Z`처럼 씁니다(`TEXT`는 쓰지 않음). 파일 오류는 `access file <경로>: <까닭>; <지금 쓰는 규칙, 또는 access closed: every request is refused>`처럼 까닭과 지금 쓰는 규칙을 씁니다.
- 루프백 콘솔 명령 `hjv-access`: 한 줄로 답합니다. `HJVACCESS1 mode=<off|count|enforce|closed>` 뒤에 hbbs가 시작한 뒤의 개수를 `<자리>.<이유>=<n>`으로(0이 아닌 것만, 자리는 `ph`, `rr`, `reg`, `udp`) 잇습니다. ID와 주소는 없습니다. 옵션이 없을 때도 `HJVACCESS1 mode=off`라서, 옵션이 있는지는 `hbbs --help`의 `--hjv-access-file`이나 `access off` 로그 줄로 압니다.
  - 이유는 4-1의 열두 가지(`ok`부터 `wrong_target`까지)에 hbbs의 상태에서 오는 둘을 더한 열네 가지입니다: `closed`(닫힘에서 모두 거절, 자리마다)와 `ignored`(강제 중에 받지 않은 UDP 등록, `udp.ignored`. 닫힘에서는 `udp.closed`).
  - 하트비트에서 닫은 연결은 거절과 같은 이름으로 `reg.<이유>`에 셉니다(`missing`, `expired`, `revoked`, `unknown_kid`, `closed`). 둘은 로그 줄로 나뉩니다.
- 콘솔의 주소 차단 목록(`ib`)에는 출입증 없는 등록의 `<주소>/noticket` 항목이 따로 보입니다(4-4). `ib <주소>/noticket -`로 풉니다.
- trace 수준에서는 WebSocket 라이브러리가 모든 프레임을 남깁니다(출입증이 든 요청까지). 운영에서는 쓰지 않습니다.

**hjv-api**

- 출입증 키는 자격 증명에만 있습니다. 출입증 값을 저장하지도 기록하지도 않고, 교직원, 묶인 PC, 심사 계정의 발급을 하나하나 기록하지 않습니다. `hjv-api admin ticket status`는 시작한 뒤 종류별로 센 수만 보입니다(메모리).
- 이용 비밀번호의 시도: 시도마다 결과, 시각, 주소의 꼬리표를 `guest` 기록으로 6개월(`HJV_API_EVENT_DAYS`, 183일) 둡니다. 주소 그대로, 대상 ID, PC ID, 비밀번호, 출입증은 남기지 않습니다. 틀린 비밀번호와 받지 않는 상태(파일이 없거나 기한이 지남)에 같은 답을 줍니다(3-5).
- 이용 비밀번호는 봉인해서 받습니다. 로그인의 봉인에 용도 칸 `p`(`guest`)를 더해, 포탈 비밀번호의 봉인과 서로 바꿔 쓰지 못하게 합니다(3-5).
- hjv-api는 `HJV_API_TICKET_PUBKEYS`(곧 `access.conf`의 `KEY=`)에 공개된 시험 키가 있으면 selfcheck를 실패로 하고 시작할 때 저널에 남깁니다(8절).

**앱**

- 교직원과 심사 계정의 출입증: LocalConfig의 `hjv-access-ticket`에 `{"t","use","until","life","from_login"}`(시각은 기기 시계의 초)로 둡니다. 로그아웃하면, 그리고 앱이 토큰(`access_token`)을 지우면(401 뒤의 앱 자신의 로그아웃 포함) 지웁니다.
- 묶인 PC의 출입증: 서비스의 설정 폴더, 묶음 정보 옆의 `HJViewer_ticket.json`에 같은 모양으로 둡니다(Unix는 0600). 동기화마다가 아니라 한 시간쯤마다 다시 쓰고, hjv-api가 멈춘 동안 서비스가 다시 시작할 때만 읽습니다. 서비스가 묶이지 않은 채 시작하면 지우고, 제거 프로그램도 지웁니다.
- hbbs가 이 PC의 등록을 받았는지: 등록하는 프로세스(설치한 PC는 서비스)의 옵션 `hjv-access-pc`(`ok`, `refused`)에 둡니다. 화면이 읽습니다.
- 출입 거절을 화면에 알리는 msgbox 종류는 `hjv-access-error`입니다.
- 이용 비밀번호로 받은 출입증: 메모리에만 둡니다. 컨트롤러는 대상 ID에서 출입증을 찾는 표에 두고 세션이 끝나면 지웁니다. PC는 앱의 메모리(설치본은 설정 파일에 쓰지 않는 IPC로 넘긴 서비스의 메모리)에 두고, 다시 시작하거나 12시간이 지나면 끝납니다.
- 이용 비밀번호: 어디에도 저장하지 않고, 출입증으로 바꾼 뒤 바로 버립니다.

## 8. 시험 벡터

- **파일**: [server/api/tests/vectors/access-tickets.json](../server/api/tests/vectors/access-tickets.json). 같은 폴더의 `access-tickets.py`가 만듭니다.

  ```sh
  python3 -I -B server/api/tests/vectors/access-tickets.py          # 파일을 씀
  python3 -I -B server/api/tests/vectors/access-tickets.py --check  # 만든 것과 파일을 비교
  ```

  표준 라이브러리만 씁니다. Ed25519는 PyNaCl이 있으면 그것을, 없으면 openssl 명령(OpenSSL 3의 `pkeyutl -rawin`)을 씁니다. 서명이 결정적이라 어느 쪽이든 같은 파일이 나옵니다. 스크립트는 시작할 때 RFC 8032의 시험 값으로 그 구현을 확인합니다.
- **시험 키**: seed가 한 바이트를 32번 되풀이한 것입니다. 출입증 키 1은 0x74, 2는 0x75, 3은 0x76(`KEY` 줄에 없음), PC의 기기 키는 0x02([tests/data/v3-vectors.json](../server/api/tests/data/v3-vectors.json)의 `pc_device`), 다른 PC는 0x77, `sub`를 만드는 기록 키는 0x6c입니다. 공개키는 [scripts/public-test-keys.txt](../scripts/public-test-keys.txt)에도 적었습니다. 시험 밖에서 쓰지 않습니다. hjv-api는 `HJV_API_TICKET_PUBKEYS`(곧 `access.conf`의 `KEY=`)에 이런 공개된 시험 키(한 바이트를 되풀이하거나 연속한 바이트 32개로 된 seed의 키)가 있으면 selfcheck를 실패로 하고 시작할 때 저널에 남깁니다.
- **판정의 조건**: `rule`과 `access_conf`는 `MODE=enforce`, `KEY` 1과 2, `NOT_BEFORE=2025-10-09T00:00:00Z`이고, `now`는 1760003600(2025-10-09T09:53:20Z)입니다. 맞는 출입증은 대개 그 한 시간 전(1760000000)에 만든 것입니다.
- **내용**
  - `tickets`(84개): 출입증과 자리마다의 기대 이유(`checks`의 `at`, 대상이나 등록 값, `expect`). 맞는 출입증 14개에는 값(`fields`), 서명한 키(`signed_by`), 서명한 바이트(`signed_hex`)가 있습니다. 4-1의 모든 이유가 하나 이상 나옵니다.
  - `register_pk`(7개): 필드 1001을 담은 `RegisterPk`의 바이트와 그것을 담은 `RendezvousMessage`의 바이트, 읽어 낸 출입증, 기대 이유.
  - `access_conf_cases`(41개): 파일 내용과 읽은 규칙, 또는 `error`. `=` 없는 줄(`MODE=off` 아래의 `KEY`, 값 없는 `NOT_BEFORE`)도 오류입니다.
- **누가 무엇을 맞추나**
  - hjv-api: `fields`로 2-3의 본문을 만들어 `signed_by`의 키로 서명하면 `ticket`과 바이트까지 같아야 합니다. `sub`는 `accounts`의 기록 키와 로그인 키로 만든 `Tagger::account`입니다.
  - hbbs: 모든 출입증과 `checks`에서 4-1의 판정이 `expect`와 같아야 합니다. `register_pk`의 바이트에서 3-2대로 출입증을 읽어 같은 판정을 내야 하고, `access_conf_cases`를 6-2대로 읽어야 합니다. `MODE=count`에서는 같은 이유로 세고 모두 통과시킵니다.
  - 앱: `register_pk`의 첫 사례 `ticket`의 `id`, uuid(`vector-uuid-0001`), `pk`, 출입증으로 `RegisterPk`를 만들면 `register_pk_hex`와 같은 바이트여야 합니다.
- **사본과 고정**: hjv-api의 시험(`tests/tickets.rs`)과 hbbs의 시험(서버 패치의 `src/hjv_access.rs`)은 이 파일의 SHA-256을 고정하고, hbbs는 자기 트리에 사본(`tests/vectors/access-tickets.json`)을 둡니다. CI의 server 단계는 hbbs 시험을 `HJV_ACCESS_VECTORS`를 이 파일로 두고 돌려, 사본이 아직 이 파일인지 봅니다. 앱의 시험은 쓰는 값(위의 바이트, README의 요청 서명)을 코드에 옮겨 두고 주석에 SHA-256을 적습니다. 파일을 다시 만들면 세 곳을 함께 고칩니다.
- **스크립트가 확인하는 것**: 사례 표에 손으로 적은 기대 이유를 스크립트가 이 문서대로 다시 판정해서 맞춰 보고, 하나라도 다르면 멈춥니다. 이 파일을 처음 만들 때 따로 확인한 것도 있습니다. 모든 서명 판정이 엄격한 Ed25519 구현(`tools/hjv-release`의 생성 스크립트에 있는 것)과 같고, 맞는 출입증 14개는 그 구현으로 다시 서명해도 같은 바이트입니다. `register_pk`는 rust-protobuf 3.7.2와 같고(3-2), 본문의 정규형 판정은 serde_json과 같습니다(2-3).
- 형식을 바꾸면 스크립트의 표와 이 문서를 함께 고치고 파일을 다시 만듭니다. hjv-api와 hbbs의 시험은 이 파일을 그대로 읽습니다.
