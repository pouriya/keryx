# keryx REST API

Everything here is under `{origin}/api/v1`. Every reply is wrapped:

    {"ok": true, "data": {...}}
    {"ok": false, "code": "...", "message": "..."}

Success is 2xx and failure is 4xx or 5xx. `code` is for programs and `message` for people. Binary
fields are standard base64 with padding, and ids are ULID text. Secrets go in bodies and headers,
never in a URL.

## Proving who you are

A browser logs in with `POST /login`, which sets an `HttpOnly` cookie named `keryx_session`. Anything
else logs in with `POST /login/token`, which returns a token, and sends it as
`Authorization: Bearer <token>`. An agent logs in with `POST /login/agent`, giving the token its owner
was shown, and gets the same kind of session token back.

## Anyone

    GET  /api/v1                 this guide
    GET  /public/features        -> registration, captcha
    GET  /captcha                -> id, image. Only with a [captcha] section
    POST /register               username, password, display_name?, captcha?{id, answer} -> account
    POST /login                  username, password, captcha?, device?{id, challenge, signature}
                                 -> account, device?
    POST /login/token            the same -> account, device?, token
    POST /login/agent            token, device?{id, challenge, signature} -> account, device?, token
    POST /setup                  token, password -> account, username
    POST /challenge              username or account, purpose -> challenge, account
    POST /password/reset         username, challenge, new_password, signature

`purpose` is `password_reset` or `device_login`. A challenge lasts 5 minutes and works once. A
returning device signs one with its own key and gives it at login, and otherwise a login has no
device. A reset is signed by the master key, and ends all of the account's sessions.

Registering and the one-time link do not log you in.

An agent's token is a JWT naming its account in `sub`, so an agent asks its challenge by `account`. It
has no password and never logs in with one. A renewed or revoked token is refused with `bad_login`, and
a disabled agent's with `agent_disabled`. Without an `[agents]` section, agents are off: `agents_off`.

## Any session

    GET  /sessions               -> session_list
    DEL  /sessions/{id}
    PUT  /account/keys           master_public, account_public, account_stamp, invite_public,
                                 invite_stamp -> self_chat
    GET  /accounts/{id}          -> id, username, display_name?, master_public?, account_public?,
                                    account_stamp?, invite_public?, invite_stamp?, agent?, owner?
    GET  /devices                -> device_list
    POST /devices/id             -> device
    POST /devices                device, public_key, stamp, platform, os, os_version,
                                 web_platform?, web_platform_version? -> device
    POST /devices/{id}/terminate signature
    POST /devices/{id}/logout    signature

`PUT /account/keys` works once. A session with no device gets a reduced `device_list`: `id`,
`platform` and `last_active_day`, which is days since the Unix epoch.

To add a device: ask `POST /devices/id` for its id, which lasts 5 minutes and works once, stamp that id
with the master key, then `POST /devices`. `platform` is 0 for desktop and 1 for mobile. The session
then belongs to that device, and the device's notice appears in the account's self chat.

`terminate` is signed by the master key, and `logout` by the device's own key. Both end the device for
good. An agent's device is ended only by its owner, so both are refused for it with `agent_device`.
An agent has one device: a second is refused with `device_limit`.

`agent` says the account is an agent, run by keryx-agent, and `owner` names its owner. An agent's username ends in `_agent`, and a person's never does.

## With a named permission

    POST /accounts               username, admin?, display_name? -> account, link
    GET  /accounts?search=       -> account_list
    POST /sticker_packs          sticker.manage: name -> pack
    DEL  /sticker_packs/{id}     sticker.manage: the pack and its stickers
    POST /sticker_packs/{id}/stickers?emoji=   sticker.manage: the raw WebP -> sticker
    DEL  /stickers/{id}          sticker.manage
    PUT  /accounts/{id}/names    display_name? (profile.edit), username? and password
                                 (account.rename), for yourself -> version. Admin: anyone's, with no
                                 password
    DEL  /accounts/{id}/photo    admin: removes the account's photo

## Your agents, from a registered device

    POST /agents                 agent.create: username, display_name, permission_list, admin?
                                 -> account, token
    GET  /agents                 -> agent_list{id, username, display_name?, disabled, admin,
                                    permission_list, token_at?, last_connected_at?, device?}
    PUT  /agents/{id}            display_name?, permission_list?, admin?
    POST /agents/{id}/token      statement?{device, signature} -> token: a new token
    DEL  /agents/{id}/token      statement?{device, signature}: no token until a renew
    POST /agents/{id}/disable    statement?{device, signature}
    PUT  /accounts/{id}/keys     your agent's keys, once, as `PUT /account/keys` takes them

