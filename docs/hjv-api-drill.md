# hjv-api 운영 연습 (감시·백업·복원)

설계 3판 5-9와 작업 묶음 WP-O는 출시 전에 hjv-api의 복원 절차를 시험 서버에서 한 번 연습하라고 합니다. 이 문서는 그 연습의 기록입니다. 연습은 `tools/e2e/hjv-api-drill.sh`가 하므로 언제든 다시 돌릴 수 있습니다. 감시와 백업의 동작 원리와 명령은 [server/api/README.md](../server/api/README.md)의 "Monitoring"과 "Backup and restore"에 있습니다.

```bash
tools/e2e/hjv-api-drill.sh                     # 약 13분(동기화가 10분 묵기를 기다리는 시간 포함)
HJV_DRILL_QUICK=1 tools/e2e/hjv-api-drill.sh   # 그 기다림 없이 약 3분
```

출력은 `build/drill/drill.log`에 남습니다. 필요한 것은 cargo, sudo로 쓰는 Docker(`scripts/ci.sh`와 같음), 이미지를 만들 때 Ubuntu 패키지 저장소입니다.

## 시험 서버

- 개발 PC(WSL2 Ubuntu 24.04)의 Docker 컨테이너입니다. 운영 서버와 같은 Ubuntu 24.04이고, systemd 255가 PID 1로 돕니다. 첫 서버(A)와 다시 세운 서버(B), 두 대를 차례로 띄우고 끝나면 지웁니다. 밖으로 나가는 연결은 없습니다.
- 설치는 런북 7절의 7-3~7-5를 그대로 따릅니다. 서버 묶음과 같은 배치(`bin/hjv-api`, `systemd/`의 `hjviewer-api*` 유닛·env 예시·`journald@hjviewer.conf`)를 쓰고, 유닛은 `systemd-analyze verify`로 확인하고, 자격 증명은 `systemd-creds encrypt`로 넣고(봉인 키와 목록 키 2번은 "이동식 매체" 파일에서 표준 입력으로), 설정은 `sed`로 고칩니다.
- 대역(가짜)은 셋입니다. 가짜 SSO(`hjv-fake-sso`), 시험 PC를 켜짐으로 답하는 가짜 hbbs 콘솔, 받은 메시지를 적어 두는 가짜 텔레그램입니다. 경보 유닛만 드롭인으로 가짜 텔레그램을 가리킵니다. 키는 `server/api/tests/common`의 공개된 시험 키입니다. 앱과 PC의 요청은 `server/api/tests/drill.rs`가 컨테이너 안에서 보냅니다.
- 컨테이너 사정 하나: Docker는 `/`를 마운트 전파 없이(private) 띄웁니다. 그러면 systemd가 유닛의 자격 증명을 그 유닛에 넘기지 못합니다. 그래서 컨테이너를 띄운 뒤 `mount --make-rshared /`를 합니다. 운영 서버의 `/`는 이미 shared이므로 이 일은 운영과 상관없습니다.

## 결과 (2026-10-13, 커밋 e4880b9)

처음 연습은 커밋 1556981에서 했고, 아래 "검토로 찾아서 고친 것"을 고친 뒤 같은 날 다시 했습니다. 표는 다시 한 연습의 결과입니다(전체 약 12분).

