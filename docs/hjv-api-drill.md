# hjv-api 운영 연습 (감시·백업·복원)

설계 3판 5-9와 작업 묶음 WP-O는 출시 전에 hjv-api의 복원 절차를 시험 서버에서 한 번 연습하라고 합니다. 이 문서는 그 연습의 기록입니다. 연습은 `tools/e2e/hjv-api-drill.sh`가 하므로 언제든 다시 돌릴 수 있습니다. 감시와 백업의 동작 원리와 명령은 [server/api/README.md](../server/api/README.md)의 "Monitoring"과 "Backup and restore"에 있습니다. 원격 켜기의 서버 쪽(WP-W2)이 들어간 뒤로는 명령 키, NAS 중계기 승인, 켜기 한 번도 이 연습에 들어 있습니다.

```bash
tools/e2e/hjv-api-drill.sh                     # 약 13분(동기화가 10분 묵기를 기다리는 시간 포함)
HJV_DRILL_QUICK=1 tools/e2e/hjv-api-drill.sh   # 그 기다림 없이 약 3분
```

출력은 `build/drill/drill.log`에 남습니다. 필요한 것은 cargo, sudo로 쓰는 Docker(`scripts/ci.sh`와 같음), 이미지를 만들 때 Ubuntu 패키지 저장소입니다.

## 시험 서버

- 개발 PC(WSL2 Ubuntu 24.04)의 Docker 컨테이너입니다. 운영 서버와 같은 Ubuntu 24.04이고, systemd 255가 PID 1로 돕니다. 첫 서버(A)와 다시 세운 서버(B), 두 대를 차례로 띄우고 끝나면 지웁니다. 밖으로 나가는 연결은 없습니다.
- 설치는 런북 7절의 7-3~7-5를 그대로 따릅니다. 서버 묶음과 같은 배치(`bin/hjv-api`, `systemd/`의 `hjviewer-api*` 유닛·env 예시·`journald@hjviewer.conf`)를 쓰고, 유닛은 `systemd-analyze verify`로 확인하고, 자격 증명은 `systemd-creds encrypt`로 넣고(봉인 키, 목록 키 2번, 명령 키 2번은 "이동식 매체" 파일에서 표준 입력으로), 설정은 `sed`로 고칩니다.
- 대역(가짜)은 셋입니다. 가짜 SSO(`hjv-fake-sso`), 시험 PC를 켜짐으로 답하는 가짜 hbbs 콘솔, 받은 메시지를 적어 두는 가짜 텔레그램입니다. 경보 유닛만 드롭인으로 가짜 텔레그램을 가리킵니다. 키는 `server/api/tests/common`의 공개된 시험 키입니다. 명령 키 두 개와 출입증 키만 연습 때마다 `hjv-api keygen command`, `keygen ticket`으로 새로 만듭니다. 앱과 PC의 요청은 `server/api/tests/drill.rs`가 컨테이너 안에서 보냅니다.
- NAS 중계기도 그 시험 프로그램이 맡습니다. 시험용 중계기 키로 서명한 hello와 poll을 보내고, 받은 켜기 명령의 서명을 중계기처럼 고정된 명령 공개키로 확인한 뒤 결과(패킷 12개)를 돌려줍니다. 매직 패킷은 나가지 않습니다. 진짜 hjv-relay의 루프는 `cargo test`의 `tests/relay_loop.rs`가 hjv-api에 붙여 돌리고, 진짜 실행 파일과 패킷은 WOL E2E(WP-W5)가 봅니다.
- 컨테이너 사정 하나: Docker는 `/`를 마운트 전파 없이(private) 띄웁니다. 그러면 systemd가 유닛의 자격 증명을 그 유닛에 넘기지 못합니다. 그래서 컨테이너를 띄운 뒤 `mount --make-rshared /`를 합니다. 운영 서버의 `/`는 이미 shared이므로 이 일은 운영과 상관없습니다.

## 결과 (2026-10-13, 커밋 3813e40)

처음 연습은 커밋 1556981에서 했고, WP-O 검토의 지적을 고친 뒤 e4880b9에서, 원격 켜기의 서버 쪽(WP-W2)을 넣은 뒤 3813e40에서 다시 했습니다. 표는 마지막 연습의 결과입니다(전체 13분 23초, 빠른 연습은 2분 53초). WP-W2로 새로 생기거나 달라진 것에는 **(W2)**를 붙였습니다.