The token is shown once. Making an agent needs `agent.create` even while registration is closed, and
you may have `[limits] agents` agents that are not disabled, or `too_many_agents`. Each ticked name in
`permission_list` is a server permission you hold, or `not_held`, and never `agent.create`; `admin`
needs you to be admin. The agent holds each ticked permission only while you do. Someone else's agent
is `not_owner`, and a disabled one `agent_disabled`.

A renew, a revoke and disabling end the agent's sessions and its device at once. When it has a device,
`statement` is that device's termination, signed with the agent's master key over `keryx v1 revoke
device`, the agent and the device, as `terminate` is: so each needs the agent's words, or
`bad_signature`. Disabling also takes it out of every group and channel, as leaving does. Its profile
and photo are yours to set, through the routes below, signed by your own device.

## With a registered device, under the account's device limit

    POST /key_packages                    body_list -> count
    POST /accounts/{id}/key_packages/claim   -> key_package_list{device, body}
    POST /chats                           account -> chat, created; or type (group | channel),
                                          topics?, findable?{name, bio?} -> chat
    GET  /chats/{id}/group_info           -> epoch, group_info
    GET  /chats/{id}/devices              -> device_list{account, device, public_key, stamp,
                                             master_public, statement?}
    GET  /chats/{id}/wraps                -> wrap_list{epoch, account, wrapped}
    GET  /chats/{id}/messages/{seq}/times?after= -> member_list{account, received_at?, seen_at?}, more
    PUT  /chats/{id}/draft                {draft?: {epoch, body}}: keeps your sealed draft, or clears it
    POST /chat_folders/id                 -> folder: a new folder's id, yours for 5 minutes
    PUT  /chat_folders                    {folder_list, default}: your whole arrangement of folders
    PUT  /devices/{id}/presence           this session's device: {in_app, trouble?, text?}
                                          -> presence_list
    GET  /accounts/{id}/presence          presence.in_app or presence.connected -> account, state?,
                                          in_app_at?, connected_at?
    GET  /kv?version=                     -> version, and kv_list{key, value} when yours differs
    PATCH /kv                             {version, set: [{key, value}], remove: [key]} -> version
    GET  /accounts/{id}/profile           yourself, or sharing a chat -> account, username,
                                          display_name?, version, profile?{sealed, signature,
                                          device, device_public, stamp}, photo?
    PUT  /accounts/{id}/profile           profile.edit, yourself or your agent: sealed?, signature?,
                                          photo? -> version
    POST /accounts/{id}/photos            profile.edit, yourself or your agent: the sealed photo's
                                          raw bytes, one chunk -> photo
    GET  /photos/{id}                     yourself, or sharing a chat -> the sealed bytes
    POST /chats/{id}/files                kind, size, chunks -> file
    PUT  /files/{id}/chunks/{n}           the encrypted chunk's raw bytes -> size, chunks, received
    GET  /files/{id}                      -> size, chunks, received
    GET  /files/{id}/chunks/{n}           -> the encrypted chunk's raw bytes
    GET  /calls/{chat}/{seq}/moq          a WebSocket: the call's MoQ, while the call is open
    GET  /stickers                        -> pack_list{id, name, sticker_list{id, emoji?, width, height,
                                             size}}, also with sticker.manage
    GET  /stickers/{id}                   -> the sticker's WebP, which never changes

Claiming hands out one package per device and deletes it. `POST /chats` gives the pair's chat, and your
own account id gives your self chat. The `/chats/{id}` routes need you to be in that chat. In
`device_list`, a device with a `statement` has ended, and that is its signed revocation or logout.

A file is encrypted by its sender in chunks of 1 MiB, each followed by its 16-byte tag. `kind` is the
type that picks the upload limit (1 voice, 3 image, 4 music, 5 video, 6 doc, 7 file), and `size` counts the
encrypted bytes. Chunks go up in order, and sending the last one again is fine, so after a drop, ask
`GET /files/{id}` and go on from `received`. Only the uploader's account puts chunks. A `send` keeps
its files by listing them in `persist`, and until then only the uploader reads them. An edit or delete
lists the files it drops in `remove`. An upload no sent message keeps is removed after a day.