| 단계 | 한 것 | 결과 |
|---|---|---|
| 설치(7-3) | 묶음 배치의 파일을 7-3의 블록으로 설치하고 `systemd-analyze verify`(경보 템플릿은 `@check.` 인스턴스로) | `UNITS_OK` |
| 키(7-4) | SSO 값 둘, 기록 키, 목록 키 1번, 텔레그램(가짜) 봇을 `systemd-creds encrypt`로. 봉인 키는 매체 파일에서 표준 입력으로 | 자격 증명 6개 |
| 시작(7-5) | 소켓과 서비스, 파일 권한, 스위치, selfcheck, 명단, 타이머 | 소켓 660 root:www-data, 상태·로그 디렉터리 700, DB·운영자 기록 600(hjviewer-api). 스위치 off. selfcheck 다섯 줄 `[ OK ]`. 점검·백업 타이머 둘 |
| 하루 사용 | PC 셋 묶기(하나는 바로 풂), 휴대폰 둘, 틀린 비밀번호 한 번, PC 둘 동기화 | 두 PC `active` |
| 매일 백업 | `hjviewer-api-backup.service` | `/var/lib/hjviewer-api/backup/hjv-api-20261013-034738Z.sqlite3`: 600 root, 디렉터리 700 root(그 위 `/var/lib/hjviewer-api`는 700 hjviewer-api). 14개 표 55행을 서비스의 개수와 대조해 확인. 운영자 기록 두 줄(파일 이름, 개수) |
| 저널 | 이름 공간 `hjviewer` | `MaxRetentionSec=30day`, `MaxFileSec=1day`. 서비스·점검·백업·경보 유닛의 줄이 모두 이 공간에 |
| 경보: 시험 | `systemctl start hjviewer-api-alert@test.service` | 텔레그램 1건(호스트 이름, 시각) |
| 경보: invalid_client | 가짜 SSO가 `invalid_client`로 답하는 로그인(401) → 다음 로그인 503 → 점검 | 점검 실패(`sso_client`), 텔레그램 1건. 바로 다시 점검하면 같은 경보이므로 저널에만("6시간 뒤 다시") |
| 경보: 비정상 종료 | `systemctl kill -s SIGKILL hjviewer-api.service` | systemd가 5초 뒤 다시 시작. 텔레그램 1건(`종료: signal, killed KILL`) |
| 경보: 소켓 멈춤 | 소켓과 서비스를 멈추고 점검 | `socket`, `admin` 두 결과로 텔레그램 1건. 다시 시작한 뒤 통과 |
| 경보: 동기화 멈춤 | PC 둘이 켜진 채(가짜 hbbs) 동기화하지 않음 | 마지막 동기화 약 10분 뒤 `sync` 결과로 텔레그램 1건. 그동안 5분 타이머의 점검은 제 시각에 돌았음 |
| 꺼진 PC | 같은 PC들을 꺼짐으로(가짜 hbbs) | 점검 통과(`on 0`) |
| 복원 | 백업 뒤의 변경(로그인 둘, KIM이 자기 PC를 풂, 잃어버린 휴대폰처럼 HONG의 키 3개·토큰 3개 폐기: HONG의 PC는 `active`에 키 0개) 다음에 아침 백업으로 | 지금 DB는 `before-restore-202610130358.sqlite3`(77행)로 백업 폴더에. 복원한 서비스가 시작한 직후, 운영자의 다음 명령 전에 동기화한 HONG의 PC가 `disabled`(키 0개)를 받음. 저널에 "restored backup" 한 줄, 두 번째 `off`는 `(no change)`. `user revoke-all --all`이 키 5개·토큰 5개 폐기, 명단 4, 스위치 off. 그 전 토큰 7개 모두 401. HONG의 PC는 묶인 채 `disabled`, 허용 표시를 지운 KIM의 PC는 `listed`(목록 없음). `passwordless on` 뒤 다시 로그인한 휴대폰의 새 키만 목록에 |
| 업그레이드 스냅숏 | `admin backup …/before-9.9.9.sqlite3`, 옛 실행 파일과 설정 복사, 새 판의 저장 이전을 흉내(`PRAGMA user_version = 3`), 옛 실행 파일로 시작 | 옛 실행 파일이 시작을 거부(`the database has schema 3; this hjv-api knows up to 2`). 그 실패로 텔레그램 1건(`종료: exit-code, exited 1`, 비정상 종료와 다른 경보). 실행 파일·스냅숏·설정을 함께 되돌리자 시작했고, `report` 첫 줄이 스냅숏 때와 같음. 스냅숏도 꺼진 채로 시작(`off`는 `(no change)`) → `user revoke-all --all`(키 1개·토큰 1개) → 켬 |
| 14일 | 20일 된 백업 하나와 20일 된 `notes.txt`를 두고 백업 | 그 백업만 지움. `before-*`, 새 백업, `notes.txt`는 남음 |
| 호스트 재구축 | 새 컨테이너(B): 봉인 키는 매체에서, 목록 키는 매체의 2번(`HJV_API_LIST_KID=2`), 기록 키는 새로. A의 마지막 백업으로 시작(꺼진 채, 첫 `off`는 `(no change)`) → `user revoke-all --all` → 켬 | selfcheck: 쓰는 목록 키가 고정된 2번. 같은 앱(같은 봉인 공개키)으로 로그인. HONG의 PC가 목록 키 2번으로 서명된 목록을 받아 PC의 확인을 통과 |

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

## 런북 7-14와 다른 것 (저장소를 합칠 때 런북에 넣음)