| 단계 | 한 것 | 결과 |
|---|---|---|
| 설치(7-3) | 묶음 배치의 파일을 7-3의 블록으로 설치하고 `systemd-analyze verify`(경보 템플릿은 `@check.` 인스턴스로) | `UNITS_OK` |
| 키(7-4) | SSO 값 둘, 기록 키, 목록 키 1번, 텔레그램(가짜) 봇을 `systemd-creds encrypt`로. 봉인 키는 매체 파일에서 표준 입력으로. **(W2)** 명령 키 2번은 매체에 `keygen command --kid 2 --out`으로, 1번은 서버에서 `keygen command --kid 1 --encrypt-to`로 | 자격 증명 7개. **(W2)** 명령 키를 만들 때 화면에는 공개키 한 줄(`N:<base64>`)과 지문 한 줄만 나옴. 두 공개키를 `HJV_API_COMMAND_PUBKEYS`에 |
| 시작(7-5) | 소켓과 서비스, 파일 권한, 스위치, selfcheck, 명단, 타이머 | 소켓 660 root:www-data, 상태·로그 디렉터리 700, DB·운영자 기록 600(hjviewer-api). 스위치 off. selfcheck 일곱 줄 `[ OK ]`. **(W2)** 그중 두 줄이 명령 키(중계기가 고정한 두 키의 지문, 쓰는 키가 고정된 1번)이고, 그 지문이 키를 만들 때 화면에 나온 지문과 같음. 점검·백업 타이머 둘 |
| 하루 사용 | PC 셋 묶기(하나는 바로 풂), 휴대폰 둘, 틀린 비밀번호 한 번, PC 둘 동기화 | 두 PC `active` |
| **(W2)** 원격 켜기 | PC가 동기화에 켜기 정보를 실음 → 중계기(대역)의 hello → `relay approve <등록 코드> --owner-of <PC ID>` → 휴대폰이 꺼진 PC의 켜기를 요청 → 중계기가 poll로 명령을 받아 서명을 확인하고 결과를 보냄 → PC가 켜져 동기화 | hello는 `pending`과 등록 코드. 승인 출력은 판정뿐(`on the PC's network: yes (the same network and gateway)`, `the same egress address as the PC: yes`, 첫 hello와 PC의 보고가 얼마나 됐는지)이고 주소·망·MAC이 없음. 켜기 요청 202 `queued`(중계기 이름 `NAS OWMX`, `exact`) → 서비스의 5초 점검이 `confirmed`(패킷 12개). `relay list` 활성 1, `health`의 `relays`가 `{"active":1,"silent":0}`, 5분 점검 통과(`relays 1 active`) |
| 매일 백업 | `hjviewer-api-backup.service` | `/var/lib/hjviewer-api/backup/hjv-api-20261013-060514Z.sqlite3`: 600 root, 디렉터리 700 root(그 위 `/var/lib/hjviewer-api`는 700 hjviewer-api). **(W2)** 스키마 3, 20개 표 65행(중계기 1, 켜기 작업 1, 켜기 정보 1 포함)을 서비스의 개수와 대조해 확인. 운영자 기록 두 줄(파일 이름, 개수) |
| 저널 | 이름 공간 `hjviewer` | `MaxRetentionSec=30day`, `MaxFileSec=1day`. 서비스·점검·백업·경보 유닛의 줄이 모두 이 공간에 |
| 경보: 시험 | `systemctl start hjviewer-api-alert@test.service` | 텔레그램 1건(호스트 이름, 시각) |
| 경보: invalid_client | 가짜 SSO가 `invalid_client`로 답하는 로그인(401) → 다음 로그인 503 → 점검 | 점검 실패(`sso_client`), 텔레그램 1건. 바로 다시 점검하면 같은 경보이므로 저널에만("6시간 뒤 다시") |
| 경보: 비정상 종료 | `systemctl kill -s SIGKILL hjviewer-api.service` | systemd가 5초 뒤 다시 시작. 텔레그램 1건(`종료: signal, killed KILL`) |
| 경보: 소켓 멈춤 | 소켓과 서비스를 멈추고 점검 | `socket`, `admin` 두 결과로 텔레그램 1건. 다시 시작한 뒤 통과 |
| 경보: 동기화 멈춤 | PC 둘이 켜진 채(가짜 hbbs) 동기화하지 않음 | 마지막 동기화 약 10분 뒤 `sync` 결과로 텔레그램 1건. 그동안 5분 타이머의 점검은 제 시각에 돌았음 |
| 꺼진 PC | 같은 PC들을 꺼짐으로(가짜 hbbs) | 점검 통과(`on 0`) |
| 복원 | 백업 뒤의 변경(로그인 둘, KIM이 자기 PC를 풂, 잃어버린 휴대폰처럼 HONG의 키 3개·토큰 3개 폐기: HONG의 PC는 `active`에 키 0개) 다음에 아침 백업으로 | 지금 DB는 `before-restore-202610130616.sqlite3`(87행)로 백업 폴더에. 복원한 서비스가 시작한 직후, 운영자의 다음 명령 전에 동기화한 HONG의 PC가 `disabled`(키 0개)를 받음. 저널에 "restored backup" 두 줄: 모든 계정을 껐다는 줄과, **(W2)** 그 백업에 활성 중계기가 1개 있다는 줄. 두 번째 `off`는 `(no change)`. `user revoke-all --all`이 키 5개·토큰 5개 폐기, 명단 4, 스위치 off, **(W2)** `relay list` 첫 줄(활성 1). 그 전 토큰 7개 모두 401. HONG의 PC는 묶인 채 `disabled`, 허용 표시를 지운 KIM의 PC는 `listed`(목록 없음). `passwordless on` 뒤 다시 로그인한 휴대폰의 새 키만 목록에 |
| 업그레이드 스냅숏 | `admin backup …/before-9.9.9.sqlite3`, 옛 실행 파일과 설정 복사, 새 판의 저장 이전을 흉내(**(W2)** `PRAGMA user_version`을 지금 번호보다 하나 위인 4로), 옛 실행 파일로 시작 | 옛 실행 파일이 시작을 거부(`the database has schema 4; this hjv-api knows up to 3`). 그 실패로 텔레그램 1건(`종료: exit-code, exited 1`, 비정상 종료와 다른 경보). 실행 파일·스냅숏·설정을 함께 되돌리자 시작했고, `report` 첫 줄이 스냅숏 때와 같음. 스냅숏도 꺼진 채로 시작(`off`는 `(no change)`) → `user revoke-all --all`(키 1개·토큰 1개) → 켬 |
| 14일 | 20일 된 백업 하나와 20일 된 `notes.txt`를 두고 백업 | 그 백업만 지움. `before-*`, 새 백업, `notes.txt`는 남음 |
| 호스트 재구축 | 새 컨테이너(B): 봉인 키는 매체에서, 목록 키는 매체의 2번(`HJV_API_LIST_KID=2`), **(W2)** 명령 키도 매체의 2번(`HJV_API_COMMAND_KID=2`), 기록 키는 새로. A의 마지막 백업으로 시작(꺼진 채, 첫 `off`는 `(no change)`) → `user revoke-all --all` → 켬 | selfcheck: 쓰는 목록 키가 고정된 2번, **(W2)** 쓰는 명령 키가 고정된 2번이고 그 지문이 매체에서 키를 만들 때 나온 지문과 같음. 같은 앱(같은 봉인 공개키)으로 로그인. HONG의 PC가 목록 키 2번으로 서명된 목록을 받아 PC의 확인을 통과. **(W2)** 백업에 있던 중계기가 다시 승인받지 않고 명령 키 2번으로 서명된 켜기 명령을 받아 서명을 확인함(`confirmed`, 패킷 12개). 중계기 빌드가 1번과 2번을 함께 고정하므로 서버만 바꾸면 됨 |

