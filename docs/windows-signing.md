# Windows Authenticode 서명

Windows 출시 빌드는 SSL.com의 개인용 IV Code Signing 인증서와 eSigner로 서명합니다. `.github/workflows/client-build.yml`에 서명 경로를, `scripts/windows-signing.ps1`에 서명 전후 검사와 파일 보존 검사를 구현했습니다. **실제 인증서 발급, 시크릿 설정, 서명 빌드, Microsoft Store 재제출은 아직 하지 않았습니다.** 이 문서는 그 준비와 첫 실행 절차입니다(2026-10-18 기준).

운영 중인 1.8.0의 Windows 파일과 설치용 사본은 이미 정책에 SHA-256이 고정되어 있습니다. 이 파일에 서명을 덧붙이거나 같은 URL의 파일을 바꾸지 않습니다. 첫 서명 출시는 **새 버전**(예: 1.8.1)으로 만들며, 최소 버전은 1.8.0으로 유지할 수 있습니다. 앱 파일의 Authenticode 서명과 서버가 배포하는 버전 정책의 서명은 별개입니다. 새 정책 서명과 운영 반영은 [배포 절차](distribution.md#운영자-새-버전을-낼-때)에 따라 사용자와 운영 담당자에게 인계합니다.

## 인증서와 비용

IV 인증서는 법인 등록 없이 개인의 검증된 이름으로 발급하는 코드 서명 인증서입니다. Windows에 표시되는 게시자도 인증서의 개인 이름입니다. SSL.com의 신원 확인 안내는 한국을 지원 대상 지역에 포함합니다. 실제 발급은 SSL.com의 신원 확인을 통과해야 합니다. [IV 제품 안내](https://www.ssl.com/products/software-integrity/code-signing/iv/), [신원 확인 안내](https://www.ssl.com/guide/identity-validation-for-ssl-com-certificates-a-complete-guide/)

아래는 2026-10-18에 확인한 미국 달러 표시 가격입니다. 인증서와 eSigner 구독은 별도이며, 최종 결제 금액과 사용량은 구매 화면에서 확인합니다.

| 항목 | 표시 가격 | 포함량 |
|---|---|---|
| 개인용 IV 인증서, 1년 | 연 $129 | 인증서 |
| eSigner IV Tier 1, 월 결제 | 월 $20 | 월 20회 서명, 자격 증명 1개 |
| eSigner IV Tier 1, 연 결제 | 연 $180 | 연 240회 서명, 자격 증명 1개 |
| Tier 1 초과 서명 | 서명당 $1 | 포함량 초과분 |

가격 근거: [IV 인증서](https://www.ssl.com/products/software-integrity/code-signing/iv/), [eSigner 가격표](https://www.ssl.com/guide/esigner-pricing/).

**배치 실행 한 번을 과금 한 번으로 세지 않습니다.** 내부의 미서명 PE 파일 각각과 바깥 설치 실행 파일의 서명을 합쳐 사용량을 예상합니다. 이미 유효하게 서명된 파일은 다시 서명하지 않고, 설치용 이름의 사본도 서명된 바깥 파일을 복사하므로 추가 서명하지 않습니다. 실제 사용량은 eSigner 계정에서 확인합니다. `check_only=true`는 eSigner를 호출하지 않으므로 서명 사용량을 소모하지 않습니다. GitHub Actions 빌드 비용은 별개입니다.

## 사용자가 한 번 준비할 것

1. SSL.com에서 개인용 IV Code Signing 인증서를 주문하고 신원 확인과 결제를 마칩니다. eSigner에 사용할 인증서를 발급받습니다. 신분증은 SSL.com의 검증 화면에 직접 제출합니다.
2. 그 인증서의 eSigner 자격 증명을 만들고 자동화용 TOTP 설정을 준비합니다. 인증서의 코드 서명 용도, 유효 기간, 게시자 이름을 확인합니다.
3. 인증서의 공개 SHA-1 지문을 확인합니다. 아래 값에는 공백과 콜론을 뺀 **40자리 16진수**를 넣습니다. 파일의 SHA-256이나 eSigner Credential ID와 다른 값입니다.
4. 비공개 빌드 저장소 `yonsei-projecttf/hjviewer-private`의 Settings → Secrets and variables → Actions → **Repository secrets**에 아래 다섯 항목을 직접 넣습니다. Environment secrets는 이 재사용 워크플로의 `secrets: inherit`로 전달되지 않습니다.

| Repository secret | 내용 |
|---|---|
| `SSLCOM_USERNAME` | SSL.com/eSigner 계정 사용자 이름 |
| `SSLCOM_PASSWORD` | 그 계정의 비밀번호 |
| `SSLCOM_CREDENTIAL_ID` | 서명에 사용할 eSigner 자격 증명의 ID |
| `SSLCOM_TOTP_SECRET` | 자동화용 TOTP 비밀값. 화면에 잠깐 표시되는 일회용 숫자 코드가 아님 |
| `WINDOWS_SIGNING_CERT_THUMBPRINT` | 발급된 코드 서명 인증서의 SHA-1 지문, 공백·콜론 없는 40자리 16진수 |

지문은 공개 값이지만 설정을 한곳에서 관리하기 위해 secret으로 전달합니다. 비밀번호, TOTP 비밀값, 신분증, 개인키를 채팅·이슈·문서에 보내지 않습니다. 에이전트에게는 발급과 설정을 마쳤는지만 알립니다. 개인키는 eSigner가 관리하며 저장소에 PFX나 개인키를 넣지 않습니다.

인증서를 갱신하거나 바꾸면 새 인증서의 지문과 Credential ID를 함께 확인합니다. 지문이 맞지 않는 서명을 허용하도록 검사를 끄지 않습니다.

## CI가 서명하는 순서

Windows의 `check_only=false` 출시 빌드는 비공개 빌드 저장소에서만 서명하며 위 다섯 설정이 필수입니다. 설정이 없거나 검증이 실패하면 미서명 출시 파일로 대체하지 않고 작업을 실패시킵니다.

1. Rust·Flutter 앱, 보조 DLL과 기존 드라이버를 빌드·수집합니다. `windows-signing.ps1`이 내부 파일의 해시와 서명 상태를 조사합니다. 확장자만 보고 고르지 않고 PE 형식을 확인합니다.
2. **서명이 없는 내부 PE만** 파일명이 겹치지 않는 평면 디렉터리로 복사해 eSigner에 전달합니다. 원래 경로와 해시는 별도 기록으로 보존합니다. 기존의 유효한 Microsoft 드라이버 서명과 CAT 파일은 그대로 둡니다. 폐기되었거나 변조된 서명을 새 서명으로 덮어 고치지 않고 실패시킵니다.
3. 서명 결과 전체를 먼저 확인합니다. Authenticode 상태 `Valid`, 예상 인증서 지문, 코드 서명 용도, 신뢰할 수 있는 타임스탬프와 Windows 인증서 체인을 확인합니다. 원본과 파일 목록이 일치해야 하고, PE 본문은 서명에 필요한 체크섬·인증서 디렉터리·서명 블록 외에는 바이트가 같아야 합니다. 모두 통과한 뒤에만 내부 파일에 반영합니다.
4. 서명된 내부 파일을 packer로 묶어 자동 압축 해제 실행 파일을 만듭니다. 이 **바깥 실행 파일도 서명하고 같은 검사를 거칩니다.**
5. 검증된 바깥 파일을 `HJViewer.exe`와 설치용 `HJViewer-install.exe`로 복사합니다. `Audit` 단계가 내부와 바깥 파일의 서명을 다시 확인하며, 두 배포 파일은 같은 바이트와 SHA-256이어야 합니다. 최종 파일로 `release.json`과 `SHA256SUMS`를 만들므로 서명 전 해시가 정책에 들어가지 않습니다.
6. 기존 설치·업데이트·packer smoke test를 돌립니다. 최종 패키지와 해시를 받은 뒤에는 다시 서명하거나 파일 내용을 바꾸지 않습니다.

확인 빌드(`check_only=true`)는 eSigner 호출을 건너뛰고 미서명 Windows 결과물을 `-check` 이름으로 만듭니다. 서명 설정 없이 실행할 수 있고 `release.json`이 없으므로 릴리스 정책의 입력으로 쓸 수 없습니다. [CI의 확인 빌드와 릴리스 기록](ci.md)을 함께 봅니다.

### 서명 도구 고정과 기록

- eSigner 액션은 [`SSLcom/esigner-codesign` v1.3.2의 커밋 `cf5f6c1d38ad10f47e3ed9aca873f429b1a8d85b`](https://github.com/SSLcom/esigner-codesign/tree/cf5f6c1d38ad10f47e3ed9aca873f429b1a8d85b)에 고정합니다.
- 액션 내부에서 받는 도구도 따로 고정합니다. `Install pinned CodeSignTool` 단계는 [CodeSignTool 1.3.0 Windows ZIP](https://github.com/SSLcom/CodeSignTool/releases/download/v1.3.0/CodeSignTool-v1.3.0-windows.zip)의 SHA-256이 `e22094505decbe622afe5b0c27abc618ed2ba179bd94f3450490352399d5ef2a`와 같아야 압축을 풉니다. 버전이나 digest를 바꿀 때는 액션과 실제 다운로드 경로를 함께 검토합니다.
- 서명 검사 보고서 `runtime-report.json`, `installer-report.json`은 비공개 빌드 실행의 별도 아티팩트 `<BUILD_NAME>-windows-signatures`에 7일 보관합니다. 보고서는 인증서 지문과 검사 결과를 담으므로 일반 앱 배포 묶음에 추가하지 않습니다. 실제 인증서 이름이나 시크릿을 문서에 옮기지 않습니다.

## 첫 서명 출시

인증서와 다섯 시크릿이 준비된 뒤 진행합니다. 아래 명령은 실행 예이며, 이 문서 작성 과정에서 서명 빌드를 실행한 것은 아닙니다.

1. `client/version.env`를 새 버전으로 올리고(예: 1.8.1) 확인한 공개 저장소 커밋을 push합니다. 비공개 빌드 저장소의 `uses`와 `source_ref`를 그 커밋의 같은 40자리 SHA로 고정합니다. 현재 운영 중인 1.8.0 커밋으로 다시 출시 빌드하지 않습니다.
2. 필요하면 먼저 `check_only=true`로 서명 비용 없이 빌드 경로를 확인합니다. 확인 빌드가 실제 eSigner 인증·서명 성공을 증명하지는 않습니다.
3. 서명 출시 빌드를 실행합니다.

   ```bash
   gh workflow run build.yml -R yonsei-projecttf/hjviewer-private \
     -f platforms=windows -f check_only=false
   ```

4. Windows 작업과 서명 검사·smoke test가 모두 성공했는지 확인하고 7일 안에 앱 아티팩트와 별도 서명 보고서를 받습니다. `SHA256SUMS`, 소스 커밋, 빌드 실행 ID, 두 exe의 동일성을 확인합니다. 운영자 Windows PC에서 게시자 이름과 설치·실행·업데이트도 확인합니다.
5. 새 `client-<버전>` 태그와 최종 서명 파일의 해시로 정책 초안을 만듭니다. 최소 버전은 1.8.0으로 유지할 수 있습니다. **정책 서명은 사용자가 하고**, 운영 반영은 [배포 절차](distribution.md#운영자-새-버전을-낼-때)로 인계합니다. 기존 1.8.0 파일이나 URL을 새 파일로 덮어쓰지 않습니다.

실패한 작업을 다시 실행하기 전에는 실패 지점을 확인합니다. 인증 실패, 예상 지문 불일치, 기존 서명 오류, 타임스탬프·인증서 체인 오류, PE 본문 변경을 각각 해결하며 검사를 우회하지 않습니다. 이미 서명 요청을 보낸 실행의 재시도는 사용량을 추가로 소모할 수 있습니다.

## Microsoft Store 제출과의 관계

Microsoft Store의 EXE/MSI 경로는 설치 실행 파일뿐 아니라 포함된 **모든 PE 파일**에 신뢰하는 CA로 이어지는 코드 서명을 요구합니다. 제출 URL은 버전별이어야 하고 제출한 URL의 바이너리는 바꾸면 안 됩니다. [Microsoft의 EXE/MSI 패키지 요건](https://learn.microsoft.com/en-us/windows/apps/publish/publish-your-app/msi/app-package-requirements)

서명 추가만으로 Store 승인이 보장되지는 않습니다. HJViewer는 설치된 Windows 서비스, 자동 시작, 무인 원격 접속, 가상 디스플레이와 프라이버시 기능을 사용합니다. 기존 WHCP/Microsoft 서명이 있는 드라이버와 CAT는 그 서명과 파일을 보존하며, 개인용 앱 서명으로 드라이버 인증을 대신하지 않습니다. 제출 때 설치·제거·서비스 동작과 드라이버가 필요한 기능을 인증 노트에 설명하고 실제 설치 결과를 확인합니다. 이 서명 준비 작업에는 Store 재제출이나 운영 파일 교체가 포함되지 않습니다.
