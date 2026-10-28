# Privacy policy / 개인정보 처리방침

English first. 한국어는 [아래](#한국어)에 있습니다. The two parts say the same thing.

## English

### 1. In short

HJViewer is a self-hosted remote desktop app. It has no analytics, crash reports or ads. The team
that builds it, 프로젝트TF, also runs the university's HJViewer server and its account service (section
9); the sections below say what they keep. Apart from the devices it connects with, the app talks
only to the server it was built for, which that server's operator runs, and to public STUN servers
(section 2). It opens pages outside the app only when you tap a link or button: this project's pages
on GitHub (this statement, the source code and its guides) and, to update the app, TestFlight or the
App Store on an iPhone, or Google Play or the server's download link on Android. Up to version
1.9.1, a message from or about a connected device could also carry a button to a page of RustDesk,
the open-source app HJViewer is built on (its help on rustdesk.com, or an issue on its GitHub); from
1.9.2 the app shows no such button.

From version 1.6.0 the server can also have accounts. Faculty and staff whom the Yonsei SSO accepts
log in with their Yonsei portal ID; they then see their own Windows PCs in the app, connect between
their own devices without the PC's password (when the operator has turned this on), and can switch a
sleeping PC on through a relay in its network. This statement says what the server, your devices and
that relay keep, for how long, and how to delete it in the app or have it deleted (section 8). An
app that is not logged in, on a device that is not in an account, sends the account service only the
access check of section 2 and, when someone types them, a login, the access password or, on a PC, a
support code (section 3). On the university's HJViewer server, such an app connects only after the
access password, and such a PC accepts connections only after the access password or a support code
(below). A PC in an account is different: whether or not its app is logged in, its HJViewer service
keeps syncing with the account service, and the PC accepts connections as section 3 says ("Your
PCs", "Access tickets"). A login on it that ends without a logout on that PC, for example from
another device or when it expires, leaves the PC in the account; removing it from My PCs takes it
out (section 8).

From version 1.6.0 the operator can also set a minimum app version. Apps check the server's signed
version policy, and new versions come from the server (section 2, "Updates").

From version 1.8.0 the operator can keep the server to the university's faculty and staff: an app
then connects, and a PC accepts connections, only while it is logged in (a PC also while it is in
an account), or for a while after someone typed the access password that the operator gives out,
or, for a PC from version 1.9.0, a support code that a logged-in staff member made (section 3,
"Access tickets", "The access password" and "Support codes"). While the operator does not turn
this check on, apps work as before.
The university's HJViewer server has had this check on since 2026-10-19.

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
  connected to which PC through the relay, and when. Once the operator enforces a minimum app
  version, the server also logs the connections it refuses from older apps (one by one up to 30
  every 10 seconds, beyond that only their number): that app's IP address and port, the ID of the
  device it tried to reach, and the version it reported. Once the operator turns the access check
  on (version 1.8.0), it logs in the same way the connections and registrations it refuses for want
  of a valid access ticket, and the PCs it disconnects when their ticket ends: the device's IP
  address and port, the ID it tried to reach or to register, and the reason, never the ticket. They
  are kept as long as the server's system logs are (the operator's settings).
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
- **Updates: the version policy** (from version 1.6.0): apps check the server's version policy
  (`/api/hjv/version`) at start, before a connection when the last check is over 30 minutes old,
  when they come back to the front after an hour, after a login or a refused connection, and every
  6 hours. The policy is a public file, the same for everyone, signed by the operator. The request
  carries no ID, account or device information, and the web server keeps no access log of it.
- **The server's access check** (from version 1.8.0): apps ask whether the server checks access
  (`/api/hjv/access`) at start, when they come back to the front, and when they are about to show
  a screen that depends on it (the PC's notice, a session's offline or restart message; once a
  minute at most), to choose the login hints, the logout warnings and, from version 1.9.0,
  whether the PC offers the support code. The answer is the mode, one word, and from version 1.9.0
  whether support codes work; it is the same for everyone.
  The request carries no ID, account or device information, the account service records nothing
  of it, and the web server keeps no access log of it.
