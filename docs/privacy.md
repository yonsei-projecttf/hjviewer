# Privacy statement / 개인정보 처리방침

HJViewer is a self-hosted remote desktop app. The people who build it collect nothing: there are no
analytics, crash reports or accounts, and the app talks only to the server it was built for, which
the operator of that server runs.

HJViewer는 직접 운영하는 서버에 붙는 원격 데스크톱 앱입니다. 앱을 만든 쪽은 아무 정보도 수집하지 않습니다. 분석, 오류 보고, 계정 기능이 없고, 앱은 빌드할 때 정해진 서버하고만 통신합니다. 그 서버는 서버 운영자가 관리합니다.

## What the server keeps / 서버가 보관하는 정보

- **Device registry**: each device's ID, a random installation UUID, its public key and the IP address it last connected from, so other devices can find it. 기기 ID, 설치별 임의 UUID, 공개키, 마지막 접속 IP를 보관합니다. 다른 기기가 이 기기를 찾을 수 있게 하기 위해서입니다.
- **Server logs**: connection times and IP addresses, kept as long as the operator keeps logs. 접속 시각과 IP 주소를 남깁니다. 보관 기간은 운영자가 정합니다.
- **Session contents are not readable by the server.** Screen, input, clipboard and files are end-to-end encrypted between the two devices; when the server relays a session it only forwards encrypted bytes. 화면, 입력, 클립보드, 파일은 두 기기 사이에서 종단 간 암호화됩니다. 서버가 중계할 때도 암호화된 바이트만 전달합니다.

## On the devices / 기기에 저장되는 정보

The app stores its settings, its device ID and key pair, and the IDs of devices you connected to, on
your device only. 앱은 설정, 기기 ID와 키 쌍, 접속했던 기기 ID를 사용자 기기에만 저장합니다.

## Contact / 문의

Ask the operator of the server you use, or open an issue at https://github.com/ysfaculty/hjviewer.
사용 중인 서버의 운영자에게 문의하거나, 위 저장소에 이슈를 남겨 주세요.