런북 7-14는 문서 작업(`v3-docs`)에 있고, 이 연습과 구현은 아래처럼 다릅니다. 설계 12-1의 5단계에서 저장소를 합칠 때 7-14를 이대로 고칩니다.

1. **복원 블록**: 앞에서 점검 타이머를 멈춥니다. 그대로 두면 서비스를 멈춘 동안 점검이 경보를 냅니다(위의 "소켓 멈춤"이 그 경보). 지금 DB는 `/root/hjv-backup/`로 옮기지 않고, 서비스를 멈추기 전에 `admin backup`으로 백업 폴더에 남깁니다. 백업 폴더의 파일은 14일 뒤 지워지므로 개인정보 처리방침의 "백업 14일"과 맞습니다. 복원한 백업은 모든 계정이 꺼진 채로 시작하므로 두 번째 `passwordless off`는 확인입니다. 런북에 넣을 블록입니다.
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
   $A passwordless status; $A allow list | head -1; $A report | head -5
   ```
   두 번째 `off`가 `(no change)`가 아니면 그대로 보고합니다. `STOP: 지금 DB를 백업하지 못함`이면 사용자에게 보고하고 점검 타이머를 다시 켭니다. DB가 망가져 백업이 되지 않는 경우에만, 사용자 승인 뒤 `rm -f` 대신 그 파일들을 `/root/hjv-backup/before-restore-<시각>/`(700)로 옮기고 그 사본은 14일 안에 지웁니다. 명단과 스위치를 확인한 뒤 사용자 승인을 받아 `passwordless on`과 `sudo systemctl start hjviewer-api-check.timer`.
2. **업그레이드에서 되돌리기**: 옛 실행 파일과 설정 파일을 제자리에 넣고 위 블록을 `B`=스냅숏으로 합니다. 새 판이 돌지 않아 `backup` 줄이 실패하면 1번처럼 사용자 승인 뒤 파일을 옮깁니다. 스냅숏도 꺼진 채로 시작하고, 확인과 `user revoke-all --all` 뒤 사용자 승인을 받아 켭니다.
3. **감시와 경보 경로**: 7-14의 두 항목을 [README](../server/api/README.md)의 "Monitoring"에 맞춥니다. 점검은 `report`의 줄이 아니라 관리 소켓의 `health`를 봅니다. 소켓이나 관리 소켓의 무응답, 켜진 PC(hbbs가 저장된 키로 90초 안에 본 것이 4분 동안 점검마다 보인 PC)가 있는데 어느 것도 10분 안에 동기화하지 않음, hbbs 콘솔의 무응답, SSO 호출 중지(`invalid_client`와 주소 거절을 구분), 2단계의 중계기가 경보입니다. 꺼진 PC와 needs-relogin인 PC는 경보를 내지 않습니다. 경보는 언제나 저널에 한 줄이고, 텔레그램은 같은 유닛이 같은 방식으로 끝나고 결과도 같을 때만 6시간에 한 번이며 그 밖에는 바로 갑니다. 사용자가 봇을 넣는 명령과 시험 메시지(`sudo systemctl start hjviewer-api-alert@test.service`)도 런북에 둡니다.

호스트 재구축은 런북 그대로입니다.

## 이 연습이 다루지 않는 것

- 진짜 텔레그램과 진짜 연세 SSO, 패치 0004의 진짜 hbbs, nginx를 거친 요청. nginx 뒤의 hjv-api와 유닛의 `systemd-analyze verify`는 `scripts/ci.sh deploy`가 봅니다.
- 운영자 기록의 `chattr +a`: 컨테이너의 파일 시스템이 지원하지 않습니다.
- 진짜 판 올림: 스키마 3이 아직 없어 `PRAGMA user_version`으로 새 판의 저장 이전을 흉내 냈습니다.
- 디스크를 잃고 백업도 없는 재구축: DB가 빈 채 시작하는 것은 A의 첫 설치와 같고, 그 뒤 명단을 다시 넣고 모두 다시 로그인합니다.
- 실제 시간의 14일과 30일: 14일은 파일 시각을 앞당겨 봤고, 30일은 저널 설정만 확인했습니다.
- 매일 백업 타이머가 03:30에 도는 것: 타이머를 켜고 그 서비스를 직접 시작했습니다. 5분 점검 타이머는 연습 중에도 제 시각에 돌았습니다.