같은 복원 절차는 `cargo test`의 `tests/ops.rs`(`the_restore_procedure`)도 DB 수준에서 매번 돌립니다.

## 이 연습으로 찾아서 고친 것

- **시작 직후의 관리 명령**: `systemctl start` 바로 다음의 `hjv-api admin …`가 관리 소켓이 생기기 전에 실패했습니다. 런북 7-14 복원 블록의 `start` 다음 줄이 그 경우입니다. 이제 관리 명령은 시작 중인 서비스의 소켓을 10초까지 기다립니다.
- **경보의 호스트 이름**: 경보 유닛의 `ProcSubset=pid` 때문에 `/proc/sys/kernel/hostname`을 읽지 못해 이름이 비었습니다. `/etc/hostname`도 봅니다.
- **같은 유닛의 다른 실패**: 비정상 종료 뒤 6시간 안에 일어난 시작 실패(스키마 거부)가 같은 경보로 묶여 텔레그램에 가지 않았습니다. systemd가 `OnFailure=` 유닛에 넘기는 `MONITOR_SERVICE_RESULT`, `MONITOR_EXIT_CODE`, `MONITOR_EXIT_STATUS`를 메시지와 6시간 규칙에 넣었습니다.
- (시험 서버의 사정) 컨테이너에서는 `/`를 shared로 해야 자격 증명이 유닛에 닿습니다. 운영 서버와는 상관없습니다.

