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
`Authorization: Bearer <token>`.

## Anyone

    GET  /api/v1                 this guide
    GET  /public/features        -> registration, captcha
    GET  /captcha                -> id, image. Only with a [captcha] section
    POST /register               username, password, display_name?, captcha?{id, answer} -> account
    POST /login                  username, password, captcha?, device?{id, challenge, signature}
                                 -> account, device?
    POST /login/token            the same -> account, device?, token
    POST /setup                  token, password -> account, username
    POST /challenge              username, purpose -> challenge, account
    POST /password/reset         username, challenge, new_password, signature

`purpose` is `password_reset` or `device_login`. A challenge lasts 5 minutes and works once. A
returning device signs one with its own key and gives it at login, and otherwise a login has no
device. A reset is signed by the master key, and ends all of the account's sessions.

Registering and the one-time link do not log you in.

## Any session

    GET  /sessions               -> session_list
    DEL  /sessions/{id}
    PUT  /account/keys           master_public, account_public, account_stamp -> self_chat
    GET  /accounts/{id}          -> id, username, display_name?, master_public?, account_public?,
                                    account_stamp?
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
good.

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

## With a registered device, under the account's device limit

    POST /key_packages                    body_list -> count
    POST /accounts/{id}/key_packages/claim   -> key_package_list{device, body}
    POST /chats                           account -> chat, created
    GET  /chats/{id}/group_info           -> epoch, group_info
    GET  /chats/{id}/devices              -> device_list{account, device, public_key, stamp,
                                             master_public, statement?}
    GET  /chats/{id}/wraps                -> wrap_list{epoch, account, wrapped}
    GET  /chats/{id}/messages/{seq}/times -> member_list{account, received_at?, seen_at?}
    PUT  /chats/{id}/draft                {draft?: {epoch, body}}: keeps your sealed draft, or clears it
    PUT  /pins                            {chat_list}: your pinned chats, the whole order, first on top
    GET  /accounts/{id}/profile           yourself, or sharing a chat -> account, username,
                                          display_name?, version, profile?{sealed, signature,
                                          device, device_public, stamp}, photo?
    PUT  /accounts/{id}/profile           profile.edit, yourself: sealed?, signature?, photo?
                                          -> version
    POST /accounts/{id}/photos            profile.edit, yourself: the sealed photo's raw bytes,
                                          one chunk -> photo
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
kind 12, names it inside its encryption as `{"sticker": "<id>"}`, and cannot be edited. A question
message, kind 10, holds a tree of questions, and an answer, kind 11, one account's answer to it, both
sealed like text, so the server reads neither.

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
`call_event`, `times`, `draft`, `pins`, `ping`.

The server sends: `features`, `sent`, `done`, `refused`, `message`, `chats`, `welcome`, `typing`,
`marks`, `revoked`, `call_event`, `times`, `draft`, `pins`, `profile`, `profile_keys`.

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
`pins` carries `chat_list`, your pinned chats in order, first on top, each a chat you are in and named
once, and every chat it leaves out is unpinned. It is answered with `done`, and your other online
devices get it as `pins`. Each `chats` entry carries its place in `pin`, from 0, when it is pinned.
`times` carries `chat` and `seq` and is answered with `times`: `member_list` of each member but the
sender, with the server's time for `received_at` and `seen_at`.

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
