# Privacy statement / 개인정보 처리방침

English first. 한국어는 [아래](#한국어)에 있습니다. The two parts say the same thing.

## English

### 1. In short

HJViewer is a self-hosted remote desktop app. The people who build it collect nothing: there are
no analytics, crash reports or ads. The app talks only to the server it was built for, which that
server's operator runs, and to public STUN servers (section 2).

From version 1.6.0 the server can also have accounts. People whom the operator has put on a list
log in with their Yonsei portal ID; they then see their own Windows PCs in the app, connect between
their own devices without the PC's password, and can switch a sleeping PC on through a relay in its
network. This statement says what the server, your devices and that relay keep, for how long, and
how to have it deleted. Logging in is a choice: an app that is not logged in, and a build without
the account feature, sends nothing to the account service and connects with the PC's ID and
password as before.

### 2. Connecting devices (everyone)

- **Device registry**: the server keeps each device's ID, its public key, an identifier of the
  machine (on a PC the one the operating system gives it, on Windows its MachineGuid; on a phone
  the device's public key) and the IP address it last connected from, so that other devices can
  find it.
- **Server logs**: the server logs a device's ID, IP address and port, machine identifier and public
  key when it first registers and when its address or key changes; the relay server logs, for each
  session it relays, the IP addresses of the two ends and the time; and the web server logs
  connection times and IP addresses. A request to the account service while that service is
  stopped or past a limit can leave, in the web server's error log, the client's IP address and
  the request's path, which can name a PC's ID. Put together, these logs can show which device
  connected to which PC through the relay, and when. They are kept as long as the server's system logs are (the
  operator's settings).
- **Session contents are not readable by the server.** Screen, input, clipboard, files, camera
  pictures and sound are end-to-end encrypted between the two devices; when the server relays a
  session it only forwards encrypted bytes. The account service keeps no record of each connection
  (section 3).
- **System information and connection records**: as upstream RustDesk does, a PC that accepts
  connections tries to send to the server's `/api` addresses heartbeats, its host name, OS user
  name, OS, CPU and memory, and a record of each connection and file transfer (the other device's
  address, ID and name, file names and sizes); a logged-in app also tries to send there the notes
  you may write on a session. This deployment answers those addresses with 404 without reading the
  requests, and keeps nothing. Only the account requests of section 3 reach the account service.
- **STUN servers**: to connect two devices directly (peer to peer), the app asks public STUN
  servers which public IP address and port it is reachable at: stun.cloudflare.com,
  stun.l.google.com, stun.antisip.com and stun.nextcloud.com. Those operators see the device's IP
  address and the time; they see nothing of the session.

### 3. Accounts (from version 1.6.0)

Only for people on the operator's list who log in. The account service runs on the same server. A
login with an ID that is not on the list still leaves a login record (tags of the ID typed and of
the device, the client's IP address, the result, the time; 6 months) and the login limit counters.

**Logging in.** You type your Yonsei portal ID and password in the app. The app seals the password
to the server's key, so that only the account service can open it, even on a network that inspects
encrypted traffic. The service has the Yonsei SSO (the university's login system) check it and
forgets it at once; the password and the SSO's token are never stored or logged. From the SSO the
service keeps only your portal ID and name. Each login also registers a new **account key** of
that device: the server keeps its public key, the private key stays on the device, and every
account request after the login is signed with it.

**Your PCs.** A Windows PC with HJViewer installed joins your account when you log in to HJViewer
on it. From then on your other logged-in devices can connect to it without the PC's password, for
the screen, file transfer and the camera; the terminal and port forwarding still ask for the PC's
password. The PC's HJViewer service signs a short request to the server with the PC's device key,
usually every 2 minutes and at most 5 minutes apart while it reaches the server (when it cannot, it
tries again after 30 seconds, waiting longer each time, up to 10 minutes), and gets back the signed
list of your account's keys. With it, the PC reports its app version, whether connections without
the password are switched off on it, for each account key the time of the last such connection it
let in today and yesterday and their number, and how many attempts it refused for each reason
(counts only).

**What the account service keeps**

| What | Content | Kept |
|---|---|---|
| List | the portal IDs the operator put on the list, also of people who never logged in | until the operator takes them off |
| Account | portal ID and name (from the SSO), times of creation and last login, the account's switch for connections without the PC's password | until a year without use (an account still on the list with a PC in use stays), or deleted on request |
| Login tokens | a hash of each token, never the token | until logout, 90 days without use, a year at most |
| Account keys (one per logged-in device) | public key; device ID; a tag computed from the machine identifier of section 2 (not the identifier); platform, the device model the server names from it, the device name and the app version, as the device reported them; whether the login came from the university's network or from outside (not the address); times of creation and last login; your devices' answers to the notice below; when and how its login ended and which of your devices ended it | while the device is logged in, then 30 days |
| PCs in your account | PC ID, OS, host name, the PC's public key, a random binding ID, times added, last seen and last synced, the IP address its last sync came from (replaced by the next one's), app version, whether switched off on the PC; per reason, counts of refused attempts (30 days) | until removed from the account or 180 days unseen. After that the binding ID, the PC ID and its public key stay 180 days, so that the PC learns that it was removed |
| Connections without the PC's password | per account key, PC and day: the time of the last one and their number, as the PC reported them. There is no record per connection | 30 days; deleted with the key or the PC |
| Records of logins, devices and keys | tags of the account and the device (not your ID), the client's IP address, the result, the time | 6 months |
| Operator record | which administration command ran when, its targets as tags; each look of a super operator at the users and PCs (below), with that super operator as a tag and how many users and PCs it showed | at least a year |
| Database backups | a copy of all of the above, on the server, readable by root only: one a day, and one before an upgrade or a restore | 14 days |
| The service's log | counts and tags, never IDs, addresses or secrets | 30 days |

Short-lived: the one-time values of requests (up to 20 minutes) and the login limit counters,
under tags (as long as their longest window, at least a day). In memory only, never on disk: the
service's answer to a login or to an account request that changes something, so that the same
request sent again within 30 seconds gets the same answer; it is cleared at the next such request,
within the hour at the latest.

**Not kept** by the account service: passwords, Yonsei SSO tokens, e-mail addresses, affiliation,
the machine identifier itself, OS user names, CPU and memory details, heartbeats, device
information, a record of each connection or file transfer, and session contents.

**What your other devices show.** When a device logs in to your account, your other devices show
it with the time and whether it was on the university's network, which the server saw, and the app
version and the device model, which the device reported about itself and nobody checks, so that
you notice a login you did not make. The notice stays until you answer it on one of your devices
("It is mine" or "Not me"), however long that takes; it also comes for a device that logged in
before the one you are using, and for one whose login ended before anyone answered, saying how it
ended. The list of logged-in devices shows, for each of your PCs, when a device last connected to
it without its password and how often in 30 days, and for 30 days the devices whose login ended:
their platform and name, login time, last use and how they left (logged out by themselves, logged
out from another of your devices and which one, expired, replaced by a new login of the same
device, logged out by the administrator).

**What others see.** When you connect to someone else's PC, that PC shows your Windows user name
(from a phone, the device's name), as before 1.6.0: logging in does not send your name or portal
ID to other people's PCs. On your own PC, the connection window shows the connecting device's name
and your name masked (such as 홍\*동), taken from the signed list; that device name is the model the
server named from what the device reported at login, and the PC's log names the device by the ID
the device itself gave.

**What the operator sees.** The operator names the super operators (최고운영자) by their portal
IDs in the server's own settings, not in the app or its code; everyone else is a normal user. A
super operator sees in the app, and can only look at, the users of the service and the PCs in
their accounts: for each user the portal ID and name, when one of their devices or PCs was last
used and whether that was within 30 days, and how many logged-in devices and PCs they have; for
each PC its ID and host name, its owner's portal ID and name, the OS and app version, the time of
its last sync and the IP address that sync came from, the IPv4 addresses of its wired network
adapters (from the report of section 5) and the state of connections without the password. A PC
shows only once someone has logged in to HJViewer on it. The view cannot connect to anyone's PC or
change anything. It is there so that the operator knows who uses the service and on which PCs, to
help them and to protect the service. Each look is recorded in the operator record: which super
operator (as a tag), when, and how many users and PCs it showed.

### 4. On your devices

- **Every app**: its settings, its device ID and key pair, and the IDs of devices you connected to.
- **A logged-in app** also keeps the account key's private key: on an iPhone in the Keychain, for
  this device only and usable while it is unlocked, so it is not in backups; on Android in the
  app's data, which is left out of cloud backups and device-to-device transfers; on Windows in
  `%LOCALAPPDATA%\HJViewer-account\`, encrypted with Windows DPAPI for your Windows user and not
  roaming. It also keeps the login token, the name and portal ID the server returned, the list of
  your PCs (PC ID, public key, state) and the difference between its clock and the server's. A
  token without its account key opens nothing.
- **A PC in an account** keeps, in the Windows registry (`HKLM\SOFTWARE\HJViewer\Account`), its
  binding ID, your name masked, whether connections without the password are switched off on it,
  a record of a binding it let go, and for each account key the time of the last such connection
  today and yesterday and their number; in its service's folder, the signed list (the names, IDs
  and public keys of your devices and your masked name); and in HJViewer's log, a line for each
  connection it let in without the password (the device's ID and name, the time). HJViewer keeps
  about 90 days of logs.
  Other people who use that PC can read the masked name and, with some effort, the list.
  Uninstalling HJViewer deletes the registry key; each Windows user's settings and
  `%LOCALAPPDATA%\HJViewer-account\`, and the service's settings, list file and logs stay.

### 5. Switching a PC on (Wake-on-LAN)

- **PCs in an account** report what the server needs to judge whether they can be woken: the MAC
  addresses, IPv4 addresses and subnets of their wired network adapters, the gateway's IP and MAC
  addresses, each adapter's name, driver, link speed and whether it is a USB adapter, its
  Wake-on-LAN settings and whether Windows lets it wake the PC, the sleep states the PC supports,
  whether fast startup is on, whether the PC runs on mains power, how long Windows has been
  running (which the server does not keep), and the public address the PC uses. Only PCs in an
  account report this, and it is deleted as soon as the PC leaves the account. The apps never
  receive MAC addresses.
- **Relays**: for a relay on a NAS in a PC's network, the server keeps its ID and public key, the
  account it serves, the name the administrator gave it, the NAS's network adapters (their names,
  IP and MAC addresses and networks, the gateway's IP and MAC addresses), the versions of the relay
  program, DSM and the kernel, the public address it uses, and the times it first asked, was
  approved and last asked. A relay that asked and was not approved is forgotten after 24 hours; an
  enrollment code approved ahead of its relay is kept 7 days. When a relay is revoked, only its
  ID, key and times stay.
- **Wake requests**: each request to switch a PC on leaves one record: a random number for the
  request, the account and its tag, the PC's ID, the relay's ID, how well the relay's network
  matched the PC's, the network the signal went to (written like 192.0.2.0/24, which names the
  network, not the PC's own address or a MAC address), a hash of the signed command, the times
  (asked, sent, the PC seen on, finished), the number of packets sent and the result. The app's
  progress display reads this record, and the limits count it (a PC once a minute, an account 20
  times an hour, a relay 10 times a minute). The network is kept so that a relay that already woke
  the PC on that network is chosen for it again. The record is kept for 6 months, also after the
  PC leaves the account or the relay is revoked, and it is deleted with the account. The signed
  command itself names the MAC addresses of the PC's adapters on that network; it is kept only
  until the relay answers, about two minutes at most.
- **The relay's log on the NAS**: for each wake request, its number, the time, the MAC address
  with only its last two bytes shown, the PC's network and the NAS adapter's address it sent from;
  when it starts, the server's name, the relay's ID and its enrollment code. While it runs, the relay deletes the lines older than 180
  days. It does not tidy the log while it is stopped, so when it is stopped for long, the NAS's
  administrator deletes the log. Every administrator of that NAS can read it.

### 6. Camera and microphone

On phones, the app uses the microphone only during a voice call you start in a session; the sound
goes end-to-end encrypted to the other device and is not stored. The camera (on an iPhone also a
picture you pick) is used only to read a QR code from the settings screen; the image stays on the
device. Phones only control other devices; they do not share their own screen.

On a Windows PC that accepts connections, a connected device sees its screen and, as far as the
PC's permission settings allow, its cameras and sound, and can hold a voice call that someone at
the PC accepts. All of it is end-to-end encrypted and not stored.

<!-- 설계 3판 10-2의 nginx 전환 관문(질문 7): 답이 오면 7절의 처리 근거(학교가 공공기관이면 제15조 제1항의 다른 호가 될 수 있음), 9절의 처리자 이름, 10절의 연락처를 그 답대로 고치고, AI 운영 에이전트가 처리위탁·국외 이전에 해당하면 받는 곳과 나라도 9절에 적는다. 영어와 한국어를 같이 고친다. -->

### 7. Why, and on what basis

The account data is used to log you in and keep you logged in, to show your PCs and their state,
to let your own devices connect to your PCs without the PC's password, to let you notice and stop
logins and connections you did not make, to switch your PCs on, to protect the service (login
limits, records, backups), and to let the super operators see who uses it and on which PCs
(section 3). The basis is the service you ask for by logging in (Personal Information
Protection Act, Article 15(1)4). Nothing is used for anything else, sold or shared beyond section
9.

### 8. Deleting your data, and your rights

- **Yourself, in the app**: log out on a device (its key and token end at once); on another of your
  devices, log out a lost one ("로그인한 기기", Logged-in devices) or all others; remove a PC from
  your list; log out on a PC before you give it away. Uninstalling HJViewer from a PC deletes what
  it kept in the registry. Changing your portal password does not log out the devices already
  logged in: after a change, also use "Log out all other devices". A logout from another device
  stops new connections without the password; a connection that is open already stays until it is
  ended in the PC's connection window, by "Restart remote device" from your own device with the
  PC's password, or by restarting the PC.
- **Through the operator**: you can ask to see what is kept about you, to have it corrected or
  deleted, or to stop its processing. Deleting your account takes you off the list, revokes its
  relays and deletes everything kept about it, the wake requests too. What stays: the removed PCs'
  binding IDs, PC IDs and public keys (180 days), the revoked relays' IDs, keys and times, the
  records of logins, devices and keys under tags (6 months) and the operator record; the backups
  age out after 14 days.
- Once you are taken off the list, your devices are logged out at once.

### 9. Who handles the data

- **The operator** of the server runs the account service, answers requests and is responsible for
  this data. Administration commands show IDs shortened and leave out names, addresses and device
  names unless a person asks for them; every command is recorded, except the one the five-minute
  check reads, which returns counts only and changes nothing. The super operators the operator
  names in the server's settings see in the app the users and PCs of section 3; each look is
  recorded.
- **The Yonsei SSO** checks your portal ID and password at login.
- **An AI assistant**: the operator uses an AI assistant for work on the server. Its rules let it
  see only shortened, masked output and counts, and what it sees goes to the external AI service
  that runs it. Showing full values and making keys are done by people only.
- **STUN servers** (section 2). Nobody else: no advertising or analytics companies.

### 10. Contact and changes

Send requests about your data (section 8) to the operator of the server you use, directly. Do not
write personal information, such as your portal ID or name, in a public issue. Questions about the
app itself can go to an issue at https://github.com/yonsei-projecttf/hjviewer. Changes to this
statement are in the history of that repository.

## 한국어

### 1. 요약

HJViewer는 직접 운영하는 서버에 붙는 원격 데스크톱 앱입니다. 앱을 만든 쪽은 아무 정보도 수집하지 않습니다. 분석, 오류 보고, 광고가 없습니다. 앱은 빌드할 때 정해진 서버, 그리고 공개 STUN 서버(2절)하고만 통신합니다. 서버는 그 서버의 운영자가 관리합니다.

1.6.0부터 서버에 계정 기능이 생깁니다. 운영자가 명단에 넣은 사람은 연세포탈 ID로 로그인하고, 앱에서 자기 Windows PC를 보고, 자기 기기끼리 PC 비밀번호 없이 연결하고, PC와 같은 네트워크의 중계기로 꺼진 PC를 켤 수 있습니다. 이 문서는 서버, 사용자의 기기, 그 중계기가 무엇을 얼마 동안 보관하는지와 지우는 방법을 적습니다. 로그인은 선택입니다. 로그인하지 않은 앱과 계정 기능이 없는 빌드는 계정 서비스에 아무것도 보내지 않고, 지금처럼 PC의 ID와 비밀번호로 연결합니다.

### 2. 기기 연결 (모든 사용자)

- **기기 등록부**: 서버는 기기마다 ID, 공개키, 기기 식별값(PC는 운영체제가 정한 값으로 Windows에서는 MachineGuid, 휴대폰은 기기 공개키), 마지막 접속 IP를 보관합니다. 다른 기기가 이 기기를 찾을 수 있게 하기 위해서입니다.
- **서버 로그**: 서버는 기기가 처음 등록할 때와 주소나 키가 바뀔 때 기기 ID, IP 주소와 포트, 기기 식별값, 공개키를 기록합니다. 중계 서버는 중계한 세션마다 두 끝의 IP 주소와 시각을, 웹 서버는 접속 시각과 IP 주소를 기록합니다. 계정 서비스가 멈춰 있거나 한도에 걸린 동안의 계정 요청은 웹 서버의 오류 로그에 접속한 IP 주소와 요청 경로(PC ID가 들어갈 수 있음)를 남길 수 있습니다. 이 로그를 맞춰 보면 어느 기기가 언제 어느 PC에 중계로 연결했는지 알 수 있습니다. 이 로그는 서버의 시스템 로그가 보관되는 동안 남습니다(운영자 설정).
- **서버는 세션 내용을 볼 수 없습니다.** 화면, 입력, 클립보드, 파일, 카메라 영상, 소리는 두 기기 사이에서 종단 간 암호화됩니다. 서버가 중계할 때도 암호화된 바이트만 전달합니다. 계정 서비스는 연결마다의 기록을 두지 않습니다(3절).
- **시스템 정보와 접속 기록**: 원격 접속을 받는 PC는 upstream RustDesk처럼 서버의 `/api` 주소로 하트비트, 호스트 이름, OS 사용자 이름, OS, CPU, 메모리 정보, 그리고 연결과 파일 전송마다의 기록(상대 기기의 주소·ID·이름, 파일 이름과 크기)을 보내려 합니다. 로그인한 앱도 세션에 남길 수 있는 메모를 그 주소로 보내려 합니다. 이 배포는 그 주소에 요청을 읽지 않고 404로 답하며 아무것도 저장하지 않습니다. 계정 서비스에는 3절의 계정 요청만 갑니다.
- **STUN 서버**: 두 기기를 직접 연결(P2P)하려고, 앱은 공개 STUN 서버(stun.cloudflare.com, stun.l.google.com, stun.antisip.com, stun.nextcloud.com)에 자신의 공인 IP와 포트를 묻습니다. 이 서버 운영자는 기기의 IP 주소와 시각을 알 수 있지만, 세션 내용은 알 수 없습니다.

### 3. 계정 (1.6.0부터)

운영자의 명단에 있고 로그인한 사람에게만 해당합니다. 계정 서비스는 같은 서버에서 돕니다. 명단에 없는 ID로 로그인을 시도해도 시도 기록(입력한 ID와 기기의 꼬리표, 접속한 IP 주소, 결과, 시각; 6개월)과 로그인 한도 기록은 남습니다.

**로그인.** 앱에 연세포탈 ID와 비밀번호를 넣습니다. 앱은 비밀번호를 서버의 키로 봉인하므로, 암호화된 통신을 들여다보는 네트워크에서도 계정 서비스만 열 수 있습니다. 계정 서비스는 연세 SSO(학교의 로그인 시스템)에 비밀번호를 확인받은 뒤 바로 버립니다. 비밀번호와 SSO 토큰은 저장하지도 기록하지도 않습니다. SSO에서 받은 것 가운데 포탈 ID와 성명만 보관합니다. 로그인할 때마다 그 기기의 **계정 키**도 새로 등록합니다. 서버는 공개키만 보관하고 개인키는 기기에 남으며, 로그인 뒤의 모든 계정 요청에 그 키로 서명합니다.

**내 PC.** HJViewer를 설치한 Windows PC에서 HJViewer에 로그인하면 그 PC가 내 계정에 들어갑니다. 그때부터 로그인한 다른 내 기기는 그 PC에 PC 비밀번호 없이 원격 화면, 파일 전송, 카메라로 연결할 수 있습니다. 터미널과 포트 포워딩은 지금처럼 PC 비밀번호를 묻습니다. 그 PC의 HJViewer 서비스는 보통 2분마다, 서버에 닿는 동안에는 늦어도 5분마다 PC의 기기 키로 서명한 짧은 요청을 서버에 보내고, 서명된 내 계정의 키 목록을 받습니다(닿지 못하면 30초 뒤부터 간격을 늘려 가며 최대 10분마다 다시 해 봅니다). 이때 PC는 앱 버전, 비밀번호 없는 연결을 그 PC에서 꺼 두었는지, 계정 키마다 오늘과 어제 받아들인 비밀번호 없는 연결의 마지막 시각과 횟수, 거절한 시도의 이유별 개수(개수만)를 함께 알립니다.

**계정 서비스가 보관하는 것**

| 항목 | 내용 | 보관 |
|---|---|---|
| 명단 | 운영자가 명단에 넣은 포탈 ID(로그인한 적이 없는 사람 것도) | 운영자가 뺄 때까지 |
| 계정 | 포탈 ID와 성명(SSO에서 받음), 만든 시각과 마지막 로그인 시각, 그 계정의 비밀번호 없는 연결 스위치 | 1년 동안 쓰지 않을 때까지(명단에 있고 쓰는 PC가 있는 계정은 남김), 또는 요청하면 삭제 |
| 로그인 토큰 | 토큰의 해시만(토큰 자체는 없음) | 로그아웃할 때까지, 90일 동안 쓰지 않거나 최대 1년 |
| 계정 키(로그인한 기기마다 하나) | 공개키, 기기 ID, 기기 식별값(2절)으로 만든 꼬리표(식별값 자체는 없음), 기기가 알린 플랫폼·기기 이름·앱 버전과 그것으로 서버가 정한 기기 모델 이름, 학내 네트워크에서 로그인했는지(주소는 없음), 만든 시각·마지막 로그인 시각, 아래 알림에 내 기기들이 한 답, 로그인이 언제 어떻게 끝났는지와 끝낸 내 기기 | 그 기기가 로그인해 있는 동안, 그 뒤 30일 |
| 내 계정의 PC | PC ID, OS, 호스트 이름, PC 공개키, 무작위 묶음 ID, 추가한 시각·마지막으로 보인 시각·마지막 동기화 시각, 마지막 동기화가 온 IP 주소(다음 동기화의 주소로 바뀜), 앱 버전, 그 PC에서 꺼 두었는지, 거절한 시도의 이유별 개수(30일) | 계정에서 빼거나 180일 동안 보이지 않을 때까지. 그 뒤 묶음 ID, PC ID, PC 공개키만 180일 남깁니다. PC가 빠졌다는 것을 그 PC에 알리기 위해서입니다 |
| 비밀번호 없는 연결 | 계정 키·PC·날짜마다 마지막 연결 시각과 횟수(PC가 알린 값). 연결마다의 기록은 없음 | 30일. 그 키나 PC와 함께 삭제 |
| 로그인·기기·키 기록 | 계정과 기기의 꼬리표(ID가 아님), 접속한 IP 주소, 결과, 시각 | 6개월 |
| 운영자 기록 | 어떤 관리 명령을 언제 실행했는지, 대상은 꼬리표로. 최고운영자가 사용자와 PC 목록(아래)을 볼 때마다 그 최고운영자(꼬리표)와 보인 사용자·PC의 수 | 1년 이상 |
| DB 백업 | 위 전부의 사본(서버 안, root만 읽음). 매일 하나, 그리고 업그레이드나 복원 전에 하나 | 14일 |
| 계정 서비스의 로그 | 개수와 꼬리표만(ID, 주소, 비밀값은 없음) | 30일 |

잠깐만 두는 것: 요청의 일회용 값(최대 20분), 꼬리표로 센 로그인 한도 기록(가장 긴 한도 기간, 최소 하루). 메모리에만 두고 디스크에는 쓰지 않는 것: 로그인과 무언가를 바꾸는 계정 요청에 한 답. 30초 안에 같은 요청이 다시 오면 같은 답을 주려는 것이고, 다음 그런 요청 때나 늦어도 1시간 안에 지웁니다.

**계정 서비스가 보관하지 않는 것**: 비밀번호, 연세 SSO 토큰, 이메일 주소, 소속, 기기 식별값 자체, OS 사용자 이름, CPU·메모리 정보, 하트비트, 기기 정보, 연결이나 파일 전송마다의 기록, 세션 내용.

**다른 내 기기에 보이는 것.** 어떤 기기가 내 계정에 로그인하면 다른 내 기기에 알림이 뜹니다. 서버가 본 시각과 학내 네트워크인지, 그리고 그 기기가 스스로 알린(아무도 확인하지 않는) 앱 버전과 기기 모델이 나오므로, 내가 하지 않은 로그인을 알아챌 수 있습니다. 알림은 내 기기 하나에서 [내 기기 맞음]이나 [아님]으로 답할 때까지 기한 없이 남습니다. 지금 쓰는 기기보다 먼저 로그인한 기기에도, 아무도 답하기 전에 로그인이 끝난 기기에도 알림이 뜨고, 끝난 기기는 어떻게 끝났는지도 나옵니다. '로그인한 기기' 목록에는 내 PC마다 그 기기가 마지막으로 비밀번호 없이 연결한 시각과 30일 동안의 횟수가 나오고, 로그인이 끝난 기기도 30일 동안 따로 나옵니다. 플랫폼과 이름, 로그인 시각, 마지막 사용, 어떻게 끝났는지(스스로 로그아웃, 다른 내 기기 중 어느 것에서 로그아웃시킴, 기간 만료, 같은 기기의 새 로그인으로 바뀜, 관리자가 로그아웃시킴)가 나옵니다.

**다른 사람에게 보이는 것.** 다른 사람의 PC에 연결하면 그 PC에는 1.6.0 전처럼 내 Windows 사용자 이름(휴대폰은 기기 이름)이 보입니다. 로그인해도 성명이나 포탈 ID를 다른 사람의 PC에 보내지 않습니다. 내 PC의 연결 창에는 연결한 기기의 이름과 가린 내 이름(예: 홍\*동)이 서명된 목록에서 나옵니다. 그 기기 이름은 기기가 로그인할 때 알린 값으로 서버가 정한 모델 이름이고, PC의 로그에 남는 기기 ID도 그 기기가 스스로 알린 값입니다.

**운영자에게 보이는 것.** 운영자는 최고운영자를 포탈 ID로 서버의 설정에서만 정합니다(앱이나 코드에는 없음). 그 밖의 사람은 모두 일반 사용자입니다. 최고운영자는 앱에서 이 서비스의 사용자와 그 계정의 PC를 보기만 할 수 있습니다. 사용자마다 포탈 ID와 성명, 그 사람의 기기나 PC를 마지막으로 쓴 때와 그것이 30일 안인지, 로그인한 기기 수와 PC 수가 보이고, PC마다 PC ID와 호스트 이름, 주인의 포탈 ID와 성명, OS와 앱 버전, 마지막 동기화 시각과 그 동기화가 온 IP 주소, 유선 랜카드의 IPv4 주소(5절의 보고에서), 비밀번호 없는 연결 상태가 보입니다. PC는 누군가 그 PC에서 HJViewer에 로그인한 뒤에야 보입니다. 이 화면으로는 다른 사람의 PC에 연결하거나 무엇을 바꿀 수 없습니다. 누가 어느 PC에서 이 서비스를 쓰는지 운영자가 알고, 사용자를 돕고 서비스를 지키기 위한 것입니다. 볼 때마다 운영자 기록에 남습니다: 어느 최고운영자인지(꼬리표), 언제, 몇 명의 사용자와 몇 대의 PC를 보였는지.

### 4. 기기에 저장되는 정보

- **모든 앱**: 설정, 기기 ID와 키 쌍, 접속했던 기기 ID.
- **로그인한 앱**은 계정 키의 개인키도 보관합니다. 아이폰은 Keychain에 이 기기 전용, 잠금이 풀렸을 때만 쓰도록 두므로 백업에 들어가지 않습니다. 안드로이드는 앱 데이터에 두며, 클라우드 백업과 기기 간 이전에서 뺍니다. Windows는 `%LOCALAPPDATA%\HJViewer-account\`에 Windows DPAPI로 그 Windows 사용자만 풀 수 있게 암호화해 두고, 로밍하지 않습니다. 그 밖에 로그인 토큰, 서버가 보낸 성명과 포탈 ID, 내 PC 목록(PC ID, 공개키, 상태), 서버와의 시계 차이를 보관합니다. 계정 키가 없는 토큰으로는 아무것도 할 수 없습니다.
- **계정에 들어간 PC**는 Windows 레지스트리(`HKLM\SOFTWARE\HJViewer\Account`)에 묶음 ID, 가린 내 이름, 비밀번호 없는 연결을 그 PC에서 꺼 두었는지, 풀린 묶음의 기록, 계정 키마다 오늘과 어제의 비밀번호 없는 연결의 마지막 시각과 횟수를 둡니다. 서비스 폴더에는 서명된 목록(내 기기들의 이름·ID·공개키와 가린 내 이름)을, HJViewer 로그에는 비밀번호 없이 받아들인 연결마다 한 줄(그 기기의 ID와 이름, 시각)을 둡니다. HJViewer 로그는 약 90일치를 둡니다. 그 PC를 쓰는 다른 사람은 가린 이름을 볼 수 있고, 마음먹으면 그 목록도 볼 수 있습니다. HJViewer를 제거하면 레지스트리 키가 지워지고, Windows 사용자마다의 설정과 `%LOCALAPPDATA%\HJViewer-account\`, 서비스의 설정·목록 파일·로그는 남습니다.

### 5. 꺼진 PC 켜기 (Wake-on-LAN)

- **계정에 들어간 PC**는 켤 수 있는지 서버가 판단하는 데 필요한 것을 알립니다. 유선 랜카드의 MAC 주소, IPv4 주소와 서브넷, 게이트웨이의 IP와 MAC 주소, 랜카드의 이름·드라이버·링크 속도와 USB 랜카드인지, Wake-on-LAN 설정과 Windows가 그 랜카드로 PC를 깨우도록 허용했는지, PC가 쓸 수 있는 절전 방식, 빠른 시작을 켰는지, 전원에 연결되어 있는지, Windows가 켜진 뒤 지난 시간(서버는 이 값을 보관하지 않음), PC가 쓰는 공인 주소입니다. 계정에 들어간 PC만 알리고, PC가 계정에서 빠지면 바로 지웁니다. 앱에는 MAC 주소를 보내지 않습니다.
- **중계기**: PC와 같은 네트워크의 NAS에서 도는 중계기에 대해 서버는 ID와 공개키, 맡은 계정, 관리자가 붙인 이름, NAS의 랜카드(이름, IP와 MAC 주소, 네트워크, 게이트웨이의 IP와 MAC 주소), 중계기 프로그램·DSM·커널 버전, NAS가 쓰는 공인 주소, 처음 요청한 시각·승인한 시각·마지막으로 물은 시각을 보관합니다. 요청만 하고 승인받지 못한 중계기는 24시간 뒤 지우고, 중계기보다 먼저 승인해 둔 등록 코드는 7일 동안 둡니다. 중계기를 폐기하면 ID, 키, 시각만 남깁니다.
- **켜기 요청**: 켜기 요청마다 기록을 하나 남깁니다. 요청의 무작위 번호, 계정과 그 꼬리표, PC ID, 중계기 ID, 중계기와 PC의 네트워크가 얼마나 맞았는지, 신호를 보낸 네트워크(192.0.2.0/24처럼 네트워크를 가리키는 값. PC의 주소나 MAC 주소는 아님), 서명된 명령의 해시, 시각(요청, 전송, PC가 켜진 것을 확인한 때, 끝난 때), 보낸 패킷 수, 결과입니다. 앱의 진행 표시가 이 기록을 읽고, 요청 한도(PC는 1분에 한 번, 계정은 1시간에 20번, 중계기는 1분에 10번)를 이 기록으로 셉니다. 네트워크는 그 네트워크에서 이 PC를 켠 적이 있는 중계기를 다음에도 고르려고 둡니다. 6개월 동안 보관하고, PC가 계정에서 빠지거나 중계기가 폐기된 뒤에도 그때까지 남으며, 계정을 지우면 함께 지웁니다. 서명된 명령 자체에는 그 네트워크에 있는 PC 랜카드의 MAC 주소가 들어 있고, 중계기가 답할 때까지만(길어야 2분쯤) 둡니다.
- **NAS의 중계기 로그**: 켜기 요청마다 번호, 시각, 끝 2바이트만 보이는 MAC 주소, PC의 네트워크, 신호를 보낸 NAS 랜카드의 주소를 남기고, 시작할 때 서버 이름, 중계기 ID, 등록 코드를 남깁니다. 중계기는 실행 중일 때 180일이 지난 줄을 지웁니다. 멈춰 둔 동안에는 정리하지 않으므로, 오래 멈춰 둘 때는 NAS 관리자가 로그를 지웁니다. 그 NAS의 관리자는 모두 읽을 수 있습니다.

### 6. 카메라와 마이크

휴대폰 앱은 세션 중 직접 시작한 음성 통화에서만 마이크를 씁니다. 소리는 상대 기기로 종단 간 암호화되어 가고 저장하지 않습니다. 카메라(아이폰에서는 고른 사진도)는 설정 화면의 QR 코드 읽기에만 쓰고, 영상은 기기 밖으로 나가지 않습니다. 휴대폰은 다른 기기를 제어하기만 하고, 자기 화면을 내보내지 않습니다.

원격 접속을 받는 Windows PC에서는, 연결한 기기가 그 PC의 화면을 보고, PC의 권한 설정이 허락하는 만큼 카메라 영상과 소리도 받으며, PC 앞 사람이 받아들인 음성 통화를 할 수 있습니다. 모두 종단 간 암호화되고 저장하지 않습니다.

### 7. 처리 목적과 근거

계정 정보는 로그인과 로그인 유지, 내 PC와 그 상태 보이기, 내 기기끼리 PC 비밀번호 없이 연결하기, 내가 하지 않은 로그인과 연결을 알아채고 막기, 내 PC 켜기, 서비스 보호(로그인 한도, 기록, 백업), 최고운영자가 누가 어느 PC에서 서비스를 쓰는지 보기(3절)에 씁니다. 처리 근거는 로그인으로 요청한 서비스의 제공입니다(개인정보 보호법 제15조 제1항 제4호). 다른 목적으로 쓰거나 팔지 않고, 9절 밖으로 넘기지 않습니다.

### 8. 삭제와 권리 행사

- **앱에서 직접**: 기기에서 로그아웃하면 그 기기의 키와 토큰이 바로 끝납니다. 잃어버린 기기는 다른 내 기기의 '로그인한 기기'에서 로그아웃시키거나 다른 기기를 모두 로그아웃시킵니다. 내 목록에서 PC를 뺄 수 있고, PC를 남에게 넘기기 전에는 그 PC에서 로그아웃합니다. PC에서 HJViewer를 제거하면 레지스트리에 둔 것이 지워집니다. 연세포탈 비밀번호를 바꿔도 이미 로그인한 기기는 남으므로, 바꾼 뒤에는 [다른 기기 모두 로그아웃]도 누릅니다. 다른 기기에서 로그아웃시키면 비밀번호 없는 새 연결은 막히지만, 이미 열려 있는 연결은 PC의 연결 창에서 끊거나, 내 기기에서 PC 비밀번호로 그 PC에 연결해 '원격 장치 다시 시작'을 누르거나, PC를 다시 시작할 때까지 남습니다.
- **운영자에게 요청**: 나에 대해 보관하는 것을 보여 달라고, 고치거나 지워 달라고, 처리를 멈춰 달라고 요청할 수 있습니다. 계정을 지우면 명단에서도 빠지고, 그 계정의 중계기는 폐기되며, 켜기 요청 기록을 포함해 그 계정에 대해 보관하던 것이 모두 지워집니다. 남는 것은 뺀 PC의 묶음 ID·PC ID·공개키(180일), 폐기된 중계기의 ID·키·시각, 꼬리표로 남은 로그인·기기·키 기록(6개월), 운영자 기록이고, 백업은 14일 뒤에 사라집니다.
- 명단에서 빠지면 내 기기들은 바로 로그아웃됩니다.

### 9. 정보를 다루는 곳

- **운영자**: 서버의 계정 서비스를 운영하고, 요청에 답하며, 이 정보를 책임집니다. 관리 명령은 사람이 따로 요청하지 않는 한 ID를 줄여 보이고 이름, 주소, 기기 이름을 보이지 않습니다. 관리 명령은 모두 기록됩니다. 5분마다 도는 점검이 읽는 명령 하나만 예외이고, 그 명령은 개수만 돌려주며 아무것도 바꾸지 않습니다. 운영자가 서버의 설정에서 정한 최고운영자는 앱에서 3절의 사용자와 PC를 보며, 볼 때마다 기록됩니다.
- **연세 SSO**: 로그인할 때 포탈 ID와 비밀번호를 확인합니다.
- **AI 도우미**: 운영자는 서버 작업에 AI 도우미를 씁니다. AI 도우미는 줄이고 가린 출력과 개수만 보도록 규칙으로 정해져 있고, 본 내용은 AI 도우미를 실행하는 외부 AI 서비스로 갑니다. 원래 값 보기와 키 만들기는 사람만 합니다.
- **STUN 서버**(2절). 그 밖에는 없습니다. 광고나 분석 회사에 넘기지 않습니다.

### 10. 문의와 변경

내 정보에 관한 요청(8절)은 사용 중인 서버의 운영자에게 직접 해 주세요. 공개 이슈에는 포탈 ID나 성명 같은 개인정보를 쓰지 마세요. 앱 자체에 관한 문의는 https://github.com/yonsei-projecttf/hjviewer 에 이슈로 남길 수 있습니다. 이 문서가 바뀐 내역은 그 저장소의 기록에 남습니다.