## 검토로 찾아서 고친 것 (WP-O 검토)

- **복원 직후의 동기화**: 첫 연습 때의 hjv-api는 복원한 DB로 시작한 때부터 두 번째 `passwordless off`까지 백업 때의 스위치(켜짐)와 백업 뒤에 폐기한 키로 PC의 동기화에 답했습니다. 검토가 이 연습으로 재현했습니다. 백업 뒤 `user revoke-all --all`(키 0개)을 한 다음 복원하자, 그 사이에 동기화한 PC가 폐기한 키 3개가 든 서명된 `active` 목록을 받았습니다. PC 10~20대가 2분마다 동기화하므로 몇 초 사이에도 일어나고, 그 PC는 다음 동기화까지, 서버에 닿지 못하면 최대 24시간 그 목록을 믿습니다. 이제 hjv-api는 백업의 응용 ID를 가진 DB를 처음 열 때, 첫 응답 전에 `passwordless off`와 같이 모든 계정을 끄고(같은 트랜잭션에서 응용 ID를 지움) 저널에 남깁니다. 연습은 아침 백업 뒤 HONG의 키를 폐기하고, 복원한 서비스가 시작한 직후 운영자의 다음 명령 전에 PC를 동기화해 `disabled`(키 0개)를 확인하고, 두 번째 `off`가 `(no change)`인지 봅니다. 업그레이드에서 되돌릴 때 넣는 스냅숏도 같습니다.
- **다시 설치한 PC의 거짓 경보**: 새 키로 다시 설치되어 동기화가 거절되는 PC(needs-relogin)를 hbbs가 켜짐으로 보면, 그 PC만 켜진 때 점검이 `sync` 경보를 냈습니다. 이제 점검은 hbbs가 저장된 키로 본 PC만 켜짐으로 셉니다(`tests/ops.rs`의 `the_check_with_the_service`).
- **백업 디렉터리**: `admin backup`이 없는 디렉터리를 부모만 있으면 어디든(`/etc/nologin/` 같은 곳도) 만들고, 백업이 실패해도 남겼습니다. 이제 매일 백업의 `/var/lib/hjviewer-api/backup`이거나 호출자의, 다른 사람에게 닫힌 디렉터리 안일 때만 만들고, 실패하면 지웁니다. `--keep-days`는 이 명령의 임시 파일 꼴(`.<이름>.sqlite3.<16진수 12자>.partial`)만 지웁니다.

## 원격 켜기(WP-W2)를 넣으며 달라진 것