A sticker is a static or animated WebP of at most 512x512 and `[limits.upload] sticker`, kept in a pack
whose name is unique. It is not encrypted, since everyone here can see it. A sticker message, content
kind 12, names it inside its encryption as `{"sticker": "<id>"}`, and cannot be edited. The pack named
`Default` is built into the server and listed after the others unless `[stickers] default_pack` is false;
its stickers are always served, and it cannot be changed, removed or named again. A question
message, kind 10, holds a tree of questions, and an answer, kind 11, one account's answer to it, both
sealed like text, so the server reads neither. A suggestion, kind 13, suggests what to do with its chat,
such as the folder it goes in, and an ack, kind 14, is one account's accepting or declining it. Neither
can be edited, and an ack moves neither the chat's time nor its preview. An activity, kind 15, is one
step an agent says it is taking, `{"text"}` of 1 to 140 characters, and cannot be edited either.

## The WebSocket

    GET /ws

Send the cookie or the Bearer header, and offer the `keryx.v1` subprotocol. A refused connection closes
with code 4403 and the refusal's code as the reason. A server shutting down closes with 1001.

The first frame is always `features`, with the server's `ping_after`, `ping_every` and `ping_timeout` in
seconds. After `ping_after` with nothing from you, the server sends a WebSocket ping, then one every
`ping_every`, and you have `ping_timeout` to answer each or the connection closes. A browser answers by
itself. To check the server yourself, send a WebSocket ping, or a `ping` frame, which gets `done`.

Every frame is JSON with a `type`. A request's `ref` is a ULID you make, which comes back in its answer.

You send: `send`, `seen`, `received`, `typing`, `sync`, `group_info`, `wraps`, `profile_keys`,
`call_event`, `times`, `draft`, `folder_id`, `chat_folders`, `presence`, `presence_of`, `kv`,
`kv_set`, `ping`, and for groups and channels `chat_remove`, `leave`, `chat_role`, `join_request`,
`join_cancel`, `join_decline`, `invite`, `invite_answer`, `topic_id`, `topics`, `chat_profile`,
`chat_options`, `topic_profile`, `topic_options`, `topic_role` and `findable`.

The server sends: `features`, `sent`, `done`, `refused`, `message`, `chats`, `welcome`, `typing`,
`marks`, `revoked`, `call_event`, `times`, `draft`, `folder_id`, `chat_folders`, `profile`,
`profile_keys`, `presence`, `presence_of`, `kv`, `chat_event`, `topic_id`, `topics`, `chat_profile`,
`chat_options`, `topic`, `findable`, `join_request`, `invitation`.

`profile` carries an `account` sharing a chat with you, or your own, with its `version`, `username` and
`display_name`, whenever its names, photo, bio or links change. Each `chats` entry carries every
member's `version` in `profile_key_list`, with their profile key sealed under the chat's key at `epoch`
once someone shared it. A commit carries `profile_key_list`, `{account, sealed}` at its epoch, and
`profile_keys` carries `chat`, `epoch` and the same list for keys a chat is missing, answered with
`done`. The server keeps the newest per chat and account. Once either is saved, the chat's other online
devices get `profile_keys` with `chat` and those accounts' items as a `chats` entry carries them.

`received` and `seen` each carry `chat` and `up_to_seq`, and the server keeps the highest of each. When
one rises, the other members' online devices get `marks` with the `account`, its `received_seq` and its
`seen_seq`. Each `chats` entry carries the same positions in `mark_list`, and its newest content
message in `last_content`. Neither goes out in a channel.