- **Updates: downloads** (from version 1.6.0): the account service hands out the installers of new
  versions: to logged-in apps, to the HJViewer service of a PC in an account, and through three
  kinds of links that need no login: the migration link that the operator gives to people still on
  1.5.x, short links that a logged-in app makes for another PC (they work for 10 minutes and three
  downloads, and live in the service's memory only), and, from version 1.9.0, the support link that
  comes with a support code (section 3), which works while that code does and for three downloads.
  For each download and each short link
  made, the service records which way and for which platform, the time, and tags: of the account
  and of the PC when there are any, and of the client's IP address (never the address itself).
  These records are kept 6 months; downloads through the migration link are recorded at most 20 a
  minute. The codes of the links are never stored; the service's log shows a code only as the
  first 8 hex digits of its tag. The web server keeps no access log of these requests (its error
  log: "Server logs" above).

### 3. Accounts (from version 1.6.0)

Only for people who log in. Only the university's faculty and staff can log in: the account
service takes only their portal IDs (by their form: Z or Y first, not ZA) and checks the code the
Yonsei SSO gives for the person against the same rule. Each of them may use HJViewer unless the
operator blocks them; the account service puts the portal ID on its list at the first login. The
operator can also go back to letting in only the IDs put on the list ahead. The account service
runs on the same server. A login that fails (a wrong password, a blocked account) still leaves a
login record (tags of the ID typed and of the device, the client's IP address, the result, the
time; 6 months) and the login limit counters. An ID of another form (but the App Review account's,
below) is refused at once, before the SSO: neither it nor the password is sent on, and its record
keeps no tag of the ID.

**The App Review account.** While Apple reviews the iOS app or Google reviews the Android app for
Google Play, and only until a time the operator sets, one more account can log in: a demo account
for their reviewers. Its ID, of another form than the portal IDs above, is only in a file on the
server, which also holds a hash of its password; the password typed is checked against that hash,
not by the Yonsei SSO, and is not kept, as no password is. The account gets no new app version,
and it ends with that file. From version 1.8.0 it connects only to the demo PCs that the same file
names (one to three). From version 1.9.2 it holds those demo PCs, which the operator logs in on with
it, and no other PC; a demo PC is kept as any PC in an account (below), and gets an access ticket
that only lets it register, until the file's end. Its logins leave the same records as any other,
with the tag of its ID, and the account is kept and deleted as any other, also in the app (section
8); its list entry then stays, since its ID names no person and the next review login needs it, and
so do its demo PCs, bound to a new, empty account of the same ID, until the operator releases them
when the review ends.

**Logging in.** You type your Yonsei portal ID and password in the app. If you use the device's
password autofill (Passwords on iOS, an autofill service on Android), it can fill in the ID and
password it saved, and after a login that succeeded it may offer to save them; what it saves stays
with that service, not with the app, and the app does not store the password (from version
1.9.0). The app seals the password
to the server's key, so that only the account service can open it, even on a network that inspects
encrypted traffic. The service has the Yonsei SSO (the university's login system) check it and
forgets it at once; the password and the SSO's token are never stored or logged. From the SSO the
service reads and keeps only the code it gives for you (your portal ID) and your name; it does not
read your e-mail address or department. Each login also registers a new **account key** of
that device: the server keeps its public key, the private key stays on the device, and every
account request after the login is signed with it.

**Your PCs.** A Windows PC with HJViewer installed joins your account when you log in to HJViewer on
it. From then on, when the operator has turned this on (section 1), your other logged-in devices can
connect to it without the PC's password, for the screen, file transfer and the camera; the terminal
and port forwarding still ask for the PC's password. The PC's HJViewer service signs a short request
to the server with the PC's device key, usually every 2 minutes and at most 5 minutes apart while it
reaches the server (when it cannot, it tries again after 30 seconds, waiting longer each time, up to
10 minutes), and gets back the signed list of your account's keys. With it, the PC reports its app
version, whether connections without the password are switched off on it, for each account key the
time of the last such connection it let in today and yesterday and their number, and how many
attempts it refused for each reason (counts only).

**Access tickets** (from version 1.8.0). To keep the server to faculty and staff, the account
service signs short-lived access tickets, and the server's connection broker checks them before it
sets up a connection or takes a PC's registration. A logged-in app gets one with its login and again
about halfway through its life (it lives 24 hours); a PC in your account gets one with each sync (72
hours); the App Review account gets one that reaches only its demo PCs, and its demo PCs one that
only lets them register (24 hours, never past the end of the account's file). A ticket holds the kind of
holder (staff, App Review, the access password or a support code), what it is good for, a tag of the
account (never the portal ID; none for the access password or a support code), the PC's ID and
public key when it lets that PC register,
the PCs it reaches when it is limited to some, and when it was made and when it ends. It is signed,
not encrypted: whoever holds it can read it and use it until it ends, so the app sends it only over
its encrypted connection to the server and never logs it. The account service does not keep tickets
or a record of each one; it counts them by kind, in memory. The connection broker keeps, while a PC
stays registered with a ticket, that ticket's key number and times, in memory.

**The access password** (from version 1.8.0). Someone who does not log in can type the access
password the operator gives out. The app seals it to the server's key, as it seals a login's
password, and the account service checks it against a hash in a file on the server and trades it
for a ticket that reaches one PC for 12 hours: the PC the app connects to, or the PC it was typed
on, which signs the request with its device key. The app forgets the password at once and keeps the
ticket in memory only. The account service records each try (the result, the time and a tag of the
client's IP address) and counts tries and tickets per address for its limits; it never keeps the
password, the address itself, the PC's ID or the ticket.

**Support codes** (from version 1.9.0). Instead of giving out the access password, a staff member
who is logged in can make a support code in the app for someone who is not: 8 characters that work
once, within 10 minutes, and only on a PC. Typed on that PC, the code is sealed as the access
password is, and the account service trades it for a ticket that lets that one PC accept
connections for 2 hours (the PC signs the request with its device key). The service keeps each
code in its memory only, looked up by a keyed hash of the code, with the account that
made it, for 20 minutes at most (10 while it works, and 10 more to tell a late or a used code from
a wrong one), and, as every account answer, the answer that showed it, for the app's retry; a
restart of the service ends them. It records each code made (a tag of the account
that made it, the time) and each try to use one (the result, the time, a tag of the client's IP
address and, once the code was found, the tag of the account that made it), and counts the tries
with the access password's for its limits; it never records the code, the address itself, the PC's
ID or the ticket. The app that made the code shows it in its window only; the PC forgets it once it
is sent and keeps the ticket in memory only.
From version 1.9.0 the answer that shows a code also brings a support link for the person being
helped, and a waiting number for the staff member's app; both are random values that live with the
code, in memory only (the link is looked up by a keyed hash too; the waiting number is kept as it
is). To show the code on the link's page, the service from 1.9.0 also keeps the code itself with it
while the code can be used, 10 minutes at most, and drops it once the code is used, ended or past
its 10 minutes. The link opens a page of the account service that shows the code and offers
HJViewer for Windows, three downloads at most; each download is recorded as the other downloads are
(section 2), with the tag of the account that made the code. While the code is open, the app that
made it asks the service with its waiting number, and its account key, whether a PC has used it;
once one has, the service tells that app, and no other, the PC's ID and public key, which it holds
in memory with the code (20 minutes at most) and never records. With that PC's ticket the service
also signs, for that PC only, which device made the code: the public key of that device's account
key and its device ID, no name. The PC keeps this in memory with its ticket and, when that device
connects, shows "Yonsei faculty or staff (verified login)" in its connection window; it opens
nothing by itself, and the person at the PC still accepts or refuses.

**What the account service keeps**

| What | Content | Kept |
|---|---|---|
| List | the portal ID of each person who logged in, put on the list at the first login, or whom the operator put on it ahead; how and when the entry came; whether the operator blocked it, and since when | with the account (below); an entry without an account a year after it came on the list; a blocked entry until the operator lifts the block; the App Review account's entry stays when that account is deleted in the app |
| Account | portal ID and name (from the SSO), times of creation, last login and last use (to the hour), the account's switch for connections without the PC's password | until a year without use (an account with a PC in use stays, unless it is blocked), or deleted in the app or on request (section 8) |
| Login tokens | a hash of each token, never the token | until logout, 90 days without use, a year at most |
| Account keys (one per logged-in device) | public key; device ID; a tag computed from the machine identifier of section 2 (not the identifier); platform, the device model the server names from it, the device name and the app version, as the device reported them; whether the login came from the university's network or from outside (not the address); times of creation and last login; your devices' answers to the notice below; when and how its login ended and which of your devices ended it | while the device is logged in, then 30 days |
| PCs in your account | PC ID, OS, host name, the PC's public key, a random binding ID, times added, last seen and last synced, the IP address its last sync came from (replaced by the next one's), app version, whether switched off on the PC; per reason, counts of refused attempts (30 days) | until removed from the account or 180 days unseen. After that the binding ID, the PC ID and its public key stay 180 days, so that the PC learns that it was removed |
| Connections without the PC's password | per account key, PC and day: the time of the last one and their number, as the PC reported them. There is no record per connection | 30 days; deleted with the key or the PC |
| Records of logins, devices, keys and account deletions | tags of the account and the device (not your ID), the client's IP address, the result, the time | 6 months |
| Access password tries (from 1.8.0) | the result, the time, a tag of the client's IP address (not the address, the PC's ID, the password or the ticket) | 6 months |
| Support codes (from 1.9.0) | for each code made, a tag of the account that made it and the time; for each try to use one, the result, the time, a tag of the client's IP address and, once the code was found, the tag of the account that made it (not the code, the address, the PC's ID or the ticket) | 6 months |
| Downloads of new app versions (section 2) | which way (a login, a PC's key, a short link made or used, the migration link, from 1.9.0 a support link), the platform, the time; tags of the account and the PC when there are any, and a tag of the client's IP address (not the address) | 6 months |
| Operator record | which administration command ran when, its targets as tags; as tags of the PC and of the address, when a PC's key downloads a new version from an address that PC never used before; each look of a super operator at the users and PCs (below), with that super operator as a tag and how many users and PCs it showed; each account deleted in the app, as a tag of the account, with how many PCs, keys, tokens and relays went | at least a year |
| Database backups | a copy of all of the above, on the server, readable by root only: one a day, and one before an upgrade or a restore | 14 days |
| The service's log | counts and tags, never IDs, addresses or secrets | 30 days |

Short-lived: the one-time values of requests (up to 20 minutes), the login, download and access
password limit counters, under tags (as long as their longest window, at least a day; the support
codes' tries count in the access password's), the short links, in memory only (10 minutes; a restart
of the service ends them), and the support codes, in memory only, each looked up by a keyed hash,
with the account that made it, and from 1.9.0 its support link (looked up by a keyed hash too), its
waiting number, its downloads left, the ID and key of the PC that used it (20 minutes at most; a
restart ends them) and, while it can be used (10 minutes at most), the code itself for its link's
page. In memory only,
never on disk: the service's answer to a login or to an account request that changes something, so
that the same request sent again within 30 seconds gets the same answer; it is cleared at the next
such request, within the hour at the latest.

**Not kept** by the account service: passwords, Yonsei SSO tokens, e-mail addresses, affiliation,
the machine identifier itself, OS user names, CPU and memory details, heartbeats, device
information, a record of each connection or file transfer, session contents, access tickets, the
access password, support codes, and the codes of short links and of the migration link.

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
When a faculty or staff member connects, from the device that made it, to a PC that entered their
support code, that PC shows "Yonsei faculty or staff (verified login)" in place of the name the
device sent: it shows neither the staff member's name nor their portal ID. From version 1.9.0 an
installed PC also keeps your device's ID and that name in its list of incoming connections for 90
days (section 4); for a connection it showed as faculty or staff, it keeps the ID without a name.

**What the operator sees.** The operator names the super operators (최고운영자; the app calls them
administrators, 관리자) by their portal IDs in the server's own settings, not in the app or its
code; everyone else is a normal user. A
super operator sees in the app, and can only look at, the users of the service (the accounts that
have logged in and are not blocked) and every PC in an account, also in an account the operator blocked: for each user the portal ID and name, when one of their devices or PCs was last
used and whether that was within 30 days, and how many logged-in devices and PCs they have; for
each PC its ID and host name, its owner's portal ID and name, the OS and app version, the time of
its last sync and the IP address that sync came from, the IPv4 addresses of its wired network
adapters (from the report of section 5) and the state of connections without the password. A
blocked account is no longer among the users, but its PCs stay in the list, with the owner's portal
ID and name, as long as the table above keeps them. A PC shows only once someone has logged in to HJViewer on it. The view cannot connect to anyone's PC or
change anything. It is there so that the operator knows who uses the service and on which PCs, to
help them and to protect the service. Each look is recorded in the operator record: which super
operator (as a tag), when, and how many users and PCs it showed.

### 4. On your devices

- **Every app**: its settings, its device ID and key pair, and the IDs of devices you connected to.
- **A logged-in app** also keeps the account key's private key: on an iPhone in the Keychain, for
  this device only and usable while it is unlocked: it is not synced through iCloud Keychain, and a
  backup carries it protected by a key tied to this device, so it can be restored only to this
  device; on Android in the app's data, which is left out of cloud backups and device-to-device
  transfers; on Windows in `%LOCALAPPDATA%\HJViewer-account\`, encrypted with Windows DPAPI for
  your Windows user and not roaming. It also keeps the login token, the name and portal ID the
  server returned, the list of your PCs (PC ID, public key, state) and the difference between its
  clock and the server's. A token without its account key opens nothing.
- **A PC in an account** keeps, in the Windows registry (`HKLM\SOFTWARE\HJViewer\Account`), its
  binding ID, your name masked, whether connections without the password are switched off on it,
  a record of a binding it let go, and for each account key the time of the last such connection
  today and yesterday and their number; in its service's folder, the signed list (the names, IDs
  and public keys of your devices and your masked name); and in HJViewer's log, a line for each
  connection it let in without the password and one when it ended such a connection because your
  device was logged out or the PC no longer takes such connections (the device's ID and name, the
  time). While such a connection is open, the PC also holds that device's public account key in
  memory, and nowhere else. HJViewer keeps about 90 days of logs.
  Other people who use that PC can read the masked name and, with some effort, the list.
  Uninstalling HJViewer deletes the registry key; each Windows user's settings and
  `%LOCALAPPDATA%\HJViewer-account\`, and the service's settings, list file and logs stay.
- **Remote readiness and session ends** (from version 1.9.0): the "Remote readiness" card reads this
  PC's HJViewer, power and network-card settings on the PC itself and sends none of them; it asks
  the server only whether a relay of your account serves this PC, with the request a PC's own wake
  check already uses. When you press [Never sleep], HJViewer changes the plugged-in sleep and
  hibernate times of Windows' current power plan and keeps the old values in your Windows user's
  HJViewer settings until you undo it. To lock a PC at the end of a session from your own devices
  only if it was locked when the session began, the PC keeps in memory, for ten minutes, whether it
  was locked at a session's start; its log notes each time it locked the PC at a session's end,
  without IDs or names.
- **Incoming connections** (from version 1.9.0): a Windows PC where HJViewer is installed keeps a
  list of the sessions it let in during the last 90 days, 2,000 at most, in a file in its service's
  folder (a PC that runs HJViewer without installing it keeps no list): for each session when it
  began and ended, the other device's ID and the name that device sent (its Windows user name, or a
  phone's device name), for a connection of your own account the device name and your masked name
  from the signed list, for a connection it showed as faculty or staff (a support code, section 3)
  the ID only, without a name, whether the PC checked who it was (your account, or the faculty or
  staff member who made its support code; otherwise the name is only what the other device sent),
  how it got in (the PC's one-time or permanent password, a reconnect of the same session, accepted
  at the PC, your account), the kind (remote screen, file transfer, camera, terminal, port
  forwarding), how it ended, and the number the other device gave that session, used only to show
  its reconnects as one line. It keeps no addresses, file names or session contents, and refused
  attempts are not in it. The list is not sent anywhere: HJViewer's settings show it on that PC
  (Security, behind the unlock of the security settings). Other programs and people who use that
  PC can read it, with some effort, through HJViewer's service. Uninstalling HJViewer deletes it.
- **Recordings**: a session you record is a video file on the device that recorded it (section 6).
- **Access tickets** (from version 1.8.0): a logged-in app keeps its ticket in its settings and
  deletes it at logout; a PC in an account keeps its latest ticket in a file in its service's
  folder, next to the signed list, so that it can register after a restart while the server cannot
  be reached (uninstalling deletes it); the tickets of the access password and of a support code
  stay in memory and end with the app (on an installed PC, with its service). From version 1.9.0 a
  PC that entered a support code keeps the support grant that came with its ticket in memory only,
  as the ticket, and forgets it with the ticket. The process that
  registers a PC notes in its settings whether the server took its last registration. A support
  code goes into no file or setting: the app that made it shows it in its window only (a copied
  code stays in the device's clipboard), and the PC it is typed on forgets it once it is sent.
  The app that made a code keeps its waiting number, and the ID and public key of the PC that used
  it, in memory only until that PC's ticket ends or the app logs out.
- **Updates** (from version 1.6.0): every app keeps the last version policy it verified, when it
  last checked and the difference between its clock and the server's. A PC that refuses an older
  app logs that app's platform and version, at most a line a minute. An installed Windows PC's
  HJViewer service keeps a record of its update attempts (the version, the file's hash, the time)
  and, until it is installed, the downloaded installer in a folder only Windows' system account and
  administrators can open; an app that is not installed saves the new version in the user's
  Downloads folder. An Android app reads which store installed it; a side-loaded one keeps a
  downloaded update in its own cache until it next starts, and checks the update's package name
  and signing certificate. Both happen on the phone only: none of this is sent to the server.
- **App lock** (phones and tablets, from version 1.9.0, off by default): the app keeps only
  whether the lock is on and how long it may stay in the background (immediately, 1 or 5
  minutes), as local settings. Face ID, Touch ID, fingerprints and the device passcode are
  checked by the phone's operating system; the app gets only "yes" or "no" and never sees,
  stores or sends biometric data. Nothing about the lock is sent to the server.

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

On phones, the Android app uses the microphone only during a voice call you start in a session,
and asks for the microphone permission only then; the sound goes end-to-end encrypted to the other
device and is not stored. The iPhone and iPad app has no voice call and no microphone permission.
Neither app uses the camera or your pictures, and from version 1.9.1 neither declares those
permissions. Earlier versions declared them for the QR code reader of the open-source code this app
is built on, which sets the server; HJViewer did not show that reader. Phones only control other
devices; they do not share their own screen.

On a Windows PC that accepts connections, a connected device sees its screen and, as far as the
PC's permission settings allow, its cameras and sound, and can hold a voice call that someone at
the PC accepts. All of it is end-to-end encrypted, and the server stores none of it.

**Recordings.** The device that controls a PC can record the session's screen (on a camera
connection, the camera's picture) as a video file without sound: when its user starts a
recording, or for every outgoing session once its user turned on automatic recording. Of the
phone apps, only the Android app records; the iPhone and iPad app does not. A PC can likewise
record the sessions it accepts once someone at it turned on automatic recording of incoming
sessions. The file stays on the device that recorded it until someone deletes it: on Windows in
the user's Videos\HJViewer folder or another folder its user chose (an installed PC's recordings
of incoming sessions in C:\ProgramData\HJViewer\recording), on an Android phone in the app's own
folder. HJViewer sends it nowhere and the server never sees it. The PC's owner can switch
recording off in the PC's permission settings, also during a session. From version 1.9.0 the
PC's connection window shows "Recording" while the connected device reports that it records; a
modified app, or a recording made with another program, does not show there.

### 7. Why, and on what basis

The account data is used to log you in and keep you logged in, to show your PCs and their state,
to let your own devices connect to your PCs without the PC's password, to let you notice and stop
logins and connections you did not make, to switch your PCs on, to protect the service (login
limits, records, backups), to keep the server to faculty and staff and to the people the operator
lets in with the access password or a staff member with a support code (access tickets, section
3), and to let the super operators see who uses it and on which PCs (section 3). The basis is the
service you ask for by logging in or by typing the access password or a support code (Personal
Information Protection Act, Article 15(1)4). Nothing is used for anything else, sold or shared
beyond section 9.

### 8. Deleting your data, and your rights

- **Deleting your account in the app** (from version 1.9.1): on a device logged in to your account,
  open Settings > Account > Delete account, type the word the app asks for and confirm. The
  account is deleted at once: you leave the list, your PCs leave the account, its relays are
  revoked, and the support codes it made that nobody used yet end. Its account keys and login
  tokens end too, so every device is logged out (this one at once, the others the next time they
  reach the server), and everything kept about it is deleted, the wake requests too. What stays is
  the same as under "Through the operator" below; the operator record notes the deletion with a
  tag of the account. You can still ask the operator instead.
- **Yourself, in the app**: log out on a device (its key and token end at once); on another of
  your devices, log out a lost one ("로그인한 기기", Logged-in devices) or all others; remove a PC from My
  PCs; log out on a PC before you give it away. Uninstalling HJViewer from a PC deletes what it kept
  in the registry and its list of incoming connections (section 4). Changing your portal password
  does not log out the devices already logged in: after a change, also use "Log out all other
  devices". A logout from another device stops new connections without the password, and from
  version 1.9.0 a PC that reaches the server also ends the device's open connections within 5
  minutes; a logout on the PC itself ends its password-less connections at once. On a PC that cannot
  reach the server or runs an older version, a connection that is open already stays until it is
  ended in the PC's connection window, by "Restart remote device" from your own device with the PC's
  password, or by restarting the PC.
- **Through the operator**: you can ask to see what is kept about you, to have it corrected or
  deleted, or to stop its processing. Deleting your account takes you off the list, revokes its
  relays and deletes everything kept about it, the wake requests too. What stays: the removed PCs'
  binding IDs, PC IDs and public keys (180 days), the revoked relays' IDs, keys and times, the
  records of logins, devices and keys under tags (6 months), the operator record, and a block the
  operator set (your portal ID with it, until the block is lifted); the backups age out after 14
  days. If you log in again later, your portal ID goes on the list again (while the operator lets
  in only the IDs already on the list, the operator puts it there again first).
- If the operator blocks your account, your devices are logged out at once, and your next login
  is refused with a message that says so.

### 9. Who handles the data

- **The operator**, 프로젝트TF (the controller of this data, 개인정보 처리자), runs the server and
  its account service, answers requests and is responsible for this data. Administration commands show IDs shortened and leave out names, addresses and device
  names unless a person asks for them; every command is recorded, except the one the five-minute
  check reads, which returns counts only and changes nothing. The super operators the operator
  names in the server's settings see in the app the users and PCs of section 3; each look is
  recorded.
- **The Yonsei SSO** checks your portal ID and password at login.
- **An AI assistant**: the operator uses an AI assistant for work on the server. Its rules let it
  see only counts and output that names no person and no person's device, and what it sees goes to
  the external AI service that runs it: Anthropic, in the United States, which does not use these
  conversations to train its models (that setting is turned off). Anything about a person or a
  person's device, and showing full values and making keys, is done by people only.
- **Not the PC's two-factor codes**: HJViewer builds turn off upstream RustDesk's option to send a
  PC's two-factor codes to a Telegram bot; the codes come from the authenticator app you set up.
- **STUN servers** (section 2). Nobody else: no advertising or analytics companies.
- Not about you: as in upstream rustdesk-server, the server's hbbs asks RustDesk's update service
  (api.rustdesk.com) at start and once a day whether a newer server version exists. It sends the
  server's operating system, its version and processor type, and a fingerprint of the server
  machine; nothing about users or their devices.

**Third parties.** The Yonsei SSO and public STUN servers get only the data named here and only for
the purpose named here; besides them and the operator, nobody gets user data from HJViewer. The
Yonsei SSO, the university's own login service, gets a portal ID and password only to check a login,
and protects them as the university's privacy policy and Korean law require. Public STUN servers get
only the requesting device's IP address and port, as any server the device contacts does; HJViewer
gives them no other user data.

### 10. Contact and changes

**Privacy manager** (개인정보 보호책임자): 프로젝트TF, haejin.cho@yonsei.ac.kr. Send requests
about your data (section 8) to that address, not to a public issue, and do not write personal
information, such as your portal ID or name, in a public issue. Questions about the app itself can
go to an issue at https://github.com/yonsei-projecttf/hjviewer. Changes to this statement are in the
history of that repository.

## 한국어

### 1. 요약

HJViewer는 직접 운영하는 서버에 붙는 원격 데스크톱 앱입니다. 앱에는 분석, 오류 보고, 광고가 없습니다. 앱을 만드는 프로젝트TF가 학교의 HJViewer 서버와 그 계정 서비스도 운영합니다(9절). 그 서버와 계정 서비스가 무엇을 보관하는지는 아래 절에 적었습니다. 앱은 연결하는 기기 말고는 빌드할 때 정해진 서버, 그리고 공개 STUN 서버(2절)하고만 통신합니다. 서버는 그 서버의 운영자가 관리합니다. 앱 밖의 페이지는 링크나 단추를 누를 때만 엽니다. 여는 곳은 GitHub에 있는 이 프로젝트의 페이지(이 문서, 소스 코드와 그 안내 문서), 그리고 앱을 업데이트할 때 iPhone은 TestFlight나 App Store, Android는 Google Play나 서버의 내려받기 링크입니다. 1.9.1까지는 연결한 기기가 보낸 안내나 그 기기에 관한 안내에 HJViewer의 바탕인 공개 소스 앱 RustDesk의 페이지(rustdesk.com의 도움말, 또는 GitHub의 이슈)로 가는 단추가 붙을 수도 있었습니다. 1.9.2부터는 그런 단추를 보이지 않습니다.

1.6.0부터 서버에 계정 기능이 생깁니다. 연세 SSO를 통과한 교직원은 연세포탈 ID로 로그인하고, 앱에서 자기 Windows PC를 보고, 자기 기기끼리 PC 비밀번호 없이 연결하고(운영자가 켠 경우), PC와 같은 네트워크의 중계기로 꺼진 PC를 켤 수 있습니다. 이 문서는 서버, 사용자의 기기, 그 중계기가 무엇을 얼마 동안 보관하는지와, 앱에서 지우거나 지워 달라고 하는 방법(8절)을 적습니다. 계정에 들어가지 않은 기기에서 로그인하지 않은 앱은 계정 서비스에 2절의 출입 확인만 보내고, 그 밖에는 누가 로그인하거나 이용 비밀번호를, PC에서는 지원 코드를 넣을 때 그것만 보냅니다(3절). 학교의 HJViewer 서버에서 그런 앱은 이용 비밀번호를 넣은 뒤에만 연결하고, 그런 PC는 이용 비밀번호나 지원 코드를 넣은 뒤에만 연결을 받습니다(아래). 계정에 들어간 PC는 다릅니다. 앱이 로그인해 있든 아니든, 그 PC의 HJViewer 서비스는 계정 서비스와 계속 동기화하고, PC는 3절의 "내 PC"와 "출입증"대로 연결을 받습니다. 그 PC의 로그인이 그 PC에서 로그아웃하지 않고 끝나도(다른 기기에서 로그아웃시킴, 기간 만료 등) PC는 계정에 남습니다. 빼려면 내 PC 목록에서 뺍니다(8절).

1.6.0부터 운영자는 앱의 최소 버전도 정할 수 있습니다. 앱은 서버의 서명된 버전 정책을 확인하고, 새 버전을 서버에서 받습니다(2절의 "업데이트").

1.8.0부터 운영자는 서버를 학교의 교직원만 쓰게 할 수 있습니다. 그러면 앱은 로그인해 있을 때만, 또는 운영자가 알려 주는 이용 비밀번호를 넣은 뒤 한동안만 연결하고, PC도 그때나 계정에 들어 있는 동안, 또는 로그인한 교직원이 만든 지원 코드를 넣은 뒤 한동안만(1.9.0부터) 연결을 받습니다(3절의 "출입증", "이용 비밀번호", "지원 코드"). 운영자가 이 확인을 켜지 않는 동안 앱은 지금처럼 움직입니다. 학교의 HJViewer 서버는 2026-10-19부터 이 확인을 켜 두었습니다.

### 2. 기기 연결 (모든 사용자)

- **기기 등록부**: 서버는 기기마다 ID, 공개키, 기기 식별값(PC는 운영체제가 정한 값으로 Windows에서는 MachineGuid, 휴대폰은 기기 공개키), 마지막 접속 IP를 보관합니다. 다른 기기가 이 기기를 찾을 수 있게 하기 위해서입니다.
- **서버 로그**: 서버는 기기가 처음 등록할 때와 주소나 키가 바뀔 때 기기 ID, IP 주소와 포트, 기기 식별값, 공개키를 기록합니다. 중계 서버는 중계한 세션마다 두 끝의 IP 주소와 시각을, 웹 서버는 접속 시각과 IP 주소를 기록합니다. 계정 서비스가 멈춰 있거나 한도에 걸린 동안의 계정 요청은 웹 서버의 오류 로그에 접속한 IP 주소와 요청 경로(PC ID가 들어갈 수 있음)를 남길 수 있습니다. 이 로그를 맞춰 보면 어느 기기가 언제 어느 PC에 중계로 연결했는지 알 수 있습니다. 운영자가 최소 버전을 강제하면, 서버는 오래된 앱의 연결을 거절할 때 그 앱의 IP 주소와 포트, 그 앱이 연결하려던 기기의 ID, 그 앱이 알린 버전도 기록합니다(10초에 30건까지 한 건씩, 그보다 많으면 개수만). 운영자가 출입 확인을 켜면(1.8.0), 서버는 쓸 수 있는 출입증이 없어 거절한 연결과 등록, 출입증이 끝나 끊은 PC도 같은 방식으로 기록합니다: 그 기기의 IP 주소와 포트, 연결하거나 등록하려던 ID, 까닭입니다. 출입증 자체는 기록하지 않습니다. 이 로그는 서버의 시스템 로그가 보관되는 동안 남습니다(운영자 설정).
- **서버는 세션 내용을 볼 수 없습니다.** 화면, 입력, 클립보드, 파일, 카메라 영상, 소리는 두 기기 사이에서 종단 간 암호화됩니다. 서버가 중계할 때도 암호화된 바이트만 전달합니다. 계정 서비스는 연결마다의 기록을 두지 않습니다(3절).
- **시스템 정보와 접속 기록**: 원격 접속을 받는 PC는 upstream RustDesk처럼 서버의 `/api` 주소로 하트비트, 호스트 이름, OS 사용자 이름, OS, CPU, 메모리 정보, 그리고 연결과 파일 전송마다의 기록(상대 기기의 주소·ID·이름, 파일 이름과 크기)을 보내려 합니다. 로그인한 앱도 세션에 남길 수 있는 메모를 그 주소로 보내려 합니다. 이 배포는 그 주소에 요청을 읽지 않고 404로 답하며 아무것도 저장하지 않습니다. 계정 서비스에는 3절의 계정 요청만 갑니다.
- **STUN 서버**: 두 기기를 직접 연결(P2P)하려고, 앱은 공개 STUN 서버(stun.cloudflare.com, stun.l.google.com, stun.antisip.com, stun.nextcloud.com)에 자신의 공인 IP와 포트를 묻습니다. 이 서버 운영자는 기기의 IP 주소와 시각을 알 수 있지만, 세션 내용은 알 수 없습니다.
- **업데이트: 버전 정책**(1.6.0부터): 앱은 서버의 버전 정책(`/api/hjv/version`)을 확인합니다. 시작할 때, 마지막 확인이 30분 넘은 뒤 연결하기 직전, 1시간 넘게 뒤에 있다가 앞으로 돌아올 때, 로그인이나 거절된 연결 직후, 그리고 6시간마다입니다. 정책은 운영자가 서명한 공개 파일이고 모두에게 같습니다. 이 요청에는 ID, 계정, 기기 정보가 없고, 웹 서버는 이 요청의 접속 기록을 남기지 않습니다.
- **서버의 출입 확인**(1.8.0부터): 앱은 켤 때, 앞으로 돌아올 때, 그리고 그 답에 따라 달라지는 화면(PC의 알림, 세션의 오프라인과 다시 시작 안내)을 보이기 전에(1분에 한 번까지) 서버가 출입 확인을 켰는지(`/api/hjv/access`)를 묻고, 그 답으로 로그인 안내와 로그아웃 경고, 그리고 1.9.0부터는 PC가 지원 코드를 받을지를 고릅니다. 답은 모드 한 낱말과, 1.9.0부터는 지원 코드를 쓸 수 있는지이며 모두에게 같습니다. 이 요청에는 ID, 계정, 기기 정보가 없고, 계정 서비스는 이 요청을 기록하지 않으며, 웹 서버도 이 요청의 접속 기록을 남기지 않습니다.
- **업데이트: 내려받기**(1.6.0부터): 계정 서비스가 새 버전의 설치 파일을 내줍니다. 로그인한 앱, 계정에 들어간 PC의 HJViewer 서비스, 그리고 로그인이 필요 없는 링크 세 가지로 받습니다. 하나는 운영자가 아직 1.5.x를 쓰는 사람에게 주는 이행용 링크, 하나는 로그인한 앱이 다른 PC를 위해 만드는 짧은 링크(10분 동안 세 번까지 쓰고, 서비스의 메모리에만 있음), 그리고 1.9.0부터 지원 코드와 함께 나오는 지원 링크(그 코드가 쓰이는 동안, 세 번까지, 3절)입니다. 서비스는 내려받기와 짧은 링크 만들기마다 어느 길로 어느 플랫폼의 파일을 받았는지, 시각, 그리고 꼬리표를 기록합니다. 꼬리표는 계정과 PC가 있으면 그 꼬리표, 그리고 접속한 IP 주소의 꼬리표입니다(주소 자체는 없음). 이 기록은 6개월 동안 둡니다. 이행용 링크의 내려받기는 1분에 20개까지만 기록합니다. 링크의 코드는 저장하지 않고, 서비스의 로그에는 코드의 꼬리표 앞 16진수 8자리만 남습니다. 웹 서버는 이 요청의 접속 기록도 남기지 않습니다(오류 로그는 위 '서버 로그').

### 3. 계정 (1.6.0부터)

로그인한 사람에게만 해당합니다. 학교의 교직원만 로그인할 수 있습니다. 계정 서비스는 교직원의 포탈 ID(형식으로 가림: Z나 Y로 시작하고 ZA로 시작하지 않음)만 받고, 연세 SSO가 알려 준 그 사람의 코드도 같은 규칙으로 확인합니다. 그 사람은 운영자가 막지 않는 한 누구나 HJViewer를 쓸 수 있습니다. 계정 서비스는 처음 로그인할 때 그 포탈 ID를 명단에 올립니다. 운영자는 미리 명단에 올린 ID만 받는 방식으로 되돌릴 수도 있습니다. 계정 서비스는 같은 서버에서 돕니다. 로그인에 실패해도(틀린 비밀번호, 막힌 계정) 시도 기록(입력한 ID와 기기의 꼬리표, 접속한 IP 주소, 결과, 시각; 6개월)과 로그인 한도 기록은 남습니다. 다른 형식의 ID는(아래 App Review 계정의 ID는 빼고) SSO에 묻기 전에 바로 거절하며, 그 ID와 비밀번호를 SSO에 보내지 않고 시도 기록에도 그 ID의 꼬리표를 남기지 않습니다.

**App Review 계정.** Apple이 iOS 앱을, Google이 Google Play의 Android 앱을 심사하는 동안, 운영자가 정한 때까지만 계정 하나가 더 로그인할 수 있습니다. 심사자가 쓰는 데모 계정입니다. 그 ID는 위의 포탈 ID와 다른 형식이고 서버의 파일 하나에만 있으며, 그 파일에는 비밀번호의 해시도 있습니다. 입력한 비밀번호는 연세 SSO가 아니라 그 해시로 확인하고, 다른 비밀번호처럼 보관하지 않습니다. 이 계정은 새 버전을 받지 못하고, 그 파일이 없어지면 끝납니다. 1.8.0부터는 같은 파일에 적힌 데모 PC(1~3대)에만 연결합니다. 1.9.2부터는 운영자가 이 계정으로 로그인한 그 데모 PC를 가지고, 다른 PC는 가지지 못합니다. 데모 PC는 계정에 들어간 다른 PC처럼 보관하고(아래), 등록에만 쓰이는 출입증을 그 파일이 끝날 때까지 받습니다. 이 계정의 로그인도 다른 로그인과 같은 기록(그 ID의 꼬리표 포함)을 남기고, 계정도 다른 계정처럼 보관하고 지우며 앱에서도 지울 수 있습니다(8절). 그때 명단 항목은 남습니다. 그 ID는 사람의 포탈 ID가 아니고, 다음 심사 로그인에 필요하기 때문입니다. 데모 PC도 같은 ID의 새 빈 계정에 묶인 채 남고, 심사가 끝나면 운영자가 풉니다.

**로그인.** 앱에 연세포탈 ID와 비밀번호를 넣습니다. 기기의 암호 자동 완성(iOS의 암호, Android의 자동 완성 서비스)을 쓰면 그것이 저장해 둔 ID와 비밀번호를 넣을 수 있고, 로그인에 성공한 뒤에는 그것이 저장할지 물을 수 있습니다. 저장하면 앱이 아니라 그 서비스가 보관하고, 앱은 비밀번호를 저장하지 않습니다(1.9.0부터). 앱은 비밀번호를 서버의 키로 봉인하므로, 암호화된 통신을 들여다보는 네트워크에서도 계정 서비스만 열 수 있습니다. 계정 서비스는 연세 SSO(학교의 로그인 시스템)에 비밀번호를 확인받은 뒤 바로 버립니다. 비밀번호와 SSO 토큰은 저장하지도 기록하지도 않습니다. SSO에서 받은 것 가운데 그 사람의 코드(포탈 ID)와 성명만 읽고 보관하며, 이메일 주소와 소속은 읽지 않습니다. 로그인할 때마다 그 기기의 **계정 키**도 새로 등록합니다. 서버는 공개키만 보관하고 개인키는 기기에 남으며, 로그인 뒤의 모든 계정 요청에 그 키로 서명합니다.

**내 PC.** HJViewer를 설치한 Windows PC에서 HJViewer에 로그인하면 그 PC가 내 계정에 들어갑니다. 그때부터 운영자가 이 기능을 켠 경우(1절), 로그인한 다른 내 기기는 그 PC에 PC 비밀번호 없이 원격 화면, 파일 전송, 카메라로 연결할 수 있습니다. 터미널과 포트 포워딩은 지금처럼 PC 비밀번호를 묻습니다. 그 PC의 HJViewer 서비스는 보통 2분마다, 서버에 닿는 동안에는 늦어도 5분마다 PC의 기기 키로 서명한 짧은 요청을 서버에 보내고, 서명된 내 계정의 키 목록을 받습니다(닿지 못하면 30초 뒤부터 간격을 늘려 가며 최대 10분마다 다시 해 봅니다). 이때 PC는 앱 버전, 비밀번호 없는 연결을 그 PC에서 꺼 두었는지, 계정 키마다 오늘과 어제 받아들인 비밀번호 없는 연결의 마지막 시각과 횟수, 거절한 시도의 이유별 개수(개수만)를 함께 알립니다.

**출입증**(1.8.0부터). 서버를 교직원만 쓰게 하려고, 계정 서비스는 수명이 짧은 출입증에 서명하고, 서버의 연결 중개(hbbs)는 연결을 잇거나 PC의 등록을 받기 전에 그것을 확인합니다. 로그인한 앱은 로그인할 때와 수명이 반쯤 지날 때 받고(24시간), 계정에 들어간 PC는 동기화마다 받으며(72시간), App Review 계정은 데모 PC에만 닿는 것을, 그 데모 PC는 자기 등록에만 쓰이는 것(24시간, 계정 파일의 날짜를 넘지 않음)을 받습니다. 출입증에는 가진 쪽의 종류(교직원, App Review, 이용 비밀번호나 지원 코드), 쓸 수 있는 자리, 계정의 꼬리표(포탈 ID가 아님. 이용 비밀번호와 지원 코드에는 없음), PC를 등록하게 하는 것이면 그 PC의 ID와 공개키, 몇몇 PC에만 닿는 것이면 그 PC들, 만든 시각과 끝나는 시각이 들어 있습니다. 서명했을 뿐 암호화하지 않았으므로 가진 사람은 끝날 때까지 읽고 쓸 수 있습니다. 그래서 앱은 서버와의 암호화된 연결로만 보내고 로그에 쓰지 않습니다. 계정 서비스는 출입증을 저장하지도 하나하나 기록하지도 않고, 종류별로 센 수만 메모리에 둡니다. 연결 중개는 출입증으로 등록한 PC가 연결해 있는 동안 그 출입증의 키 번호와 시각을 메모리에 둡니다.

**이용 비밀번호**(1.8.0부터). 로그인하지 않은 사람은 운영자가 알려 주는 이용 비밀번호를 넣을 수 있습니다. 앱은 로그인의 비밀번호처럼 서버의 키로 봉인해 보내고, 계정 서비스는 서버의 파일에 있는 해시로 확인한 뒤 PC 하나에 12시간 동안 닿는 출입증으로 바꿔 줍니다. 앱이 연결하려는 PC, 또는 비밀번호를 넣은 그 PC입니다(그 PC는 요청에 기기 키로 서명합니다). 앱은 비밀번호를 바로 버리고 출입증은 메모리에만 둡니다. 계정 서비스는 시도마다 결과, 시각, 접속한 IP 주소의 꼬리표를 기록하고, 한도를 위해 주소마다 시도와 출입증의 수를 셉니다. 비밀번호, 주소 자체, PC ID, 출입증은 남기지 않습니다.

**지원 코드**(1.9.0부터). 로그인한 교직원은 이용 비밀번호를 알려 주는 대신, 로그인하지 않은 사람에게 줄 지원 코드를 앱에서 만들 수 있습니다. 8자이고, 10분 안에 한 번, PC에서만 쓰입니다. 그 PC에서 넣은 코드는 이용 비밀번호처럼 봉인해 보내고, 계정 서비스는 그 PC 하나가 2시간 동안 연결을 받는 출입증으로 바꿔 줍니다(그 PC는 요청에 기기 키로 서명합니다). 계정 서비스는 코드를 메모리에만, 키를 쓴 코드의 해시로 찾게 만든 계정과 함께 최대 20분 둡니다(쓸 수 있는 10분, 그리고 늦게 넣거나 이미 쓴 코드를 틀린 코드와 가리기 위한 10분). 다른 계정 요청의 답처럼, 그 코드를 보여 준 답도 앱이 다시 보낼 때를 위해 메모리에 둡니다. 서비스를 다시 시작하면 사라집니다. 코드를 만들 때마다(만든 계정의 꼬리표, 시각), 코드를 쓰려는 시도마다(결과, 시각, 접속한 IP 주소의 꼬리표, 코드를 찾았으면 그 코드를 만든 계정의 꼬리표) 기록하고, 한도를 위해 시도를 이용 비밀번호의 시도와 함께 셉니다. 코드, 주소 자체, PC ID, 출입증은 남기지 않습니다. 코드를 만든 앱은 그 창에서만 코드를 보이고, PC는 보낸 뒤 바로 버리며 출입증은 메모리에만 둡니다. 1.9.0부터 코드를 보이는 답에는 도움받을 분에게 줄 지원 링크와 교직원 앱이 쓸 대기 번호도 있습니다. 둘 다 코드와 함께 메모리에만 있는 무작위 값입니다(링크도 키를 쓴 해시로 찾고, 대기 번호는 그대로 둡니다). 1.9.0부터 서비스는 링크의 페이지에 코드를 보이려고, 코드를 쓸 수 있는 동안(최대 10분) 코드 자체도 함께 두고, 코드를 쓰거나 끝나거나 10분이 지나면 버립니다. 링크는 코드를 보이고 Windows용 HJViewer를 내주는 계정 서비스의 페이지를 엽니다(내려받기 세 번까지). 내려받기는 2절의 다른 내려받기처럼, 코드를 만든 계정의 꼬리표와 함께 기록합니다. 코드가 열린 동안 그 코드를 만든 앱은 대기 번호와 그 기기의 계정 키로 어느 PC가 코드를 썼는지 묻고, 서비스는 그 앱에만 그 PC의 ID와 공개키를 알려 줍니다. 이 값은 코드와 함께 메모리에만(최대 20분) 두고 기록하지 않습니다. 서비스는 그 PC의 출입증과 함께, 그 PC에만 쓰이는 "코드를 만든 기기"(그 기기 계정 키의 공개키와 기기 ID. 이름은 없음)를 서명해 줍니다. PC는 이것을 출입증 옆 메모리에만 두고, 그 기기가 연결하면 연결 창에 "연세대 교직원(로그인 확인)"을 보입니다. 이것으로 열리는 것은 없고, 받을지는 PC 앞의 사람이 정합니다.

**계정 서비스가 보관하는 것**

| 항목 | 내용 | 보관 |
|---|---|---|
| 명단 | 로그인한 사람의 포탈 ID(처음 로그인할 때 올라감)나 운영자가 미리 올린 포탈 ID, 어떻게 언제 올랐는지, 운영자가 막았는지와 언제부터인지 | 계정과 함께(아래). 계정이 없는 항목은 오른 지 1년 뒤. 막힌 항목은 운영자가 막기를 풀 때까지. App Review 계정을 앱에서 지우면 그 항목은 남음 |
| 계정 | 포탈 ID와 성명(SSO에서 받음), 만든 시각·마지막 로그인 시각·마지막 사용 시각(한 시간 단위), 그 계정의 비밀번호 없는 연결 스위치 | 1년 동안 쓰지 않을 때까지(쓰는 PC가 있는 계정은 막히지 않았으면 남김), 또는 앱에서 지우거나 요청하면 삭제(8절) |
| 로그인 토큰 | 토큰의 해시만(토큰 자체는 없음) | 로그아웃할 때까지, 90일 동안 쓰지 않거나 최대 1년 |
| 계정 키(로그인한 기기마다 하나) | 공개키, 기기 ID, 기기 식별값(2절)으로 만든 꼬리표(식별값 자체는 없음), 기기가 알린 플랫폼·기기 이름·앱 버전과 그것으로 서버가 정한 기기 모델 이름, 학내 네트워크에서 로그인했는지(주소는 없음), 만든 시각·마지막 로그인 시각, 아래 알림에 내 기기들이 한 답, 로그인이 언제 어떻게 끝났는지와 끝낸 내 기기 | 그 기기가 로그인해 있는 동안, 그 뒤 30일 |
| 내 계정의 PC | PC ID, OS, 호스트 이름, PC 공개키, 무작위 묶음 ID, 추가한 시각·마지막으로 보인 시각·마지막 동기화 시각, 마지막 동기화가 온 IP 주소(다음 동기화의 주소로 바뀜), 앱 버전, 그 PC에서 꺼 두었는지, 거절한 시도의 이유별 개수(30일) | 계정에서 빼거나 180일 동안 보이지 않을 때까지. 그 뒤 묶음 ID, PC ID, PC 공개키만 180일 남깁니다. PC가 빠졌다는 것을 그 PC에 알리기 위해서입니다 |
| 비밀번호 없는 연결 | 계정 키·PC·날짜마다 마지막 연결 시각과 횟수(PC가 알린 값). 연결마다의 기록은 없음 | 30일. 그 키나 PC와 함께 삭제 |
| 로그인·기기·키·계정 삭제 기록 | 계정과 기기의 꼬리표(ID가 아님), 접속한 IP 주소, 결과, 시각 | 6개월 |
| 이용 비밀번호 시도(1.8.0부터) | 결과, 시각, 접속한 IP 주소의 꼬리표(주소, PC ID, 비밀번호, 출입증은 없음) | 6개월 |
| 지원 코드(1.9.0부터) | 만들 때: 만든 계정의 꼬리표, 시각. 쓰려 할 때: 결과, 시각, 접속한 IP 주소의 꼬리표, 코드를 찾았으면 만든 계정의 꼬리표(코드, 주소, PC ID, 출입증은 없음) | 6개월 |
| 새 버전 내려받기(2절) | 어느 길(로그인, PC의 키, 짧은 링크를 만들거나 씀, 이행용 링크, 1.9.0부터 지원 링크), 플랫폼, 시각. 계정과 PC가 있으면 그 꼬리표, 그리고 접속한 IP 주소의 꼬리표(주소는 없음) | 6개월 |
| 운영자 기록 | 어떤 관리 명령을 언제 실행했는지, 대상은 꼬리표로. PC의 키가 그 PC가 쓴 적 없는 주소에서 새 버전을 받았을 때 그 PC와 주소의 꼬리표. 최고운영자가 사용자와 PC 목록(아래)을 볼 때마다 그 최고운영자(꼬리표)와 보인 사용자·PC의 수. 앱에서 지운 계정마다 그 계정의 꼬리표와 빠진 PC·키·토큰·중계기의 수 | 1년 이상 |
| DB 백업 | 위 전부의 사본(서버 안, root만 읽음). 매일 하나, 그리고 업그레이드나 복원 전에 하나 | 14일 |
| 계정 서비스의 로그 | 개수와 꼬리표만(ID, 주소, 비밀값은 없음) | 30일 |

잠깐만 두는 것: 요청의 일회용 값(최대 20분), 꼬리표로 센 로그인, 내려받기, 이용 비밀번호 한도 기록(가장 긴 한도 기간, 최소 하루. 지원 코드의 시도도 여기에 셈), 메모리에만 두는 짧은 링크(10분, 서비스를 다시 시작하면 사라짐)와 지원 코드(키를 쓴 해시로 찾음, 만든 계정, 1.9.0부터 지원 링크(이것도 키를 쓴 해시로 찾음), 대기 번호, 남은 내려받기 수, 코드를 쓴 PC의 ID와 공개키, 최대 20분, 서비스를 다시 시작하면 사라짐. 쓸 수 있는 동안(최대 10분)은 링크의 페이지를 위한 코드 자체도). 메모리에만 두고 디스크에는 쓰지 않는 것: 로그인과 무언가를 바꾸는 계정 요청에 한 답. 30초 안에 같은 요청이 다시 오면 같은 답을 주려는 것이고, 다음 그런 요청 때나 늦어도 1시간 안에 지웁니다.

**계정 서비스가 보관하지 않는 것**: 비밀번호, 연세 SSO 토큰, 이메일 주소, 소속, 기기 식별값 자체, OS 사용자 이름, CPU·메모리 정보, 하트비트, 기기 정보, 연결이나 파일 전송마다의 기록, 세션 내용, 출입증, 이용 비밀번호, 지원 코드, 짧은 링크와 이행용 링크의 코드.

**다른 내 기기에 보이는 것.** 어떤 기기가 내 계정에 로그인하면 다른 내 기기에 알림이 뜹니다. 서버가 본 시각과 학내 네트워크인지, 그리고 그 기기가 스스로 알린(아무도 확인하지 않는) 앱 버전과 기기 모델이 나오므로, 내가 하지 않은 로그인을 알아챌 수 있습니다. 알림은 내 기기 하나에서 [내 기기 맞음]이나 [내가 아님]으로 답할 때까지 기한 없이 남습니다. 지금 쓰는 기기보다 먼저 로그인한 기기에도, 아무도 답하기 전에 로그인이 끝난 기기에도 알림이 뜨고, 끝난 기기는 어떻게 끝났는지도 나옵니다. '로그인한 기기' 목록에는 내 PC마다 그 기기가 마지막으로 비밀번호 없이 연결한 시각과 30일 동안의 횟수가 나오고, 로그인이 끝난 기기도 30일 동안 따로 나옵니다. 플랫폼과 이름, 로그인 시각, 마지막 사용, 어떻게 끝났는지(스스로 로그아웃, 다른 내 기기 중 어느 것에서 로그아웃시킴, 기간 만료, 같은 기기의 새 로그인으로 바뀜, 관리자가 로그아웃시킴)가 나옵니다.

**다른 사람에게 보이는 것.** 다른 사람의 PC에 연결하면 그 PC에는 1.6.0 전처럼 내 Windows 사용자 이름(휴대폰은 기기 이름)이 보입니다. 로그인해도 성명이나 포탈 ID를 다른 사람의 PC에 보내지 않습니다. 내 PC의 연결 창에는 연결한 기기의 이름과 가린 내 이름(예: 홍\*동)이 서명된 목록에서 나옵니다. 그 기기 이름은 기기가 로그인할 때 알린 값으로 서버가 정한 모델 이름이고, PC의 로그에 남는 기기 ID도 그 기기가 스스로 알린 값입니다. 교직원이 지원 코드를 만든 기기로 그 코드를 넣은 PC에 연결하면, 그 PC에는 그 기기가 보낸 이름 대신 "연세대 교직원(로그인 확인)"이 보입니다. 성명도 포탈 ID도 보이지 않습니다. 1.9.0부터 설치한 PC는 내 기기의 ID와 그 이름을 '들어온 연결' 목록에 90일 동안 둡니다(4절). 교직원 표시가 붙은 연결은 이름 없이 ID만 둡니다.

**운영자에게 보이는 것.** 운영자는 최고운영자(앱에서는 '관리자')를 포탈 ID로 서버의 설정에서만 정합니다(앱이나 코드에는 없음). 그 밖의 사람은 모두 일반 사용자입니다. 최고운영자는 앱에서 이 서비스의 사용자(로그인한 적이 있고 막히지 않은 계정)와 계정에 든 모든 PC(운영자가 막은 계정의 PC도)를 보기만 할 수 있습니다. 사용자마다 포탈 ID와 성명, 그 사람의 기기나 PC를 마지막으로 쓴 때와 그것이 30일 안인지, 로그인한 기기 수와 PC 수가 보이고, PC마다 PC ID와 호스트 이름, 주인의 포탈 ID와 성명, OS와 앱 버전, 마지막 동기화 시각과 그 동기화가 온 IP 주소, 유선 랜카드의 IPv4 주소(5절의 보고에서), 비밀번호 없는 연결 상태가 보입니다. 막힌 계정은 사용자 목록에서 빠지지만, 그 계정의 PC는 위 표의 보관 기간 동안 주인의 포탈 ID·성명과 함께 PC 목록에 남습니다. PC는 누군가 그 PC에서 HJViewer에 로그인한 뒤에야 보입니다. 이 화면으로는 다른 사람의 PC에 연결하거나 무엇을 바꿀 수 없습니다. 누가 어느 PC에서 이 서비스를 쓰는지 운영자가 알고, 사용자를 돕고 서비스를 지키기 위한 것입니다. 볼 때마다 운영자 기록에 남습니다: 어느 최고운영자인지(꼬리표), 언제, 몇 명의 사용자와 몇 대의 PC를 보였는지.

### 4. 기기에 저장되는 정보

- **모든 앱**: 설정, 기기 ID와 키 쌍, 접속했던 기기 ID.
- **로그인한 앱**은 계정 키의 개인키도 보관합니다. iPhone은 Keychain에 이 기기 전용, 잠금이 풀렸을 때만 쓰도록 둡니다. iCloud 키체인으로 동기화되지 않고, 백업에는 이 기기에 묶인 키로 보호된 채 들어가므로 같은 기기에만 복원할 수 있습니다. Android는 앱 데이터에 두며, 클라우드 백업과 기기 간 이전에서 뺍니다. Windows는 `%LOCALAPPDATA%\HJViewer-account\`에 Windows DPAPI로 그 Windows 사용자만 풀 수 있게 암호화해 두고, 로밍하지 않습니다. 그 밖에 로그인 토큰, 서버가 보낸 성명과 포탈 ID, 내 PC 목록(PC ID, 공개키, 상태), 서버와의 시계 차이를 보관합니다. 계정 키가 없는 토큰으로는 아무것도 할 수 없습니다.
- **계정에 들어간 PC**는 Windows 레지스트리(`HKLM\SOFTWARE\HJViewer\Account`)에 묶음 ID, 가린 내 이름, 비밀번호 없는 연결을 그 PC에서 꺼 두었는지, 풀린 묶음의 기록, 계정 키마다 오늘과 어제의 비밀번호 없는 연결의 마지막 시각과 횟수를 둡니다. 서비스 폴더에는 서명된 목록(내 기기들의 이름·ID·공개키와 가린 내 이름)을, HJViewer 로그에는 비밀번호 없이 받아들인 연결마다 한 줄, 그 기기가 로그아웃되었거나 PC가 그런 연결을 더 받지 않아 그 연결을 끊었을 때 한 줄(그 기기의 ID와 이름, 시각)을 둡니다. 그런 연결이 열려 있는 동안 PC는 그 기기의 계정 공개키를 메모리에만 둡니다. HJViewer 로그는 약 90일치를 둡니다. 그 PC를 쓰는 다른 사람은 가린 이름을 볼 수 있고, 마음먹으면 그 목록도 볼 수 있습니다. HJViewer를 제거하면 레지스트리 키가 지워지고, Windows 사용자마다의 설정과 `%LOCALAPPDATA%\HJViewer-account\`, 서비스의 설정·목록 파일·로그는 남습니다.
- **원격 준비와 세션 끝**(1.9.0부터): '원격 준비' 카드는 이 PC의 HJViewer·전원·랜카드 설정을 그 PC 안에서 읽을 뿐 서버에 보내지 않습니다. 서버에는 내 계정의 중계기가 이 PC를 맡는지만, PC의 켜기 확인이 이미 쓰는 요청으로 묻습니다. [절전 안 함]을 누르면 Windows의 지금 전원 계획에서 '전원 연결 시' 절전·최대 절전 시간을 바꾸고, 되돌릴 때까지 원래 값을 그 Windows 사용자의 HJViewer 설정에 둡니다. 내 기기에서 연 세션이 끝날 때 '시작할 때 잠겨 있었으면' 잠그기 위해, PC는 세션 시작 때 잠겨 있었는지를 10분 동안 메모리에 두고, 세션이 끝나 잠글 때마다 ID나 이름 없이 로그에 한 줄 남깁니다.
- **들어온 연결**(1.9.0부터): HJViewer를 설치한 Windows PC는 최근 90일 동안 들여보낸 세션의 목록(많아야 2,000건)을 서비스 폴더의 파일에 둡니다(설치하지 않고 실행한 HJViewer는 목록을 두지 않음). 세션마다 시작과 끝 시각, 상대 기기의 ID와 그 기기가 보낸 이름(Windows 사용자 이름, 휴대폰은 기기 이름), 내 계정의 연결이면 서명된 목록의 기기 이름과 가린 내 이름, 교직원 표시가 붙은 연결(지원 코드, 3절)이면 이름 없이 ID만, PC가 신원을 확인했는지(내 계정, 또는 지원 코드를 만든 교직원. 그 밖에는 상대가 보낸 이름일 뿐임), 들어온 방법(PC의 일회용·영구 비밀번호, 같은 세션의 다시 연결, PC에서 수락, 내 계정), 종류(원격 화면, 파일 전송, 카메라, 터미널, 포트 포워딩), 끝난 방식, 그리고 다시 연결을 한 줄로 보이는 데만 쓰는 상대 기기의 세션 번호입니다. 주소, 파일 이름, 세션 내용은 두지 않고, 거절한 시도는 넣지 않습니다. 목록은 어디로도 보내지 않고, 그 PC의 HJViewer 설정(보안, 보안 설정 잠금 해제 뒤)에서 봅니다. 그 PC를 쓰는 다른 사람과 프로그램도 마음먹으면 HJViewer 서비스를 통해 읽을 수 있습니다. HJViewer를 제거하면 지워집니다.
- **녹화**: 세션을 녹화하면 녹화한 그 기기에 영상 파일로 남습니다(6절).
- **출입증**(1.8.0부터): 로그인한 앱은 출입증을 설정에 두고 로그아웃할 때 지웁니다. 계정에 들어간 PC는 마지막 출입증을 서비스 폴더의 서명된 목록 옆 파일에 둡니다. 서버에 닿지 못하는 동안 다시 시작해도 등록하기 위해서이고, 제거하면 지워집니다. 이용 비밀번호와 지원 코드의 출입증은 메모리에만 있고 앱(설치한 PC는 서비스)이 끝나면 사라집니다. 1.9.0부터 지원 코드를 넣은 PC는 출입증과 함께 온 지원 확인을 출입증처럼 메모리에만 두고 출입증과 함께 버립니다. PC를 등록하는 프로세스는 서버가 마지막 등록을 받았는지를 설정에 적어 둡니다. 지원 코드는 파일이나 설정에 쓰지 않습니다. 만든 앱은 그 창에서만 보이고(복사하면 기기의 클립보드에 남음), 코드를 넣은 PC는 보낸 뒤 바로 버립니다. 코드를 만든 앱은 대기 번호와, 코드를 쓴 PC의 ID와 공개키를 그 PC의 출입증이 끝나거나 로그아웃할 때까지 메모리에만 둡니다.
- **업데이트**(1.6.0부터): 모든 앱은 마지막으로 확인한 버전 정책, 마지막 확인 시각, 서버와의 시계 차이를 둡니다. 오래된 앱을 거절한 PC는 그 앱의 플랫폼과 버전을 로그에 남깁니다(1분에 한 줄까지). 설치된 Windows PC의 HJViewer 서비스는 업데이트 시도 기록(버전, 파일의 해시, 시각)을 두고, 설치하기 전까지는 내려받은 설치 파일을 Windows의 시스템 계정과 관리자만 열 수 있는 폴더에 둡니다. 설치하지 않고 쓰는 앱은 새 버전을 사용자의 '다운로드' 폴더에 둡니다. Android 앱은 어느 스토어에서 설치됐는지 읽습니다. 파일로 설치한 앱은 내려받은 업데이트를 다음에 시작할 때까지 자기 캐시에 두고, 그 패키지 이름과 서명 인증서를 확인합니다. 둘 다 휴대폰 안에서만 하고, 서버에 보내지 않습니다.
- **앱 잠금**(휴대폰과 태블릿, 1.9.0부터, 기본은 꺼짐): 앱은 잠금을 켰는지와 백그라운드에 둘 수 있는 시간(바로, 1분, 5분)만 기기의 설정에 둡니다. Face ID·Touch ID·지문과 기기 암호는 휴대폰의 운영체제가 확인하고, 앱은 맞았는지만 받습니다. 생체 정보를 보거나 저장하거나 보내지 않으며, 잠금에 관한 것은 서버에 아무것도 보내지 않습니다.

### 5. 꺼진 PC 켜기 (Wake-on-LAN)

- **계정에 들어간 PC**는 켤 수 있는지 서버가 판단하는 데 필요한 것을 알립니다. 유선 랜카드의 MAC 주소, IPv4 주소와 서브넷, 게이트웨이의 IP와 MAC 주소, 랜카드의 이름·드라이버·링크 속도와 USB 랜카드인지, Wake-on-LAN 설정과 Windows가 그 랜카드로 PC를 깨우도록 허용했는지, PC가 쓸 수 있는 절전 방식, 빠른 시작을 켰는지, 전원에 연결되어 있는지, Windows가 켜진 뒤 지난 시간(서버는 이 값을 보관하지 않음), PC가 쓰는 공인 주소입니다. 계정에 들어간 PC만 알리고, PC가 계정에서 빠지면 바로 지웁니다. 앱에는 MAC 주소를 보내지 않습니다.
- **중계기**: PC와 같은 네트워크의 NAS에서 도는 중계기에 대해 서버는 ID와 공개키, 맡은 계정, 관리자가 붙인 이름, NAS의 랜카드(이름, IP와 MAC 주소, 네트워크, 게이트웨이의 IP와 MAC 주소), 중계기 프로그램·DSM·커널 버전, NAS가 쓰는 공인 주소, 처음 요청한 시각·승인한 시각·마지막으로 물은 시각을 보관합니다. 요청만 하고 승인받지 못한 중계기는 24시간 뒤 지우고, 중계기보다 먼저 승인해 둔 등록 코드는 7일 동안 둡니다. 중계기를 폐기하면 ID, 키, 시각만 남깁니다.
- **켜기 요청**: 켜기 요청마다 기록을 하나 남깁니다. 요청의 무작위 번호, 계정과 그 꼬리표, PC ID, 중계기 ID, 중계기와 PC의 네트워크가 얼마나 맞았는지, 신호를 보낸 네트워크(192.0.2.0/24처럼 네트워크를 가리키는 값. PC의 주소나 MAC 주소는 아님), 서명된 명령의 해시, 시각(요청, 전송, PC가 켜진 것을 확인한 때, 끝난 때), 보낸 패킷 수, 결과입니다. 앱의 진행 표시가 이 기록을 읽고, 요청 한도(PC는 1분에 한 번, 계정은 1시간에 20번, 중계기는 1분에 10번)를 이 기록으로 셉니다. 네트워크는 그 네트워크에서 이 PC를 켠 적이 있는 중계기를 다음에도 고르려고 둡니다. 6개월 동안 보관하고, PC가 계정에서 빠지거나 중계기가 폐기된 뒤에도 그때까지 남으며, 계정을 지우면 함께 지웁니다. 서명된 명령 자체에는 그 네트워크에 있는 PC 랜카드의 MAC 주소가 들어 있고, 중계기가 답할 때까지만(길어야 2분쯤) 둡니다.
- **NAS의 중계기 로그**: 켜기 요청마다 번호, 시각, 끝 2바이트만 보이는 MAC 주소, PC의 네트워크, 신호를 보낸 NAS 랜카드의 주소를 남기고, 시작할 때 서버 이름, 중계기 ID, 등록 코드를 남깁니다. 중계기는 실행 중일 때 180일이 지난 줄을 지웁니다. 멈춰 둔 동안에는 정리하지 않으므로, 오래 멈춰 둘 때는 NAS 관리자가 로그를 지웁니다. 그 NAS의 관리자는 모두 읽을 수 있습니다.

### 6. 카메라와 마이크

휴대폰 앱은 Android에서 세션 중 직접 시작한 음성 통화에서만 마이크를 쓰고, 마이크 권한도 그때만 묻습니다. 소리는 상대 기기로 종단 간 암호화되어 가고 저장하지 않습니다. iPhone·iPad 앱에는 음성 통화도 마이크 권한도 없습니다. 두 앱 모두 카메라와 사진은 쓰지 않고, 1.9.1부터는 그 권한도 선언하지 않습니다. 그 전 판은 앱의 바탕이 된 공개 소스의 QR 코드 읽기(서버를 정하는 기능) 때문에 권한을 선언했지만, HJViewer는 그 화면을 보이지 않았습니다. 휴대폰은 다른 기기를 제어하기만 하고, 자기 화면을 내보내지 않습니다.

원격 접속을 받는 Windows PC에서는, 연결한 기기가 그 PC의 화면을 보고, PC의 권한 설정이 허락하는 만큼 카메라 영상과 소리도 받으며, PC 앞 사람이 받아들인 음성 통화를 할 수 있습니다. 모두 종단 간 암호화되고, 서버는 아무것도 저장하지 않습니다.

**녹화.** PC를 제어하는 기기는 세션의 화면(카메라 연결이면 카메라 영상)을 소리 없는 영상 파일로 녹화할 수 있습니다. 그 기기의 사용자가 녹화를 시작할 때, 또는 나가는 세션 자동 녹화를 켜 두었을 때입니다. 휴대폰 앱 가운데 녹화하는 것은 Android 앱뿐이고, iPhone·iPad 앱은 녹화하지 않습니다. PC도 그 앞의 사람이 들어오는 세션 자동 녹화를 켜 두면 받은 세션을 녹화합니다. 파일은 누가 지울 때까지 녹화한 그 기기에 남습니다. Windows는 그 사용자의 '동영상\HJViewer' 폴더나 사용자가 고른 다른 폴더(설치한 PC가 들어오는 세션을 녹화하면 C:\ProgramData\HJViewer\recording), Android 휴대폰은 앱 자신의 폴더입니다. HJViewer는 이 파일을 어디로도 보내지 않고, 서버는 보지 못합니다. PC 주인은 PC의 권한 설정에서 녹화를 끌 수 있고, 세션 중에도 끌 수 있습니다. 1.9.0부터는 연결한 기기가 녹화한다고 알리는 동안 PC의 연결 창에 '녹화 중'이 보입니다. 고친 앱이나 다른 프로그램으로 하는 녹화는 보이지 않습니다.

### 7. 처리 목적과 근거

계정 정보는 로그인과 로그인 유지, 내 PC와 그 상태 보이기, 내 기기끼리 PC 비밀번호 없이 연결하기, 내가 하지 않은 로그인과 연결을 알아채고 막기, 내 PC 켜기, 서비스 보호(로그인 한도, 기록, 백업), 서버를 교직원과, 운영자가 이용 비밀번호로 또는 교직원이 지원 코드로 들인 사람만 쓰게 하기(출입증, 3절), 최고운영자가 누가 어느 PC에서 서비스를 쓰는지 보기(3절)에 씁니다. 처리 근거는 로그인이나 이용 비밀번호, 지원 코드로 요청한 서비스의 제공입니다(개인정보 보호법 제15조 제1항 제4호). 다른 목적으로 쓰거나 팔지 않고, 9절 밖으로 넘기지 않습니다.

### 8. 삭제와 권리 행사

- **앱에서 계정 삭제**(1.9.1부터): 내 계정에 로그인한 기기에서 설정 → 계정 → [계정 삭제]를 누르고, 앱이 안내하는 낱말을 넣어 확인합니다. 계정은 바로 지워집니다. 명단에서 빠지고, 내 PC가 계정에서 빠지며, 그 계정의 중계기는 폐기되고, 그 계정이 만들어 아직 쓰지 않은 지원 코드는 끝납니다. 계정 키와 로그인 토큰도 끝나 모든 기기가 로그아웃됩니다(이 기기는 바로, 다른 기기는 다음에 서버에 닿을 때). 켜기 요청 기록을 포함해 그 계정에 대해 보관하던 것은 모두 지워집니다. 남는 것은 아래 "운영자에게 요청"과 같고, 운영자 기록에는 계정의 꼬리표로 삭제가 남습니다. 운영자에게 요청해도 됩니다.
- **앱에서 직접**: 기기에서 로그아웃하면 그 기기의 키와 토큰이 바로 끝납니다. 잃어버린 기기는 다른 내 기기의 '로그인한 기기'에서 로그아웃시키거나 다른 기기를 모두 로그아웃시킵니다. 내 PC 목록에서 PC를 뺄 수 있고, PC를 남에게 넘기기 전에는 그 PC에서 로그아웃합니다. PC에서 HJViewer를 제거하면 레지스트리에 둔 것과 '들어온 연결' 목록(4절)이 지워집니다. 연세포탈 비밀번호를 바꿔도 이미 로그인한 기기는 남으므로, 바꾼 뒤에는 [다른 기기 모두 로그아웃]도 누릅니다. 다른 기기에서 로그아웃시키면 비밀번호 없는 새 연결이 막히고, 1.9.0부터는 서버에 닿는 PC가 그 기기의 열린 연결도 5분 안에 끊습니다. 그 PC에서 로그아웃하면 그 PC의 비밀번호 없는 연결은 바로 끊깁니다. 서버에 닿지 못하거나 그 전 버전인 PC에서는 이미 열려 있는 연결이 PC의 연결 창에서 끊거나, 내 기기에서 PC 비밀번호로 그 PC에 연결해 '원격 PC 다시 시작'을 누르거나, PC를 다시 시작할 때까지 남습니다.
- **운영자에게 요청**: 나에 대해 보관하는 것을 보여 달라고, 고치거나 지워 달라고, 처리를 멈춰 달라고 요청할 수 있습니다. 계정을 지우면 명단에서도 빠지고, 그 계정의 중계기는 폐기되며, 켜기 요청 기록을 포함해 그 계정에 대해 보관하던 것이 모두 지워집니다. 남는 것은 뺀 PC의 묶음 ID·PC ID·공개키(180일), 폐기된 중계기의 ID·키·시각, 꼬리표로 남은 로그인·기기·키 기록(6개월), 운영자 기록, 그리고 운영자가 걸어 둔 막기(그 포탈 ID와 함께, 막기를 풀 때까지)이고, 백업은 14일 뒤에 사라집니다. 나중에 다시 로그인하면 다시 명단에 오릅니다(운영자가 명단에 있는 ID만 받는 동안에는 운영자가 먼저 다시 넣어야 합니다).
- 운영자가 내 계정을 막으면 내 기기들은 바로 로그아웃되고, 다음 로그인은 막혔다는 안내와 함께 거절됩니다.

### 9. 정보를 다루는 곳

- **운영자**: 프로젝트TF(이 정보의 개인정보 처리자)가 서버와 그 계정 서비스를 운영하고, 요청에 답하며, 이 정보를 책임집니다. 관리 명령은 사람이 따로 요청하지 않는 한 ID를 줄여 보이고 이름, 주소, 기기 이름을 보이지 않습니다. 관리 명령은 모두 기록됩니다. 5분마다 도는 점검이 읽는 명령 하나만 예외이고, 그 명령은 개수만 돌려주며 아무것도 바꾸지 않습니다. 운영자가 서버의 설정에서 정한 최고운영자는 앱에서 3절의 사용자와 PC를 보며, 볼 때마다 기록됩니다.
- **연세 SSO**: 로그인할 때 포탈 ID와 비밀번호를 확인합니다.
- **AI 도우미**: 운영자는 서버 작업에 AI 도우미를 씁니다. AI 도우미는 개수와, 사람이나 사람의 기기를 가리키지 않는 출력만 보도록 규칙으로 정해져 있고, 본 내용은 AI 도우미를 실행하는 외부 AI 서비스로 갑니다. 그 서비스는 미국의 Anthropic이고, 그 대화를 모델 학습에 쓰지 않습니다(그 설정을 꺼 둠). 사람이나 사람의 기기에 관한 일, 원래 값 보기와 키 만들기는 사람만 합니다.
- **PC의 2단계 인증 코드는 보내지 않음**: HJViewer 빌드는 upstream RustDesk의 'PC의 2단계 인증 코드를 Telegram 봇으로 보내기'를 꺼 두었습니다. 코드는 직접 설정한 인증 앱에서 나옵니다.
- **STUN 서버**(2절). 그 밖에는 없습니다. 광고나 분석 회사에 넘기지 않습니다.
- 사용자에 관한 것이 아닌 것: upstream rustdesk-server와 같이, 서버의 hbbs는 시작할 때와 하루에 한 번 RustDesk의 업데이트 서비스(api.rustdesk.com)에 더 새 서버 버전이 있는지 묻습니다. 서버의 운영체제와 그 버전, 프로세서 종류, 서버 기계의 지문을 보냅니다. 사용자나 기기에 관한 것은 보내지 않습니다.

**제3자.** 연세 SSO와 공개 STUN 서버는 여기에 적은 정보만, 적은 목적으로만 받고, 이 둘과 운영자 말고는 누구도 HJViewer에서 사용자 정보를 받지 않습니다. 학교의 로그인 서비스인 연세 SSO는 로그인을 확인하는 데에만 포탈 ID와 비밀번호를 받고, 학교의 개인정보 처리방침과 법이 요구하는 대로 보호합니다. 공개 STUN 서버는 기기가 닿는 다른 서버와 마찬가지로 요청한 기기의 IP 주소와 포트만 받고, HJViewer는 그 밖의 사용자 정보를 주지 않습니다.

### 10. 문의와 변경

**개인정보 보호책임자**: 프로젝트TF, haejin.cho@yonsei.ac.kr. 내 정보에 관한 요청(8절)은 공개 이슈가 아니라 이 주소로 보내 주세요. 공개 이슈에는 포탈 ID나 성명 같은 개인정보를 쓰지 마세요. 앱 자체에 관한 문의는 https://github.com/yonsei-projecttf/hjviewer 에 이슈로 남길 수 있습니다. 이 문서가 바뀐 내역은 그 저장소의 기록에 남습니다.