- **명령 키가 있어야 서비스가 시작합니다.** 유닛에 `LoadCredentialEncrypted=hjviewer-api-command-key`가 생겼으므로, 7-4에서 명령 키 1번을 다른 키와 함께 만들어야 7-5의 시작이 됩니다(설계 11장 5단계가 이미 그 순서입니다). 유닛 밖에서(E2E의 `--credentials <폴더>`) 그 파일 없이 시작한 hjv-api는 켜기 요청에만 503 `unavailable`로 답하고 나머지는 그대로입니다.
- **`keygen command --kid N`의 화면**: 두 줄입니다. 첫 줄은 공개키로, `N:<base64>` 꼴 그대로여서(relay/PROTOCOL.md 3.2) `HJV_COMMAND_PUBKEY`와 `HJV_API_COMMAND_PUBKEYS`에 그대로 넣습니다. 둘째 줄은 지문(`fingerprint: N:<SHA-256 16진수>`)입니다. 키를 만든 사람이 적어 두었다가 `selfcheck`와 hjv-relay 빌드 요약의 지문과 대조하는 값이고, 연습은 두 서버에서 `selfcheck`의 명령 키 줄에 그 지문이 그대로 나오는지 봅니다.
- **복원과 중계기**: 복원은 중계기도 백업 때의 상태로 되돌립니다. 백업 뒤에 폐기한 중계기는 다시 활성이 되고, 무엇을 폐기했는지는 운영자 기록에만 있습니다. hjv-api는 복원한 백업에 활성 중계기가 있으면 시작할 때 저널에 그 수를 적습니다. 그런 중계기가 받는 것은 그 계정의 PC를 주인이 켤 때의 켜기 명령뿐이고 PC에 연결할 권한은 없으므로, 계정처럼 자동으로 끄지는 않고 복원 블록에 `relay list` 한 줄을 넣었습니다(아래 1번).
- **호스트 재구축과 명령 키 2번**: WP-O 때 연습에 없던 것입니다. 중계기 빌드가 명령 키 1번과 2번을 함께 고정하므로, 1번을 잃은 서버는 매체의 2번을 넣고 `HJV_API_COMMAND_KID=2`로 바꾸면 되고 NAS는 건드리지 않습니다. 연습의 B 서버가 그렇게 해서 백업에 있던 중계기에 2번으로 서명한 명령을 보냅니다.
- **업그레이드 단계의 스키마 번호**: 새 판의 저장 이전을 흉내 내는 번호를 3으로 적어 두었던 것을 "지금 번호 + 1"로 바꿨습니다. 스키마 3은 이제 진짜이기 때문입니다. 진짜 2 → 3 이전은 WP-W2 작업 중에 따로 한 번 봤습니다. WP-O 때의 실행 파일이 만든 스키마 2 DB를 새 실행 파일이 3으로 올려 시작했고, 그 DB를 옛 실행 파일은 `the database has schema 3; this hjv-api knows up to 2`로 거부했습니다.

## 교직원 전용 출입증(1.8.0)을 넣으며 달라진 것

[access-tickets.md](access-tickets.md)의 출입증 키가 생겼습니다. 이 판으로는 아직 연습을 돌리지 않았습니다(위의 결과는 3813e40의 것입니다).

- **출입증 키가 있어야 서비스가 시작합니다.** 유닛에 `LoadCredentialEncrypted=hjviewer-api-ticket-key`가 생겼으므로, 7-4에서 출입증 키 1번을 다른 키와 함께 만들어야 7-5의 시작이 됩니다. 이미 도는 서버를 이 묶음으로 바꿀 때도 그 키를 먼저 만듭니다. 런북 6절 "B 경로 업데이트"의 4번은 새 유닛이 요구하는 자격 증명이 없으면 아무것도 설치하지 않고 멈춥니다. 유닛 밖에서(E2E의 `--credentials <폴더>`) 그 파일 없이 시작한 hjv-api는 `POST /api/hjv/ticket`에 503 `unavailable`로 답하고, 로그인과 동기화의 답에 출입증이 없을 뿐 나머지는 그대로입니다.
- **A의 출입증 키**: 명령 키 1번처럼 서버에서 `keygen ticket --kid 1`의 비밀을 바로 `systemd-creds`로 넣고, 화면의 `1:<base64>`를 `HJV_API_TICKET_PUBKEYS`에 넣습니다. 연습은 `selfcheck`의 `ticket key in use` 줄에 화면의 지문이 그대로 나오는지 봅니다.
- **B의 출입증 키**: 앱이 출입증 키를 고정하지 않으므로 오프라인 사본이 없습니다. 다시 세운 B는 새 출입증 키를 만들어 그 값을 `HJV_API_TICKET_PUBKEYS`에 넣고, 연습은 B의 `selfcheck`에 그 지문이 나오는지 봅니다. 운영에서는 hbbs의 `access.conf`의 `KEY=`도 그 값으로 바꾸므로 A가 서명한 출입증은 모두 거절됩니다. 재구축의 `user revoke-all --all` 뒤 다시 로그인한 앱과, 다음 동기화를 한 묶인 PC가 새 출입증을 받습니다([access-tickets.md](access-tickets.md) 6-4).