`draft` carries `chat` and `draft`, `{epoch, body}` sealed by your client, or none to clear it, and is
answered with `done`. The server keeps one per account and chat, never reads it, and sends it as `draft`
to your other online devices. Each `chats` entry carries yours in `draft`, with `saved_at`.
`folder_id` asks for a new folder's id and is answered with `folder_id` and `folder`, kept for you for
5 minutes, so your client can seal the folder's name with it. `chat_folders` carries your whole
arrangement at once: `folder_list` in tab order, each `{id, name?, chat_list?, pin_list}`, and
`default`, the folder a device opens on. `all` is the one folder with no `name` or `chat_list`,
always first, and holds every chat. A name is sealed by your client and never read; send an unchanged one back as the
same bytes. It is answered with `done`, and your other online devices get it as `chat_folders`. The
first page of `chats` carries yours in `chat_folders`. A change you may not make is `no_permission`,
more than `folders` folders `too_many_folders`, an id neither yours nor issued to you
`unknown_folder`, a sealed name over its limit `body_too_large`, and a broken shape `bad_body`; any
of them refuses the whole arrangement.
`presence` carries `in_app`, whether your app is on screen, and may carry `trouble`, true while the
device is having trouble, and `text`, sealed under your profile key, at most 1 KiB. Like `typing`, it is an event with no `ref`,
and nothing answers it. Until a connection sends one, its device is not there to anyone. After the first,
you get one `presence` per person sharing a chat with you. Send another on every open and close. Each
`presence` is one account: `state` (`online`, `call`, `trouble`, `away` or `offline`), `in_app_at` and
`text?` under `presence.in_app`, `connected_at` under `presence.connected`, the server merging its
devices. An agent's `text` goes to its owner alone. `trouble` and `text` are kept only while the
connection lasts, so send them again after reconnecting. Whenever it changes, everyone sharing a chat gets it
on their devices that sent theirs, and so does each person of a direct chat when it is made. `presence_of` carries `account`, anyone's, and is answered with
`presence_of` and `presence`, once.
`kv` carries `version?` and is answered with `kv`: `version`, and `kv_list`, your whole set, when yours
differs. `kv_set` carries `version`, `set` and `remove` and is answered with `kv` and the new version.
One based on an older version is refused with `kv_stale`, carrying `version` and `kv_list`. Your other
online devices get `kv` with `from`, `version`, `set` and `remove`. A name is 1 to 64 of a-z, 0-9, _ and
., and a value is bytes the server never reads, at most `kv_value`; more than `kv_keys` names is
`too_many_keys`.
`times` carries `chat`, `seq` and an optional `after` and is answered with `times`: a page of 50 of the
members but the sender in `member_list`, with the server's time for `received_at` and `seen_at`, each
left out when you may not see it, and `more` when another page follows. Those who saw it come first,
then those who only received it, then the rest, the newest first and then by account; ask the next
page with `after` set to the last account listed. A thread's message counts reading in its thread, and
a topic's reading in its topic while topics are on.
`message.view_received.own` and `message.view_seen.own` cover your own messages, and
`message.view_received.others` and `message.view_seen.others` everyone else's, under the chat type's
prefix. With neither on that message, it is refused with `no_permission`.

`sync` answers with `chats`: your chats in `chat_list`, then every message in `message_list` and call
row in `call_list` saved since `since`, the server time in milliseconds of the newest row you hold,
less 1 second. With no `since` it starts from the beginning, and with `chat` it reads that chat
only. A page holds at most `history_page` rows, in order of server time, chat, `seq` and `n`, and
`more` says another follows: ask again with `after`, the last row's `{at, chat, seq, n}`, and it
starts right after that row. Only the first page carries `chat_list`. Drop what you already hold by
`(chat, seq)` and `(chat, call_seq, n)`. A removed message comes again with only its `seq` and the
time it was removed.

A commit is a `send` with kind 3, carrying `epoch`, `wrap_list`, `welcome_list` and `group_info`, the
chat's group info at that epoch, which is saved with it. The first commit for each epoch wins, and a
loser gets `refused` with the code `commit_lost`. A chat's group starts when someone sends `group_info`
for epoch 0, and if two people start it at once the second gets `chat_started` and waits for its
Welcome. The same epoch 0 bytes sent again are accepted, as a resend whose answer was lost.

Commits are MLS public messages, and the server reads each one, in every kind of chat. A device
joining by itself must be the device sending the commit. An add may bring in only devices of the
accounts its action names, or of accounts already in the chat. A removal may take out only the
accounts its action names, ended devices, or accounts no longer in the chat. On an external rejoin,
the old leaf may also be replaced if both leaves match the sending account, device and the active
device's registered signing key. A commit that breaks these rules, or cannot be read, is refused
with `bad_commit`. Epoch 0's `group_info` must be the chat's own, or it is refused with `bad_message`.

## Groups and channels