## 열린 로그인(2026-10-14)을 넣으며 달라진 것

사용자 결정으로 연세 SSO를 통과한 사람은 누구나 로그인하고 처음 로그인할 때 명단에 오르며, 운영자는 명단에서 빼는 대신 막습니다(`admin block`, 스키마 5). 연습도 그에 맞췄습니다.

- **설치(7-5)**: 명단에 둘(HONG, KIM)과 `ZZSSO-INVALID-CLIENT`만 가져오고, LEE는 가져오지 않습니다. 하루 사용에서 LEE의 첫 로그인이 성공하고 `allow list` 첫 줄이 `allowlist: 4 IDs, 4 registered (1 by a login), 0 blocked; login open`인지 봅니다. 시작 때 `report` 둘째 줄(`login: open (…)`)도 찍습니다.
- **백업 뒤의 변경**: LEE를 막습니다(`admin block`). LEE의 토큰은 바로 401이고, 맞는 비밀번호의 로그인은 403 `blocked`입니다.
- **복원**: 복원한 DB에서는 그 막기가 풀려 있습니다. 런북 7-14대로 `user revoke-all --all` 뒤 운영자 기록의 `cmd="block" targets=<꼬리표>` 줄에서 꼬리표를 찾아 `block tag:<꼬리표>`로 다시 막고(연습에서는 기록의 막기가 모두 백업 뒤의 것), 복원 뒤 확인에서 LEE의 로그인이 다시 403인지 봅니다. `passwordless on` 뒤에는 막기를 풀고(`unblock`) LEE가 다시 로그인합니다.
- **업그레이드 단계**: 흉내 내는 번호가 "지금 번호 + 1"이므로 그대로입니다. 명단의 진짜 저장 이전(옛 명단의 항목은 `import`로, 명단에서 뺐던 계정은 막힌 항목으로)은 `cargo test`의 `db::tests::the_allowlist_of_schema_four_moves_on`이 봅니다.

## 행정허브 SSO 계약과 ID 규칙(2026-10-14)을 넣으며 달라진 것

hjv-api가 같은 서버의 행정허브 WordPress 플러그인과 같은 SSO 등록의 답(사용자 정보의 `pcode`, `name`)을 읽고, HJViewer의 ID 규칙(대문자로 바꿔 Z나 Y로 시작, ZA 제외)에 맞는 ID만 SSO에 묻게 되었습니다. 연습의 가짜 SSO도 행정허브의 모양으로 답하고, 대문자로 바꾸지 않은 사용자 이름이나 계약과 다른 양식은 거절합니다. 연습의 시험 계정은 규칙에 맞는 가짜 ID입니다: HONG `ZZTEST01`, KIM `ZZTEST02`, 가져오지 않는 LEE `YZTEST03`(Y 코드). SSO의 `invalid_client`를 흉내 내는 이름은 `ZZSSO-INVALID-CLIENT`입니다. 연습의 단계와 기대하는 결과는 그대로입니다.

## 런북과 다른 것 (런북에 반영함)

연습할 때 런북은 문서 작업(`v3-docs`)에 있었고, 이 연습과 구현은 아래처럼 달랐습니다. 저장소를 합치기 전에 1~6번은 런북(7-4, 7-5, 7-11, 7-12, 7-14, 7-15, 7-16)에, 7번은 개인정보 처리방침과 `docs/wol-pc-setup.md`에 반영했습니다. 1~3번은 7-14이고, 4번부터는 원격 켜기(WP-W2)로 생긴 것입니다.

1. **복원 블록**: 앞에서 점검 타이머를 멈춥니다. 그대로 두면 서비스를 멈춘 동안 점검이 경보를 냅니다(위의 "소켓 멈춤"이 그 경보). 지금 DB는 `/root/hjv-backup/`로 옮기지 않고, 서비스를 멈추기 전에 `admin backup`으로 백업 폴더에 남깁니다. 백업 폴더의 파일은 14일 뒤 지워지므로 개인정보 처리방침의 "백업 14일"과 맞습니다. 복원한 백업은 모든 계정이 꺼진 채로 시작하므로 두 번째 `passwordless off`는 확인입니다. 중계기는 백업 때의 상태로 돌아오므로 마지막 줄에서 `relay list`를 봅니다. 런북 7-14의 복원 블록과 같습니다.
   ```bash
   B='/var/lib/hjviewer-api/backup/<백업 파일>'   # ← 사용자가 고른 백업
   A="sudo /opt/hjviewer/bin/hjv-api admin"
   D=/var/lib/hjviewer-api/hjv-api.sqlite3
   sudo test -s "$B" || { echo 'STOP: 백업 파일이 없음'; exit 1; }
   $A passwordless off
   sudo systemctl stop hjviewer-api-check.timer   # 서비스를 멈춘 동안 점검이 경보를 내지 않게
   # 지금 DB를 백업 폴더에 남김(다른 백업과 함께 14일 뒤 지워짐)
   $A backup "/var/lib/hjviewer-api/backup/before-restore-$(date -u +%Y%m%d%H%M).sqlite3" || { echo 'STOP: 지금 DB를 백업하지 못함'; exit 1; }
   sudo systemctl stop hjviewer-api.socket hjviewer-api.service   # 소켓도 멈춤: 그대로 두면 요청이 서비스를 다시 띄움
   sudo rm -f "$D" "$D-wal" "$D-shm"
   sudo install -o hjviewer-api -g hjviewer-api -m 600 "$B" "$D"
   sudo systemctl start hjviewer-api.socket hjviewer-api.service   # 모든 계정이 꺼진 채로 시작
   $A passwordless off             # 확인: "(no change)"여야 함
   $A user revoke-all --all        # 모든 계정 키와 토큰을 폐기
   $A passwordless status; $A allow list | head -1; $A report | head -7
   $A relay list                   # 백업 뒤에 폐기한 중계기가 다시 활성이면 relay revoke
   ```
   두 번째 `off`가 `(no change)`가 아니면 그대로 보고합니다. `STOP: 지금 DB를 백업하지 못함`이면 사용자에게 보고하고 점검 타이머를 다시 켭니다. DB가 망가져 백업이 되지 않는 경우에만, 사용자 승인 뒤 `rm -f` 대신 그 파일들을 `/root/hjv-backup/before-restore-<시각>/`(700)로 옮기고 그 사본은 14일 안에 지웁니다. 백업 뒤에 폐기한 중계기는 운영자 기록의 `relay revoke` 줄로 찾아, 사용자 승인 뒤 다시 폐기합니다. 명단과 스위치를 확인한 뒤 사용자 승인을 받아 `passwordless on`과 `sudo systemctl start hjviewer-api-check.timer`.