`POST /chats` with `type` makes one, with you its owner. Your device then starts its group as a direct
chat's start does, and adds its people with one commit. A group has at most `group_members` accounts
and a channel `channel_members`. Each frame below is answered with `done` unless noted, and has the REST
route beside it:

    chat_remove {ref, chat}                     DEL  /chats/{id}                        owners
    leave {ref, chat}                           POST /chats/{id}/leave
    chat_role {ref, chat, account, role}        PUT  /chats/{id}/members/{account}/role  owners: {role}
    join_request {ref, chat, note?}             POST /chats/{id}/requests              {note?}
    join_cancel {ref, chat}                     DEL  /chats/{id}/requests
    join_decline {ref, chat, account}           POST /chats/{id}/requests/{account}/decline
                                                GET  /chats/{id}/requests   member.add -> request_list
    invite {ref, chat, account, sealed, signature}   POST /chats/{id}/invitations
    invite_answer {ref, chat, accept}           POST /chats/{id}/invitation            {accept}
    topic_id {ref, chat} -> topic_id {ref, chat, topic}   POST /chats/{id}/topics/id -> topic
    topics {ref, chat, on, topic_list}          PUT  /chats/{id}/topics                topic.manage
    chat_profile {ref, chat, sealed, signature, photo?}   PUT /chats/{id}/profile   profile.edit
    chat_options {ref, chat, allow, deny, slow_mode?}   PUT /chats/{id}/options       owners
    findable {ref, chat, findable, name?, bio?} PUT  /chats/{id}/findable              owners
    topic_profile {ref, chat, topic, sealed, signature, photo?}
                                  PUT /chats/{id}/topics/{topic}/profile       topic.edit
    topic_options {ref, chat, topic, allow?, deny?, slow_mode?, write?, closed?, hidden?}
                                  PUT /chats/{id}/topics/{topic}/options       topic.edit
    topic_role {ref, chat, topic, account, role}
                                  PUT /chats/{id}/topics/{topic}/members/{account}/role  owners

`role` is `owner`, `moderator` or `member`; the last owner cannot step down, `last_owner`, and the last
owner leaving removes the chat for everyone. Leaving takes effect at once: the chat goes from your list
and folders, and the next member online commits your devices' removal in MLS. A commit that adds or
removes people carries `action`, `{kind: add | remove, account_list}`, which the server checks against
the permissions and saves with the commit; letting a request in is an add. An add naming someone who
accepts only invitations is refused with `invite_needed` and its `account_list`, and an `invite` goes
instead: `signature` is the inviting device's over `keryx v1 invite`, the chat and the account, and
the invited device joins by external commit with `{"invite": {account, device, signature}}` as its
authenticated data. `sealed` is HPKE-sealed to the invited account's `invite_public`, whose
`invite_stamp` is its master key's over `keryx v1 invite key`, the account and the key; the inviter's
note, if any, is inside it. A join request's note is plain text the server reads, at most 140
characters. Invitations and requests expire after `invitation_lifetime`.

A group's or channel's profile is sealed under its own profile key, which never changes. The key goes
sealed at an epoch: in `group_info` at epoch 0 as `chat_key`, beside the first `chat_profile` and
`chat_options`, and in a commit's `chat_key` when it adds people or joins by invitation. The server
keeps the newest, and the profile comes as `{sealed, signature, device, version, key_epoch, key}`.