2. **업그레이드에서 되돌리기**: 옛 실행 파일과 설정 파일을 제자리에 넣고 위 블록을 `B`=스냅숏으로 합니다. 새 판이 돌지 않아 `backup` 줄이 실패하면 1번처럼 사용자 승인 뒤 파일을 옮깁니다. 스냅숏도 꺼진 채로 시작하고, 확인과 `user revoke-all --all` 뒤 사용자 승인을 받아 켭니다.
3. **감시와 경보 경로**: 7-14의 두 항목을 [README](../server/api/README.md)의 "Monitoring"에 맞춥니다. 점검은 `report`의 줄이 아니라 관리 소켓의 `health`를 봅니다. 소켓이나 관리 소켓의 무응답, 켜진 PC(hbbs가 저장된 키로 90초 안에 본 것이 4분 동안 점검마다 보인 PC)가 있는데 어느 것도 10분 안에 동기화하지 않음, hbbs 콘솔의 무응답, SSO 호출 중지(`invalid_client`와 주소 거절을 구분), 2단계의 중계기가 경보입니다. 꺼진 PC와 needs-relogin인 PC는 경보를 내지 않습니다. 경보는 언제나 저널에 한 줄이고, 텔레그램은 같은 유닛이 같은 방식으로 끝나고 결과도 같을 때만 6시간에 한 번이며 그 밖에는 바로 갑니다. 사용자가 봇을 넣는 명령과 시험 메시지(`sudo systemctl start hjviewer-api-alert@test.service`)도 런북에 둡니다.
4. **7-5의 selfcheck**: 명령 키 두 줄이 더 나옵니다(`command keys the relays pin …`, `command key in use (HJV_API_COMMAND_KID 1): the pinned key 1, …`). `HJV_API_COMMAND_PUBKEYS`를 넣지 않으면 쓰는 키를 대조할 수 없어 그 줄이 `[FAIL]`이므로, 7-5에서 목록 키의 값과 함께 넣습니다. 7-16의 보고에는 명령 키 지문도 적습니다.
5. **7-11의 승인**: 판정이 "같은 망"이 아니면 승인 도구가 스스로 멈춥니다(`nothing changed: …`). PC 주인이 코드를 확인했는데도 판정이 나오지 않을 때만(PC가 아직 켜기 정보를 보내지 않았을 때 등) 사용자 승인 뒤 `--force`를 붙입니다. 중계기 이름은 `--name <이름>`으로 정합니다. 앱이 그 이름을 그대로 보이므로 "연구실 NAS"처럼 혼자서 읽히는 이름이 좋고, 정하지 않으면 `NAS`와 코드 첫 묶음입니다. 이미 승인한 중계기에 같은 명령을 `--name`과 함께 다시 하면 이름만 바뀝니다. 승인한 뒤 `relay list`로 그 계정의 PC마다 판정을 다시 봅니다.
6. **7-12와 7-15**: 폐기한 중계기의 키는 다시 승인되지 않습니다. 잘못 폐기했으면 NAS에서 중계기의 상태 폴더를 지워([relay-dsm.md](relay-dsm.md)의 제거 절차) 새 키와 새 등록 코드로 다시 승인받습니다. 계정을 지우면(`user delete`) 그 계정의 중계기는 폐기되고 켜기 기록은 함께 지워집니다.
7. **다른 문서**: 개인정보 처리방침의 "켜기 요청" 줄에 그 PC의 네트워크(망 번호)를 더합니다. 같은 네트워크에서 켠 적이 있는 중계기를 고르는 데 씁니다. PC 준비 안내(문서 작업의 `docs/wol-pc-setup.md`)의 "안 될 때"에는 두 가지를 더합니다. 앱에 켜기가 없는 경우에 "내 계정에 승인된 중계기가 없을 때"가 있고, 준비 안내에 "랜카드 드라이버가 매직 패킷 깨우기 설정을 알려 주지 않음"(3장 4번)이 있습니다. 서버가 켤 수 없다고 판단해 켜기를 빼는 것은 유선 랜카드가 없는 PC뿐이고, 나머지는 안내로만 붙습니다.

호스트 재구축은 런북 그대로이고, 명령 키도 7-15의 "명령 키 바꾸기"대로 2번을 넣습니다.

## 이 연습이 다루지 않는 것

- 진짜 텔레그램과 진짜 연세 SSO, 패치 0004의 진짜 hbbs, nginx를 거친 요청. nginx 뒤의 hjv-api와 유닛의 `systemd-analyze verify`는 `scripts/ci.sh deploy`가 봅니다.
- 진짜 hjv-relay 실행 파일과 매직 패킷, 실제로 깨어나는 PC. 중계기는 시험 프로그램이 대신하고, 켜짐은 가짜 hbbs와 PC의 동기화로 알립니다. WOL E2E(WP-W5)와 현장 시험의 몫입니다.
- 운영자 기록의 `chattr +a`: 컨테이너의 파일 시스템이 지원하지 않습니다.
- 진짜 판 올림: 연습은 `PRAGMA user_version`으로 다음 판의 저장 이전을 흉내 냅니다. 2 → 3은 위에 적은 대로 따로 한 번 봤습니다.
- 디스크를 잃고 백업도 없는 재구축: DB가 빈 채 시작하는 것은 A의 첫 설치와 같고, 그 뒤 막기를 서버 밖에 적어 둔 것으로 다시 하고(명단 방식이면 명단도 다시 넣고) 모두 다시 로그인하며, 중계기는 hello를 다시 보내 다시 승인받습니다.
- 실제 시간의 14일과 30일: 14일은 파일 시각을 앞당겨 봤고, 30일은 저널 설정만 확인했습니다.
- 매일 백업 타이머가 03:30에 도는 것: 타이머를 켜고 그 서비스를 직접 시작했습니다. 5분 점검 타이머는 연습 중에도 제 시각에 돌았습니다.