Each topic, and General under the all-zero id, has its own settings: `write` `everyone` or
`moderators` (who write in its main list; members still react and talk in threads), `closed` (only
moderators write, react or reply there: `topic_closed`), `hidden` (only the chat's owners and
moderators and the topic's own see it; to the rest it does not exist, and its messages are withheld),
its own `slow_mode` (none follows the chat's, 0 is off, and each topic counts its own messages), and
`allow` and `deny` over the reaction, sticker and file kinds and the four times
permissions. A main-list send in a moderators-only topic is refused with `moderators_only`. A topic's
own moderators, appointed by owners with `topic_role` (`moderator` or `member`), hold `topic.edit`,
deleting others' messages, writing when closed, the slow-mode exemption and everyone's times there.
General cannot be hidden. A topic's profile is sealed under the chat's profile key with `keryx v1 topic
profile`, the chat and the topic, holding its bio, links, photo and rules. A change reaches the chat's
online devices as `topic` `{chat, topic, ...settings, mod_list, profile?}`; a hidden topic reaches the
rest as `{chat, topic, hidden: true}` alone. A reaction carries its message's `topic` and `thread_seq`.

Each `chats` entry of a group or channel carries `role`, `topics_on`, `topic_list`, `topic_setting_list`
(every topic you may see, General's first), `place_seen_list`,
`profile`, `options`, `findable` and `member_count`, and `request_count`, the join requests waiting,
for whoever may let people in. The first page carries `asked_list`, your own requests, and
`invitation_list`. A message in a topic or thread carries `topic` or `thread_seq` in its header.

In a group, each change to its people is a line: a message of `kind` 5 the server writes as the chat's
next seq, unencrypted and unsigned, its device the actor's, its body readable JSON `{"action",
"account_list"?}`, the action one of `created`, `added`, `let_in`, `joined`, `removed`, `left`,
`made_owner`, `made_moderator` and `made_member`. It is replied to, reacted to and opened as a thread
like any message, and removed by those holding `message.delete.others`. A client sending one is refused
with `server_only`. Channels get no lines.

The history comes as `chat_event` and through `sync` in `chat_event_list`: `{chat, n, actor, action,
target?, seq?, topic?, audience, server_at, copy_list?}`, with `action` 0 created, 1 added, 2 removed,
3 left, 4 let in, 5 joined, 6 made owner, 7 made moderator, 8 made member, 9 deleted, 10 topic added, 11
topic removed, 12 topics on, 13 topics off, 14 invitation declined, 15 chat removed, 16 a topic's
profile edited, 17 a topic's options changed, 18 made a topic's moderator, 19 made a member there
again and 20 edited. Every delete and edit in a group or channel writes one, `target` the message's
author and `copy_list` the message and its edits as they were, ciphertext, header and signature, which
the readers open and check. A row with a `topic` is that topic's history, which its own moderators read
too. `audience` 1 is owners and
moderators, 2 one account, and 0 everyone, which is live only and never saved: a removed chat, and each
change to its people, so every device keeps its member list. `topics`,
`chat_profile`, `chat_options`, `findable` `{chat, findable, name?, bio?}`, `join_request` and
`invitation` reach the devices they concern once saved; your own `join_request` and its taking back
reach every device of yours, the asking one too, and so does an edited `chat_profile`, the saving
device included, so it shows the server's copy. A removed topic's messages are wiped with no delete,
so a client syncs the chat to drop them. Removing a chat sends `join_request` gone and `invitation`
with none to those who asked to join it or were invited. `GET /accounts?search=` also lists findable chats in `chat_list`. Calls are refused in groups and
channels with `call_not_allowed`.

## Calls

A call is one `send` with content kind 8, in a direct chat only, and its delivery is the ring. A chat
has one open call at a time: starting another is refused with `call_open`, and the refusal carries the
open call's `call_seq`, so join that one instead. Calls elsewhere are refused with `call_not_allowed`.

The other account declines with a `call_event` frame of `kind` 2 and the call's `call_seq`, answered
with `done`, and in a direct chat that ends the call as missed. A call nobody answers ends as missed
after 60 seconds. Every `join`, `leave`, `decline` and `end` reaches the chat's online devices as
`call_event`: `{chat, call_seq, n, kind, device, server_at}`, with `kind` 0 join, 1 leave, 2 decline,
3 text and 4 end. The `end` carries `outcome`, 0 missed or 1 answered, and its `server_at` is when the
call ended. `sync` hands every one of them, text included, in `call_list`. A call with no `end` is still
open, and an ended call cannot be joined again.

Joining is a WebSocket to `/api/v1/calls/{chat}/{seq}/moq`, with the same cookie or Bearer token as
`/api/v1/ws` and the subprotocol `qmux-02.moq-lite-05`: MoQ over qmux, moq-lite 05 only. A refusal
comes after the upgrade, as close code 4403 with the refusal's code as the reason, such as
`call_ended`. A device publishes the broadcast named by its device id, with `state`, `audio`, `video`
and `text` tracks, every frame sealed with SFrame and every MoQ timestamp zero. It reads the others'
broadcasts and the broadcast `server`, whose `events` track carries each call event's JSON, text
included. A join by the other account answers the call. A clean close is a leave at once; a socket
that drops gets 10 seconds to come back before it counts as a leave, and the last device to leave
ends the call. A frame over the server's limit closes the socket with 4403 `frame_too_large`, and
text that is not `{client_at, epoch, signature, body}` with 4403 `bad_frame`. When the call ends, open
call sockets close with 1000 `call_ended`, and a newer socket from the same device closes the older
one with 1000 `replaced`. A decline after the call was answered is refused with `call_answered`.

## Common refusal codes

`not_logged_in`, `no_permission`, `no_device`, `device_limit`, `not_member`, `bad_body`,
`body_too_large`, `captcha_wrong`, `bad_login`, `bad_signature`, `unknown_device_id`.
